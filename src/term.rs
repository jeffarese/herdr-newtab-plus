//! Raw terminal plumbing: alt screen, raw mode, size, input reads.
//!
//! `Terminal` owns raw mode and the alternate screen while it lives, reports
//! its size, says when input is waiting and hands over the raw bytes.
//! `detect_appearance` asks for the background color before any of that
//! starts. POSIX only, as the plugin is: Herdr hands it a pty.

use crate::sequences::{appearance_from_reply, enter_sequence, exit_sequence, reply_complete, APPEARANCE_QUERY};

/// shutil.get_terminal_size's environment override and fallback, for when
/// the terminal itself will not say.
fn fallback_size(query: impl Fn() -> Option<(i64, i64)>) -> (i64, i64) {
    let env = |name: &str| std::env::var(name).ok().and_then(|v| crate::pystr::parse_int(&v)).map(|v| v as i64).unwrap_or(0);
    let mut columns = env("COLUMNS");
    let mut lines = env("LINES");
    if columns <= 0 || lines <= 0 {
        let (c, l) = query().unwrap_or((80, 24));
        if columns <= 0 {
            columns = if c != 0 { c } else { 80 };
        }
        if lines <= 0 {
            lines = if l != 0 { l } else { 24 };
        }
    }
    (columns, lines)
}

#[cfg(unix)]
mod backend {
    use super::*;
    use std::sync::atomic::{AtomicBool, Ordering};

    static RESIZED: AtomicBool = AtomicBool::new(false);

    extern "C" fn on_winch(_signal: libc::c_int) {
        RESIZED.store(true, Ordering::SeqCst);
    }

    pub fn isatty(fd: i32) -> bool {
        unsafe { libc::isatty(fd) == 1 }
    }

    fn tcgetattr(fd: i32) -> Option<libc::termios> {
        let mut mode: libc::termios = unsafe { std::mem::zeroed() };
        if unsafe { libc::tcgetattr(fd, &mut mode) } != 0 {
            return None;
        }
        Some(mode)
    }

    /// tty.setraw as Python 3.12+ spells it, applied with TCSAFLUSH.
    fn setraw(fd: i32, mut mode: libc::termios) -> bool {
        mode.c_iflag &= !(libc::IGNBRK
            | libc::BRKINT
            | libc::IGNPAR
            | libc::PARMRK
            | libc::INPCK
            | libc::ISTRIP
            | libc::INLCR
            | libc::IGNCR
            | libc::ICRNL
            | libc::IXON
            | libc::IXANY
            | libc::IXOFF);
        mode.c_oflag &= !libc::OPOST;
        mode.c_cflag &= !(libc::PARENB | libc::CSIZE);
        mode.c_cflag |= libc::CS8;
        mode.c_lflag &= !(libc::ECHO
            | libc::ECHOE
            | libc::ECHOK
            | libc::ECHONL
            | libc::ICANON
            | libc::IEXTEN
            | libc::ISIG
            | libc::NOFLSH
            | libc::TOSTOP);
        mode.c_cc[libc::VMIN] = 1;
        mode.c_cc[libc::VTIME] = 0;
        unsafe { libc::tcsetattr(fd, libc::TCSAFLUSH, &mode) == 0 }
    }

    pub fn write_all(fd: i32, mut data: &[u8]) -> std::io::Result<()> {
        while !data.is_empty() {
            let written = unsafe { libc::write(fd, data.as_ptr() as *const libc::c_void, data.len()) };
            if written < 0 {
                let error = std::io::Error::last_os_error();
                if error.kind() == std::io::ErrorKind::Interrupted {
                    continue;
                }
                return Err(error);
            }
            data = &data[written as usize..];
        }
        Ok(())
    }

    /// Wait for `fd` to be readable; false on timeout or a signal.
    fn readable(fd: i32, timeout: f64) -> bool {
        let mut poll = libc::pollfd { fd, events: libc::POLLIN, revents: 0 };
        let ms = (timeout.max(0.0) * 1000.0).ceil().min(i32::MAX as f64) as i32;
        let ready = unsafe { libc::poll(&mut poll, 1, ms) };
        ready > 0
    }

