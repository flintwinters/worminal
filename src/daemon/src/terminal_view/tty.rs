use std::{
    io::{self, IsTerminal, Write},
    mem::MaybeUninit,
};
use worminal_protocol::{WORKSPACE_MAX_COLS, WORKSPACE_MAX_ROWS};

pub(super) fn size() -> io::Result<(u32, u32)> {
    if !io::stdin().is_terminal() || !io::stdout().is_terminal() {
        return Err(io::Error::other(
            "worminald terminal needs a terminal on stdin and stdout (use ssh -t).",
        ));
    }
    let mut size = MaybeUninit::<libc::winsize>::zeroed();
    if unsafe { libc::ioctl(0, libc::TIOCGWINSZ, size.as_mut_ptr()) } < 0 {
        return Err(io::Error::last_os_error());
    }
    let size = unsafe { size.assume_init() };
    Ok((
        u32::from(size.ws_col).clamp(1, WORKSPACE_MAX_COLS),
        u32::from(size.ws_row).clamp(1, WORKSPACE_MAX_ROWS + super::tabs::ROWS),
    ))
}

pub(super) struct Terminal {
    pub output: io::StdoutLock<'static>,
    original: libc::termios,
}

impl Terminal {
    pub fn enter() -> io::Result<Self> {
        let mut original = MaybeUninit::<libc::termios>::uninit();
        if unsafe { libc::tcgetattr(0, original.as_mut_ptr()) } < 0 {
            return Err(io::Error::last_os_error());
        }
        let original = unsafe { original.assume_init() };
        let mut raw = original;
        unsafe { libc::cfmakeraw(&mut raw) };
        if unsafe { libc::tcsetattr(0, libc::TCSANOW, &raw) } < 0 {
            return Err(io::Error::last_os_error());
        }
        let mut terminal = Self {
            output: io::stdout().lock(),
            original,
        };
        terminal.output.write_all(
            b"\x1b[?1049h\x1b[?7l\x1b[?25l\x1b[0m\x1b[2J\x1b[?1002h\x1b[?1006h\x1b[>1u",
        )?;
        terminal.output.flush()?;
        Ok(terminal)
    }
}

impl Drop for Terminal {
    fn drop(&mut self) {
        unsafe { libc::tcsetattr(0, libc::TCSANOW, &self.original) };
        let _ = self
            .output
            .write_all(b"\x1b[<u\x1b[?1002l\x1b[?1006l\x1b[0m\x1b[?1l\x1b>\x1b[?2004l\x1b[?5l\x1b[?7h\x1b[?1049l\x1b[?25h");
        let _ = self.output.flush();
    }
}
