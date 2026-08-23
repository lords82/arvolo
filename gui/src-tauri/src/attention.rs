//! The tray icon's attention state.
//!
//! No counter and no badge: the mark says it itself. The small V is the file in
//! flight, so while something is waiting on the user it **breathes** — drifting
//! down toward the big V and back up, for exactly as long as anything is pending.
//! That leaves the glyph whole, which a corner badge never managed — at 36px a
//! circle either shrinks to an invisible sliver or eats the big V's right arm
//! until the mark reads as a check.
//!
//! The frames are pre-rendered by `icons/source/gen-attention.py` and are one full
//! breath: frame 0 is the resting position at the top, frame 8 the bottom of the
//! travel, and the sequence loops. Their spacing follows a cosine rather than a
//! ruler — see that file for why the shape lives in the bitmaps now and only the
//! period lives here. Nothing below is macOS-only: every platform goes through
//! `TrayIcon::set_icon`.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Duration;

use arvolo_ipc::client::DaemonClient;
use tauri::image::Image;
use tauri::{AppHandle, Manager};

/// One breath, resting position first.
#[cfg(target_os = "macos")]
const FRAMES: [&[u8]; 16] = [
    include_bytes!("../icons/attention/tray-0.png"),
    include_bytes!("../icons/attention/tray-1.png"),
    include_bytes!("../icons/attention/tray-2.png"),
    include_bytes!("../icons/attention/tray-3.png"),
    include_bytes!("../icons/attention/tray-4.png"),
    include_bytes!("../icons/attention/tray-5.png"),
    include_bytes!("../icons/attention/tray-6.png"),
    include_bytes!("../icons/attention/tray-7.png"),
    include_bytes!("../icons/attention/tray-8.png"),
    include_bytes!("../icons/attention/tray-9.png"),
    include_bytes!("../icons/attention/tray-10.png"),
    include_bytes!("../icons/attention/tray-11.png"),
    include_bytes!("../icons/attention/tray-12.png"),
    include_bytes!("../icons/attention/tray-13.png"),
    include_bytes!("../icons/attention/tray-14.png"),
    include_bytes!("../icons/attention/tray-15.png"),
];
#[cfg(not(target_os = "macos"))]
const FRAMES: [&[u8]; 16] = [
    include_bytes!("../icons/attention/app-0.png"),
    include_bytes!("../icons/attention/app-1.png"),
    include_bytes!("../icons/attention/app-2.png"),
    include_bytes!("../icons/attention/app-3.png"),
    include_bytes!("../icons/attention/app-4.png"),
    include_bytes!("../icons/attention/app-5.png"),
    include_bytes!("../icons/attention/app-6.png"),
    include_bytes!("../icons/attention/app-7.png"),
    include_bytes!("../icons/attention/app-8.png"),
    include_bytes!("../icons/attention/app-9.png"),
    include_bytes!("../icons/attention/app-10.png"),
    include_bytes!("../icons/attention/app-11.png"),
    include_bytes!("../icons/attention/app-12.png"),
    include_bytes!("../icons/attention/app-13.png"),
    include_bytes!("../icons/attention/app-14.png"),
    include_bytes!("../icons/attention/app-15.png"),
];

const RESTING: usize = 0;
/// The bottom of the travel. Frames below it are on the way down, above it on the
/// way back up, which is all `settle` needs to know to pick the shorter way home.
const BOTTOM: usize = FRAMES.len() / 2;

/// How long one breath takes, in milliseconds. Every frame is held for the same
/// slice of it — the cosine spacing of the bitmaps is what makes the mark dwell at
/// the top and the bottom and hurry through the middle, so there is no schedule
/// here to disagree with, and this is the one number worth retuning.
///
/// The travel itself is `AMPLITUDE` in `icons/source/gen-attention.py`, and it is
/// measured in *points*: it was set to 12 (~2.2pt) on a reading of "four and a
/// half pixels", which is the same distance counted in Retina pixels and half the
/// size the eye is offered. At 2.2pt the breath is running and invisible, and gets
/// reported as an animation that does not work.
///
/// 3.6 seconds was picked over 2.4 and 1.6 played side by side at this exact size:
/// slower reads as "waiting for you", faster reads as a pulse and wears out its
/// welcome in a bar the user stares at all day. It also matters more than it used
/// to that this number be generous — the old fall ran three times and stopped,
/// while this runs until the last request has been dealt with, so at sixteen
/// frames it repaints the tray about four and a half times a second, indefinitely.
const BREATH_MS: u64 = 3600;
const STEP_MS: u64 = BREATH_MS / FRAMES.len() as u64;

