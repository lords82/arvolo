use std::time::Duration;

use anyhow::Result;
use arvolo_core::flow::{self};

use crate::{book, deposits, sessions};

use crate::args::SessionAction;
use crate::util::*;

pub(crate) async fn sessions_cmd(action: SessionAction) -> Result<()> {
    match action {
        SessionAction::List => {
            let dep_list = deposits::list();
            let resumable = sessions::list();
            if dep_list.is_empty() && resumable.is_empty() {
                eprintln!(
                    "(no sessions yet — saved automatically when you `arvolo send` to the mailbox or as a link)"
                );
                return Ok(());
            }

            // Relay deposits (public links + sealed offline sends). We poll the
            // relay for each one's live status (bounded, so a dead relay can't
            // hang the listing).
            if !dep_list.is_empty() {
                println!("Relay deposits — `arvolo sessions rm <id>` deletes from the relay:\n");
                for r in dep_list {
                    let info = tokio::time::timeout(
                        Duration::from_secs(5),
                        flow::claim_info(&r.relay, &r.claim),
                    )
                    .await;
                    let (on_relay, live_downloads) = match &info {
                        Ok(Ok(i)) if i.present => ("present", i.downloads),
                        Ok(Ok(_)) => ("gone (downloaded / expired / revoked)", None),
                        _ => ("unknown (relay unreachable)", None),
                    };
                    let expiry = if r.expired() {
                        "EXPIRED".to_string()
                    } else {
                        format!(
                            "in {}",
                            human_duration(r.expires.saturating_sub(now_unix()))
                        )
                    };
                    let kind = if r.kind == deposits::KIND_LINK {
                        "link"
                    } else {
                        "sealed"
                    };
                    println!("● {}  [{kind}]  {}  ({})", r.id, r.name, human_size(r.size));
                    println!("    relay:      {}", r.relay);
                    if let Some(l) = &r.link {
                        println!("    link:       {l}");
                    }
                    if let Some(rcpt) = &r.recipient {
                        let disp = book::resolve_name(rcpt).unwrap_or_else(|| rcpt.clone());
                        println!("    to:         {disp}");
                    }
                    // Show the live fetch count (from the relay) over the cap when
                    // the relay reports it; else just the cap.
                    match live_downloads {
                        Some(n) => println!("    downloads:  {n} / {}", r.max_label()),
                        None => println!("    downloads:  {}", r.max_label()),
                    }
                    println!(
                        "    created:    {} ago",
                        human_duration(now_unix().saturating_sub(r.created))
                    );
                    println!("    expires:    {expiry}");
                    println!("    on relay:   {on_relay}");
                    println!("    remove:     arvolo sessions rm {}\n", r.id);
                }
            }

            // Resumable P2P send sessions.
            if !resumable.is_empty() {
                println!("Resumable sends — `arvolo send --resume <id>`:\n");
                for rec in resumable {
                    let kind = if rec.archive { "archive" } else { "file" };
                    println!(
                        "  {}  {}  {} chunk(s), {} bytes  [{kind}]",
                        rec.id, rec.name, rec.chunks, rec.total_size
                    );
                }
            }
        }
        SessionAction::Rm { id } => {
            // A relay-deposit session (link / sealed offline): revoke it on the
            // relay first, then drop the local record — so the file and link
            // stop existing, not just the local bookkeeping.
            if let Some(r) = deposits::load(&id) {
                match flow::revoke_offline(&r.relay, &r.claim, &r.revoke_token).await {
                    Ok(()) => println!(
                        "Revoked on the relay — '{}' is deleted; the link/ticket no longer works.",
                        r.name
                    ),
                    Err(e) => {
                        eprintln!("⚠ relay revoke failed ({e}); removing the local session anyway.")
                    }
                }
                deposits::remove(&id)?;
                println!("Removed session '{id}'.");
                return Ok(());
            }
            sessions::remove(&id)?;
            println!("Removed session '{id}'.");
        }
    }
    Ok(())
}

// ---- identity -------------------------------------------------------------
