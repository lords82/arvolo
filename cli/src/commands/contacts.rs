use std::io::IsTerminal;

use anyhow::{Context, Result};

use crate::book;

use crate::args::ContactAction;
use crate::ui::*;
use crate::util::*;

/// How long one presence probe may take. The probes run concurrently, so this is
/// also roughly the worst case for the whole listing — where before, a relay that
/// accepted the connection and then went quiet could hang the command forever,
/// once per contact, because the shared client sets no timeout at all.
const PRESENCE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(3);

/// One contact, with everything the listing shows about them already gathered.
struct Row {
    name: String,
    id: String,
    fingerprint: Option<String>,
    verified: bool,
    trusted: bool,
    /// Approved advertised name, already sanitized.
    advertised: Option<String>,
    /// Advertised name awaiting approval, already sanitized.
    pending: Option<String>,
    /// `Some(true)` online, `Some(false)` offline, `None` not asked or not known.
    online: Option<bool>,
    /// Seconds since the verified mark was made; `None` when unverified, or
    /// verified before the stamp was recorded.
    verified_ago: Option<u64>,
}

/// `arvolo contacts list` — the address book, and who is reachable right now.
async fn list_contacts(filter: Option<String>, no_presence: bool, json: bool) -> Result<()> {
    let mut list = book::contact_list();
    if list.is_empty() {
        eprintln!("(no contacts yet — add one: arvolo contacts add <name> <id>)");
    }
    // Optional filter: match a full/prefix public id (case-insensitive) or a
    // substring of the contact name.
    if let Some(q) = &filter {
        let needle = q.to_lowercase();
        list.retain(|(name, id)| {
            id.to_lowercase().starts_with(&needle) || name.to_lowercase().contains(&needle)
        });
        if list.is_empty() {
            eprintln!("(no contact matching '{q}')");
        }
    }

    let mut rows: Vec<Row> = list
        .into_iter()
        .map(|(name, id)| Row {
            fingerprint: book::fingerprint_of(&id),
            verified: book::is_verified(&id),
            verified_ago: book::verified_since(&id).and_then(book::marked_ago),
            trusted: book::is_trusted(&id),
            advertised: book::display_name_of(&id).map(|n| sanitize_display(&n)),
            pending: book::pending_name_of(&id).map(|n| sanitize_display(&n)),
            online: None,
            name,
            id,
        })
        .collect();

    if !no_presence {
        probe_presence(&mut rows).await;
    }

    if json {
        print_json(&rows);
    } else {
        print_table(&rows, !no_presence);
    }
    Ok(())
}

/// Ask the relay who is online, all at once.
///
/// `default_relay_or_builtin` rather than `default_relay`: a zero-config install
/// still sends through the built-in relay, so reporting "unknown" for everyone
/// would describe a machine that isn't this one.
async fn probe_presence(rows: &mut [Row]) {
    let Some(relay) = book::default_relay_or_builtin() else {
        return;
    };
    let client = arvolo_core::http::client_with_timeout(PRESENCE_TIMEOUT);

    let mut set = tokio::task::JoinSet::new();
    for (idx, row) in rows.iter().enumerate() {
        let Ok(pk) = book::decode_id(&row.id) else {
            continue;
        };
        let (client, relay) = (client.clone(), relay.clone());
        set.spawn(async move {
            (
                idx,
                arvolo_core::presence::check_online(&client, &relay, &pk).await,
            )
        });
    }

    // A network failure is NOT "offline": collapsing the two is what made a dead
    // relay look exactly like everyone being away. Track it so we can say so.
    let (mut asked, mut failed) = (0usize, 0usize);
    while let Some(joined) = set.join_next().await {
        let Ok((idx, result)) = joined else { continue };
        asked += 1;
        match result {
            Ok(online) => rows[idx].online = Some(online),
            Err(_) => failed += 1,
        }
    }
    if asked > 0 && failed == asked {
        eprintln!("(the relay didn't answer — online status is unknown, not offline)");
    }
}

