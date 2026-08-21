use std::path::PathBuf;

use anyhow::{Context, Result};
use arvolo_core::flow::{self};

use crate::{book, deposits};

use crate::output::vprintln;

use crate::ui::*;
use crate::util::*;

/// Say so when the relay kept the file for less time than was asked.
///
/// Worth a line of its own rather than only quoting the real deadline further down:
/// a relay's `ARVOLO_MAX_TTL` is invisible from the outside, so someone who passed
/// `--ttl` and reads a smaller number back has no way to tell a typo from a policy.
/// It is not an error — the deposit is placed and the file is deliverable — so it
/// prints and carries on.
fn note_shortened_ttl(asked: u64, granted: u64) {
    if granted < asked {
        eprintln!(
            "note: this relay keeps deposits for at most {} — you asked for {}, \
             and that is what it will honour.",
            human_duration(granted),
            human_duration(asked)
        );
    }
}

/// The shared prologue of both deposit shapes: resolve the relay and pack the
/// payload. Returns `(relay, payload, name, temp_to_cleanup, size)`.
fn prepare_deposit(
    paths: &[PathBuf],
    relay: Option<String>,
) -> Result<(String, PathBuf, String, Option<PathBuf>, u64)> {
    anyhow::ensure!(
        !paths.is_empty(),
        "provide at least one file or folder to send"
    );
    let relay = relay
        .map(|r| book::normalize_relay(&r))
        .or_else(book::default_relay_or_builtin)
        .context("no relay: pass --relay <host>, set ARVOLO_RELAY, or configure `relay`")?;
    vprintln!("using relay: {relay}");
    let (payload, name, archive, temp) = resolve_payload(paths)?;
    if archive {
        eprintln!("Packing {} item(s) into an archive…", paths.len());
    }
    let size = std::fs::metadata(&payload).map(|m| m.len()).unwrap_or(0);
    Ok((relay, payload, name, temp, size))
}

/// A public, browser-openable download URL (no recipient). A link has NO
/// download cap by default (it expires only when its session is removed or the
/// TTL lapses); `--max` optionally sets one.
pub(crate) async fn send_link(
    paths: Vec<PathBuf>,
    relay: Option<String>,
    ttl: u64,
    max: Option<u32>,
    qr: bool,
) -> Result<()> {
    let (relay, payload, _name, temp, _size) = prepare_deposit(&paths, relay)?;
    vprintln!("mode: public browser link — TTL {}", human_duration(ttl));
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
    // The relay's answer, not our request, from here on: the local record's
    // deadline has to match the one the relay will actually enforce, or
    // `arvolo status` keeps listing a link that stopped working days ago.
    note_shortened_ttl(ttl, out.ttl_secs);
    let ttl = out.ttl_secs;
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
    // The URL alone on stdout (`arvolo send --link f | pbcopy` copies just the
    // address); everything around it is narration and goes to stderr.
    println!("{}", out.link);
    eprintln!(
        "\nEncrypted and deposited ({}, expires in {}). File: {} ({}).",
        cap,
        human_duration(ttl),
        out.name,
        human_size(out.size),
    );
    eprintln!("Anyone with the link above can download it in a browser — no arvolo needed.");
    if qr {
        print_qr(&out.link);
    }
    // Say that the *address* is kept, not only the row. A link scrolled out
    // of the terminal is otherwise assumed lost, and the file gets sent a
    // second time rather than the URL handed over again.
    eprintln!(
        "Listed as '{}' in `arvolo status`, which prints this address again\nwhenever you need it — cancel the link (and delete it from the relay) with:\n",
        rec.id
    );
    eprintln!("    arvolo cancel {}", rec.id);
    Ok(())
}

/// Deposit `paths` on the relay mailbox, HPKE-sealed to `to`. If `offer` is set
/// an inbox offer is posted too so the recipient's daemon can auto-fetch it (a
/// shareable `arvm…` ticket is printed either way).
#[allow(clippy::too_many_arguments)]
pub(crate) async fn send_sealed(
    paths: Vec<PathBuf>,
    to: String,
    relay: Option<String>,
    ttl: u64,
    max: Option<u32>,
    password: Option<String>,
    offer: bool,
    note: &str,
) -> Result<()> {
    let me = my_identity()?;
    let (relay, payload, name, temp, size) = prepare_deposit(&paths, relay)?;
    vprintln!("mode: sealed to recipient — TTL {}", human_duration(ttl));
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
        &payload.as_path().into(),
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
    // Everything below deadlines off what the relay granted, not what we asked for.
    // The inbox offer especially: given the requested TTL it would sit in the
    // recipient's list long after the relay reaped the blob, and they would find out
    // by accepting an arrival that 404s.
    note_shortened_ttl(ttl, deposited.ttl_secs);
    let ttl = deposited.ttl_secs;

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
            // Stamped even for a send to somebody else: it costs a byte, and it is
            // what stops *our own* daemon from picking this up when the recipient
            // is another device of ours.
            origin: Some(book::load_or_init_device()),
        };
        match arvolo_core::presence::post_offer(
            &arvolo_core::http::client(),
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
    // The `arvm…` ticket alone on stdout; the words around it go to stderr.
    println!("{encoded}");
    eprintln!(
        "\nEncrypted and deposited ({max} download(s), expires in {}).",
        human_duration(ttl)
    );
    if password.is_some() {
        eprintln!("Password-protected — share the password out-of-band (not with the ticket).");
    }
    if offer {
        eprintln!(
            "The recipient's daemon will fetch it automatically. To hand it over instead:\n"
        );
    } else {
        eprintln!("Send this ticket to the recipient:\n");
    }
    eprintln!("    arvolo recv {encoded}\n");
    eprintln!(
        "Listed as '{}' in `arvolo status` — cancel the delivery (and delete it\nfrom the relay) with:\n",
        rec.id
    );
    eprintln!("    arvolo cancel {}", rec.id);
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
    // genuine — surface it (mailbox tickets are always sealed to a recipient).
    // The chunk loop reports progress: a mailbox fetch used to be silent until
    // the end, which on a big file reads as a hang.
    let total = arvolo_core::offline::OfflineTicket::decode(&ticket)
        .map(|t| t.total_size)
        .unwrap_or(0);
    let progress = crate::ui::Progress::new("downloading from the mailbox", total);
    let (path, n) = flow::fetch_offline_with_progress(&ticket, out, &me, password.as_deref(), |done, _| {
        progress.update(done);
    })
    .await?;
    progress.finish();
    vprintln!("HPKE authentication passed — the sender in the ticket is genuine");
    let sender = arvolo_core::offline::OfflineTicket::decode(&ticket)
        .ok()
        .and_then(|t| print_sender_banner(Some(&t.sender)));
    crate::ui::saved(&path, n as u64);
    if let Some(id) = sender {
        crate::ui::offer_to_save_contact(&id).await;
    }
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

