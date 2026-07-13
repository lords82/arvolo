//! Tauri command bridge: thin `#[tauri::command]` wrappers that forward each UI
//! action to the daemon over IPC. No engine logic lives here — the daemon owns it.
//!
//! Every call opens a fresh short-lived RPC connection (a local unix socket, cheap
//! to (re)dial); the long-lived *event* subscription is handled separately in the
//! event pump (see `main.rs`). Errors are stringified for the frontend.

use std::path::PathBuf;

use arvolo_ipc::client::DaemonClient;
use arvolo_ipc::protocol::{ContactDto, OfferDto, StatusDto, TransferDto};
use serde::Serialize;

/// A ticket-serve result handed back to the UI.
#[derive(Debug, Clone, Serialize)]
pub struct ServedDto {
    pub id: u64,
    pub ticket: String,
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
pub async fn accept_offer(offer_id: String, out: Option<String>) -> Result<u64, String> {
    let mut c = client().await?;
    c.accept(offer_id, out.map(PathBuf::from))
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
pub async fn mark_verified(name: String) -> Result<(), String> {
    let mut c = client().await?;
    c.mark_verified(name).await.or_else(err)
}

/// This GUI binary's version — the frontend compares it with the daemon's
/// `StatusDto::version` to surface a "daemon obsoleto, riavvialo" banner.
#[tauri::command]
pub fn gui_version() -> String {
    env!("CARGO_PKG_VERSION").to_string()
}
