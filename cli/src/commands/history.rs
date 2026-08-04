//! `arvolo history` — what already happened.
//!
//! This used to be the bottom section of `arvolo status`, printed in full every
//! time. On a machine that has been used for a while that meant answering "what's
//! going on?" with a hundred lines of what is emphatically *not* going on, the
//! handful of actionable rows buried underneath.
//!
//! The split follows what you can do with each: everything `status` shows can
//! still be cancelled, accepted, paused or resumed. Nothing here can — it is a
//! log. That difference is also why `clear` needs no scope flag on either side:
//! under `status` it drops the rows that are over, here it drops the log, and
//! both are the same sentence — *get rid of what this view shows and is over*.

use anyhow::Result;

use crate::args::HistoryAction;
use crate::commands::status::print_history_row;
use crate::history;

/// How many records `arvolo history` shows before you have to ask for `--all`.
/// Enough to cover "what did I do today" without turning into the wall this
/// command exists to undo.
const RECENT: usize = 20;

pub(crate) fn history_cmd(all: bool, action: Option<HistoryAction>) -> Result<()> {
    if let Some(HistoryAction::Clear) = action {
        let n = history::clear()?;
        println!(
            "Forgot {n} history record(s) — the live list, your relay deposits and your \
             resumable sends are untouched."
        );
        return Ok(());
    }

    // Newest first, per `history::list`.
    let list = history::list();
    if list.is_empty() {
        println!("history: (none)");
        return Ok(());
    }
    let total = list.len();
    let shown: &[_] = if all || total <= RECENT {
        &list
    } else {
        &list[..RECENT]
    };
    if shown.len() < total {
        println!(
            "history: {} most recent of {total} — `--all` for the rest:",
            shown.len()
        );
    } else {
        println!("history: {total} record(s):");
    }
    for rec in shown {
        print_history_row(rec);
    }
    Ok(())
}
