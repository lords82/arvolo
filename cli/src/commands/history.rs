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

pub(crate) fn history_cmd(action: Option<HistoryAction>) -> Result<()> {
    if let Some(HistoryAction::Clear) = action {
        let n = history::clear()?;
        println!(
            "Forgot {n} history record(s) — the live list, your relay deposits and your \
             resumable sends are untouched."
        );
        return Ok(());
    }

    // Newest first, per `history::list` — all of it. There used to be a 20-row
    // cap with an `--all` to undo it, i.e. a flag to undo a truncation this
    // command applied to itself; the pipe is the pager (`arvolo history | head`
    // works — SIGPIPE is restored at startup for exactly this).
    let list = history::list();
    if list.is_empty() {
        println!("history: (none)");
        return Ok(());
    }
    println!("history: {} record(s):", list.len());
    for rec in &list {
        print_history_row(rec);
    }
    Ok(())
}
