//! Semantic input encoding against a terminal's modes. The daemon encodes Worp `Key`, `Mouse`,
//! `Paste` and `Wheel` commands with a tab's live modes; a view encodes them with the modes it
//! displays to decide what to send, and to talk to daemons without `WIRE_FEATURE_INPUT`.
use crate::{keys, paste};
use worminal_protocol::{
    cell::*,
    input::*,
    worp::commands::{CommandKind, Control, MouseAction},
};

#[derive(Debug, PartialEq, Eq)]
pub enum Encoded {
    /// Bytes for the application, which acquire control of the tab.
    Input(Vec<u8>),
    /// History scrolling; the application sees nothing.
    Scroll { up: bool, lines: u32 },
}
impl Encoded {
    /// The equivalent byte-level command for daemons without `WIRE_FEATURE_INPUT`.
    pub fn command(self) -> CommandKind {
        match self {
            Self::Input(bytes) => CommandKind::Input {
                bytes,
                control: Control::Acquire,
            },
            Self::Scroll { up, lines } => CommandKind::Scroll { up, amount: lines },
        }
    }
}

/// Encodes input commands, including raw `Input`, for a terminal in `mode`; `alternate` reports
/// its alternate screen. `None` means the terminal takes nothing, as with a locked keyboard.
pub fn encode(kind: &CommandKind, mode: u32, alternate: bool) -> Option<Encoded> {
    let encoded = match *kind {
        CommandKind::Input { ref bytes, .. } => Encoded::Input(bytes.clone()),
        CommandKind::Key {
            keysym,
            modifiers,
            character,
            numlock,
        } => Encoded::Input(keys::encode(keysym, character, modifiers, mode, numlock)?),
        CommandKind::Mouse {
            button,
            column,
            row,
            modifiers,
            action,
        } => Encoded::Input(mouse(mode, button, column, row, modifiers, action)?),
        CommandKind::Paste {
            ref bytes,
            begin,
            end,
        } => Encoded::Input(paste::envelope(bytes, begin, end, mode)),
        CommandKind::Wheel {
            up,
            notches,
            lines,
            column,
            row,
            modifiers,
        } => wheel(
            mode,
            alternate,
            up,
            notches,
            lines,
            (column, row),
            modifiers,
        )?,
        _ => return None,
    };
    (mode & MODE_KBDLOCK == 0 || matches!(encoded, Encoded::Scroll { .. })).then_some(encoded)
}

fn wheel(
    mode: u32,
    alternate: bool,
    up: bool,
    notches: u32,
    lines: u32,
    (column, row): (u32, u32),
    modifiers: u32,
) -> Option<Encoded> {
    let notches = notches.min(MAX_WHEEL_NOTCHES) as usize;
    // Shift reserves the wheel for the terminal, as it reserves the pointer for selection.
    if mode & MODE_MOUSE != 0 && modifiers & MOD_SHIFT == 0 {
        let button = if up { MOUSE_WHEEL_UP } else { MOUSE_WHEEL_DOWN };
        let report = mouse(mode, button, column, row, modifiers, MouseAction::Press)?;
        return Some(Encoded::Input(report.repeat(notches)));
    }
    if alternate && mode & MODE_ALTSCROLL != 0 {
        let key: &[u8] = match (mode & MODE_APPCURSOR != 0, up) {
            (true, true) => b"\x1bOA",
            (true, false) => b"\x1bOB",
            (false, true) => b"\x1b[A",
            (false, false) => b"\x1b[B",
        };
        return Some(Encoded::Input(key.repeat(notches)));
    }
    Some(Encoded::Scroll { up, lines })
}

