//! Tauri command bridge: thin `#[tauri::command]` wrappers that forward each UI
//! action to the daemon over IPC. No engine logic lives here — the daemon owns it.
//!
//! Every call opens a fresh short-lived RPC connection (a local unix socket, cheap
//! to (re)dial); the long-lived *event* subscription is handled separately in the
//! event pump (see `main.rs`). Errors are stringified for the frontend.

use std::path::PathBuf;

use arvolo_ipc::client::DaemonClient;
use arvolo_ipc::protocol::{
    ConfigDto, ConfigPatch, ContactDto, DepositDto, HistoryDto, OfferDto, PairKind, PresenceDto,
    StatusDto, SyncDto, TransferDto,
};
use serde::Serialize;

/// A ticket-serve result handed back to the UI.
#[derive(Debug, Clone, Serialize)]
pub struct ServedDto {
    pub id: u64,
    pub ticket: String,
}

/// A code-serve result handed back to the UI.
#[derive(Debug, Clone, Serialize)]
pub struct CodeDto {
    pub id: u64,
    pub code: String,
}

/// Connect to the daemon, mapping the "no daemon" error to a UI-friendly string.
async fn client() -> Result<DaemonClient, String> {
    DaemonClient::connect().await.map_err(|e| format!("{e:#}"))
}

fn err<T>(e: anyhow::Error) -> Result<T, String> {
    Err(format!("{e:#}"))
}

#[tauri::command]
pub async fn status() -> Result<StatusDto, String> {
    let mut c = client().await?;
    c.status().await.or_else(err)
}

#[tauri::command]
pub async fn list_transfers() -> Result<Vec<TransferDto>, String> {
    let mut c = client().await?;
    c.list().await.or_else(err)
}

#[tauri::command]
pub async fn list_pending() -> Result<Vec<OfferDto>, String> {
    let mut c = client().await?;
    c.list_pending().await.or_else(err)
}

#[tauri::command]
pub async fn list_contacts() -> Result<Vec<ContactDto>, String> {
    let mut c = client().await?;
    c.list_contacts().await.or_else(err)
}

// ---- picked files: the only way a path enters a send ------------------------
//
// The webview never handles filesystem paths. Picking (native dialog) and
// dropping (handled on the window, in `main.rs`) register paths HERE and hand the
// frontend an opaque id plus display metadata; the send commands below take those
// ids and resolve them in this registry. A webview that is compromised can still
// *ask* to send, but only files a human just picked or dropped — it cannot name a
// path of its own, so `~/.ssh/id_rsa` is out of its reach unless the user drags
// it in personally.

/// One picked file as the frontend sees it: no path.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PickedItemDto {
    pub id: String,
    pub name: String,
    pub size: u64,
    pub is_dir: bool,
}

/// The id → path registry behind [`PickedItemDto`]. Bounded FIFO: entries are
/// dropped oldest-first past the cap, which only matters to a session that picks
/// thousands of files without ever sending them.
pub struct PickedFiles {
    inner: std::sync::Mutex<PickedInner>,
}

#[derive(Default)]
struct PickedInner {
    order: std::collections::VecDeque<String>,
    by_id: std::collections::HashMap<String, PathBuf>,
}

const PICKED_CAP: usize = 2048;

impl PickedFiles {
    pub fn new() -> Self {
        Self {
            inner: std::sync::Mutex::new(PickedInner::default()),
        }
    }

    /// Register one path and mint its id. The id is random, not derived: a
    /// predictable id would let the webview mint ids for paths nobody picked.
    fn register(&self, path: PathBuf) -> PickedItemDto {
        let id = {
            let b: [u8; 16] = rand::random();
            data_encoding::HEXLOWER.encode(&b)
        };
        let meta = std::fs::metadata(&path).ok();
        let dto = PickedItemDto {
            id: id.clone(),
            name: path
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| path.to_string_lossy().into_owned()),
            size: meta.as_ref().map(|m| m.len()).unwrap_or(0),
            is_dir: meta.map(|m| m.is_dir()).unwrap_or(false),
        };
        let mut inner = self.inner.lock().unwrap();
        inner.order.push_back(id.clone());
        inner.by_id.insert(id, path);
        while inner.order.len() > PICKED_CAP {
            if let Some(old) = inner.order.pop_front() {
                inner.by_id.remove(&old);
            }
        }
        dto
    }

    /// The paths behind `ids`, in order. Any unknown id fails the whole call: a
    /// partial send the user did not ask for is worse than an error they can see.
    fn resolve(&self, ids: &[String]) -> Result<Vec<String>, String> {
        let inner = self.inner.lock().unwrap();
        ids.iter()
            .map(|id| {
                inner
                    .by_id
                    .get(id)
                    .map(|p| p.to_string_lossy().into_owned())
                    .ok_or_else(|| "stale file selection — pick the files again".to_string())
            })
            .collect()
    }
}

