//! Untrusted text on its way to a screen.

/// Make a remote-supplied, untrusted string safe to show on one line.
///
/// A sender fully controls the offer's file name, note, and advertised display
/// name; shown raw they could carry ANSI escape sequences (to forge output such
/// as a fake "verified" banner, or to hide a traversal name) or bidirectional
/// override characters (to reverse the visible reading order). We drop every
/// control character and Unicode bidi/format override, and bound the length.
///
/// Not a terminal concern, though a terminal is where the attack was first
/// imagined: the daemon hands these strings to whatever is attached, and a window
/// that renders them has the same problem with fewer visible symptoms.
pub fn sanitize_display(s: &str) -> String {
    // Unicode bidi/format overrides that can reorder or hide visible text.
    fn is_bidi_override(c: char) -> bool {
        matches!(c,
            '\u{200E}' | '\u{200F}' | '\u{061C}'
            | '\u{202A}'..='\u{202E}'
            | '\u{2066}'..='\u{2069}')
    }
    s.chars()
        .filter(|c| !c.is_control() && !is_bidi_override(*c))
        .take(256)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::sanitize_display;

    // A remote-supplied name/note/display-name must have terminal control sequences
    // and bidi overrides stripped before printing, so a malicious sender can't forge
    // output or reverse the visible reading order.
    #[test]
    fn strips_ansi_and_control_chars() {
        // ANSI escape (ESC [ 31m … ESC [ 0m) is removed; the letters survive.
        assert_eq!(
            sanitize_display("\u{1b}[31mVERIFIED\u{1b}[0m"),
            "[31mVERIFIED[0m"
        );
        // CR/LF/NUL/tab can't inject new lines or overwrite the current one.
        assert_eq!(sanitize_display("a\r\nb\tc\0d"), "abcd");
        // Plain text (incl. non-control unicode) is preserved.
        assert_eq!(sanitize_display("Lorénzo 名前"), "Lorénzo 名前");
    }

    #[test]
    fn strips_bidi_overrides() {
        // A right-to-left override that would display "gpj.exe" as "exe.jpg".
        assert_eq!(sanitize_display("photo\u{202e}gpj.exe"), "photogpj.exe");
        assert_eq!(sanitize_display("\u{2066}x\u{2069}"), "x");
    }

    #[test]
    fn bounds_length() {
        let long = "a".repeat(1000);
        assert_eq!(sanitize_display(&long).chars().count(), 256);
    }
}
