//! One plugin process on a pseudo-terminal, with every output byte
//! timestamped (bench_process.py's Session).
//!
//! Spawned with posix_spawn rather than fork, so the child never holds this
//! process's pages: on macOS the peak RSS wait4 reports would otherwise be the
//! bench's, not the plugin's. `std::process::Command` would fork as soon as it
//! is asked for a new session, so this calls posix_spawn itself, and reaps
//! with wait4 for the child's rusage.

use std::ffi::CString;
use std::os::unix::ffi::OsStrExt;
use std::path::Path;
use std::time::{Duration, Instant};

/// The manifest's 110x30 popup, as the pty sees it: columns, rows.
pub const SIZE: (u16, u16) = (107, 28);
/// A frame is over once the plugin has been quiet this long.
pub const QUIET: f64 = 0.02;
pub const TIMEOUT: f64 = 15.0;
const APPEARANCE_QUERY: &[u8] = b"\x1b]11;?";
const APPEARANCE_REPLY: &[u8] = b"\x1b]11;rgb:1e1e/1e1e/2e2e\x1b\\";

#[cfg(target_os = "macos")]
const POSIX_SPAWN_SETSID: libc::c_short = 0x0400; // <spawn.h>, not in libc
#[cfg(not(target_os = "macos"))]
const POSIX_SPAWN_SETSID: libc::c_short = libc::POSIX_SPAWN_SETSID;

pub fn find(haystack: &[u8], needle: &[u8], since: usize) -> Option<usize> {
    if since > haystack.len() {
        return None;
    }
    haystack[since..].windows(needle.len()).position(|w| w == needle).map(|at| at + since)
}

fn cstring(bytes: &[u8]) -> CString {
    CString::new(bytes).expect("no NUL in an argument or the environment")
}

fn cloexec(fd: i32) {
    unsafe {
        let flags = libc::fcntl(fd, libc::F_GETFD);
        libc::fcntl(fd, libc::F_SETFD, flags | libc::FD_CLOEXEC);
    }
}

fn check(rc: libc::c_int, what: &str) {
    assert_eq!(rc, 0, "{}: {}", what, std::io::Error::from_raw_os_error(rc));
}

pub struct Session {
    master: i32,
    pub pid: libc::pid_t,
    pub started: Instant,
    pub output: Vec<u8>,
    /// `(when, output length after it)` for every read.
    chunks: Vec<(Instant, usize)>,
    pub status: Option<i32>,
    rusage: Option<libc::rusage>,
    answered: usize,
}

impl Session {
    /// `program` (already resolved to a path) with `argv` and exactly `env`.
    pub fn spawn(program: &Path, argv: &[String], env: &[(String, String)]) -> Result<Session, String> {
        let (mut master, mut slave) = (0, 0);
        let rc = unsafe {
            libc::openpty(&mut master, &mut slave, std::ptr::null_mut(), std::ptr::null_mut(), std::ptr::null_mut())
        };
        if rc != 0 {
            return Err(format!("openpty: {}", std::io::Error::last_os_error()));
        }
        // Non-inheritable, as os.openpty's are; the dup2s below give the child
        // its three.
        cloexec(master);
        cloexec(slave);
        let size = libc::winsize { ws_row: SIZE.1, ws_col: SIZE.0, ws_xpixel: 0, ws_ypixel: 0 };
        unsafe { libc::ioctl(slave, libc::TIOCSWINSZ, &size) };

        let path = cstring(program.as_os_str().as_bytes());
        let args: Vec<CString> = argv.iter().map(|a| cstring(a.as_bytes())).collect();
        let vars: Vec<CString> = env.iter().map(|(k, v)| cstring(format!("{}={}", k, v).as_bytes())).collect();
        let mut arg_ptrs: Vec<*mut libc::c_char> = args.iter().map(|a| a.as_ptr() as *mut _).collect();
        arg_ptrs.push(std::ptr::null_mut());
        let mut env_ptrs: Vec<*mut libc::c_char> = vars.iter().map(|v| v.as_ptr() as *mut _).collect();
        env_ptrs.push(std::ptr::null_mut());

        let mut pid: libc::pid_t = 0;
        let started;
        unsafe {
            let mut actions: libc::posix_spawn_file_actions_t = std::mem::zeroed();
            let mut attr: libc::posix_spawnattr_t = std::mem::zeroed();
            check(libc::posix_spawn_file_actions_init(&mut actions), "posix_spawn_file_actions_init");
            for fd in 0..3 {
                check(libc::posix_spawn_file_actions_adddup2(&mut actions, slave, fd), "adddup2");
            }
            check(libc::posix_spawn_file_actions_addclose(&mut actions, master), "addclose");
            check(libc::posix_spawnattr_init(&mut attr), "posix_spawnattr_init");
            check(libc::posix_spawnattr_setflags(&mut attr, POSIX_SPAWN_SETSID), "posix_spawnattr_setflags");
            started = Instant::now();
            let rc = libc::posix_spawn(&mut pid, path.as_ptr(), &actions, &attr, arg_ptrs.as_ptr(), env_ptrs.as_ptr());
            libc::posix_spawn_file_actions_destroy(&mut actions);
            libc::posix_spawnattr_destroy(&mut attr);
            libc::close(slave);
            if rc != 0 {
                libc::close(master);
                return Err(format!("spawn {}: {}", program.display(), std::io::Error::from_raw_os_error(rc)));
            }
        }
        Ok(Session {
            master,
            pid,
            started,
            output: Vec::new(),
            chunks: Vec::new(),
            status: None,
            rusage: None,
            answered: 0,
        })
    }

