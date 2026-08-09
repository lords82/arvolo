use std::path::PathBuf;

use anyhow::{Context, Result};
use arvolo_core::flow::{self};

use crate::{book, deposits};

use crate::output::vprintln;

use crate::ui::*;
use crate::util::*;

/// Deposit `paths` on the relay mailbox (or as a `--link`). Internal helper for
/// the unified `send`: `link` → public browser URL; otherwise HPKE-sealed to
/// `to`, and if `offer` is set an inbox offer is posted too so the recipient's
/// daemon can auto-fetch it (a shareable `arvm…` ticket is printed either way).
#[allow(clippy::too_many_arguments)]
pub(crate) async fn send_offline(
    paths: Vec<PathBuf>,
    to: Option<String>,
    link: bool,
    relay: Option<String>,
    use_http: bool,
    ttl: u64,
    max: Option<u32>,
    password: Option<String>,
    qr: bool,
    offer: bool,
    note: &str,
) -> Result<()> {
    anyhow::ensure!(
        !paths.is_empty(),
        "provide at least one file or folder to send"
    );
    let me = my_identity()?;
    let relay = relay
        .map(|r| book::normalize_relay(&r, use_http))
        .or_else(book::default_relay_or_builtin)
        .context("no relay: pass --relay <host>, set ARVOLO_RELAY, or configure `relay`")?;
    vprintln!("using relay: {relay}");
    let (payload, name, archive, temp) = resolve_payload(&paths)?;
    if archive {
        eprintln!("Packing {} item(s) into an archive…", paths.len());
    }
    let size = std::fs::metadata(&payload).map(|m| m.len()).unwrap_or(0);
    vprintln!(
        "mode: {} — TTL {}",
        if link {
            "public browser link (--link)"
        } else {
            "sealed to recipient (--to)"
        },
        human_duration(ttl)
    );

    // Link mode: a public, browser-openable download URL (no recipient). A link
    // has NO download cap by default (it expires only when its session is
    // removed or the TTL lapses); `--max` optionally sets one.
    if link {
        anyhow::ensure!(
            to.is_none(),
            "--link produces a public link; --to is not used (anyone with the link can download)"
        );
        anyhow::ensure!(
            password.is_none(),
            "--password is not yet supported with --link (the browser page can't unwrap it)"
        );
        let max = max.unwrap_or(deposits::UNLIMITED);
        vprintln!("encrypting locally and uploading to the relay (key stays in the link's #fragment; the relay only sees ciphertext)…");
        let out = match arvolo_core::link::deposit_link(&payload, &relay, ttl, max).await {
            Ok(o) => o,
            Err(e) => {
                if let Some(t) = &temp {
                    let _ = std::fs::remove_file(t);
                }
                return Err(e);
            }
        };
        if let Some(t) = &temp {
            let _ = std::fs::remove_file(t);
        }
        let rec = deposits::save(
            deposits::KIND_LINK,
            &relay,
            &out.claim,
            &out.revoke_token,
            &out.name,
            out.size,
            max,
            Some(out.link.clone()),
            // A public link has no `arvm…` ticket: the URL above is the whole of it.
            "",
            None,
            now_unix().saturating_add(ttl),
            // A link has no engine row and no recipient — it is nobody's arrival, so
            // there is no inbox offer to retract either. The blob is the whole of it.
            None,
            None,
        )?;
        let cap = if max == deposits::UNLIMITED {
            "no download limit".to_string()
        } else {
            format!("{max} download(s)")
        };
        println!(
            "\nEncrypted and deposited ({}, expires in {}). File: {} ({}).",
            cap,
            human_duration(ttl),
            out.name,
            human_size(out.size),
        );
        println!("Anyone with this link can download it in a browser — no arvolo needed:\n");
        println!("    {}\n", out.link);
        // Say that the *address* is kept, not only the row. A link scrolled out
        // of the terminal is otherwise assumed lost, and the file gets sent a
        // second time rather than the URL handed over again.
        println!(
            "Listed as '{}' in `arvolo status`, which prints this address again\nwhenever you need it — cancel the link (and delete it from the relay) with:\n",
            rec.id
        );
        println!("    arvolo cancel {}\n", rec.id);
        if qr {
            print_qr(&out.link);
        }
        return Ok(());
    }

    let to = to.context("--to <name|id> is required (or pass --link for a public link)")?;
    let max = max.unwrap_or(1);
    let recipient = book::resolve_recipient(&to)?;
    vprintln!(
        "recipient {to} resolved (fingerprint {})",
        recipient.fingerprint()
    );
    vprintln!(
        "HPKE-sealing to the recipient and depositing on the relay ({max} download(s){})…",
        if password.is_some() {
            ", password-protected"
        } else {
            ""
        }
    );
    let deposited = match flow::deposit_offline(
        &payload,
        &name,
        &recipient,
        &me,
        &relay,
        ttl,
        max,
        password.as_deref(),
    )
    .await
    {
        Ok(d) => d,
        Err(e) => {
            if let Some(t) = &temp {
                let _ = std::fs::remove_file(t);
            }
            match e {
                flow::DepositError::TooLarge => anyhow::bail!(
                    "the relay refused this file — it's larger than the mailbox allows. \
                     Send it directly instead (`arvolo send …` P2P while both devices are \
                     online), or use a private relay with a higher limit."
                ),
                flow::DepositError::Unavailable(m) => anyhow::bail!(
                    "the relay is unavailable ({m}). Try again later, or check ARVOLO_RELAY \
                     / --relay. A direct P2P send works while both devices are online."
                ),
                flow::DepositError::Fatal(err) => return Err(err),
            }
        }
    };
    if let Some(t) = &temp {
        let _ = std::fs::remove_file(t);
    }
    let encoded = deposited.ticket.encode();

    // Also drop an inbox offer so the recipient's daemon can auto-fetch it (the
    // offer carries this same arvm ticket; best-effort — the printed ticket still
    // works if this fails).
    //
    // Keep what comes back. The offer outlives this command, and withdrawing later
    // has to retract it as well as revoke the blob — revoking alone would leave the
    // recipient an arrival that can never be fetched. Only the poster token can
    // retract it, so the token has to survive the process that minted it: it goes in
    // the record, next to the revoke token, and `arvolo cancel <id>` uses both.
    let mut posted = None;
    if offer {
        let off = arvolo_core::presence::Offer {
            name: name.clone(),
            size,
            chunks: 0,
            ticket: encoded.clone(),
            note: note.to_string(),
            sender_name: book::my_display_name(),
        };
        match arvolo_core::presence::post_offer(
            &reqwest::Client::new(),
            &relay,
            &recipient,
            &me,
            &off,
            Some(ttl),
        )
        .await
        {
            Ok(p) => posted = Some(p),
            Err(e) => eprintln!(
                "(warning: couldn't post an inbox offer, so the recipient's daemon won't auto-fetch: {e:#})"
            ),
        }
    }

    let rec = deposits::save(
        deposits::KIND_OFFLINE,
        &relay,
        &deposited.ticket.claim,
        &deposited.revoke_token,
        &name,
        size,
        max,
        None,
        // The ticket printed below, kept: it is the sender's only copy, it cannot be
        // rebuilt from the claim, and the terminal it was printed in will scroll.
        &encoded,
        // The resolved key, not `to` — which is whatever was typed on the command
        // line, a contact name as often as an id. The record's reader has no way to
        // tell one from the other, and needs the key: to retract the offer below, and
        // to match the deposit to a contact in a UI. (The daemon path has always put
        // the key here; this one used to disagree.)
        Some(encode_id(&recipient)),
        now_unix().saturating_add(ttl),
        // No engine row behind a one-shot deposit: the withdrawal happens from the
        // record itself, which is why the offer below rides along in it.
        None,
        posted.as_ref(),
    )?;
    println!(
        "\nEncrypted and deposited ({max} download(s), expires in {}).",
        human_duration(ttl)
    );
    if password.is_some() {
        println!("Password-protected — share the password out-of-band (not with the ticket).");
    }
    if offer {
        println!("The recipient's daemon will fetch it automatically. To hand it over instead:\n");
    } else {
        println!("Send this ticket to the recipient:\n");
    }
    println!("    arvolo recv {encoded}\n");
    println!(
        "Listed as '{}' in `arvolo status` — cancel the delivery (and delete it\nfrom the relay) with:\n",
        rec.id
    );
    println!("    arvolo cancel {}\n", rec.id);
    if qr {
        print_qr(&encoded);
    }
    Ok(())
}

