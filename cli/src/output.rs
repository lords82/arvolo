//! Which channel a sentence goes out on.
//!
//! `docs/COPY.md` splits everything the user reads into three slots: what
//! happened, what they can do about it, and why. The first two go to stderr
//! unconditionally — an action nobody sees is an action nobody takes. The third
//! is what [`vprintln!`] is for: still there, still true, and out of the way of
//! somebody who has read it before.
//!
//! An error cannot use that split on its own, because an `anyhow::Error` is a
//! value travelling up the stack rather than a line being printed. [`refuse`]
//! is the same shape for that case.

use std::fmt::Display;
use std::sync::atomic::{AtomicU8, Ordering};

/// Process-global verbosity, set once from the `-v/--verbose` count in `main`.
/// Read by [`verbosity`] and the [`vprintln!`] macro so command functions and
/// their event callbacks can narrate extra detail without threading a flag
/// through every signature.
pub(crate) static VERBOSITY: AtomicU8 = AtomicU8::new(0);

/// How many `-v` the user passed (0 = quiet, 1 = steps, 2+ = network internals).
pub(crate) fn verbosity() -> u8 {
    VERBOSITY.load(Ordering::Relaxed)
}

/// `eprintln!` that only fires at `-v` or higher — a step explanation for users
/// who want to follow (or debug) what a command is doing. Prefixed so verbose
/// narration is easy to tell apart from normal output.
macro_rules! vprintln {
    ($($arg:tt)*) => {
        if crate::output::verbosity() >= 1 {
            eprintln!("· {}", format!($($arg)*));
        }
    };
}

/// Like [`vprintln!`] but only at `-vv` or higher — finer-grained detail (e.g.
/// per-source transitions) that would be too chatty at a single `-v`.
macro_rules! vvprintln {
    ($($arg:tt)*) => {
        if crate::output::verbosity() >= 2 {
            eprintln!("·· {}", format!($($arg)*));
        }
    };
}

/// Build the tracing filter from the `-v` count, unless `RUST_LOG` is set (an
/// explicit filter always wins).
///
/// The transfer narration at `-v`/`-vv` is printed by the CLI itself (see
/// [`vprintln!`]), not by tracing — so tracing's job here is the opposite of
/// noisy: keep iroh's chatty transport logs (relay probing, the IPv6
/// "no route to host" warning, per-datagram errors) from drowning that
/// narration. iroh is muted at `-v`/`-vv` and only comes back at `-vvv`, where
/// you're explicitly asking for the raw networking firehose.
pub(crate) fn init_tracing(verbose: u8) {
    let default = match verbose {
        // No flag: unchanged — warnings from every crate (incl. iroh) surface.
        0 => "warn",
        // -v / -vv: arvolo's narration carries the story; drop iroh below warn so
        // its transport chatter doesn't bury it. Genuine iroh *errors* still pass.
        1 | 2 => {
            "warn,iroh=error,iroh_quinn=error,iroh_quinn_udp=error,\
             iroh_quinn_proto=error,iroh_relay=error,iroh_net=error,iroh_base=error"
        }
        // -vvv+: raw iroh networking logs for deep debugging.
        _ => "info,iroh=debug",
    };
    let filter =
        tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| default.into());
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(std::io::stderr)
        .init();
}

/// Refuse to do something: `short` becomes the error, `why` goes to `-v`.
///
/// The three slots do not apply to an error by themselves. `bail!` and `ensure!`
/// build a value that some caller will print much later, so there is no second
/// channel to put the reason on — which is how the longest strings in this
/// binary came to be errors, one of them 299 characters explaining a config key
/// to somebody who only needed to know what to run instead.
///
/// So `short` carries the outcome **and the way out** — never demote the way out,
/// or the refusal is a dead end — and `why` carries the reasoning, printed now if
/// the user asked for it and dropped if they did not.
///
/// Note that `why` reaches the terminal *before* the error does, since the error
/// is printed by whoever unwinds to it. That reads correctly with the `·` prefix
/// (it is narration arriving in order) but it is worth knowing when reading a
/// `-v` transcript.
pub(crate) fn refuse(short: impl Display, why: impl Display) -> anyhow::Error {
    vprintln!("{why}");
    anyhow::anyhow!("{short}")
}

pub(crate) use {vprintln, vvprintln};