    /// Read whatever arrives within `timeout` seconds; false once the pty is closed.
    pub fn pump(&mut self, timeout: f64) -> bool {
        let mut poll = libc::pollfd { fd: self.master, events: libc::POLLIN, revents: 0 };
        let ms = (timeout.max(0.0) * 1000.0).ceil() as i32;
        let ready = unsafe { libc::poll(&mut poll, 1, ms) };
        if ready <= 0 {
            return true; // nothing yet, or EINTR
        }
        let mut buffer = [0u8; 65536];
        let n = unsafe { libc::read(self.master, buffer.as_mut_ptr().cast(), buffer.len()) };
        if n < 0 {
            let error = std::io::Error::last_os_error().raw_os_error();
            return error == Some(libc::EINTR) || error == Some(libc::EAGAIN); // EIO: the child is gone
        }
        if n == 0 {
            return false;
        }
        self.output.extend_from_slice(&buffer[..n as usize]);
        self.chunks.push((Instant::now(), self.output.len()));
        // Be the terminal: answer the background colour query the way most do.
        while let Some(found) = find(&self.output, APPEARANCE_QUERY, self.answered) {
            self.answered = found + APPEARANCE_QUERY.len();
            self.write(APPEARANCE_REPLY);
        }
        true
    }

    fn write(&self, data: &[u8]) {
        let mut rest = data;
        while !rest.is_empty() {
            let n = unsafe { libc::write(self.master, rest.as_ptr().cast(), rest.len()) };
            if n <= 0 {
                return;
            }
            rest = &rest[n as usize..];
        }
    }

    /// When the read that completed `marker` (searched from `since`) arrived.
    pub fn wait_for(&mut self, marker: &[u8], since: usize) -> Result<Instant, String> {
        let deadline = Instant::now() + Duration::from_secs_f64(TIMEOUT);
        while Instant::now() < deadline {
            if let Some(found) = find(&self.output, marker, since) {
                let end = found + marker.len();
                return Ok(self.chunks.iter().find(|(_, length)| *length >= end).unwrap().0);
            }
            if !self.pump(0.05) {
                break;
            }
        }
        let tail = &self.output[self.output.len().saturating_sub(400)..];
        Err(format!(
            "timed out waiting for {:?}; got:\n{:?}",
            String::from_utf8_lossy(marker),
            String::from_utf8_lossy(tail)
        ))
    }

    /// Send a key; the milliseconds from writing it to the last byte of its frame.
    pub fn key(&mut self, data: &[u8]) -> Option<f64> {
        let before = self.output.len();
        let sent = Instant::now();
        self.write(data);
        let deadline = sent + Duration::from_secs(2);
        while self.output.len() == before && Instant::now() < deadline {
            let left = deadline.saturating_duration_since(Instant::now()).as_secs_f64();
            if !self.pump(left) {
                return None;
            }
        }
        if self.output.len() == before {
            return None;
        }
        let last = self.settle(QUIET).unwrap_or(self.chunks.last().unwrap().0);
        Some(ms(last - sent))
    }

    /// Read until it has been quiet for `quiet` seconds; when the last read came.
    pub fn settle(&mut self, quiet: f64) -> Option<Instant> {
        let mut last = None;
        loop {
            let before = self.output.len();
            if !self.pump(quiet) || self.output.len() == before {
                return last;
            }
            last = Some(self.chunks.last().unwrap().0);
        }
    }

    pub fn send(&self, data: &[u8]) -> Instant {
        let sent = Instant::now();
        self.write(data);
        sent
    }

    /// When the plugin was seen to have exited, collecting its rusage.
    pub fn wait_exit(&mut self) -> Result<Instant, String> {
        let deadline = Instant::now() + Duration::from_secs_f64(TIMEOUT);
        while Instant::now() < deadline {
            let mut status = 0;
            let mut usage: libc::rusage = unsafe { std::mem::zeroed() };
            let pid = unsafe { libc::wait4(self.pid, &mut status, libc::WNOHANG, &mut usage) };
            if pid == self.pid {
                self.status = Some(status);
                self.rusage = Some(usage);
                return Ok(Instant::now());
            }
            self.pump(0.001);
        }
        Err("plugin did not exit".into())
    }

    /// User plus system CPU time of the plugin process, in milliseconds.
    pub fn cpu_ms(&self) -> f64 {
        let Some(usage) = &self.rusage else { return 0.0 };
        // As Python's resource module turns each timeval into a float.
        let seconds = |t: libc::timeval| t.tv_sec as f64 + t.tv_usec as f64 * 0.000001;
        (seconds(usage.ru_utime) + seconds(usage.ru_stime)) * 1000.0
    }

    /// Peak resident set of the plugin process, in KB.
    #[allow(clippy::unnecessary_cast)] // c_long, i32 on 32-bit targets
    pub fn peak_kb(&self) -> i64 {
        let Some(usage) = &self.rusage else { return 0 };
        let peak = usage.ru_maxrss as i64;
        if cfg!(target_os = "macos") {
            peak / 1024 // bytes there, KB elsewhere
        } else {
            peak
        }
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        if self.status.is_none() {
            unsafe {
                libc::killpg(self.pid, libc::SIGKILL);
                let mut status = 0;
                libc::waitpid(self.pid, &mut status, 0);
            }
        }
        unsafe { libc::close(self.master) };
    }
}

/// A duration in milliseconds.
pub fn ms(duration: Duration) -> f64 {
    duration.as_secs_f64() * 1000.0
}
