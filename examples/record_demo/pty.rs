//! The plugin on a pseudo-terminal, as `tests/parity/machine.rs` runs it,
//! with a reader thread that keeps a screen model current in real time.

use crate::screen::Screen;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::os::unix::process::CommandExt;
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};

/// The background colour query the picker sends on opening (src/term.rs).
const QUERY: &[u8] = b"\x1b]11;?";
/// The answer: the panel's own colour, so the picker picks its dark palette
/// for exactly the background it is filmed on.
const REPLY: &[u8] = b"\x1b]11;rgb:1414/1616/1d1d\x1b\\";

pub struct Session {
    master: Arc<OwnedFd>,
    child: Child,
}

fn cloexec(fd: i32) {
    unsafe {
        let flags = libc::fcntl(fd, libc::F_GETFD);
        libc::fcntl(fd, libc::F_SETFD, flags | libc::FD_CLOEXEC);
    }
}

fn write_all(fd: i32, data: &[u8]) {
    let mut rest = data;
    while !rest.is_empty() {
        let n = unsafe { libc::write(fd, rest.as_ptr().cast(), rest.len()) };
        if n <= 0 {
            return;
        }
        rest = &rest[n as usize..];
    }
}

/// How much of `pending` can go to the screen model now: everything but a
/// trailing escape sequence or UTF-8 character that has not fully arrived.
fn complete_prefix(pending: &[u8]) -> usize {
    let mut cut = pending.len();
    if let Some(esc) = pending.iter().rposition(|b| *b == 0x1b) {
        let rest = &pending[esc + 1..];
        let done = match rest.first() {
            None => false,
            Some(b'[') => rest[1..].iter().any(|b| (0x40..=0x7e).contains(b)),
            Some(b']') => rest.contains(&0x07) || rest.windows(2).any(|w| w == b"\x1b\\"),
            Some(_) => true,
        };
        if !done {
            cut = esc;
        }
    }
    match std::str::from_utf8(&pending[..cut]) {
        Err(error) if error.error_len().is_none() => error.valid_up_to(),
        _ => cut,
    }
}

impl Session {
    pub fn spawn(mut command: Command, width: u16, height: u16, screen: Arc<Mutex<Screen>>) -> Session {
        let (mut master, mut slave) = (0, 0);
        let size = libc::winsize { ws_row: height, ws_col: width, ws_xpixel: 0, ws_ypixel: 0 };
        let rc = unsafe {
            libc::openpty(&mut master, &mut slave, std::ptr::null_mut(), std::ptr::null_mut(), std::ptr::null_mut())
        };
        assert_eq!(rc, 0, "openpty: {}", std::io::Error::last_os_error());
        cloexec(master);
        cloexec(slave);
        let (master, slave) = unsafe { (OwnedFd::from_raw_fd(master), OwnedFd::from_raw_fd(slave)) };
        unsafe { libc::ioctl(slave.as_raw_fd(), libc::TIOCSWINSZ, &size) };
        let io = |fd: &OwnedFd| Stdio::from(fd.try_clone().unwrap());
        command.stdin(io(&slave)).stdout(io(&slave)).stderr(io(&slave));
        // A session of its own with the pty as its controlling terminal, as
        // Herdr gives a popup.
        unsafe {
            command.pre_exec(|| {
                if libc::setsid() < 0 {
                    return Err(std::io::Error::last_os_error());
                }
                libc::ioctl(0, libc::TIOCSCTTY as _, 0);
                Ok(())
            });
        }
        let child = command.spawn().expect("spawn herdr-newtab-plus");
        drop(command);
        drop(slave);
        let master = Arc::new(master);
        {
            let master = master.clone();
            std::thread::spawn(move || read_loop(&master, &screen));
        }
        Session { master, child }
    }

    pub fn send(&self, data: &[u8]) {
        write_all(self.master.as_raw_fd(), data);
    }

    /// Whether the plugin has exited by itself, and how.
    pub fn exited(&mut self) -> Option<std::process::ExitStatus> {
        self.child.try_wait().ok().flatten()
    }
}

/// Until the slave side is gone: the plugin exited.
fn read_loop(master: &OwnedFd, screen: &Mutex<Screen>) {
    let mut pending: Vec<u8> = Vec::new();
    // How much of `pending` has been searched for the query already, so a
    // query that arrives in pieces is still answered once.
    let mut answered = 0;
    let mut buffer = vec![0u8; 65536];
    loop {
        let n = unsafe { libc::read(master.as_raw_fd(), buffer.as_mut_ptr().cast(), buffer.len()) };
        if n < 0 && std::io::Error::last_os_error().raw_os_error() == Some(libc::EINTR) {
            continue;
        }
        if n <= 0 {
            return;
        }
        pending.extend_from_slice(&buffer[..n as usize]);
        // Be the terminal: answer the background colour query.
        while let Some(found) = pending[answered..].windows(QUERY.len()).position(|w| w == QUERY) {
            answered += found + QUERY.len();
            write_all(master.as_raw_fd(), REPLY);
        }
        let cut = complete_prefix(&pending);
        if cut > 0 {
            let text = String::from_utf8_lossy(&pending[..cut]).into_owned();
            screen.lock().unwrap().feed(&text);
            pending.drain(..cut);
            answered = answered.saturating_sub(cut);
        }
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        if let Ok(None) = self.child.try_wait() {
            unsafe { libc::killpg(self.child.id() as i32, libc::SIGKILL) };
            let _ = self.child.kill();
        }
        let _ = self.child.wait();
    }
}