/// Register externally-arrived paths (the window drop handler in `main.rs`).
pub fn register_paths(
    state: &tauri::State<'_, PickedFiles>,
    paths: &[PathBuf],
) -> Vec<PickedItemDto> {
    paths.iter().map(|p| state.register(p.clone())).collect()
}

/// Open the native picker (files, or folders with `directory`) and register the
/// choice. Returns display items; the webview never sees the paths.
#[tauri::command]
pub async fn pick_files(
    app: tauri::AppHandle,
    state: tauri::State<'_, PickedFiles>,
    directory: bool,
) -> Result<Vec<PickedItemDto>, String> {
    use tauri_plugin_dialog::DialogExt;
    let (tx, rx) = tokio::sync::oneshot::channel();
    let dialog = app.dialog().file();
    if directory {
        dialog.pick_folders(move |p| {
            let _ = tx.send(p);
        });
    } else {
        dialog.pick_files(move |p| {
            let _ = tx.send(p);
        });
    }
    let Some(picked) = rx.await.map_err(|e| e.to_string())? else {
        return Ok(Vec::new()); // cancelled
    };
    let mut out = Vec::with_capacity(picked.len());
    for fp in picked {
        let path = fp.into_path().map_err(|e| e.to_string())?;
        out.push(state.register(path));
    }
    Ok(out)
}

#[tauri::command]
pub async fn send_to(
    state: tauri::State<'_, PickedFiles>,
    to: String,
    items: Vec<String>,
    note: String,
) -> Result<u64, String> {
    let paths = state.resolve(&items)?;
    let mut c = client().await?;
    // The GUI's live send carries no mailbox options: its mailbox mode goes
    // through `deposit` below, which has always carried them.
    c.push(to, paths, note, None, None, None).await.or_else(err)
}

/// `send --deposit`: straight to the recipient's mailbox, with the options that
/// only mean anything there. Returns the `arvm…` ticket so the sender can also
/// hand it over themselves.
#[tauri::command]
pub async fn deposit_to(
    state: tauri::State<'_, PickedFiles>,
    to: String,
    items: Vec<String>,
    note: String,
    ttl: Option<u64>,
    max: Option<u32>,
    password: Option<String>,
) -> Result<ServedDto, String> {
    let paths = state.resolve(&items)?;
    let mut c = client().await?;
    match c.deposit(to, paths, note, ttl, max, password).await {
        Ok((id, ticket)) => Ok(ServedDto { id, ticket }),
        Err(e) => err(e),
    }
}

#[tauri::command]
pub async fn get_config() -> Result<ConfigDto, String> {
    let mut c = client().await?;
    c.get_config().await.or_else(err)
}

#[tauri::command]
pub async fn set_config(patch: ConfigPatch) -> Result<ConfigDto, String> {
    let mut c = client().await?;
    c.set_config(patch).await.or_else(err)
}

/// Who is reachable right now. `online: null` in a row means the relay could not
/// be asked — the UI must render that as "unknown", never as "offline".
#[tauri::command]
pub async fn presence(ids: Vec<String>) -> Result<Vec<PresenceDto>, String> {
    let mut c = client().await?;
    c.presence(ids).await.or_else(err)
}

#[tauri::command]
pub async fn prune_names() -> Result<usize, String> {
    let mut c = client().await?;
    c.prune_names().await.or_else(err)
}

#[tauri::command]
pub async fn sync_status() -> Result<SyncDto, String> {
    let mut c = client().await?;
    c.sync_status().await.or_else(err)
}