/// Whether anything is pending, plus a generation counter that lets a newer call
/// retire an animation still in flight — otherwise a burst of arrivals would leave
/// several loops fighting over the icon.
#[derive(Default)]
pub struct State {
    pending: AtomicBool,
    generation: AtomicU64,
}

/// How many things are waiting for the user to decide.
///
/// Only decisions count. A finished download already fired its notification, and
/// folding those in would turn this into something to clear rather than something
/// to read. Trusted senders never land here either: the daemon auto-accepts them,
/// so they never reach the pending list and the small V stays up.
async fn pending_count() -> Option<usize> {
    let mut c = DaemonClient::connect().await.ok()?;
    let offers = c.list_pending().await.ok()?.len();
    // A contact advertising a new display name blocks on the user the same way an
    // offer does: nothing moves until it is accepted or ignored.
    let names = c
        .list_contacts()
        .await
        .ok()?
        .iter()
        .filter(|k| !k.pending_name.is_empty())
        .count();
    Some(offers + names)
}

/// Ask the daemon what is pending and set the tray to match. Quiet on failure: a
/// state that cannot be read is not worth an error in the UI, and the next event
/// runs this again anyway.
pub async fn refresh(app: &AppHandle) {
    if let Some(n) = pending_count().await {
        apply(app, n > 0);
    }
}

/// Drive the tray to `pending`.
///
/// Only a *change* does anything. A second arrival while the mark is already
/// breathing must be a no-op: the breath is already saying that something is
/// waiting, and restarting it would snap the small V back to the top mid-stroke
/// for no reason the user could name. Clearing the last request does not paint
/// anything either — it flips the flag and lets the running loop walk itself home,
/// which is the only way the return is smooth.
pub fn apply(app: &AppHandle, pending: bool) {
    let state = app.state::<State>();
    if state.pending.swap(pending, Ordering::SeqCst) == pending {
        return;
    }
    if !pending {
        return;
    }
    // Retire whatever loop may still be settling: its generation is now stale.
    let generation = state.generation.fetch_add(1, Ordering::SeqCst) + 1;
    let app = app.clone();
    tauri::async_runtime::spawn(async move { breathe(app, generation).await });
}

/// Breathe until nothing is pending, then walk back to rest. Bails the moment a
/// newer call bumps the generation, so the last caller always owns the icon.
async fn breathe(app: AppHandle, generation: u64) {
    let live = || {
        app.try_state::<State>()
            .is_some_and(|s| s.generation.load(Ordering::SeqCst) == generation)
    };
    let pending = || {
        app.try_state::<State>()
            .is_some_and(|s| s.pending.load(Ordering::SeqCst))
    };

    let mut frame = RESTING;
    loop {
        if !live() {
            return;
        }
        set_frame(&app, frame);
        tokio::time::sleep(Duration::from_millis(STEP_MS)).await;
        if !pending() {
            break;
        }
        frame = (frame + 1) % FRAMES.len();
    }
    settle(&app, frame, live).await;
}

/// Walk from `from` back to the resting frame the short way round.
///
/// Cutting straight to frame 0 would be a jump of up to twelve units — visible,
/// and it would land on the one moment the user is least likely to be looking at
/// the menu bar, having just dealt with the thing that was waiting. Finishing the
/// whole breath instead would keep the mark moving for up to another 3.6 seconds
/// over nothing. So it retreats the way it came if it has not reached the bottom
/// yet, and carries on round if it has: either way home is at most half a breath.
async fn settle(app: &AppHandle, from: usize, live: impl Fn() -> bool) {
    let back: Vec<usize> = if from <= BOTTOM {
        (0..from).rev().collect()
    } else {
        (from + 1..FRAMES.len())
            .chain(std::iter::once(RESTING))
            .collect()
    };
    for frame in back {
        if !live() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(STEP_MS)).await;
        set_frame(app, frame);
    }
}

/// Paint one frame onto the tray icon.
fn set_frame(app: &AppHandle, frame: usize) {
    let Some(tray) = app.tray_by_id("main") else {
        return;
    };
    let Ok(icon) = Image::from_bytes(FRAMES[frame]) else {
        return;
    };
    // Atomic on macOS: setting the icon and the template flag separately makes the
    // menu bar render it twice and flicker — which an animation would show off.
    let _ = tray.set_icon_with_as_template(Some(icon), cfg!(target_os = "macos"));
}
