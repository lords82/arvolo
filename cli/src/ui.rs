use arvolo_core::crypto::PublicId;
use tokio_util::sync::CancellationToken;

use crate::book;

use crate::util::encode_id;

/// A cancellation token that fires on Ctrl-C.
pub(crate) fn cancel_on_ctrl_c() -> CancellationToken {
    let token = CancellationToken::new();
    let t = token.clone();
    tokio::spawn(async move {
        let _ = tokio::signal::ctrl_c().await;
        t.cancel();
    });
    token
}

/// Announce who a received transfer is from, and record it in the TOFU ledger.
///
/// `id` is `None` for a plain (anonymous, unauthenticated) ticket, or the
/// sender's HPKE-authenticated public-key bytes for a sealed one. Prints to
/// stderr before the progress bar starts.
pub(crate) fn print_sender_banner(id: Option<&[u8]>) {
    let bytes = match id {
        None => {
            eprintln!(
                "⚠  From: anonymous sender — this ticket is not authenticated; anyone \
                 holding it could have created it."
            );
            return;
        }
        Some(b) => b,
    };
    let Ok(pubid) = PublicId::from_bytes(bytes) else {
        eprintln!("⚠  From: a sender with an unreadable identity in the ticket.");
        return;
    };
    let id_b32 = encode_id(&pubid);
    let fp = pubid.fingerprint();
    let status = book::sender_status(&id_b32);
    match (status.name, status.seen_before) {
        (Some(name), _) if status.verified => {
            eprintln!("✓ From: {name}  (verified — fingerprint: {fp})");
        }
        (Some(name), _) => {
            eprintln!("From: {name}  (saved, not verified — fingerprint: {fp})");
            eprintln!("      verify out-of-band, then: arvolo contacts verify {name}");
        }
        (None, true) => {
            eprintln!("From: known sender  (fingerprint: {fp})");
            eprintln!("      id: {id_b32}");
            eprintln!("      not in contacts — save with: arvolo contacts add <name> {id_b32}");
        }
        (None, false) => {
            eprintln!("⚠  From: NEW sender (first time you receive from this identity)");
            eprintln!("      fingerprint: {fp}");
            eprintln!("      id: {id_b32}");
            eprintln!(
                "      Verify the fingerprint out-of-band, then: arvolo contacts add <name> {id_b32}"
            );
        }
    }
    book::record_seen(&id_b32);
}

/// Make a remote-supplied, untrusted string safe to print on one terminal line.
/// A sender fully controls the offer's file name, note, and advertised display
/// name; printed raw they could carry ANSI escape sequences (to forge output such
/// as a fake "verified" banner or to hide a traversal name) or bidirectional
/// override characters (to reverse the visible reading order). We drop every
/// control character and Unicode bidi/format override, and bound the length.
pub(crate) fn sanitize_display(s: &str) -> String {
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

/// Observe a sender's advertised display name on an incoming offer and print its
/// TOFU status (new / changed), recording any change as pending. Never blocks the
/// transfer — the name is a petname claim, shown as unverified and never a trust
/// signal. Prints nothing when no name is advertised or it matches the approved one.
pub(crate) fn note_advertised_name(id_b32: &str, sender_name: &str) {
    match book::observe_advertised_name(id_b32, sender_name) {
        book::NameStatus::None => {}
        book::NameStatus::Unchanged(name) => {
            eprintln!("   🏷  calls themselves: {}", sanitize_display(&name));
        }
        book::NameStatus::New(name) => {
            eprintln!(
                "   🏷  NEW sender — calls themselves \"{}\" (unverified name)",
                sanitize_display(&name)
            );
            eprintln!("      approve with: arvolo contacts accept-name {id_b32}");
        }
        book::NameStatus::Changed { old, new } => {
            eprintln!(
                "   ⚠  now calls themselves \"{}\" (was \"{}\") — keeping \"{}\" for now",
                sanitize_display(&new),
                sanitize_display(&old),
                sanitize_display(&old)
            );
            eprintln!("      approve the new name: arvolo contacts accept-name {id_b32}");
        }
    }
}

/// Render a ticket as a QR code on stdout (best-effort).
pub(crate) fn print_qr(data: &str) {
    match qrcode::QrCode::new(data) {
        Ok(code) => {
            let art = code
                .render::<qrcode::render::unicode::Dense1x2>()
                .quiet_zone(true)
                .build();
            println!("{art}");
        }
        Err(e) => eprintln!("(could not render QR: {e})"),
    }
}

// ---- P2P ------------------------------------------------------------------

#[cfg(test)]
mod sanitize_tests {
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
