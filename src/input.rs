use alacritty_terminal::term::TermMode;
use winit::keyboard::{Key, KeyCode, ModifiersState, NamedKey, PhysicalKey};

/// Preserve sub-line wheel motion, but don't carry it into a reversed gesture.
#[derive(Default)]
pub struct ScrollAccumulator {
    remainder: f64,
}
impl ScrollAccumulator {
    pub fn lines(&mut self, delta: f64) -> i32 {
        if !delta.is_finite() || delta == 0.0 {
            return 0;
        }
        if self.remainder.signum() != delta.signum() {
            self.remainder = 0.0;
        }
        self.remainder += delta;
        let lines = self.remainder.trunc();
        self.remainder -= lines;
        lines as i32
    }
}

/// A click becomes a drag permanently once it crosses the logical-pixel threshold.
#[derive(Clone, Copy)]
pub struct ClickGesture {
    origin: [f64; 2],
    threshold: f64,
    dragged: bool,
}
impl ClickGesture {
    pub fn new(origin: [f64; 2], scale: f64) -> Self {
        Self {
            origin,
            threshold: 4.0 * scale,
            dragged: false,
        }
    }
    pub fn update(&mut self, position: [f64; 2]) -> bool {
        let dx = position[0] - self.origin[0];
        let dy = position[1] - self.origin[1];
        self.dragged |= dx * dx + dy * dy >= self.threshold * self.threshold;
        self.dragged
    }
}

/// With Control held, non-Latin layouts report e.g. Cyrillic letters; map the physical
/// key back to its US letter so Ctrl+C, Ctrl+D and shortcuts keep working.
pub fn latin(key: &Key, physical: PhysicalKey, mods: ModifiersState) -> Key {
    let Key::Character(value) = key else {
        return key.clone();
    };
    if !mods.control_key() || value.is_ascii() {
        return key.clone();
    }
    let PhysicalKey::Code(code) = physical else {
        return key.clone();
    };
    let letter = match code {
        KeyCode::KeyA => 'a',
        KeyCode::KeyB => 'b',
        KeyCode::KeyC => 'c',
        KeyCode::KeyD => 'd',
        KeyCode::KeyE => 'e',
        KeyCode::KeyF => 'f',
        KeyCode::KeyG => 'g',
        KeyCode::KeyH => 'h',
        KeyCode::KeyI => 'i',
        KeyCode::KeyJ => 'j',
        KeyCode::KeyK => 'k',
        KeyCode::KeyL => 'l',
        KeyCode::KeyM => 'm',
        KeyCode::KeyN => 'n',
        KeyCode::KeyO => 'o',
        KeyCode::KeyP => 'p',
        KeyCode::KeyQ => 'q',
        KeyCode::KeyR => 'r',
        KeyCode::KeyS => 's',
        KeyCode::KeyT => 't',
        KeyCode::KeyU => 'u',
        KeyCode::KeyV => 'v',
        KeyCode::KeyW => 'w',
        KeyCode::KeyX => 'x',
        KeyCode::KeyY => 'y',
        KeyCode::KeyZ => 'z',
        _ => return key.clone(),
    };
    Key::Character(letter.to_string().into())
}

