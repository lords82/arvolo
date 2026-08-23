//! In-app updates: ask whether a newer release exists, and install it.
//!
//! Everything here runs in the Rust process rather than through the updater
//! plugin's JavaScript side, for one reason: installing a version that is *not*
//! the latest one needs that release's own manifest, and only the Rust builder
//! lets an endpoint be chosen per call. The webview is granted no updater
//! permission at all and reaches this through the two commands below.
//!
//! The order of operations in [`install_update`] is the part worth reading. The
//! app ships the CLI beside its own executable and runs a *detached* daemon from
//! it, so the process that has to survive the installer is not the one Tauri
//! knows about. See the comments there.

use serde::Serialize;
use tauri::{AppHandle, Emitter};
use tauri_plugin_updater::UpdaterExt;

use crate::daemon;

/// Where releases live. Taken from the crate metadata rather than written out,
/// so it cannot drift from `Cargo.toml` — and the `endpoints` in
/// `tauri.conf.json` is the same host and repository spelled once more, which is
/// the one copy that has to be kept in step by hand.
const REPOSITORY: &str = env!("CARGO_PKG_REPOSITORY");

/// Progress of a running download, pushed to the webview.
const EV_UPDATE_PROGRESS: &str = "update://progress";

/// What is on offer, as the UI needs it.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateInfo {
    /// The version to install. Normally the newest release; the newest one this
    /// machine's relay can carry, when those differ.
    pub version: String,
    /// What this app is running now — so the banner can name both.
    pub current: String,
    /// The release notes, as the manifest carries them (often empty).
    pub notes: String,
    /// Set when the newest release was *not* offered, and says what it was. The
    /// UI turns this into "0.15.0 is out, but your relay only reaches 0.14.0".
    #[serde(skip_serializing_if = "Option::is_none")]
    pub held_back: Option<HeldBack>,
}

/// Why a newer release was passed over.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HeldBack {
    /// The newest release, the one not being offered.
    pub latest: String,
    /// The relay standing in the way, and what it says it is — both shown,
    /// because "update your relay" is not an instruction anyone can follow
    /// without being told which one.
    pub relay: Option<String>,
    pub relay_version: Option<String>,
}

/// Bytes so far and, when the server said, the total.
#[derive(Debug, Clone, Serialize)]
struct Progress {
    downloaded: u64,
    total: Option<u64>,
}

/// An updater pointed either at `latest.json` (the configured endpoint) or at
/// one specific release's copy of it.
///
/// The pinned form exists because "the newest release" and "the newest release
/// this user's relay can talk to" are not always the same version — see the
/// compatibility check in the frontend. Every release publishes its own
/// `latest.json` among its assets, so pinning is just a different URL.
fn updater(app: &AppHandle, pinned: Option<&str>) -> Result<tauri_plugin_updater::Updater, String> {
    let mut builder = app.updater_builder();
    if let Some(version) = pinned {
        let url = format!("{REPOSITORY}/releases/download/v{version}/latest.json");
        let url = tauri::Url::parse(&url).map_err(|e| format!("{url}: {e}"))?;
        builder = builder
            .endpoints(vec![url])
            .map_err(|e| format!("update endpoint: {e}"))?;
    }
    builder.build().map_err(|e| format!("updater: {e}"))
}

/// Ask whether there is something newer. `None` means there is not.
///
/// A failure here is a real error and is reported as one — but only the manual
/// "check for updates" button shows it. The automatic check swallows it, because
/// a laptop that was closed in a tunnel is not a thing to interrupt anyone about.
#[tauri::command]
pub async fn check_update(
    app: AppHandle,
    pinned: Option<String>,
) -> Result<Option<UpdateInfo>, String> {
    let updater = updater(&app, pinned.as_deref())?;
    let Some(found) = updater.check().await.map_err(|e| format!("{e}"))? else {
        return Ok(None);
    };
    let info = UpdateInfo {
        version: found.version.clone(),
        current: found.current_version.clone(),
        notes: found.body.clone().unwrap_or_default(),
        held_back: None,
    };
    // A pinned check is already the answer to this question — it is how the
    // held-back version gets looked up in the first place — so asking again
    // would be a loop.
    if pinned.is_some() {
        return Ok(Some(info));
    }
    Ok(Some(hold_back_for_relay(&app, info).await))
}