#[tauri::command]
pub async fn sync_now() -> Result<SyncDto, String> {
    let mut c = client().await?;
    c.sync_now().await.or_else(err)
}

/// Begin a pairing exchange. Returns the session handle; the code and the outcome
/// arrive on the event stream (`engine://event`) as `pairing_*`.
#[tauri::command]
pub async fn start_pairing(
    kind: PairKind,
    relay: Option<String>,
    code: Option<String>,
    name: Option<String>,
) -> Result<String, String> {
    let mut c = client().await?;
    c.start_pairing(kind, relay, code, name).await.or_else(err)
}

#[tauri::command]
pub async fn cancel_pairing(session: String) -> Result<(), String> {
    let mut c = client().await?;
    c.cancel_pairing(session).await.or_else(err)
}

#[tauri::command]
pub async fn serve_ticket(
    state: tauri::State<'_, PickedFiles>,
    items: Vec<String>,
    seed_relay: Option<String>,
) -> Result<ServedDto, String> {
    let paths = state.resolve(&items)?;
    let mut c = client().await?;
    match c.serve_ticket(paths, seed_relay).await {
        Ok((id, ticket)) => Ok(ServedDto { id, ticket }),
        Err(e) => err(e),
    }
}

#[tauri::command]
pub async fn create_link(
    state: tauri::State<'_, PickedFiles>,
    item: String,
    ttl: Option<u64>,
    max: Option<u32>,
) -> Result<String, String> {
    let path = state.resolve(std::slice::from_ref(&item))?.remove(0);
    let mut c = client().await?;
    c.create_link(path, ttl, max).await.or_else(err)
}

#[tauri::command]
pub async fn accept_offer(
    state: tauri::State<'_, PickedFiles>,
    offer_id: String,
    out_item: Option<String>,
    password: Option<String>,
) -> Result<u64, String> {
    let out = match out_item {
        Some(id) => Some(state.resolve(std::slice::from_ref(&id))?.remove(0)),
        None => None,
    };
    let mut c = client().await?;
    c.accept_with_password(offer_id, out.map(PathBuf::from), password)
        .await
        .or_else(err)
}

#[tauri::command]
pub async fn reject_offer(app: tauri::AppHandle, offer_id: String) -> Result<(), String> {
    let mut c = client().await?;
    let done = c.reject(offer_id).await.or_else(err);
    // Rejecting emits no engine event, so the pump never hears about it and the
    // wedge would stay down over an offer that is gone. Accepting needs no such
    // nudge: it turns into a `Started`, which the pump already watches.
    crate::attention::refresh(&app).await;
    done
}

#[tauri::command]
pub async fn pause(id: u64) -> Result<(), String> {
    let mut c = client().await?;
    c.pause(id).await.or_else(err)
}

#[tauri::command]
pub async fn resume(id: u64) -> Result<(), String> {
    let mut c = client().await?;
    c.resume(id).await.or_else(err)
}

#[tauri::command]
pub async fn cancel(id: u64) -> Result<(), String> {
    let mut c = client().await?;
    c.cancel(id).await.or_else(err)
}

#[tauri::command]
pub async fn remove(id: u64) -> Result<(), String> {
    let mut c = client().await?;
    c.remove(id).await.or_else(err)
}

#[tauri::command]
pub async fn serve_code(
    state: tauri::State<'_, PickedFiles>,
    items: Vec<String>,
    relay: Option<String>,
    keep: bool,
) -> Result<CodeDto, String> {
    let paths = state.resolve(&items)?;
    let mut c = client().await?;
    match c.serve_code(paths, relay, keep).await {
        Ok((id, code)) => Ok(CodeDto { id, code }),
        Err(e) => err(e),
    }
}

#[tauri::command]
pub async fn recv(
    state: tauri::State<'_, PickedFiles>,
    ticket: String,
    out_item: Option<String>,
    password: Option<String>,
) -> Result<u64, String> {
    // The destination is an id from the registry too — a *write* destination the
    // webview could name freely is the same hole as a readable path, pointed the
    // other way (drop an attacker-sent file into any directory).
    let out = match out_item {
        Some(id) => Some(state.resolve(std::slice::from_ref(&id))?.remove(0)),
        None => None,
    };
    let mut c = client().await?;
    c.recv(ticket, out.map(PathBuf::from), password)
        .await
        .or_else(err)
}