/// Encode legacy xterm input. Application shortcuts are handled before this boundary.
pub fn encode(key: &Key, text: Option<&str>, mods: ModifiersState, mode: TermMode) -> Vec<u8> {
    let modifier = 1
        + u8::from(mods.shift_key())
        + 2 * u8::from(mods.alt_key())
        + 4 * u8::from(mods.control_key());
    let sequence = match key {
        Key::Named(NamedKey::Enter) => "\r".into(),
        Key::Named(NamedKey::Tab) if mods.shift_key() => "\x1b[Z".into(),
        Key::Named(NamedKey::Tab) => "\t".into(),
        Key::Named(NamedKey::Backspace) => if mods.control_key() { "\x08" } else { "\x7f" }.into(),
        Key::Named(NamedKey::Escape) => "\x1b".into(),
        Key::Named(NamedKey::Space) if mods.control_key() => "\0".into(),
        Key::Named(NamedKey::Space) => " ".into(),
        Key::Named(
            k @ (NamedKey::ArrowUp
            | NamedKey::ArrowDown
            | NamedKey::ArrowRight
            | NamedKey::ArrowLeft
            | NamedKey::Home
            | NamedKey::End),
        ) => {
            let suffix = match k {
                NamedKey::ArrowUp => 'A',
                NamedKey::ArrowDown => 'B',
                NamedKey::ArrowRight => 'C',
                NamedKey::ArrowLeft => 'D',
                NamedKey::Home => 'H',
                _ => 'F',
            };
            return if modifier != 1 {
                format!("\x1b[1;{modifier}{suffix}")
            } else if mode.contains(TermMode::APP_CURSOR) {
                format!("\x1bO{suffix}")
            } else {
                format!("\x1b[{suffix}")
            }
            .into_bytes();
        }
        Key::Named(
            k @ (NamedKey::Insert
            | NamedKey::Delete
            | NamedKey::PageUp
            | NamedKey::PageDown
            | NamedKey::F5
            | NamedKey::F6
            | NamedKey::F7
            | NamedKey::F8
            | NamedKey::F9
            | NamedKey::F10
            | NamedKey::F11
            | NamedKey::F12),
        ) => {
            let code = match k {
                NamedKey::Insert => 2,
                NamedKey::Delete => 3,
                NamedKey::PageUp => 5,
                NamedKey::PageDown => 6,
                NamedKey::F5 => 15,
                NamedKey::F6 => 17,
                NamedKey::F7 => 18,
                NamedKey::F8 => 19,
                NamedKey::F9 => 20,
                NamedKey::F10 => 21,
                NamedKey::F11 => 23,
                _ => 24,
            };
            return if modifier == 1 {
                format!("\x1b[{code}~")
            } else {
                format!("\x1b[{code};{modifier}~")
            }
            .into_bytes();
        }
        Key::Named(k @ (NamedKey::F1 | NamedKey::F2 | NamedKey::F3 | NamedKey::F4)) => {
            let suffix = match k {
                NamedKey::F1 => 'P',
                NamedKey::F2 => 'Q',
                NamedKey::F3 => 'R',
                _ => 'S',
            };
            return if modifier == 1 {
                format!("\x1bO{suffix}")
            } else {
                format!("\x1b[1;{modifier}{suffix}")
            }
            .into_bytes();
        }
        Key::Character(value) if mods.control_key() => {
            let c = value.chars().next().unwrap_or('\0');
            let byte = match c.to_ascii_lowercase() {
                'a'..='z' => c.to_ascii_lowercase() as u8 - b'a' + 1,
                ' ' | '@' | '2' => 0,
                '[' | '3' => 27,
                '\\' | '4' => 28,
                ']' | '5' => 29,
                '^' | '6' => 30,
                '_' | '7' | '/' => 31,
                '?' | '8' => 127,
                _ => return Vec::new(),
            };
            let mut bytes = vec![byte];
            if mods.alt_key() {
                bytes.insert(0, 27);
            }
            return bytes;
        }
        _ => text.unwrap_or_default().to_owned(),
    };
    if mods.alt_key() && !sequence.is_empty() {
        format!("\x1b{sequence}").into_bytes()
    } else {
        sequence.into_bytes()
    }
}

pub fn paste(text: &str, bracketed: bool) -> Vec<u8> {
    // Like xterm's default, drop C0/C1 controls other than tab and newlines. ESC can
    // never terminate bracketed paste, and pasted ^C/^D cannot act as keystrokes.
    let text: String = text
        .replace("\r\n", "\n")
        .chars()
        .filter(|c| !c.is_control() || matches!(c, '\t' | '\n' | '\r'))
        .map(|c| if c == '\n' { '\r' } else { c })
        .collect();
    if bracketed {
        format!("\x1b[200~{text}\x1b[201~").into_bytes()
    } else {
        text.into_bytes()
    }
}