pub(crate) async fn recv_offline(
    ticket: String,
    out: Option<PathBuf>,
    password: Option<String>,
) -> Result<()> {
    let me = my_identity()?;
    if let Ok(t) = arvolo_core::offline::OfflineTicket::decode(&ticket) {
        vprintln!(
            "fetching ciphertext from relay {} and unsealing with your identity…",
            t.relay
        );
    }
    if password.is_some() {
        vprintln!("deriving the decryption key from the supplied password");
    }
    // A successful fetch means HPKE auth passed, so the sender in the ticket is
    // genuine — surface it (offline tickets are always sealed to a recipient).
    let (path, n) = flow::fetch_offline(&ticket, out, &me, password.as_deref()).await?;
    vprintln!("HPKE authentication passed — the sender in the ticket is genuine");
    if let Ok(t) = arvolo_core::offline::OfflineTicket::decode(&ticket) {
        print_sender_banner(Some(&t.sender));
    }
    println!("Saved {n} bytes to {}", path.display());
    Ok(())
}

/// The relay and claim a withdrawal target names, if it names one: an `arvm…`
/// offline ticket or a `…/dl/<claim>` download link. `None` for anything else —
/// which is how [`crate::commands::cancel`] tells a ticket from an id.
pub(crate) fn withdrawal_target(target: &str) -> Option<(String, String)> {
    if let Ok(t) = arvolo_core::offline::OfflineTicket::decode(target) {
        return Some((t.relay, t.claim));
    }
    parse_dl_link(target).ok()
}

/// Delete a mailbox blob or a browser link from the relay using the revoke token
/// printed when it was sent.
///
/// This is the path for something *this machine has no record of* — typically
/// sent from another machine — which is exactly why the token has to be supplied
/// by hand: the local record is where it would otherwise have come from.
pub(crate) async fn revoke_by_token(relay: &str, claim: &str, token: &str) -> Result<()> {
    vprintln!("asking relay {relay} to delete claim {claim}…");
    flow::revoke_offline(relay, claim, token).await?;
    println!("Revoked — the file is deleted from the relay; the ticket/link no longer works.");
    Ok(())
}