#[tauri::command]
pub async fn add_contact(name: String, id: String) -> Result<(), String> {
    let mut c = client().await?;
    c.add_contact(name, id).await.or_else(err)
}

#[tauri::command]
pub async fn remove_contact(name: String) -> Result<(), String> {
    let mut c = client().await?;
    c.remove_contact(name).await.or_else(err)
}

#[tauri::command]
pub async fn rename_contact(old: String, new: String) -> Result<(), String> {
    let mut c = client().await?;
    c.rename_contact(old, new).await.or_else(err)
}

#[tauri::command]
pub async fn mark_unverified(name: String) -> Result<(), String> {
    let mut c = client().await?;
    c.mark_unverified(name).await.or_else(err)
}

#[tauri::command]
pub async fn mark_trusted(who: String, force: bool) -> Result<(), String> {
    let mut c = client().await?;
    c.mark_trusted(who, force).await.or_else(err)
}

#[tauri::command]
pub async fn mark_untrusted(who: String) -> Result<(), String> {
    let mut c = client().await?;
    c.mark_untrusted(who).await.or_else(err)
}

#[tauri::command]
pub async fn block_contact(who: String) -> Result<(), String> {
    let mut c = client().await?;
    c.block(who).await.or_else(err)
}

#[tauri::command]
pub async fn unblock_contact(who: String) -> Result<(), String> {
    let mut c = client().await?;
    c.unblock(who).await.or_else(err)
}

#[tauri::command]
pub async fn accept_name(app: tauri::AppHandle, who: String) -> Result<(), String> {
    let mut c = client().await?;
    let done = c.accept_name(who).await.or_else(err);
    // An approved name is one decision fewer. The daemon may or may not announce
    // the change as a `ContactsChanged`; refreshing here does not depend on it.
    crate::attention::refresh(&app).await;
    done
}

#[tauri::command]
pub async fn list_history() -> Result<Vec<HistoryDto>, String> {
    let mut c = client().await?;
    c.list_history().await.or_else(err)
}

#[tauri::command]
pub async fn clear_history() -> Result<usize, String> {
    let mut c = client().await?;
    c.clear_history().await.or_else(err)
}

#[tauri::command]
pub async fn clear_finished() -> Result<usize, String> {
    let mut c = client().await?;
    c.clear_finished().await.or_else(err)
}

#[tauri::command]
pub async fn set_my_name(name: String) -> Result<(), String> {
    let mut c = client().await?;
    c.set_my_name(name).await.or_else(err)
}

/// Stop the daemon so the event pump's `ensure_running` brings a fresh one up —
/// which is the whole point: the pair is a restart the user doesn't have to open
/// a terminal for. Asked politely over IPC first (`Request::Shutdown`); the pid
/// file is only the fallback for a daemon too old or too wedged to answer. A
/// leftover `daemon stop` marker is cleared: an in-app restart is the user
/// asking for it to be up again.
#[tauri::command]
pub async fn restart_daemon() -> Result<(), String> {
    let _ = std::fs::remove_file(arvolo_ipc::stop_marker_path());
    if let Ok(mut c) = client().await {
        if c.shutdown().await.is_ok() {
            return Ok(());
        }
    }
    let pid = std::fs::read_to_string(arvolo_ipc::pid_path())
        .map_err(|e| format!("pid file: {e}"))?
        .trim()
        .parse::<i32>()
        .map_err(|e| format!("pid file: {e}"))?;
    #[cfg(unix)]
    {
        // SIGTERM — the daemon shuts down cleanly (it persists resumable state).
        let rc = unsafe { libc::kill(pid, libc::SIGTERM) };
        if rc != 0 {
            return Err(format!(
                "could not stop daemon (pid {pid}): {}",
                std::io::Error::last_os_error()
            ));
        }
        Ok(())
    }
    #[cfg(windows)]
    {
        // No SIGTERM here, and no polite equivalent that reaches a process with no
        // console: `taskkill` without /F asks a window to close, and the daemon has
        // none. So this is the abrupt one — which the daemon is built to survive,
        // since a machine can lose power mid-transfer just as easily: records for
        // resumable downloads and sends are written as it goes, not at shutdown.
        // What is lost is the tidy removal of the pid file, which the next start
        // overwrites anyway.
        let out = std::process::Command::new("taskkill")
            .args(["/PID", &pid.to_string(), "/T", "/F"])
            .output()
            .map_err(|e| format!("could not run taskkill: {e}"))?;
        if out.status.success() {
            Ok(())
        } else {
            Err(format!(
                "could not stop daemon (pid {pid}): {}",
                String::from_utf8_lossy(&out.stderr).trim()
            ))
        }
    }
}