    fn read_fd(fd: i32, size: usize) -> Vec<u8> {
        let mut buffer = vec![0u8; size];
        loop {
            let count = unsafe { libc::read(fd, buffer.as_mut_ptr() as *mut libc::c_void, size) };
            if count < 0 {
                if std::io::Error::last_os_error().kind() == std::io::ErrorKind::Interrupted {
                    continue;
                }
                return Vec::new();
            }
            buffer.truncate(count as usize);
            return buffer;
        }
    }

    fn winsize(fd: i32) -> Option<(i64, i64)> {
        let mut size: libc::winsize = unsafe { std::mem::zeroed() };
        if unsafe { libc::ioctl(fd, libc::TIOCGWINSZ, &mut size) } != 0 {
            return None;
        }
        Some((size.ws_col as i64, size.ws_row as i64))
    }

    pub struct Terminal {
        in_fd: i32,
        mouse: bool,
        saved: Option<libc::termios>,
        previous_winch: Option<libc::sighandler_t>,
        out: Vec<u8>,
    }

    impl Terminal {
        pub fn enter(mouse: bool) -> std::io::Result<Terminal> {
            let in_fd = 0;
            let saved = tcgetattr(in_fd).ok_or_else(std::io::Error::last_os_error)?;
            let mut terminal = Terminal { in_fd, mouse, saved: Some(saved), previous_winch: None, out: Vec::new() };
            if !setraw(in_fd, saved) {
                return Err(std::io::Error::last_os_error());
            }
            terminal.write(&enter_sequence(mouse));
            terminal.flush();
            let handler = on_winch as extern "C" fn(libc::c_int) as libc::sighandler_t;
            let previous = unsafe { libc::signal(libc::SIGWINCH, handler) };
            if previous != libc::SIG_ERR {
                terminal.previous_winch = Some(previous);
            }
            Ok(terminal)
        }

        pub fn take_resize(&mut self) -> bool {
            RESIZED.swap(false, Ordering::SeqCst)
        }

        pub fn size(&self) -> (i64, i64) {
            let (columns, rows) = winsize(self.in_fd).unwrap_or_else(|| fallback_size(|| winsize(1)));
            (columns.max(1), rows.max(1))
        }

        pub fn write(&mut self, text: &str) {
            self.out.extend_from_slice(text.as_bytes());
        }

        pub fn flush(&mut self) {
            let data = std::mem::take(&mut self.out);
            let _ = write_all(1, &data);
        }

        /// Whether input is waiting, within `timeout` seconds. A resize
        /// interrupts the wait, so the new size is drawn at once.
        pub fn wait(&mut self, timeout: f64) -> bool {
            readable(self.in_fd, timeout)
        }

        pub fn read(&mut self) -> Vec<u8> {
            read_fd(self.in_fd, 4096)
        }
    }

    impl Drop for Terminal {
        fn drop(&mut self) {
            self.write(&exit_sequence(self.mouse));
            self.flush();
            if let Some(saved) = self.saved {
                unsafe {
                    libc::tcsetattr(self.in_fd, libc::TCSADRAIN, &saved);
                }
            }
            if let Some(previous) = self.previous_winch {
                unsafe {
                    libc::signal(libc::SIGWINCH, previous);
                }
            }
        }
    }

    /// Ask the terminal for its background color via OSC 11.
    ///
    /// "dark", "light", or None when the terminal stays quiet. Must run
    /// before raw-mode input handling starts so the reply cannot be mistaken
    /// for typing.
    pub fn detect_appearance(timeout: f64) -> Option<&'static str> {
        if !isatty(0) || !isatty(1) {
            return None;
        }
        let fd = 0;
        let saved = tcgetattr(fd)?;
        let mut buffer: Vec<u8> = Vec::new();
        let ok = (|| {
            if !setraw(fd, saved) {
                return false;
            }
            if write_all(1, APPEARANCE_QUERY.as_bytes()).is_err() {
                return false;
            }
            let mut parts = 0;
            while parts < 4 {
                if !readable(fd, timeout) {
                    break;
                }
                buffer.extend_from_slice(&read_fd(fd, 256));
                if reply_complete(&buffer) {
                    break;
                }
                parts += 1;
            }
            true
        })();
        unsafe {
            libc::tcsetattr(fd, libc::TCSANOW, &saved);
        }
        if !ok {
            return None;
        }
        appearance_from_reply(&buffer)
    }

    pub fn restore_console() {}
}

pub use backend::{detect_appearance, isatty, restore_console, Terminal};
