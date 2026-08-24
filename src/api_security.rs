pub const DEFAULT_MENU_COLOR: [u8; 3] = [190, 40, 255];

pub fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= crate::gallery_format::NAME_LEN
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.'))
}

pub fn parse_hex_color(value: &str) -> Option<[u8; 3]> {
    let value = value.strip_prefix('#').unwrap_or(value);
    if value.len() != 6 || !value.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    Some([
        u8::from_str_radix(&value[0..2], 16).ok()?,
        u8::from_str_radix(&value[2..4], 16).ok()?,
        u8::from_str_radix(&value[4..6], 16).ok()?,
    ])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_are_json_safe_and_bounded() {
        assert!(valid_name("rainbow-spiral_2.wasm"));
        assert!(!valid_name(""));
        assert!(!valid_name("has spaces"));
        assert!(!valid_name("quote\""));
        assert!(!valid_name("abcdefghijklmnopqrstuvwxyz1234567"));
    }

    #[test]
    fn colors_require_exact_rgb_hex() {
        assert_eq!(parse_hex_color("#12aBef"), Some([0x12, 0xab, 0xef]));
        assert_eq!(parse_hex_color("000000"), Some([0, 0, 0]));
        assert_eq!(parse_hex_color("12345"), None);
        assert_eq!(parse_hex_color("nothex"), None);
    }
}