/// Render the listing, dropping any column nothing has anything to say in — an
/// empty column is width spent on the absence of information.
fn print_table(rows: &[Row], asked_presence: bool) {
    // Widths from the content, so ids and fingerprints line up instead of relying
    // on tab stops that break the moment one name is long.
    let name_w = rows
        .iter()
        .map(|r| display_width(&r.name))
        .max()
        .unwrap_or(0);
    let label_w = rows
        .iter()
        .map(|r| r.advertised.as_deref().map_or(0, display_width))
        .max()
        .unwrap_or(0);
    // "verified 3mo ago" only earns a column if some row can fill it.
    let ages: Vec<String> = rows
        .iter()
        .map(|r| r.verified_ago.map(human_duration).unwrap_or_default())
        .collect();
    let age_w = ages.iter().map(|a| display_width(a)).max().unwrap_or(0);

    for (i, r) in rows.iter().enumerate() {
        let mut line = String::new();
        if asked_presence {
            line.push_str(match r.online {
                Some(true) => "● ",
                Some(false) => "○ ",
                None => "? ",
            });
        }
        line.push_str(match (r.verified, r.trusted) {
            (true, true) => "✓ ⬇ ",
            (true, false) => "✓   ",
            (false, true) => "  ⬇ ",
            (false, false) => "    ",
        });
        line.push_str(&r.name);
        line.push_str(&" ".repeat(name_w - display_width(&r.name)));
        if label_w > 0 {
            let label = r.advertised.as_deref().unwrap_or("");
            if label.is_empty() {
                line.push_str(&" ".repeat(label_w + 3));
            } else {
                line.push_str(&format!(
                    "  “{label}”{}",
                    " ".repeat(label_w - display_width(label))
                ));
            }
        }
        if age_w > 0 {
            line.push_str(&format!("  {:<age_w$}", ages[i]));
        }
        line.push_str(&format!("  {}", r.id));
        if let Some(fp) = &r.fingerprint {
            line.push_str(&format!("  {fp}"));
        }
        println!("{line}");

        if let Some(p) = &r.pending {
            println!(
                "    ⚠ wants to be called “{p}” — approve: arvolo contacts accept-name {}",
                r.name
            );
        }
    }
}

/// Character count, which is what a monospace terminal advances by for the text
/// this listing prints (`sanitize_display` has already dropped the control and
/// bidi characters that would make this a lie).
fn display_width(s: &str) -> usize {
    s.chars().count()
}