/// Swap the offer for the newest release this machine's relay can carry, if
/// those differ.
///
/// Every failure here — no daemon, no relay, no table, no answer — leaves the
/// offer exactly as it was. That is the deliberate direction: the check exists
/// to stop the app recommending an upgrade the user's own relay cannot carry,
/// and on the day it ships no relay in the world reports a version yet, so a
/// version of this that failed the other way would offer nobody anything.
async fn hold_back_for_relay(app: &AppHandle, info: UpdateInfo) -> UpdateInfo {
    let Ok(mut client) = arvolo_ipc::client::DaemonClient::connect().await else {
        return info;
    };
    let Ok(relay) = client.relay_info().await else {
        return info;
    };
    let Some(compat) = crate::compat::fetch(REPOSITORY).await else {
        return info;
    };
    let Some(fallback) =
        crate::compat::held_back_to(&compat, &info.version, relay.version.as_deref())
    else {
        return info;
    };
    // The fallback's own manifest carries its notes and confirms it is really
    // installable. If it cannot be read, say nothing and leave the newest one
    // on offer rather than pointing at a version that may not be there.
    let latest = info.version.clone();
    match updater(app, Some(&fallback)) {
        Ok(u) => match u.check().await {
            Ok(Some(found)) => UpdateInfo {
                version: found.version.clone(),
                current: found.current_version.clone(),
                notes: found.body.clone().unwrap_or_default(),
                held_back: Some(HeldBack {
                    latest,
                    relay: relay.url,
                    relay_version: relay.version,
                }),
            },
            _ => info,
        },
        Err(_) => info,
    }
}

/// Download the update, hand the daemon over, install, restart.
///
/// The sequence is deliberate and each step is where it is for a reason:
///
/// 1. **Download first, with the daemon still up.** It is the slow step, and
///    there is no reason to take the engine away from someone for the length of
///    it. Transfers keep running; if the download fails, nothing has changed.
/// 2. **Then stop the daemon, and wait for it to be gone.** It runs from the
///    files the installer is about to replace — on Windows it holds them, and an
///    MSI that finds them in use asks for a reboot instead of installing; on
///    Linux the AppImage it runs from is the file being overwritten. The hold
///    flag goes up first so the event pump does not helpfully put a new one back
///    while the installer works.
/// 3. **Install, then restart.** On Windows the installer takes the app down
///    itself and the restart below never runs. On macOS and Linux the bundle is
///    replaced under a still-running process, so someone has to ask for the
///    restart — and on the way back up the pump spawns the *new* daemon from the
///    *new* sidecar, which is the whole point of stopping the old one.
///
/// If the install fails, the hold is released: the app stays on the old version,
/// and leaving it permanently without a daemon would turn a failed update into a
/// broken installation.
#[tauri::command]
pub async fn install_update(app: AppHandle, version: Option<String>) -> Result<(), String> {
    let updater = updater(&app, version.as_deref())?;
    let Some(update) = updater.check().await.map_err(|e| format!("{e}"))? else {
        return Err("no update to install".into());
    };

    let mut downloaded: u64 = 0;
    let progress_app = app.clone();
    let bytes = update
        .download(
            move |chunk, total| {
                downloaded += chunk as u64;
                let _ = progress_app.emit(EV_UPDATE_PROGRESS, Progress { downloaded, total });
            },
            || {},
        )
        .await
        .map_err(|e| format!("download: {e}"))?;

    daemon::hold_for_update();
    if let Err(e) = daemon::stop().await {
        daemon::release_hold();
        return Err(format!(
            "could not stop the daemon before installing: {e:#}"
        ));
    }

    if let Err(e) = update.install(bytes) {
        daemon::release_hold();
        return Err(format!("install: {e}"));
    }

    app.restart()
}