#[tauri::command]
pub async fn list_deposits() -> Result<Vec<DepositDto>, String> {
    let mut c = client().await?;
    c.list_deposits().await.or_else(err)
}

#[tauri::command]
pub async fn revoke_deposit(id: String) -> Result<(), String> {
    let mut c = client().await?;
    c.revoke_deposit(id).await.or_else(err)
}

#[tauri::command]
pub async fn mark_verified(name: String) -> Result<(), String> {
    let mut c = client().await?;
    c.mark_verified(name).await.or_else(err)
}

/// Open a native file picker and read the chosen address-book export. `None`
/// if the user cancelled.
///
/// The dialog lives HERE, on the native side, and that placement is the security
/// boundary — not a convenience. The previous shape (`read_text_file(path)` /
/// `write_text_file(path, contents)`) took whatever path the webview supplied,
/// trusting it had come from a dialog; any script that got into the webview could
/// call it directly and read or overwrite arbitrary files as the user. Now the
/// webview never handles a filesystem path at all: it asks for "an import", and
/// the user's choice in the native dialog is what grants access to one file.
///
/// Still two small commands rather than the `fs` plugin, whose scope language and
/// capability files exist to grant blanket access under whole directories — all
/// this app ever touches is the single file a human just picked.
#[tauri::command]
pub async fn import_contacts(app: tauri::AppHandle) -> Result<Option<String>, String> {
    use tauri_plugin_dialog::DialogExt;
    let (tx, rx) = tokio::sync::oneshot::channel();
    app.dialog()
        .file()
        .add_filter("JSON", &["json"])
        .pick_file(move |p| {
            let _ = tx.send(p);
        });
    let Some(picked) = rx.await.map_err(|e| e.to_string())? else {
        return Ok(None); // user cancelled — not an error
    };
    let path = picked.into_path().map_err(|e| e.to_string())?;
    tokio::fs::read_to_string(&path)
        .await
        .map(Some)
        .map_err(|e| format!("{}: {e}", path.display()))
}

/// Save the address-book export where the user picks. Returns the chosen file
/// name (for the toast), or `None` if they cancelled.
///
/// `default_name` is only a *suggestion* shown in the dialog; path separators are
/// stripped so it can never smuggle directories into the suggestion.
/// A ticket from a `.arvolo` file passed on the command line (double click on
/// Windows/Linux launches a fresh instance with the path as argv). Held here
/// until the webview is up and asks for it — an event emitted before the
/// frontend subscribes would just be lost.
pub struct PendingArvolo(pub std::sync::Mutex<Option<String>>);

#[tauri::command]
pub fn take_pending_ticket(state: tauri::State<'_, PendingArvolo>) -> Option<String> {
    state.0.lock().unwrap().take()
}

/// Save an `arvc…` ticket as a `.arvolo` file — the CLI's default send artefact,
/// shareable like a .torrent. Same shape as `export_contacts`: native save
/// dialog, path stays Rust-side, the webview only learns the chosen file name.
#[tauri::command]
pub async fn save_ticket(
    app: tauri::AppHandle,
    default_name: String,
    ticket: String,
) -> Result<Option<String>, String> {
    use tauri_plugin_dialog::DialogExt;
    let default_name: String = default_name
        .chars()
        .map(|c| if matches!(c, '/' | '\\') { '_' } else { c })
        .collect();
    let (tx, rx) = tokio::sync::oneshot::channel();
    app.dialog()
        .file()
        .add_filter("Arvolo ticket", &["arvolo"])
        .set_file_name(&default_name)
        .save_file(move |p| {
            let _ = tx.send(p);
        });
    let Some(picked) = rx.await.map_err(|e| e.to_string())? else {
        return Ok(None);
    };
    let path = picked.into_path().map_err(|e| e.to_string())?;
    tokio::fs::write(&path, format!("{ticket}\n"))
        .await
        .map_err(|e| format!("{}: {e}", path.display()))?;
    Ok(Some(
        path.file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default(),
    ))
}

