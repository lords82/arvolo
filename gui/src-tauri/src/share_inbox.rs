//! What another app handed over through the system share sheet.
//!
//! Both platforms deliver it the same way from this side: a list of files
//! already copied somewhere this app can read — by `Shared.kt` into the app's
//! own storage on Android, by the share extension into the App Group container
//! on iOS — and the job here is to take them in, register them as a pick (an
//! opaque id, never a path, like everything the webview sends) and tell the
//! screen, which opens Scambia with the files in hand.
//!
//! Timing is the whole difficulty. On Android the share arrives in the
//! activity's `onCreate`, before Tauri has built the app, let alone loaded a
//! page that could listen; on iOS it arrives through a URL, or not at all when
//! the extension could not open the app. So nothing here pushes: shares are
//! queued, the screen is nudged when there is one, and the screen *pulls* with
//! `take_pending_shared` — at launch, on coming to the front, and on the nudge.
//! A share that landed before anyone was listening is simply there on the first
//! pull.
#![cfg(mobile)]
#![allow(dead_code)]

use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};

use tauri::{AppHandle, Emitter};

/// The nudge: "there is something to take". Carries nothing — the pull does.
pub const EV_SHARED_ARRIVED: &str = "shared://arrived";

static APP: OnceLock<AppHandle> = OnceLock::new();
static PENDING: Mutex<Vec<PathBuf>> = Mutex::new(Vec::new());

/// Un `.arvolo` aperto da fuori, in attesa che la schermata lo chieda.
///
/// Stessa forma della coda qui sopra e per la stessa ragione: su Android il
/// file arriva in `onCreate`, prima che esista una pagina che possa ascoltare.
/// Chi lo ritira è `take_pending_ticket`, lo stesso comando che sul desktop
/// svuota il biglietto passato da riga di comando — una porta sola per una cosa
/// sola, da qualunque parte sia entrata.
#[cfg(target_os = "android")]
static TICKET: Mutex<Option<String>> = Mutex::new(None);

/// Il biglietto letto da un file che qualcuno ci ha passato. Messo in coda, e
/// annunciato se c'è già una schermata ad ascoltare.
#[cfg(target_os = "android")]
pub fn offer_ticket(text: String) {
    let t = text.trim().to_string();
    if t.is_empty() {
        return;
    }
    *TICKET.lock().unwrap() = Some(t.clone());
    if let Some(app) = APP.get() {
        let _ = app.emit(crate::EV_ARVOLO_TICKET, t);
    }
}

/// Quello che è arrivato e non è ancora stato ritirato, una volta sola.
#[cfg(target_os = "android")]
pub fn take_ticket() -> Option<String> {
    TICKET.lock().unwrap().take()
}

/// From the app's setup: the handle that lets a later share nudge the screen.
pub fn attach(app: &AppHandle) {
    let _ = APP.set(app.clone());
    #[cfg(target_os = "ios")]
    collect_app_group(app);
}

/// Something arrived. Queued, and the screen told if there is one to tell.
pub fn offer(paths: Vec<PathBuf>) {
    if paths.is_empty() {
        return;
    }
    PENDING.lock().unwrap().extend(paths);
    if let Some(app) = APP.get() {
        let _ = app.emit(EV_SHARED_ARRIVED, ());
    }
}

/// Everything shared and not yet taken, registered as picked files. The one
/// door to the queue, so a share is handed over exactly once.
#[tauri::command]
pub fn take_pending_shared(
    app: AppHandle,
    state: tauri::State<'_, crate::bridge::PickedFiles>,
) -> Vec<crate::bridge::PickedItemDto> {
    #[cfg(target_os = "ios")]
    collect_app_group(&app);
    let paths: Vec<PathBuf> = std::mem::take(&mut *PENDING.lock().unwrap());
    let adopted: Vec<PathBuf> = paths
        .into_iter()
        .filter(|p| p.is_file())
        .map(|p| {
            // Into the picked-copies folder, where a copy lives as long as a
            // send claims it and not a day longer — see `sweep_picked`.
            let dest = crate::bridge::adopt_shared(&app, p.clone());
            // The per-share folder is empty now; nothing needs it.
            if let Some(parent) = p.parent() {
                let _ = std::fs::remove_dir(parent);
            }
            dest
        })
        .collect();
    crate::bridge::register_paths(&state, &adopted)
}