/// An xterm mouse report, or `None` when the mode requests no report for this event.
fn mouse(
    mode: u32,
    button: u32,
    column: u32,
    row: u32,
    modifiers: u32,
    action: MouseAction,
) -> Option<Vec<u8>> {
    let (release, motion) = (
        action == MouseAction::Release,
        action == MouseAction::Motion,
    );
    if mode & MODE_MOUSE == 0 || mode & MODE_MOUSEX10 != 0 && (release || motion) {
        return None;
    }
    if motion
        && mode & MODE_MOUSEMANY == 0
        && !(button != MOUSE_NONE && mode & MODE_MOUSEMOTION != 0)
    {
        return None;
    }
    let mut code = button.checked_add(if motion { 32 } else { 0 })?;
    if mode & MODE_MOUSEX10 == 0 {
        for (modifier, value) in [(MOD_SHIFT, 4), (MOD_ALT, 8), (MOD_CONTROL, 16)] {
            if modifiers & modifier != 0 {
                code = code.checked_add(value)?;
            }
        }
    }
    if mode & MODE_MOUSESGR != 0 {
        let (column, row) = (u64::from(column) + 1, u64::from(row) + 1);
        let end = if release { 'm' } else { 'M' };
        return Some(format!("\x1b[<{code};{column};{row}{end}").into_bytes());
    }
    if column >= 223 || row >= 223 || code > 223 {
        return None;
    }
    Some(vec![
        0x1b,
        b'[',
        b'M',
        if release { 35 } else { 32 + code as u8 },
        33 + column as u8,
        33 + row as u8,
    ])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn report(mode: u32, button: u32, modifiers: u32, action: MouseAction) -> Option<Vec<u8>> {
        mouse(mode, button, 4, 2, modifiers, action)
    }

    #[test]
    fn mouse_reports_follow_the_requested_protocol() {
        let sgr = MODE_MOUSEBTN | MODE_MOUSESGR;
        assert_eq!(
            report(sgr, 0, MOD_CONTROL, MouseAction::Press).unwrap(),
            b"\x1b[<16;5;3M"
        );
        assert_eq!(
            report(sgr, 0, 0, MouseAction::Release).unwrap(),
            b"\x1b[<0;5;3m"
        );
        assert_eq!(
            report(MODE_MOUSEBTN, 2, MOD_SHIFT, MouseAction::Press).unwrap(),
            b"\x1b[M&%#"
        );
        assert_eq!(
            report(MODE_MOUSEBTN, 2, 0, MouseAction::Release).unwrap(),
            b"\x1b[M#%#"
        );
        assert_eq!(
            report(MODE_MOUSEX10, 0, MOD_SHIFT, MouseAction::Press).unwrap(),
            b"\x1b[M %#"
        );
        assert_eq!(report(MODE_MOUSEX10, 0, 0, MouseAction::Release), None);
        assert_eq!(
            report(0, 0, 0, MouseAction::Press),
            None,
            "live modes own the mouse"
        );
        assert_eq!(report(MODE_MOUSEBTN, 0, 0, MouseAction::Motion), None);
        let drag = MODE_MOUSEMOTION | MODE_MOUSESGR;
        assert_eq!(
            report(drag, MOUSE_NONE, 0, MouseAction::Motion),
            None,
            "no button held"
        );
        assert_eq!(
            report(drag, 0, 0, MouseAction::Motion).unwrap(),
            b"\x1b[<32;5;3M"
        );
        let any = MODE_MOUSEMANY | MODE_MOUSESGR;
        assert_eq!(
            report(any, MOUSE_NONE, 0, MouseAction::Motion).unwrap(),
            b"\x1b[<35;5;3M"
        );
        assert_eq!(mouse(MODE_MOUSEBTN, 0, 223, 0, 0, MouseAction::Press), None);
    }

    #[test]
    fn wheel_reports_arrows_or_scrolls_history() {
        let wheel = |mode, alternate, modifiers| {
            let kind = CommandKind::Wheel {
                up: true,
                notches: 2,
                lines: 3,
                column: 0,
                row: 0,
                modifiers,
            };
            encode(&kind, mode, alternate).unwrap()
        };
        let sgr = MODE_MOUSEBTN | MODE_MOUSESGR;
        assert_eq!(
            wheel(sgr, false, 0),
            Encoded::Input(b"\x1b[<64;1;1M\x1b[<64;1;1M".to_vec())
        );
        let history = Encoded::Scroll { up: true, lines: 3 };
        assert_eq!(wheel(sgr, false, MOD_SHIFT), history);
        assert_eq!(
            wheel(MODE_ALTSCROLL, true, 0),
            Encoded::Input(b"\x1b[A\x1b[A".to_vec())
        );
        let app = MODE_ALTSCROLL | MODE_APPCURSOR;
        assert_eq!(
            wheel(app, true, 0),
            Encoded::Input(b"\x1bOA\x1bOA".to_vec())
        );
        assert_eq!(wheel(MODE_ALTSCROLL, false, 0), history);
        assert_eq!(
            wheel(MODE_ALTSCROLL | MODE_KBDLOCK, false, 0),
            history,
            "locks keep history"
        );
    }

    #[test]
    fn a_locked_keyboard_takes_no_input() {
        let key = CommandKind::Key {
            keysym: 0xff0d,
            modifiers: 0,
            character: None,
            numlock: true,
        };
        assert_eq!(encode(&key, 0, false), Some(Encoded::Input(b"\r".to_vec())));
        assert_eq!(encode(&key, MODE_KBDLOCK, false), None);
    }
}
