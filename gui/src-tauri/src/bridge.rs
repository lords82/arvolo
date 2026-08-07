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

#[tauri::command]
pub async fn send_to(to: String, paths: Vec<String>, note: String) -> Result<u64, String> {
    let mut c = client().await?;
    c.push(to, paths, note).await.or_else(err)
}

/// `send --deposit`: straight to the recipient's mailbox, with the options that
/// only mean anything there. Returns the `arvm…` ticket so the sender can also
/// hand it over themselves.
#[tauri::command]
pub async fn deposit_to(
    to: String,
    paths: Vec<String>,
    note: String,
    ttl: Option<u64>,
    max: Option<u32>,
    password: Option<String>,
) -> Result<ServedDto, String> {
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
    paths: Vec<String>,
    seed_relay: Option<String>,
) -> Result<ServedDto, String> {
    let mut c = client().await?;
    match c.serve_ticket(paths, seed_relay).await {
        Ok((id, ticket)) => Ok(ServedDto { id, ticket }),
        Err(e) => err(e),
    }
}

#[tauri::command]
pub async fn create_link(
    path: String,
    ttl: Option<u64>,
    max: Option<u32>,
) -> Result<String, String> {
    let mut c = client().await?;
    c.create_link(path, ttl, max).await.or_else(err)
}

#[tauri::command]
pub async fn accept_offer(
    offer_id: String,
    out: Option<String>,
    password: Option<String>,
) -> Result<u64, String> {
    let mut c = client().await?;
    c.accept_with_password(offer_id, out.map(PathBuf::from), password)
        .await
        .or_else(err)
}

#[tauri::command]
pub async fn reject_offer(offer_id: String) -> Result<(), String> {
    let mut c = client().await?;
    c.reject(offer_id).await.or_else(err)
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
    paths: Vec<String>,
    relay: Option<String>,
    keep: bool,
) -> Result<CodeDto, String> {
    let mut c = client().await?;
    match c.serve_code(paths, relay, keep).await {
        Ok((id, code)) => Ok(CodeDto { id, code }),
        Err(e) => err(e),
    }
}

#[tauri::command]
pub async fn recv(
    ticket: String,
    out: Option<String>,
    password: Option<String>,
) -> Result<u64, String> {
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
pub async fn accept_name(who: String) -> Result<(), String> {
    let mut c = client().await?;
    c.accept_name(who).await.or_else(err)
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

/// Stop a (stale) daemon by its pid file. The event pump's `ensure_running` then
/// brings a fresh one up — which is the whole point: the pair is a restart the
/// user doesn't have to open a terminal for.
#[tauri::command]
pub async fn restart_daemon() -> Result<(), String> {
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
    #[cfg(not(unix))]
    {
        let _ = pid;
        Err("restart is unix-only for now".into())
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

/// Read a UTF-8 file the user picked in a dialog. Used for address-book import.
///
/// Two ten-line commands rather than the `fs` plugin: the plugin brings a scope
/// language, a capability file and a permission set to grant blanket read/write
/// under whole directories, and all this app ever touches is the single path a
/// human just chose in a native picker. The narrower surface is the point.
#[tauri::command]
pub async fn read_text_file(path: String) -> Result<String, String> {
    tokio::fs::read_to_string(&path)
        .await
        .map_err(|e| format!("{path}: {e}"))
}

/// Write a UTF-8 file the user picked in a save dialog (address-book export).
#[tauri::command]
pub async fn write_text_file(path: String, contents: String) -> Result<(), String> {
    tokio::fs::write(&path, contents)
        .await
        .map_err(|e| format!("{path}: {e}"))
}

/// This GUI binary's version — the frontend compares it with the daemon's
/// `StatusDto::version` to surface a "daemon obsoleto, riavvialo" banner.
#[tauri::command]
pub fn gui_version() -> String {
    env!("CARGO_PKG_VERSION").to_string()
}
