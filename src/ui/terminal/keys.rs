//! Keystrokes, wheel and mouse → bytes for the PTY (xterm conventions).
use alacritty_terminal::term::TermMode;
use gpui::Keystroke;

/// ⌘← / ⌘→ / ⌘⌫ as the line editing chords readline, zsh and the agent CLIs understand.
pub fn platform_bytes(ks: &Keystroke) -> Option<&'static [u8]> {
    let m = &ks.modifiers;
    if !m.platform || m.control || m.alt || m.shift {
        return None;
    }
    match ks.key.as_str() {
        "left" => Some(b"\x01"),
        "right" => Some(b"\x05"),
        "backspace" => Some(b"\x15"),
        _ => None,
    }
}

fn cursor(app: bool, normal: &'static [u8], application: &'static [u8]) -> Vec<u8> {
    if app { application.to_vec() } else { normal.to_vec() }
}

pub fn to_bytes(ks: &Keystroke, mode: TermMode) -> Option<Vec<u8>> {
    let m = &ks.modifiers;
    if m.platform {
        return None;
    }
    let (ctrl, alt, shift) = (m.control, m.alt, m.shift);
    let app = mode.contains(TermMode::APP_CURSOR);
    let key = ks.key.as_str();
    let plain = !ctrl && !alt && !shift;
    let named: Option<Vec<u8>> = match key {
        "enter" if alt => Some(b"\x1b\r".to_vec()),
        "enter" if shift => Some(b"\n".to_vec()),
        "enter" => Some(b"\r".to_vec()),
        "tab" if shift => Some(b"\x1b[Z".to_vec()),
        "tab" => Some(b"\t".to_vec()),
        "escape" => Some(b"\x1b".to_vec()),
        "backspace" if ctrl => Some(b"\x08".to_vec()),
        "backspace" if alt => Some(b"\x1b\x7f".to_vec()),
        "backspace" => Some(b"\x7f".to_vec()),
        "space" if ctrl => Some(vec![0]),
        "left" if alt && !ctrl && !shift => Some(b"\x1bb".to_vec()),
        "right" if alt && !ctrl && !shift => Some(b"\x1bf".to_vec()),
        "up" if plain => Some(cursor(app, b"\x1b[A", b"\x1bOA")),
        "down" if plain => Some(cursor(app, b"\x1b[B", b"\x1bOB")),
        "right" if plain => Some(cursor(app, b"\x1b[C", b"\x1bOC")),
        "left" if plain => Some(cursor(app, b"\x1b[D", b"\x1bOD")),
        "home" if plain => Some(cursor(app, b"\x1b[H", b"\x1bOH")),
        "end" if plain => Some(cursor(app, b"\x1b[F", b"\x1bOF")),
        "insert" if plain => Some(b"\x1b[2~".to_vec()),
        "delete" if plain => Some(b"\x1b[3~".to_vec()),
        "pageup" if plain => Some(b"\x1b[5~".to_vec()),
        "pagedown" if plain => Some(b"\x1b[6~".to_vec()),
        _ => None,
    };
    if named.is_some() {
        return named;
    }
    if !plain {
        let code = 1 + shift as u8 + 2 * alt as u8 + 4 * ctrl as u8;
        let tail = match key {
            "up" => Some("A"),
            "down" => Some("B"),
            "right" => Some("C"),
            "left" => Some("D"),
            "home" => Some("H"),
            "end" => Some("F"),
            _ => None,
        };
        if let Some(t) = tail {
            return Some(format!("\x1b[1;{code}{t}").into_bytes());
        }
        let tilde = match key {
            "delete" => Some(3),
            "pageup" => Some(5),
            "pagedown" => Some(6),
            _ => None,
        };
        if let Some(n) = tilde {
            return Some(format!("\x1b[{n};{code}~").into_bytes());
        }
    }
    if let Some(n) = key.strip_prefix('f').and_then(|n| n.parse::<u8>().ok()).filter(|_| plain) {
        let seq: &[u8] = match n {
            1 => b"\x1bOP",
            2 => b"\x1bOQ",
            3 => b"\x1bOR",
            4 => b"\x1bOS",
            5 => b"\x1b[15~",
            6 => b"\x1b[17~",
            7 => b"\x1b[18~",
            8 => b"\x1b[19~",
            9 => b"\x1b[20~",
            10 => b"\x1b[21~",
            11 => b"\x1b[23~",
            12 => b"\x1b[24~",
            _ => return None,
        };
        return Some(seq.to_vec());
    }
    if ctrl && !alt {
        let c = key.chars().next()?;
        if key.chars().count() == 1 {
            let b = match c.to_ascii_lowercase() {
                c @ 'a'..='z' => Some(c as u8 - b'a' + 1),
                '@' | '2' => Some(0),
                '[' => Some(0x1b),
                '\\' => Some(0x1c),
                ']' => Some(0x1d),
                '^' | '6' => Some(0x1e),
                '_' | '-' => Some(0x1f),
                '?' => Some(0x7f),
                _ => None,
            };
            if let Some(b) = b {
                return Some(vec![b]);
            }
        }
    }
    // printable text; on macOS option-x composes a character into key_char
    let text = ks.key_char.clone().or_else(|| (key.chars().count() == 1 && !ctrl).then(|| key.to_string()))?;
    if alt && ks.key_char.is_none() {
        let mut v = vec![0x1b];
        v.extend_from_slice(text.as_bytes());
        return Some(v);
    }
    Some(text.into_bytes())
}