fn print_json(rows: &[Row]) {
    let out: Vec<_> = rows
        .iter()
        .map(|r| {
            serde_json::json!({
                "name": r.name,
                "id": r.id,
                "fingerprint": r.fingerprint,
                "verified": r.verified,
                "verified_seconds_ago": r.verified_ago,
                "trusted": r.trusted,
                "advertised_name": r.advertised,
                "pending_name": r.pending,
                "online": r.online,
            })
        })
        .collect();
    match serde_json::to_string_pretty(&out) {
        Ok(s) => println!("{s}"),
        Err(e) => eprintln!("(could not render JSON: {e})"),
    }
}

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
                // Say *both*: `contact_add` clears trusted as well, so someone who
                // had this contact auto-downloading has just lost that too, and
                // needs to know it isn't coming back on its own.
                eprintln!(
                    "   The 'verified' and 'trusted' marks were both cleared — their files\n   \
                     will ask for approval again. Confirm the new fingerprint out-of-band,"
                );
                eprintln!("   then: arvolo contacts verify {name}");
            }
        }
        ContactAction::List {
            filter,
            no_presence,
            json,
        } => return list_contacts(filter, no_presence, json).await,
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
        ContactAction::Rename { old, new } => {
            book::contact_rename(&old, &new)?;
            println!("Renamed '{old}' to '{new}' — verified and trusted marks kept.");
        }
        ContactAction::Pair {
            code,
            name,
            relay,
            use_http,
            qr,
        } => {
            return match code {
                Some(c) => crate::commands::pair::pair_join(c, name).await,
                None => crate::commands::pair::pair_host(name, relay, use_http, qr).await,
            }
        }
        ContactAction::Export => {
            // Same shape `list --json` produces, minus the things that describe
            // *this* machine rather than the book: presence is a live reading, and
            // a pending name is an unanswered question only you can answer.
            let rows: Vec<_> = book::contact_list()
                .into_iter()
                .map(|(name, id)| {
                    serde_json::json!({
                        "name": name,
                        "id": id,
                        "verified": book::is_verified(&id),
                        "trusted": book::is_trusted(&id),
                    })
                })
                .collect();
            println!("{}", serde_json::to_string_pretty(&rows)?);
            eprintln!("({} contact(s) exported)", rows.len());
        }
        ContactAction::Import { file, with_marks } => {
            let text = if file == "-" {
                std::io::read_to_string(std::io::stdin()).context("read stdin")?
            } else {
                std::fs::read_to_string(&file).with_context(|| format!("read {file}"))?
            };
            let rows: Vec<serde_json::Value> =
                serde_json::from_str(&text).context("parse the address book (expected JSON)")?;

            let existing: std::collections::BTreeSet<String> =
                book::contact_list().into_iter().map(|(n, _)| n).collect();
            let (mut added, mut skipped, mut marked) = (0usize, 0usize, 0usize);
            for row in rows {
                let (Some(name), Some(id)) = (
                    row.get("name").and_then(|v| v.as_str()),
                    row.get("id").and_then(|v| v.as_str()),
                ) else {
                    continue;
                };
                // Never rebind a name that is already yours: an import that
                // overwrote an existing contact's key would be a key change nobody
                // asked about, which is the one thing the whole trust model tries
                // to make impossible to do by accident.
                if existing.contains(name) {
                    eprintln!("   skipped '{name}' — already a contact here");
                    skipped += 1;
                    continue;
                }
                if book::contact_add(name, id).is_err() {
                    eprintln!("   skipped '{name}' — not a valid public id");
                    skipped += 1;
                    continue;
                }
                added += 1;
                if with_marks {
                    if row.get("verified").and_then(|v| v.as_bool()) == Some(true) {
                        book::mark_verified(name).ok();
                        marked += 1;
                    }
                    if row.get("trusted").and_then(|v| v.as_bool()) == Some(true) {
                        book::mark_trusted(name).ok();
                    }
                }
            }
            println!("Imported {added} contact(s); {skipped} skipped.");
            if with_marks {
                eprintln!("   {marked} imported as verified — you did not check those fingerprints yourself.");
            } else if added > 0 {
                eprintln!("   All unverified: verify each one out-of-band with `arvolo contacts verify <name>`.");
            }
        }
        ContactAction::Block { who } => match who {
            None => {
                let list = book::blocked_list();
                if list.is_empty() {
                    eprintln!("(nobody is blocked)");
                }
                for (id, since) in list {
                    let when = match book::marked_ago(since) {
                        Some(secs) => format!("  (blocked {} ago)", human_duration(secs)),
                        None => String::new(),
                    };
                    match book::resolve_name(&id) {
                        Some(name) => println!("{name}\t{id}{when}"),
                        None => println!("{id}{when}"),
                    }
                }
            }
            Some(who) => {
                let id = book::mark_blocked(&who)?;
                println!("Blocked '{who}' — their offers are dropped on arrival, silently.");
                eprintln!("   id: {id}");
                eprintln!("   Undo with: arvolo contacts unblock {who}");
            }
        },
        ContactAction::Unblock { who } => {
            if book::unmark_blocked(&who)? {
                println!("Unblocked '{who}' — their offers reach you again.");
            } else {
                eprintln!("'{who}' was not blocked — nothing to undo.");
            }
        }
        ContactAction::Prune => {
            let n = book::prune_orphan_names()?;
            match n {
                0 => eprintln!("(nothing to prune)"),
                1 => println!("Dropped 1 leftover advertised-name record."),
                n => println!("Dropped {n} leftover advertised-name records."),
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
            // Keyed by id, not by the name typed — `is_verified` reads the ledger,
            // which is id-keyed, and `id` was already resolved above for the
            // fingerprint.
            let already = book::is_verified(&id);
            book::mark_verified(&name)?;
            if already {
                println!("Re-verified '{name}' — the clock on that check starts again now.");
            } else {
                println!("Marked '{name}' verified.");
            }
        }
        ContactAction::Unverify { name } => {
            // Report what actually happened: claiming to have cleared a mark that
            // was never set reads as "done" for a security state the user may
            // have meant to change on a different contact.
            if book::unmark_verified(&name)? {
                println!("Cleared verified mark for '{name}'.");
            } else {
                eprintln!("'{name}' was not marked verified — nothing to clear.");
            }
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
            if book::unmark_trusted(&name)? {
                println!("Cleared trust for '{name}' — their files will ask for approval again.");
            } else {
                eprintln!("'{name}' was not trusted — nothing to clear.");
            }
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
                // The approved name is the sender's own text — sanitize it here as
                // everywhere else, or the one place we echo it back becomes the
                // way to get escape sequences onto the user's terminal.
                println!(
                    "Approved “{}” as the advertised name for '{who}'.",
                    sanitize_display(&approved)
                );
            }
        }
    }
    Ok(())
}
