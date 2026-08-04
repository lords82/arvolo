use anyhow::Result;

use crate::book;

use crate::util::*;

/// `arvolo me` — everything about who you are here: the public id, the
/// fingerprint that confirms it out-of-band, and the display name you advertise.
///
/// The id goes to **stdout** and everything else to stderr, deliberately: this
/// is what people pipe into a message to a friend, and `arvolo me | pbcopy` has
/// to yield the id alone.
pub(crate) fn me() -> Result<()> {
    let id = my_identity()?;
    let pubid = id.public();
    println!("{}", encode_id(&pubid));
    eprintln!("fingerprint: {}", pubid.fingerprint());
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
pub(crate) fn name_cmd(name: Option<String>) -> Result<()> {
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
            book::set_my_display_name(&n)?;
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