/// The wheel: mouse reports when the child tracks the mouse, arrows on the alternate screen
/// (the agent TUIs), else `None` (scroll our own scrollback). Shift always means scrollback.
pub fn wheel_bytes(mode: TermMode, lines: i32, col: usize, row: usize, shift: bool) -> Option<Vec<u8>> {
    if lines == 0 || shift {
        return None;
    }
    let up = lines > 0;
    let n = lines.unsigned_abs() as usize;
    if mode.intersects(TermMode::MOUSE_MODE) {
        let button: u8 = if up { 64 } else { 65 };
        let one = if mode.contains(TermMode::SGR_MOUSE) {
            format!("\x1b[<{button};{};{}M", col + 1, row + 1).into_bytes()
        } else {
            vec![0x1b, b'[', b'M', 32 + button, 33 + col.min(222) as u8, 33 + row.min(222) as u8]
        };
        return Some(one.repeat(n));
    }
    if mode.contains(TermMode::ALT_SCREEN | TermMode::ALTERNATE_SCROLL) {
        let app = mode.contains(TermMode::APP_CURSOR);
        let k: &[u8] = match (up, app) {
            (true, false) => b"\x1b[A",
            (true, true) => b"\x1bOA",
            (false, false) => b"\x1b[B",
            (false, true) => b"\x1bOB",
        };
        return Some(k.repeat(n));
    }
    None
}

/// Whole lines to scroll for this event, carrying the fraction into the next one.
pub fn scroll_lines(remainder: &mut f32, delta_lines: f32) -> i32 {
    if (*remainder > 0.0 && delta_lines < 0.0) || (*remainder < 0.0 && delta_lines > 0.0) {
        *remainder = 0.0;
    }
    *remainder += delta_lines;
    let n = remainder.trunc();
    *remainder -= n;
    n as i32
}

#[cfg(test)]
mod tests {
    use super::{platform_bytes, scroll_lines, to_bytes, wheel_bytes};
    use alacritty_terminal::term::TermMode;
    use gpui::{Keystroke, Modifiers};

    fn ks(key: &str, ch: Option<&str>, m: Modifiers) -> Keystroke {
        Keystroke { modifiers: m, key: key.into(), key_char: ch.map(String::from) }
    }

    #[test]
    fn keys() {
        let none = Modifiers::default();
        let e = TermMode::empty();
        assert_eq!(to_bytes(&ks("a", Some("a"), none), e).unwrap(), b"a");
        assert_eq!(to_bytes(&ks("enter", None, none), e).unwrap(), b"\r");
        assert_eq!(to_bytes(&ks("up", None, none), TermMode::APP_CURSOR).unwrap(), b"\x1bOA");
        let ctrl = Modifiers { control: true, ..Default::default() };
        assert_eq!(to_bytes(&ks("c", None, ctrl), e).unwrap(), b"\x03");
        let shift = Modifiers { shift: true, ..Default::default() };
        assert_eq!(to_bytes(&ks("up", None, shift), e).unwrap(), b"\x1b[1;2A");
        assert_eq!(to_bytes(&ks("enter", None, shift), e).unwrap(), b"\n");
        let cmd = Modifiers { platform: true, ..Default::default() };
        assert!(to_bytes(&ks("v", None, cmd), e).is_none());
        assert_eq!(platform_bytes(&ks("left", None, cmd)), Some(&b"\x01"[..]));
        assert_eq!(to_bytes(&ks("f5", None, none), e).unwrap(), b"\x1b[15~");
    }

    #[test]
    fn wheel() {
        assert_eq!(wheel_bytes(TermMode::empty(), 3, 0, 0, false), None);
        assert_eq!(wheel_bytes(TermMode::ALT_SCREEN | TermMode::ALTERNATE_SCROLL, 2, 0, 0, false).unwrap(), b"\x1b[A\x1b[A");
        let sgr = TermMode::MOUSE_MOTION | TermMode::SGR_MOUSE;
        assert_eq!(wheel_bytes(sgr, -1, 4, 9, false).unwrap(), b"\x1b[<65;5;10M");
        assert_eq!(wheel_bytes(sgr, 1, 0, 0, true), None);
        let mut rem = 0.0;
        let total: i32 = (0..12).map(|_| scroll_lines(&mut rem, 0.2)).sum();
        assert_eq!(total, 2);
    }
}
