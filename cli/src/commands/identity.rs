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
    // Printed next to it precisely because the two get confused: the line above
    // is what other people see and is the same on every machine holding this
    // identity, while this one names *this* computer and never leaves it.
    let device = book::load_devices()
        .devices
        .get(&arvolo_core::sync::device_key(&book::load_or_init_device()))
        .map(|r| r.name.clone())
        .unwrap_or_else(book::default_device_name);
    eprintln!("this device: {device} (only you see this — `arvolo device`)");
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
            // Through the daemon when one runs, so the change is on the air at
            // once rather than at the next restart. (`SetMyName` persists it and
            // stamps the sync register, same as the direct call below.) The
            // daemon's book watcher would catch a direct write within the second
            // — it watches `me.toml` — but only because the name now lives in a
            // ledger; asking the daemon remains the one path that does not depend
            // on that.
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
