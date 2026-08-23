use anyhow::Result;

use crate::book;

use crate::util::*;

/// `arvolo me` — everything about who you are here: the public id, the
/// fingerprint that confirms it out-of-band, and the display name you advertise.
///
/// The id goes to **stdout** and everything else to stderr, deliberately: this
/// is what people pipe into a message to a friend, and `arvolo me | pbcopy` has
/// to yield the id alone.
///
/// That id carries `@relay` — where you listen — because the person pasting it
/// into `arvolo contacts add` may not be on your relay, and a bare id gives them
/// no way to find that out. It is the same shape as a pairing code's
/// `N-word-word@relay`, and it is only routing: the id half is still what proves
/// who you are. `--plain` prints the bare id for scripts that parsed the old
/// output.
pub(crate) fn me(plain: bool) -> Result<()> {
    let id = my_identity()?;
    let pubid = id.public();
    let id_b32 = encode_id(&pubid);
    let relay = (!plain).then(book::default_relay_or_builtin).flatten();
    println!("{}", book::shareable_id(&id_b32, relay.as_deref()));
    eprintln!("fingerprint: {}", pubid.fingerprint());
    match &relay {
        Some(r) => eprintln!("relay: {r}"),
        None if plain => {}
        // `default_relay_or_builtin` only comes up empty on a build with the
        // built-in relay compiled out, so this is rare rather than routine.
        None => eprintln!("relay: (none configured — set ARVOLO_RELAY or `relay` in config.toml)"),
    }
    let name = book::my_display_name();
    if name.is_empty() {
        eprintln!("display name: (none — set one: arvolo me name \"Your Name\")");
    } else {
        eprintln!("display name: {name}");
    }
    eprintln!("(identity stored at {})", identity_path().display());
    Ok(())
}

/// `arvolo me name [NAME]` — show or set the display name advertised in offers.
pub(crate) async fn name_cmd(name: Option<String>) -> Result<()> {
    match name {
        None => {
            let current = book::my_display_name();
            if current.is_empty() {
                eprintln!("(no display name set — set one: arvolo me name \"Your Name\")");
            } else {
                println!("{current}");
            }
        }
        Some(n) => {
            // Through the daemon when one runs: it advertises the name inside
            // every offer, and its config watcher does not watch config.toml —
            // a name written only to the file kept the OLD one on the air until
            // the next restart. (`SetMyName` persists to the file too.) Without
            // a daemon the file is the whole state, so writing it is enough.
            if let Some(mut client) = crate::commands::daemon::daemon_client().await {
                client.set_my_name(n.clone()).await?;
            } else {
                book::set_my_display_name(&n)?;
            }
            let n = n.trim();
            if n.is_empty() {
                println!("Cleared your display name (offers will no longer advertise one).");
            } else {
                println!("Display name set to “{n}” — advertised inside offers you send.");
            }
        }
    }
    Ok(())
}

// There is no `version_cmd`: `arvolo --version` prints this binary's version, and
// whether a daemon is up — and on which version — is answered by `arvolo status`
// in both directions. A verb whose two halves were each already covered elsewhere
// was one verb too many.
