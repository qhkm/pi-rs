use bitflags::bitflags;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeyEvent {
    pub key: Key,
    pub modifiers: Modifiers,
    pub event_type: KeyEventType,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Key {
    Char(char),
    Enter,
    Escape,
    Tab,
    Backspace,
    Delete,
    Up,
    Down,
    Left,
    Right,
    Home,
    End,
    PageUp,
    PageDown,
    F(u8),
}

bitflags! {
    #[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
    pub struct Modifiers: u8 {
        const SHIFT = 0b0001;
        const ALT   = 0b0010;
        const CTRL  = 0b0100;
        const SUPER = 0b1000;
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KeyEventType {
    Press,
    Release,
    Repeat,
}

/// Parse a raw input string into key events.
///
/// Handles:
/// - Regular printable characters
/// - Common control characters (Ctrl+A, etc.)
/// - Kitty protocol CSI sequences: CSI codepoint ; modifiers [u|~]
/// - Legacy xterm sequences for arrows, home, end, etc.
/// - Bracketed paste (ignored — consumers should handle \x1b[200~ / \x1b[201~ themselves)
pub fn parse_input(data: &str) -> Vec<KeyEvent> {
    let mut events = Vec::new();
    let mut chars = data.char_indices().peekable();

    while let Some((i, ch)) = chars.next() {
        if ch == '\x1b' {
            // ESC — start of escape sequence
            let rest = &data[i..];

            if let Some(ev) = parse_escape_sequence(rest) {
                let seq_len = ev.1;
                // Advance iterator past the consumed bytes
                let mut consumed = 1usize; // the ESC itself
                while consumed < seq_len {
                    if let Some((_, _)) = chars.next() {
                        consumed += 1;
                    } else {
                        break;
                    }
                }
                events.push(ev.0);
            } else {
                // Bare ESC
                events.push(KeyEvent {
                    key: Key::Escape,
                    modifiers: Modifiers::empty(),
                    event_type: KeyEventType::Press,
                });
            }
        } else {
            // Control characters
            match ch {
                '\x01' => events.push(ctrl_key('a')),
                '\x02' => events.push(ctrl_key('b')),
                '\x03' => events.push(ctrl_key('c')),
                '\x04' => events.push(ctrl_key('d')),
                '\x05' => events.push(ctrl_key('e')),
                '\x06' => events.push(ctrl_key('f')),
                '\x07' => events.push(ctrl_key('g')),
                '\x08' => events.push(ctrl_key('h')),
                '\x09' => events.push(KeyEvent {
                    key: Key::Tab,
                    modifiers: Modifiers::empty(),
                    event_type: KeyEventType::Press,
                }),
                '\x0A' | '\x0D' => events.push(KeyEvent {
                    key: Key::Enter,
                    modifiers: Modifiers::empty(),
                    event_type: KeyEventType::Press,
                }),
                '\x0B' => events.push(ctrl_key('k')),
                '\x0C' => events.push(ctrl_key('l')),
                '\x0E' => events.push(ctrl_key('n')),
                '\x0F' => events.push(ctrl_key('o')),
                '\x10' => events.push(ctrl_key('p')),
                '\x11' => events.push(ctrl_key('q')),
                '\x12' => events.push(ctrl_key('r')),
                '\x13' => events.push(ctrl_key('s')),
                '\x14' => events.push(ctrl_key('t')),
                '\x15' => events.push(ctrl_key('u')),
                '\x16' => events.push(ctrl_key('v')),
                '\x17' => events.push(ctrl_key('w')),
                '\x18' => events.push(ctrl_key('x')),
                '\x19' => events.push(ctrl_key('y')),
                '\x1A' => events.push(ctrl_key('z')),
                '\x7F' => events.push(KeyEvent {
                    key: Key::Backspace,
                    modifiers: Modifiers::empty(),
                    event_type: KeyEventType::Press,
                }),
                _ if !ch.is_control() => {
                    events.push(KeyEvent {
                        key: Key::Char(ch),
                        modifiers: Modifiers::empty(),
                        event_type: KeyEventType::Press,
                    });
                }
                _ => {}
            }
        }
    }

    events
}

fn ctrl_key(ch: char) -> KeyEvent {
    KeyEvent {
        key: Key::Char(ch),
        modifiers: Modifiers::CTRL,
        event_type: KeyEventType::Press,
    }
}

/// Try to parse an escape sequence starting at the beginning of `s`.
/// Returns `Some((event, bytes_consumed))` on success.
fn parse_escape_sequence(s: &str) -> Option<(KeyEvent, usize)> {
    let bytes = s.as_bytes();
    if bytes.len() < 2 {
        return None;
    }

    match bytes[1] {
        b'[' => parse_csi(&s[2..]).map(|(ev, len)| (ev, 2 + len)),
        b'O' => parse_ss3(&s[2..]).map(|(ev, len)| (ev, 2 + len)),
        // Alt + key: ESC <char>
        b => {
            let ch = b as char;
            if !ch.is_control() {
                Some((
                    KeyEvent {
                        key: Key::Char(ch),
                        modifiers: Modifiers::ALT,
                        event_type: KeyEventType::Press,
                    },
                    2,
                ))
            } else {
                None
            }
        }
    }
}

/// Parse CSI sequences (ESC [ ...)
fn parse_csi(s: &str) -> Option<(KeyEvent, usize)> {
    // Find the terminating byte (A-Z, a-z, ~, u)
    let bytes = s.as_bytes();
    let mut end = 0;
    while end < bytes.len() {
        let b = bytes[end];
        if b.is_ascii_alphabetic() || b == b'~' {
            break;
        }
        end += 1;
    }
    if end >= bytes.len() {
        return None;
    }

    let params_str = &s[..end];
    let terminator = bytes[end] as char;
    let total_len = end + 1;

    // Kitty protocol: CSI codepoint ; modifiers ; event_type u
    // Legacy xterm:   CSI num ~ or CSI letter
    match terminator {
        'u' => {
            // Kitty unicode key: CSI codepoint [; modifiers [; event_type]] u
            let parts: Vec<&str> = params_str.split(';').collect();
            let codepoint: u32 = parts.first().and_then(|p| p.parse().ok())?;
            let mods_raw: u8 = parts
                .get(1)
                .and_then(|p| p.parse::<u8>().ok())
                .unwrap_or(1)
                .saturating_sub(1);
            let ev_type_raw: u8 = parts.get(2).and_then(|p| p.parse().ok()).unwrap_or(1);

            let modifiers = decode_kitty_mods(mods_raw);
            let event_type = match ev_type_raw {
                1 => KeyEventType::Press,
                2 => KeyEventType::Repeat,
                3 => KeyEventType::Release,
                _ => KeyEventType::Press,
            };

            let key = match codepoint {
                13 => Key::Enter,
                27 => Key::Escape,
                9 => Key::Tab,
                127 => Key::Backspace,
                57358 => Key::Backspace, // Kitty extended backspace
                57359 => Key::Delete,
                57351 => Key::Up,
                57352 => Key::Down,
                57353 => Key::Left,
                57354 => Key::Right,
                57360 => Key::Home,
                57361 => Key::End,
                57362 => Key::PageUp,
                57363 => Key::PageDown,
                57364..=57373 => Key::F((codepoint - 57363) as u8),
                cp => {
                    if let Some(ch) = char::from_u32(cp) {
                        Key::Char(ch)
                    } else {
                        return None;
                    }
                }
            };

            Some((
                KeyEvent {
                    key,
                    modifiers,
                    event_type,
                },
                total_len,
            ))
        }
        '~' => {
            // Legacy tilde sequences: CSI num [; mods] ~
            let parts: Vec<&str> = params_str.split(';').collect();
            let num: u32 = parts.first().and_then(|p| p.parse().ok())?;
            let mods_raw: u8 = parts
                .get(1)
                .and_then(|p| p.parse::<u8>().ok())
                .unwrap_or(1)
                .saturating_sub(1);
            let modifiers = decode_kitty_mods(mods_raw);

            let key = match num {
                1 => Key::Home,
                2 => Key::Delete, // Insert — map to Delete for simplicity
                3 => Key::Delete,
                4 => Key::End,
                5 => Key::PageUp,
                6 => Key::PageDown,
                7 => Key::Home,
                8 => Key::End,
                11 => Key::F(1),
                12 => Key::F(2),
                13 => Key::F(3),
                14 => Key::F(4),
                15 => Key::F(5),
                17 => Key::F(6),
                18 => Key::F(7),
                19 => Key::F(8),
                20 => Key::F(9),
                21 => Key::F(10),
                23 => Key::F(11),
                24 => Key::F(12),
                200 => return None, // Bracketed paste start — caller handles
                201 => return None, // Bracketed paste end
                _ => return None,
            };

            Some((
                KeyEvent {
                    key,
                    modifiers,
                    event_type: KeyEventType::Press,
                },
                total_len,
            ))
        }
        'A' => Some((arrow(Key::Up, params_str), total_len)),
        'B' => Some((arrow(Key::Down, params_str), total_len)),
        'C' => Some((arrow(Key::Right, params_str), total_len)),
        'D' => Some((arrow(Key::Left, params_str), total_len)),
        'H' => Some((arrow(Key::Home, params_str), total_len)),
        'F' => Some((arrow(Key::End, params_str), total_len)),
        'Z' => Some((
            KeyEvent {
                key: Key::Tab,
                modifiers: Modifiers::SHIFT,
                event_type: KeyEventType::Press,
            },
            total_len,
        )),
        'P' => Some((simple_key(Key::F(1)), total_len)),
        'Q' => Some((simple_key(Key::F(2)), total_len)),
        'R' => Some((simple_key(Key::F(3)), total_len)),
        'S' => Some((simple_key(Key::F(4)), total_len)),
        _ => None,
    }
}

fn arrow(key: Key, params: &str) -> KeyEvent {
    // CSI 1 ; mods A|B|C|D
    let mods_raw: u8 = params
        .split(';')
        .nth(1)
        .and_then(|p| p.parse::<u8>().ok())
        .unwrap_or(1)
        .saturating_sub(1);
    KeyEvent {
        key,
        modifiers: decode_kitty_mods(mods_raw),
        event_type: KeyEventType::Press,
    }
}

fn simple_key(key: Key) -> KeyEvent {
    KeyEvent {
        key,
        modifiers: Modifiers::empty(),
        event_type: KeyEventType::Press,
    }
}

/// Parse SS3 sequences (ESC O ...)
fn parse_ss3(s: &str) -> Option<(KeyEvent, usize)> {
    let b = s.as_bytes().first()?;
    let key = match b {
        b'A' => Key::Up,
        b'B' => Key::Down,
        b'C' => Key::Right,
        b'D' => Key::Left,
        b'H' => Key::Home,
        b'F' => Key::End,
        b'P' => Key::F(1),
        b'Q' => Key::F(2),
        b'R' => Key::F(3),
        b'S' => Key::F(4),
        _ => return None,
    };
    Some((simple_key(key), 1))
}

/// Decode kitty modifier bits (value already has 1 subtracted).
/// Bit layout: shift=1, alt=2, ctrl=4, super=8, hyper=16, meta=32
fn decode_kitty_mods(raw: u8) -> Modifiers {
    let mut mods = Modifiers::empty();
    if raw & 1 != 0 {
        mods |= Modifiers::SHIFT;
    }
    if raw & 2 != 0 {
        mods |= Modifiers::ALT;
    }
    if raw & 4 != 0 {
        mods |= Modifiers::CTRL;
    }
    if raw & 8 != 0 {
        mods |= Modifiers::SUPER;
    }
    mods
}

/// Check if a raw input string matches a key descriptor like `"ctrl+a"`, `"alt+left"`, `"enter"`.
///
/// The descriptor syntax is: `[modifier+]*key`
/// Modifiers: `ctrl`, `alt`, `shift`, `super`
/// Keys: `a`–`z`, `enter`, `escape`, `tab`, `backspace`, `delete`,
///       `up`, `down`, `left`, `right`, `home`, `end`, `pageup`, `pagedown`, `f1`–`f12`
pub fn matches_key(data: &str, key_desc: &str) -> bool {
    let events = parse_input(data);
    if events.is_empty() {
        return false;
    }
    // Only consider the first event for matching
    let event = &events[0];
    if event.event_type == KeyEventType::Release {
        return false;
    }
    matches_event(event, key_desc)
}

pub fn matches_event(event: &KeyEvent, key_desc: &str) -> bool {
    let parts: Vec<&str> = key_desc.split('+').collect();
    if parts.is_empty() {
        return false;
    }

    let key_name = parts.last().unwrap().to_lowercase();
    let mut expected_mods = Modifiers::empty();

    for &part in &parts[..parts.len() - 1] {
        match part.to_lowercase().as_str() {
            "ctrl" => expected_mods |= Modifiers::CTRL,
            "alt" => expected_mods |= Modifiers::ALT,
            "shift" => expected_mods |= Modifiers::SHIFT,
            "super" => expected_mods |= Modifiers::SUPER,
            _ => return false,
        }
    }

    if event.modifiers != expected_mods {
        return false;
    }

    match key_name.as_str() {
        "enter" => event.key == Key::Enter,
        "escape" | "esc" => event.key == Key::Escape,
        "tab" => event.key == Key::Tab,
        "backspace" => event.key == Key::Backspace,
        "delete" => event.key == Key::Delete,
        "up" => event.key == Key::Up,
        "down" => event.key == Key::Down,
        "left" => event.key == Key::Left,
        "right" => event.key == Key::Right,
        "home" => event.key == Key::Home,
        "end" => event.key == Key::End,
        "pageup" => event.key == Key::PageUp,
        "pagedown" => event.key == Key::PageDown,
        s if s.starts_with('f') => {
            if let Ok(n) = s[1..].parse::<u8>() {
                event.key == Key::F(n)
            } else {
                false
            }
        }
        s if s.len() == 1 => {
            if let Some(ch) = s.chars().next() {
                event.key == Key::Char(ch)
            } else {
                false
            }
        }
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_regular_chars() {
        let events = parse_input("abc");
        assert_eq!(events.len(), 3);
        assert_eq!(events[0].key, Key::Char('a'));
        assert_eq!(events[1].key, Key::Char('b'));
        assert_eq!(events[2].key, Key::Char('c'));
    }

    #[test]
    fn test_parse_ctrl_a() {
        let events = parse_input("\x01");
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].key, Key::Char('a'));
        assert_eq!(events[0].modifiers, Modifiers::CTRL);
    }

    #[test]
    fn test_parse_enter() {
        let events = parse_input("\r");
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].key, Key::Enter);
    }

    #[test]
    fn test_parse_up_arrow() {
        let events = parse_input("\x1b[A");
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].key, Key::Up);
    }

    #[test]
    fn test_matches_key() {
        assert!(matches_key("\x01", "ctrl+a"));
        assert!(matches_key("\r", "enter"));
        assert!(!matches_key("\x01", "ctrl+b"));
    }
}
