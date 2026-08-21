//! `arvolo contacts pair` — trade public ids with someone, in person or over a
//! call, and come away with each other saved *and* verified.
//!
//! Adding a contact used to mean getting 52 characters of base32 across somehow
//! and pasting them, then verifying as a second, separate act that almost nobody
//! performs. This collapses both into one short code.
//!
//! **Why the verification is real, not a courtesy.** The code is a SPAKE2 secret:
//! the rendezvous channel only forms between two parties that knew the same code,
//! so a key that arrives through it is authenticated *by* the code. That is the
//! same property `arvolo code` relies on to hand over a ticket safely, and it is
//! precisely what `docs/IDENTITY-VERIFICATION.md` §4 proposed. Its limit is worth
//! stating plainly: it is as strong as the channel you read the code over, and as
//! the relay's rate limiting against someone guessing it.
//!
//! Not to be confused with `arvolo device pair`, which shares your **secret**
//! identity to make another machine *you*. This trades **public** ids between two
//! different people, and neither side learns anything the other did not choose to
//! send.

use anyhow::{bail, Context, Result};
use arvolo_core::code;

use crate::book;
use crate::ui::*;
use crate::util::*;

/// Long enough for the other person to find their terminal and type the code.
const PAIR_WAIT: std::time::Duration = std::time::Duration::from_secs(300);

/// Show a code and wait for someone to take it — the side that speaks first.
pub(crate) async fn pair_host(name: Option<String>, relay: Option<String>, qr: bool) -> Result<()> {
    let relay = require_relay(relay)?;
    let me = my_identity()?;
    let my_id = encode_id(&me.public());

    // Checked before claiming a nameplate, so a relay that cannot do this never
    // gets a code shown for it. The v1 rendezvous has no way to carry a reply,
    // and an exchange that only goes one way is not what this command promises —
    // better to say so now than to leave someone reading a code that will half
    // work.
    if code::relay_rz_version(&relay).await != code::RzVersion::V2 {
        bail!(
            "{relay} is too old for a mutual exchange — it needs rendezvous v2, which is \
             what carries the other side's id back to you.\n   Update the relay, or add \
             them the long way: `arvolo me` on their machine, then `arvolo contacts add \
             <name> <id>` here."
        );
    }

    let (shown, sender) = code::publish_auto(my_id.as_bytes(), &relay, true)
        .await
        .context("start contact pairing")?;

    println!("\n{shown}\n");
    if qr {
        crate::print_qr(&shown);
    }
    eprintln!("Read that to the other person. On their machine:");
    eprintln!("    arvolo contacts pair {shown}");
    eprintln!(
        "\nThis shares your public id ({}) — never your identity",
        me.public().fingerprint()
    );
    eprintln!("secret, and never your address book.");
    eprintln!("Waiting… (Ctrl-C to cancel)");

    let their_id = match sender {
        // Unreachable: the version check above already refused a v1 relay. Kept as
        // a refusal rather than an `unreachable!` so a future change to
        // `publish_auto` degrades into an error instead of a panic.
        code::CodeSender::V1(_) => bail!("this relay cannot carry a mutual exchange"),
        code::CodeSender::V2(host) => {
            let opts = code::HostOpts {
                max_sessions: Some(1),
                await_reply: true,
                ..code::HostOpts::default()
            };
            let mut got: Option<Vec<u8>> = None;
            host.run(
                my_id.as_bytes(),
                &opts,
                code::HostState::default(),
                cancel_on_ctrl_c(),
                |ev| {
                    if let code::HostEvent::Paired { reply, .. } = ev {
                        got = reply;
                    }
                },
                |_| {},
            )
            .await
            .context("contact pairing")?;
            got
        }
    };

    let Some(bytes) = their_id else {
        // They took our id but sent none back. Not a relay we can blame — the
        // version was checked — so this is the other side going away mid-exchange,
        // or running something that isn't arvolo.
        bail!("the other side took your id but never sent theirs — nothing was saved here");
    };
    save_the_other_side(&String::from_utf8_lossy(&bytes), name).await
}

/// Take a code someone showed you — the side that answers.
pub(crate) async fn pair_join(code_str: String, name: Option<String>) -> Result<()> {
    let me = my_identity()?;
    let my_id = encode_id(&me.public());
    let default_relay = book::default_relay_or_builtin();

    eprintln!("Pairing… (waiting for the other side)");
    let (their_bytes, replied) = code::exchange_bytes_with(
        &code_str,
        default_relay.as_deref(),
        PAIR_WAIT,
        Some(my_id.as_bytes()),
    )
    .await
    .context("contact pairing")?;

    // Saving only our half would leave the two of you disagreeing about what just
    // happened — you have them, they don't have you — which is worse than a clean
    // failure you can retry.
    if !replied {
        bail!(
            "the relay refused to carry your id back, so only half the exchange would have \
             happened — nothing was saved.\n   That relay needs rendezvous v2; ask them to \
             update it, or trade ids the long way with `arvolo me` + `arvolo contacts add`."
        );
    }
    save_the_other_side(&String::from_utf8_lossy(&their_bytes), name).await
}

/// Save the id we just traded for, and mark it verified.
async fn save_the_other_side(their_id: &str, name: Option<String>) -> Result<()> {
    let their_id = their_id.trim();
    book::decode_id(their_id).context("the other side sent something that isn't a public id")?;

    if let Some(existing) = book::resolve_name(their_id) {
        // Already known: nothing to name, but the pairing did just authenticate
        // the key, which is the part they were probably missing.
        book::mark_verified(their_id)?;
        println!("'{existing}' is now verified — you already had them saved.");
        return Ok(());
    }

    let name = match name {
        Some(n) => n,
        None => ask_for_a_name(their_id)?,
    };
    book::contact_add(&name, their_id)?;
    book::mark_verified(&name)?;
    println!("Saved '{name}' — verified.");
    eprintln!(
        "   fingerprint: {}",
        book::fingerprint_of(their_id).unwrap_or_default()
    );
    Ok(())
}

/// Ask what to file them under. In a non-interactive shell there is nobody to
/// ask, so `--name` is required rather than inventing one.
fn ask_for_a_name(their_id: &str) -> Result<String> {
    use std::io::{IsTerminal, Write};
    if !std::io::stdin().is_terminal() {
        bail!(
            "not a terminal — pass --name to say what to file them under: \
             arvolo contacts pair --name <name>"
        );
    }
    eprintln!(
        "\nPaired with fingerprint {}",
        book::fingerprint_of(their_id).unwrap_or_default()
    );
    eprint!("Save them as: ");
    let _ = std::io::stderr().flush();
    let mut line = String::new();
    std::io::stdin().read_line(&mut line).ok();
    let name = line.trim().to_string();
    if name.is_empty() {
        bail!("no name given — nothing saved");
    }
    Ok(name)
}
