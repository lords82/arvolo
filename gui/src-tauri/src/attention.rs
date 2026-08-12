//! The tray icon's attention state.
//!
//! No counter and no badge: the mark says it itself. The wedge is the file in
//! flight, so when something is waiting on the user it **falls into the box** and
//! stays there; at rest it sits above, still in the air. That leaves the glyph
//! whole, which a corner badge never managed — at 36px a circle either shrinks to
//! an invisible sliver or eats the box's right arm until the mark reads as an L.
//!
//! The frames are pre-rendered by `icons/source/gen-attention.py`. Frame 0 is the
//! resting state, the last frame is the landed one that the tray holds for as long
//! as anything is pending, and the ones between are only played on the way in.
//! Nothing here is macOS-only: every platform goes through `TrayIcon::set_icon`.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Duration;

use arvolo_ipc::client::DaemonClient;
use tauri::image::Image;
use tauri::{AppHandle, Manager};

/// Frames of the wedge dropping, resting state first.
#[cfg(target_os = "macos")]
const FRAMES: [&[u8]; 8] = [
    include_bytes!("../icons/attention/tray-0.png"),
    include_bytes!("../icons/attention/tray-1.png"),
    include_bytes!("../icons/attention/tray-2.png"),
    include_bytes!("../icons/attention/tray-3.png"),
    include_bytes!("../icons/attention/tray-4.png"),
    include_bytes!("../icons/attention/tray-5.png"),
    include_bytes!("../icons/attention/tray-6.png"),
    include_bytes!("../icons/attention/tray-7.png"),
];
#[cfg(not(target_os = "macos"))]
const FRAMES: [&[u8]; 8] = [
    include_bytes!("../icons/attention/app-0.png"),
    include_bytes!("../icons/attention/app-1.png"),
    include_bytes!("../icons/attention/app-2.png"),
    include_bytes!("../icons/attention/app-3.png"),
    include_bytes!("../icons/attention/app-4.png"),
    include_bytes!("../icons/attention/app-5.png"),
    include_bytes!("../icons/attention/app-6.png"),
    include_bytes!("../icons/attention/app-7.png"),
];

const RESTING: usize = 0;
const LANDED: usize = FRAMES.len() - 1;
/// Two falls is enough to catch the eye without becoming a fidget. A menu bar
/// item that moves forever is an irritant, and it keeps the CPU awake for nothing.
const CYCLES: usize = 2;
const FRAME_MS: u64 = 55;
/// A beat on the landed frame before the wedge lifts back up for the second fall,
/// so the two reads as two arrivals rather than one stutter.
const HOLD_MS: u64 = 260;

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
/// so they never reach the pending list and the wedge stays up.
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

/// Drive the tray to `pending`. Going from nothing to something plays the drop;
/// staying pending just holds the landed frame, so a second arrival while one is
/// already waiting does not re-animate.
pub fn apply(app: &AppHandle, pending: bool) {
    let state = app.state::<State>();
    let was = state.pending.swap(pending, Ordering::SeqCst);
    // Retire whatever loop may be running: its generation is now stale.
    let generation = state.generation.fetch_add(1, Ordering::SeqCst) + 1;

    if !pending {
        set_frame(app, RESTING);
        return;
    }
    if was {
        set_frame(app, LANDED);
        return;
    }

    let app = app.clone();
    tauri::async_runtime::spawn(async move { fall(app, generation).await });
}

/// Play the drop, then leave the wedge in the box. Bails the moment a newer call
/// bumps the generation, so the last caller always owns the final frame.
async fn fall(app: AppHandle, generation: u64) {
    let current = || {
        app.try_state::<State>()
            .map(|s| s.generation.load(Ordering::SeqCst))
    };
    for cycle in 0..CYCLES {
        for frame in 0..FRAMES.len() {
            if current() != Some(generation) {
                return;
            }
            set_frame(&app, frame);
            tokio::time::sleep(Duration::from_millis(FRAME_MS)).await;
        }
        // Rest on the floor between falls, and after the last one leave it there.
        if cycle + 1 < CYCLES {
            tokio::time::sleep(Duration::from_millis(HOLD_MS)).await;
        }
    }
    if current() == Some(generation) {
        set_frame(&app, LANDED);
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