// ---------------------------------------------------------------- Android --

/// `Shared.onShared(json)` from Kotlin: a JSON array of absolute paths the
/// activity already copied into the app's storage.
#[cfg(target_os = "android")]
#[no_mangle]
pub extern "system" fn Java_it_termox_arvolo_Shared_onShared(
    mut env: jni::JNIEnv,
    _class: jni::objects::JClass,
    json: jni::objects::JString,
) {
    let text: String = match env.get_string(&json) {
        Ok(s) => s.into(),
        Err(_) => return,
    };
    let paths: Vec<String> = serde_json::from_str(&text).unwrap_or_default();
    offer(paths.into_iter().map(PathBuf::from).collect());
}

/// `Shared.onTicket` dal lato Kotlin: il contenuto di un `.arvolo` che un'altra
/// app ha aperto con noi.
#[cfg(target_os = "android")]
#[no_mangle]
pub extern "system" fn Java_it_termox_arvolo_Shared_onTicket(
    mut env: jni::JNIEnv,
    _class: jni::objects::JClass,
    text: jni::objects::JString,
) {
    let Ok(s) = env.get_string(&text) else { return };
    offer_ticket(s.into());
}

// -------------------------------------------------------------------- iOS --

/// The App Group the share extension writes into. Must match both
/// entitlements files under `mobile/ios`.
#[cfg(target_os = "ios")]
const APP_GROUP: &str = "group.it.termox.arvolo";

/// Take in whatever the share extension left in the group container: every
/// file under `shared/<uuid>/`, moved out of the group (a rename across
/// containers may be refused; then a copy and a delete). Called at launch, on
/// coming to the front, and on `arvolo://shared`.
#[cfg(target_os = "ios")]
pub fn collect_app_group(app: &AppHandle) {
    use objc2_foundation::{NSFileManager, NSString};
    let group = NSString::from_str(APP_GROUP);
    let Some(url) =
        NSFileManager::defaultManager().containerURLForSecurityApplicationGroupIdentifier(&group)
    else {
        return;
    };
    let Some(base) = url.path().map(|p| PathBuf::from(p.to_string())) else {
        return;
    };
    let shared = base.join("shared");
    let Ok(batches) = std::fs::read_dir(&shared) else {
        return;
    };
    let Some(dir) = crate::bridge::picked_dir(app) else {
        return;
    };
    let _ = std::fs::create_dir_all(&dir);
    let mut got = Vec::new();
    for batch in batches.flatten() {
        let Ok(files) = std::fs::read_dir(batch.path()) else {
            continue;
        };
        for f in files.flatten() {
            let src = f.path();
            if !src.is_file() {
                continue;
            }
            let name = src
                .file_name()
                .map(|n| n.to_os_string())
                .unwrap_or_default();
            let dest = crate::bridge::unclaimed_in(&dir, &name.to_string_lossy());
            let moved = std::fs::rename(&src, &dest).is_ok()
                || (std::fs::copy(&src, &dest).is_ok() && std::fs::remove_file(&src).is_ok());
            if moved {
                got.push(dest);
            }
        }
        let _ = std::fs::remove_dir(batch.path());
    }
    // Already where `take_pending_shared` wants them, so they go straight to
    // the queue as they are.
    if !got.is_empty() {
        PENDING.lock().unwrap().extend(got);
        if let Some(app) = APP.get() {
            let _ = app.emit(EV_SHARED_ARRIVED, ());
        }
    }
}