pub fn mouse(
    button: u8,
    column: usize,
    row: usize,
    release: bool,
    mods: ModifiersState,
    mode: TermMode,
) -> Vec<u8> {
    let code = button
        + if mods.shift_key() { 4 } else { 0 }
        + if mods.alt_key() { 8 } else { 0 }
        + if mods.control_key() { 16 } else { 0 };
    if mode.contains(TermMode::SGR_MOUSE) {
        format!(
            "\x1b[<{code};{};{}{}",
            column + 1,
            row + 1,
            if release { 'm' } else { 'M' }
        )
        .into_bytes()
    } else if column < 223 && row < 223 {
        vec![
            27,
            b'[',
            b'M',
            if release { 3 + 32 } else { code + 32 },
            column as u8 + 33,
            row as u8 + 33,
        ]
    } else {
        Vec::new()
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn click_gesture_scales_and_cannot_revert_from_drag_to_click() {
        for scale in [1.0, 1.5, 2.0] {
            let mut gesture = super::ClickGesture::new([10.0, 20.0], scale);
            assert!(!gesture.update([10.0 + 3.0 * scale, 20.0]));
            assert!(gesture.update([10.0 + 4.0 * scale, 20.0]));
            assert!(gesture.update([10.0, 20.0]));
            let mut diagonal = super::ClickGesture::new([0.0, 0.0], scale);
            assert!(diagonal.update([3.0 * scale, 3.0 * scale]));
        }
    }

    #[test]
    fn fractional_wheel_motion_accumulates_in_both_directions() {
        for direction in [-1.0, 1.0] {
            let mut scroll = super::ScrollAccumulator::default();
            let steps: Vec<_> = (0..8).map(|_| scroll.lines(direction * 0.25)).collect();
            assert_eq!(
                steps,
                vec![0, 0, 0, direction as i32, 0, 0, 0, direction as i32]
            );
            assert_eq!(scroll.lines(direction * 3.0), direction as i32 * 3);
        }
    }

    #[test]
    fn reversing_wheel_direction_discards_the_previous_fraction() {
        let mut scroll = super::ScrollAccumulator::default();
        assert_eq!(scroll.lines(0.75), 0);
        assert_eq!(scroll.lines(0.0), 0);
        assert_eq!(scroll.lines(-0.5), 0);
        assert_eq!(scroll.lines(-0.5), -1);
        assert_eq!(scroll.lines(2.5), 2);
        assert_eq!(scroll.lines(f64::NAN), 0);
        assert_eq!(scroll.lines(f64::INFINITY), 0);
        assert_eq!(scroll.lines(0.5), 1);
    }

    use super::*;
    #[test]
    fn cursor_modes_modifiers_and_control_keys() {
        let key = Key::Named(NamedKey::ArrowUp);
        assert_eq!(
            encode(&key, None, ModifiersState::empty(), TermMode::empty()),
            b"\x1b[A"
        );
        assert_eq!(
            encode(&key, None, ModifiersState::empty(), TermMode::APP_CURSOR),
            b"\x1bOA"
        );
        assert_eq!(
            encode(&key, None, ModifiersState::CONTROL, TermMode::empty()),
            b"\x1b[1;5A"
        );
        assert_eq!(
            encode(
                &Key::Character("c".into()),
                None,
                ModifiersState::CONTROL,
                TermMode::empty()
            ),
            [3]
        );
        let space = Key::Named(NamedKey::Space);
        assert_eq!(
            encode(
                &space,
                Some(" "),
                ModifiersState::CONTROL,
                TermMode::empty()
            ),
            [0]
        );
        assert_eq!(
            encode(&space, Some(" "), ModifiersState::ALT, TermMode::empty()),
            b"\x1b "
        );
    }
    #[test]
    fn control_keys_survive_non_latin_layouts() {
        let cyrillic = Key::Character("с".into());
        let physical = PhysicalKey::Code(KeyCode::KeyC);
        let key = latin(&cyrillic, physical, ModifiersState::CONTROL);
        assert_eq!(key, Key::Character("c".into()));
        assert_eq!(
            encode(&key, None, ModifiersState::CONTROL, TermMode::empty()),
            [3]
        );
        assert_eq!(
            latin(&cyrillic, physical, ModifiersState::empty()),
            cyrillic
        );
    }
    #[test]
    fn bracketed_paste_cannot_escape_its_envelope() {
        assert_eq!(
            paste("one\r\ntwo\x1b[201~", true),
            b"\x1b[200~one\rtwo[201~\x1b[201~"
        );
        assert_eq!(paste("a\x03b\x04\tc\u{9b}d\x7f", false), b"ab\tcd");
        assert_eq!(
            mouse(0, 2, 3, true, ModifiersState::empty(), TermMode::SGR_MOUSE),
            b"\x1b[<0;3;4m"
        );
    }
}
