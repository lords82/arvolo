//! What the daemon says, when the daemon is this program.
//!
//! Two audiences at once, and they are not the same. The log lines go to whoever
//! is watching `daemon.log` or the terminal the daemon was started in, and they
//! go out always. The desktop notification goes to the person, and only when
//! nothing else is going to tell them: a running window raises its own, in its own
//! words, and two notifications for one file is how the pair used to arrive.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use arvolo_engine::announce::{Announce, Sender};

use crate::notify;
use crate::ui::sanitize_display;
use crate::util::human_size;

/// The daemon's own voice: a line on the log, and a notification when no window
/// is attached to say it better.
pub(crate) struct Terminal {
    /// Shared with the IPC server, which maintains it.
    pub(crate) front_ends: Arc<AtomicUsize>,
}

impl Terminal {
    /// Nobody attached, so this daemon is the only thing that can speak up.
    fn alone(&self) -> bool {
        self.front_ends.load(Ordering::SeqCst) == 0
    }
}

impl Announce for Terminal {
    fn auto_downloading(&self, name: &str, from: Sender<'_>, size: u64) {
        let size_h = human_size(size);
        let name_s = sanitize_display(name);
        if from.own_device {
            eprintln!("⬇ auto-downloading {name_s} ({size_h}) from another of your devices");
        } else {
            eprintln!(
                "⬇ auto-downloading {name_s} ({size_h}) from trusted {}",
                from.who
            );
        }
        // Still surfaced, so the user knows a trusted download is happening —
        // unless a front-end is attached and will say so itself.
        if self.alone() {
            if from.own_device {
                notify::auto_downloading_own_device(name, &size_h);
            } else {
                notify::auto_downloading(name, from.who, &size_h);
            }
        }
    }

    fn auto_declined(&self, why: &str) {
        eprintln!("   … not automatically: {why}");
        eprintln!("     asking instead.");
    }

    fn auto_download_failed(&self, name: &str, from: Sender<'_>, why: &str) {
        eprintln!("   ✗ could not auto-accept: {why}");
        // Raised even with a window attached — the exception to the rule the
        // others follow. Those are announcements a window makes better in its own
        // words; this one it cannot make at all, because an offer taken
        // automatically is deliberately not drawn as a row. Without this, a failed
        // automatic download is the one event in the app that happens in silence.
        notify::auto_download_failed(name, from.who, why);
    }

    fn offer_waiting(&self, name: &str, from: Sender<'_>, size: u64, handle: &str) {
        let size_h = human_size(size);
        eprintln!(
            "📨 offer waiting: {name} ({size_h}) from {} — take it with `arvolo recv {handle}`",
            from.who
        );
        // Best-effort; a no-op on headless hosts, where the line above stands in.
        if self.alone() {
            notify::offer_awaiting(name, from.who, &size_h);
        }
    }

    fn saved(&self, where_it_is: &str) {
        eprintln!("✓ saved {where_it_is}");
    }
}

#[cfg(test)]
mod tests {
    use super::Terminal;
    use std::sync::atomic::AtomicUsize;
    use std::sync::Arc;

    fn with(count: usize) -> Terminal {
        Terminal {
            front_ends: Arc::new(AtomicUsize::new(count)),
        }
    }

    // Regression: this used to read `!front_ends.load(..) > 0` — a bitwise NOT on
    // usize, true for every count — so the daemon notified on top of the GUI and
    // every offer rang twice.
    #[test]
    fn the_daemon_stays_quiet_while_a_front_end_is_attached() {
        assert!(with(0).alone());
        assert!(!with(1).alone());
        assert!(!with(3).alone());
    }
}