#[tauri::command]
pub async fn export_contacts(
    app: tauri::AppHandle,
    default_name: String,
    contents: String,
) -> Result<Option<String>, String> {
    use tauri_plugin_dialog::DialogExt;
    let default_name: String = default_name
        .chars()
        .map(|c| if matches!(c, '/' | '\\') { '_' } else { c })
        .collect();
    let (tx, rx) = tokio::sync::oneshot::channel();
    app.dialog()
        .file()
        .add_filter("JSON", &["json"])
        .set_file_name(&default_name)
        .save_file(move |p| {
            let _ = tx.send(p);
        });
    let Some(picked) = rx.await.map_err(|e| e.to_string())? else {
        return Ok(None);
    };
    let path = picked.into_path().map_err(|e| e.to_string())?;
    tokio::fs::write(&path, contents)
        .await
        .map_err(|e| format!("{}: {e}", path.display()))?;
    Ok(Some(
        path.file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default(),
    ))
}

/// This GUI binary's version — the frontend compares it with the daemon's
/// `StatusDto::version` to surface a "daemon obsoleto, riavvialo" banner.
#[tauri::command]
pub fn gui_version() -> String {
    env!("CARGO_PKG_VERSION").to_string()
}

#[cfg(test)]
mod picked_tests {
    use super::*;

    /// Unknown ids fail the whole call. A partial send — some of what the user
    /// picked, silently minus the rest — is worse than an error they can see.
    #[test]
    fn an_unknown_id_fails_the_whole_resolve() {
        let reg = PickedFiles::new();
        let a = reg.register(PathBuf::from("/tmp/a.txt"));
        assert!(reg.resolve(std::slice::from_ref(&a.id)).is_ok());
        assert!(
            reg.resolve(&[a.id, "no-such-id".into()]).is_err(),
            "one bad id must sink the call, not shrink it"
        );
    }

    /// The id is minted, not derived: registering the same path twice yields two
    /// different ids. A derivable id would let the webview mint ids for paths
    /// nobody picked, which is the entire attack this registry exists to stop.
    #[test]
    fn ids_are_random_not_derived_from_the_path() {
        let reg = PickedFiles::new();
        let one = reg.register(PathBuf::from("/tmp/same.txt"));
        let two = reg.register(PathBuf::from("/tmp/same.txt"));
        assert_ne!(one.id, two.id);
        // And both resolve — a re-pick of the same file is not an error.
        assert!(reg.resolve(&[one.id, two.id]).is_ok());
    }

    /// The registry is bounded: past the cap the oldest entry is evicted, and a
    /// stale id turns into the visible "pick again" error rather than a wrong file.
    #[test]
    fn the_cap_evicts_oldest_first() {
        let reg = PickedFiles::new();
        let first = reg.register(PathBuf::from("/tmp/first.txt"));
        for i in 0..PICKED_CAP {
            reg.register(PathBuf::from(format!("/tmp/f{i}")));
        }
        assert!(
            reg.resolve(std::slice::from_ref(&first.id)).is_err(),
            "the oldest registration must be gone"
        );
    }

    /// What the frontend sees carries no path: name only, plus size/kind.
    #[test]
    fn the_dto_shows_a_name_never_a_path() {
        let reg = PickedFiles::new();
        let dto = reg.register(PathBuf::from("/very/secret/layout/report.pdf"));
        assert_eq!(dto.name, "report.pdf");
        let encoded = serde_json::to_string(&dto).unwrap();
        assert!(
            !encoded.contains("secret"),
            "the serialized item must not leak the directory: {encoded}"
        );
    }
}
