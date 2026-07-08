use std::io::IsTerminal;

use anyhow::{Context, Result};

use crate::book;

use crate::args::ContactAction;
use crate::ui::*;
use crate::util::*;

pub(crate) async fn contacts_cmd(action: ContactAction) -> Result<()> {
    match action {
        ContactAction::Add { name, id } => {
            let key_change = book::contact_add(&name, &id)?;
            println!("Saved contact '{name}'.");
            if let Some(kc) = key_change {
                eprintln!(
                    "\n⚠  The key for '{name}' CHANGED — this could be a reinstall, or a MITM."
                );
                eprintln!("      was fingerprint: {}", kc.old_fingerprint);
                eprintln!("      now fingerprint: {}", kc.new_fingerprint);
                eprintln!(
                    "   The 'verified' mark was cleared. Confirm the new fingerprint out-of-band,"
                );
                eprintln!("   then: arvolo contacts verify {name}");
            }
        }
        ContactAction::List { filter } => {
            let mut list = book::contact_list();
            if list.is_empty() {
                eprintln!("(no contacts yet — add one: arvolo contacts add <name> <id>)");
            }
            // Optional filter: match a full/prefix public id (case-insensitive)
            // or a substring of the contact name.
            if let Some(q) = &filter {
                let needle = q.to_lowercase();
                list.retain(|(name, id)| {
                    id.to_lowercase().starts_with(&needle) || name.to_lowercase().contains(&needle)
                });
                if list.is_empty() {
                    eprintln!("(no contact matching '{q}')");
                }
            }
            // Query presence per contact, if a relay is configured.
            let relay = book::default_relay();
            let client = reqwest::Client::new();
            for (name, id) in list {
                let verified = if book::is_verified(&id) { " ✓" } else { "" };
                let trusted = if book::is_trusted(&id) {
                    " ⬇trusted"
                } else {
                    ""
                };
                let status = match (&relay, book::resolve_recipient(&id).ok()) {
                    (Some(r), Some(pk)) => {
                        if arvolo_core::presence::check_online(&client, r, &pk)
                            .await
                            .unwrap_or(false)
                        {
                            "●"
                        } else {
                            "○"
                        }
                    }
                    _ => "?",
                };
                // The sender's advertised (self-chosen) name: pinned shown inline,
                // a pending change flagged for approval. The local contact name
                // stays the primary label.
                let advertised = match book::display_name_of(&id) {
                    Some(n) => format!("  “{}”", sanitize_display(&n)),
                    None => String::new(),
                };
                match book::fingerprint_of(&id) {
                    Some(fp) => {
                        println!("{status} {name}{verified}{trusted}{advertised}\t{id}\t({fp})")
                    }
                    None => println!("{status} {name}{verified}{trusted}{advertised}\t{id}"),
                }
                if let Some(p) = book::pending_name_of(&id) {
                    println!("    ⚠ wants to be called “{}” — approve: arvolo contacts accept-name {name}", sanitize_display(&p));
                }
            }
        }
        ContactAction::Remove { name } => {
            // Clear the ledgers first — they resolve the id via the contact name,
            // which is gone once removed.
            book::unmark_verified(&name).ok();
            book::unmark_trusted(&name).ok();
            if book::contact_remove(&name)? {
                println!("Removed contact '{name}'.");
            } else {
                eprintln!("No such contact '{name}'.");
            }
        }
        ContactAction::Verify { name, yes } => {
            // Show the fingerprint FIRST and require an explicit confirmation, so
            // marking verified is a deliberate act — not a side effect of running
            // the command to read the fingerprint.
            let id = encode_id(&book::resolve_recipient(&name)?);
            let fp = book::fingerprint_of(&id).unwrap_or_default();
            println!("Fingerprint of '{name}':  {fp}");
            if !yes {
                if !std::io::stdin().is_terminal() {
                    anyhow::bail!(
                        "not a terminal — pass --yes to confirm you've checked the fingerprint \
                         out-of-band: arvolo contacts verify {name} --yes"
                    );
                }
                use std::io::Write;
                print!("Have you confirmed this fingerprint out-of-band? [y/N]: ");
                let _ = std::io::stdout().flush();
                let mut line = String::new();
                std::io::stdin().read_line(&mut line).ok();
                if !matches!(line.trim().to_lowercase().as_str(), "y" | "yes") {
                    eprintln!("Aborted — '{name}' left unverified.");
                    return Ok(());
                }
            }
            book::mark_verified(&name)?;
            println!("Marked '{name}' verified.");
        }
        ContactAction::Unverify { name } => {
            book::unmark_verified(&name)?;
            println!("Cleared verified mark for '{name}'.");
        }
        ContactAction::Trust { name, force } => {
            // Trust means auto-download without a prompt, so it must sit on a key
            // you've confirmed is really theirs. Refuse an unverified contact
            // unless the user explicitly overrides with --force.
            let id = encode_id(&book::resolve_recipient(&name)?);
            if !book::is_verified(&id) && !force {
                let fp = book::fingerprint_of(&id).unwrap_or_default();
                anyhow::bail!(
                    "'{name}' isn't verified — trusting it would auto-download from a key you \
                     haven't confirmed out-of-band (MITM risk).\n   fingerprint: {fp}\n   \
                     Verify first: arvolo contacts verify {name}   (or override: \
                     arvolo contacts trust {name} --force)"
                );
            }
            let id = book::mark_trusted(&name)?;
            let fp = book::fingerprint_of(&id).unwrap_or_default();
            println!("Trusting '{name}' — files from them auto-download without a prompt.");
            eprintln!("   (fingerprint: {fp})");
            if !book::is_verified(&id) {
                eprintln!(
                    "   ⚠  trusted WITHOUT verification (--force) — confirm the fingerprint \
                     out-of-band, then: arvolo contacts verify {name}"
                );
            }
        }
        ContactAction::Untrust { name } => {
            book::unmark_trusted(&name)?;
            println!("Cleared trust for '{name}' — their files will ask for approval again.");
        }
        ContactAction::AcceptName { who, all } => {
            if all {
                let n = book::accept_all_names()?;
                match n {
                    0 => eprintln!("(no pending names to approve)"),
                    1 => println!("Approved 1 advertised name."),
                    n => println!("Approved {n} advertised names."),
                }
            } else {
                let who = who.context(
                    "give a contact name or id (or --all): arvolo contacts accept-name <who>",
                )?;
                let approved = book::accept_name(&who)?;
                println!("Approved “{approved}” as the advertised name for '{who}'.");
            }
        }
    }
    Ok(())
}
