//! Turning a key press into the bytes a terminal program expects. No UI types,
//! so it is unit tested.

const ESC: u8 = 0x1b;

/// What was pressed: the key's name, the text it types, and the held modifiers.
pub struct Press<'a> {
    pub key: &'a str,
    pub text: Option<&'a str>,
    pub ctrl: bool,
    pub alt: bool,
    pub shift: bool,
}

/// The bytes to send to the shell, or `None` when the press means nothing to it.
#[must_use]
pub fn bytes_for(press: &Press<'_>) -> Option<Vec<u8>> {
    if let Some(named) = named_key(press) {
        return Some(named);
    }
    if press.ctrl {
        return control_byte(press.key).map(|byte| with_alt(press.alt, vec![byte]));
    }
    let text = press.text.filter(|t| !t.is_empty())?;
    Some(with_alt(press.alt, text.as_bytes().to_vec()))
}

fn named_key(press: &Press<'_>) -> Option<Vec<u8>> {
    let bytes: &[u8] = match press.key {
        "enter" => b"\r",
        "backspace" => &[0x7f],
        "tab" if press.shift => b"\x1b[Z",
        "tab" => b"\t",
        "escape" => &[ESC],
        "up" => b"\x1b[A",
        "down" => b"\x1b[B",
        "right" => b"\x1b[C",
        "left" => b"\x1b[D",
        "home" => b"\x1b[H",
        "end" => b"\x1b[F",
        "insert" => b"\x1b[2~",
        "delete" => b"\x1b[3~",
        "pageup" => b"\x1b[5~",
        "pagedown" => b"\x1b[6~",
        "space" if press.ctrl => &[0],
        "space" => b" ",
        _ => return None,
    };
    Some(with_alt(press.alt && bytes != b" ", bytes.to_vec()))
}

/// Ctrl with a letter is that letter's position in the alphabet (Ctrl+C is 3).
fn control_byte(key: &str) -> Option<u8> {
    let mut chars = key.chars();
    let letter = chars.next()?.to_ascii_lowercase();
    if chars.next().is_some() || !letter.is_ascii_lowercase() {
        return None;
    }
    Some(letter as u8 - b'a' + 1)
}

/// Alt sends the key prefixed with Escape.
fn with_alt(alt: bool, bytes: Vec<u8>) -> Vec<u8> {
    if alt {
        [&[ESC][..], &bytes].concat()
    } else {
        bytes
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn press<'a>(key: &'a str, text: Option<&'a str>) -> Press<'a> {
        Press {
            key,
            text,
            ctrl: false,
            alt: false,
            shift: false,
        }
    }

    #[test]
    fn typed_text_is_sent_as_it_is() {
        assert_eq!(bytes_for(&press("a", Some("a"))), Some(b"a".to_vec()));
        assert_eq!(
            bytes_for(&press("ß", Some("ß"))),
            Some("ß".as_bytes().to_vec())
        );
    }

    #[test]
    fn enter_backspace_and_arrows_send_their_codes() {
        assert_eq!(bytes_for(&press("enter", None)), Some(b"\r".to_vec()));
        assert_eq!(bytes_for(&press("backspace", None)), Some(vec![0x7f]));
        assert_eq!(bytes_for(&press("up", None)), Some(b"\x1b[A".to_vec()));
    }

    #[test]
    fn ctrl_c_is_the_interrupt_byte() {
        let ctrl_c = Press {
            ctrl: true,
            ..press("c", Some("c"))
        };
        assert_eq!(bytes_for(&ctrl_c), Some(vec![3]));
    }

    #[test]
    fn alt_prefixes_escape() {
        let alt_b = Press {
            alt: true,
            ..press("b", Some("b"))
        };
        assert_eq!(bytes_for(&alt_b), Some(b"\x1bb".to_vec()));
    }

    #[test]
    fn shift_tab_goes_back_and_a_bare_modifier_sends_nothing() {
        assert_eq!(
            bytes_for(&Press {
                shift: true,
                ..press("tab", None)
            }),
            Some(b"\x1b[Z".to_vec())
        );
        assert_eq!(bytes_for(&press("shift", None)), None);
    }
}
