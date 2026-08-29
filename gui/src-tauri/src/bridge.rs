//! Tauri command bridge: thin `#[tauri::command]` wrappers that forward each UI
//! action to the daemon over IPC. No engine logic lives here — the daemon owns it.
//!
//! Every call opens a fresh short-lived RPC connection (a local unix socket, cheap
//! to (re)dial); the long-lived *event* subscription is handled separately in the
//! event pump (see `main.rs`). Errors are stringified for the frontend.

use std::path::PathBuf;

use arvolo_ipc::client::DaemonClient;
use arvolo_ipc::protocol::{
    ConfigDto, ConfigPatch, ContactDto, DepositDto, DeviceDto, HistoryDto, OfferDto, PairKind,
    PresenceDto, StatusDto, SyncDto, TransferDto,
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

/// An accept that did not happen, as a window needs it.
///
/// `code` is the daemon's own word for a *refusal* — `needsPassword`, `p2pOff`,
/// `noLongerWaiting` — and empty for everything else. It exists because the dialog
/// has to act on one of those (put the password field up) and used to work it out
/// by testing the message for the word "password": rewording an English sentence
/// in the engine would then have quietly removed the only place a password can be
/// typed. `message` is what to show, exactly as before.
#[derive(Debug, Clone, Serialize)]
pub struct AcceptFailed {
    pub code: String,
    pub message: String,
}

impl From<anyhow::Error> for AcceptFailed {
    fn from(e: anyhow::Error) -> Self {
        AcceptFailed {
            code: arvolo_core::manager::AcceptRefusal::of(&e)
                .map(|r| r.code().to_string())
                .unwrap_or_default(),
            message: format!("{e:#}"),
        }
    }
}

impl From<String> for AcceptFailed {
    fn from(message: String) -> Self {
        AcceptFailed {
            code: String::new(),
            message,
        }
    }
}

/// Connect to the daemon, mapping the "no daemon" error to a UI-friendly string.
/// The engine running inside this process, when there is one.
///
/// Empty on every desktop, where the window is a thin client of a daemon it
/// spawns. A shell that cannot keep a daemon alive — a phone — starts the engine
/// itself and leaves it here, and every command below then answers from memory
/// instead of from a socket. Nothing else in this file knows which it is.
static EMBEDDED: std::sync::OnceLock<std::sync::Arc<dyn arvolo_ipc::client::Answer>> =
    std::sync::OnceLock::new();

/// Hand the window an engine to talk to instead of a daemon. Call once, before
/// the first command. A second call is ignored: two engines on one address book
/// is not a thing to do quietly.
///
/// Unused until there is a shell that calls it — the mobile entry point, which is
/// the next phase. Kept here and not there because the seam is this file's: the
/// commands below are what has to stop caring where the answers come from, and
/// they now do.
#[allow(dead_code)]
pub fn use_embedded(engine: std::sync::Arc<dyn arvolo_ipc::client::Answer>) {
    let _ = EMBEDDED.set(engine);
}

pub(crate) async fn client() -> Result<DaemonClient, String> {
    if let Some(engine) = EMBEDDED.get() {
        return Ok(DaemonClient::embedded(engine.clone()));
    }
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

/// What a picked id stands for.
///
/// A desktop picks paths. A phone picks `content://` URIs, which are not paths
/// and never become one: the app resolves them to an open descriptor at the
/// moment of picking — while the picker's grant is live — and the send reads
/// through that. `SendSource::Handle`, built for macOS folder consent, is the
/// same idea and takes this without a change.
enum Picked {
    Path(PathBuf),
    /// `name` is what the receiver will see the file called; a descriptor has
    /// no path to derive one from.
    Handle { file: std::fs::File, name: String },
}

/// The id → source registry behind [`PickedItemDto`]. Bounded FIFO: entries are
/// dropped oldest-first past the cap, which only matters to a session that picks
/// thousands of files without ever sending them.
pub struct PickedFiles {
    inner: std::sync::Mutex<PickedInner>,
}

#[derive(Default)]
struct PickedInner {
    order: std::collections::VecDeque<String>,
    by_id: std::collections::HashMap<String, Picked>,
}

/// What a set of picked ids resolves to for the daemon: the payload strings it
/// names, and — when the payload is a descriptor rather than a path — the
/// hand-off that carries it. See `fdpass::IN_PROCESS`.
pub struct Resolved {
    pub paths: Vec<String>,
    pub fds: Option<arvolo_ipc::client::FdHandoff>,
}

impl Resolved {
    /// Give back a hand-off nobody collected. A send that failed before the
    /// engine read it would otherwise hold the descriptor for the whole
    /// session.
    fn abandon(fds: &Option<arvolo_ipc::client::FdHandoff>) {
        if let Some(h) = fds {
            arvolo_ipc::fdpass::drop_stashed(&h.token);
        }
    }
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
        let meta = std::fs::metadata(&path).ok();
        let dto = PickedItemDto {
            id: String::new(), // filled by `keep`
            name: path
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| path.to_string_lossy().into_owned()),
            size: meta.as_ref().map(|m| m.len()).unwrap_or(0),
            is_dir: meta.map(|m| m.is_dir()).unwrap_or(false),
        };
        self.keep(Picked::Path(path), dto)
    }

    /// Register an already-open file under the name it should travel as.
    fn register_handle(&self, file: std::fs::File, name: String) -> PickedItemDto {
        let dto = PickedItemDto {
            id: String::new(),
            name: name.clone(),
            size: file.metadata().map(|m| m.len()).unwrap_or(0),
            is_dir: false,
        };
        self.keep(Picked::Handle { file, name }, dto)
    }

    fn keep(&self, picked: Picked, mut dto: PickedItemDto) -> PickedItemDto {
        let id = {
            let b: [u8; 16] = rand::random();
            data_encoding::HEXLOWER.encode(&b)
        };
        dto.id = id.clone();
        let mut inner = self.inner.lock().unwrap();
        inner.order.push_back(id.clone());
        inner.by_id.insert(id, picked);
        while inner.order.len() > PICKED_CAP {
            if let Some(old) = inner.order.pop_front() {
                inner.by_id.remove(&old);
            }
        }
        dto
    }

    /// What `ids` stand for, in order. Any unknown id fails the whole call: a
    /// partial send the user did not ask for is worse than an error they can see.
    ///
    /// A selection of paths resolves to paths, exactly as it always did. One
    /// picked descriptor resolves to its *name* plus a hand-off — which is what
    /// the engine's `resolve_payload_for` was already written for: with a
    /// hand-off present it takes the name from the string and never stats it.
    /// More than one descriptor has no such route (several files are packed
    /// into an archive, and the packer reads paths), so those are spooled into
    /// the app's own storage first and sent from there.
    fn resolve(&self, ids: &[String], spool: Option<&std::path::Path>) -> Result<Resolved, String> {
        let mut inner = self.inner.lock().unwrap();
        let mut picked = Vec::with_capacity(ids.len());
        for id in ids {
            picked.push(
                inner
                    .by_id
                    .remove(id)
                    .ok_or_else(|| "stale file selection — pick the files again".to_string())?,
            );
        }
        drop(inner);

        let handles = picked
            .iter()
            .filter(|p| matches!(p, Picked::Handle { .. }))
            .count();
        // One file, one descriptor: the whole point — nothing is copied.
        if handles == 1 && picked.len() == 1 {
            let Some(Picked::Handle { file, name }) = picked.pop() else {
                unreachable!("counted one handle")
            };
            let token = arvolo_ipc::fdpass::stash_files(vec![file]);
            return Ok(Resolved {
                paths: vec![name],
                fds: Some(arvolo_ipc::client::FdHandoff {
                    socket: arvolo_ipc::fdpass::IN_PROCESS.to_string(),
                    token,
                }),
            });
        }
        let mut paths = Vec::with_capacity(picked.len());
        for p in picked {
            match p {
                Picked::Path(path) => paths.push(path.to_string_lossy().into_owned()),
                Picked::Handle { mut file, name } => {
                    let dir = spool.ok_or_else(|| {
                        "several files from the picker need somewhere to be gathered".to_string()
                    })?;
                    std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
                    let out = dir.join(&name);
                    let mut w = std::fs::File::create(&out).map_err(|e| e.to_string())?;
                    // Streamed, not read into memory: a picked video is as big
                    // as the phone's storage allows.
                    std::io::copy(&mut file, &mut w).map_err(|e| e.to_string())?;
                    paths.push(out.to_string_lossy().into_owned());
                }
            }
        }
        Ok(Resolved { paths, fds: None })
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
    media: bool,
) -> Result<Vec<PickedItemDto>, String> {
    use tauri_plugin_dialog::DialogExt;
    let (tx, rx) = tokio::sync::oneshot::channel();
    let dialog = app.dialog().file();
    // Photos and videos do not live in a file system a person recognises, and
    // hunting for one through a document browser — Photos, then a date, then a
    // filename nobody chose — is not how anybody looks for a picture. Both
    // platforms have a picker made for it: a grid of what the pictures actually
    // look like. The plugin has always been able to ask for it; nothing did.
    #[cfg(mobile)]
    let dialog = if media {
        dialog.set_picker_mode(tauri_plugin_dialog::PickerMode::Media)
    } else {
        dialog
    };
    #[cfg(desktop)]
    let _ = media;
    // Picking a *folder* is a desktop idea, and the plugin says so: there is no
    // such call on mobile, where what you get is a document picker over files.
    // Sending a folder from a phone is the share sheet's job, not this one's.
    #[cfg(desktop)]
    if directory {
        dialog.pick_folders(move |p| {
            let _ = tx.send(p);
        });
    } else {
        dialog.pick_files(move |p| {
            let _ = tx.send(p);
        });
    }
    #[cfg(mobile)]
    {
        let _ = directory;
        dialog.pick_files(move |p| {
            let _ = tx.send(p);
        });
    }
    let picked = rx.await.map_err(|e| e.to_string())?;
    // Worth saying out loud on a phone: the plugin reports a refusal, a
    // deserialisation failure and a genuine "changed my mind" as the same
    // `None`, and the difference between them is the difference between a bug
    // and a person tapping Back.
    tracing::info!(
        "picker returned {}",
        match &picked {
            None => "nothing (cancelled, or the plugin could not read the result)".to_string(),
            Some(v) => format!("{} item(s): {v:?}", v.len()),
        }
    );
    let Some(picked) = picked else {
        return Ok(Vec::new()); // cancelled
    };
    let mut out = Vec::with_capacity(picked.len());
    for fp in picked {
        // A `content://` is not a path and `into_path` cannot make one — it
        // tries `to_file_path()` and fails, which is how picking a file on a
        // phone used to end in nothing at all. Resolve it to a descriptor here,
        // where the picker's grant is still live.
        match fp {
            tauri_plugin_dialog::FilePath::Url(u) if u.scheme() != "file" => {
                let file = open_content_uri(&app, u.clone())?;
                let name = handed_name(&file, &u);
                out.push(state.register_handle(file, name));
            }
            other => {
                let path = other.into_path().map_err(|e| e.to_string())?;
                out.push(state.register(path));
            }
        }
    }
    Ok(out)
}

/// Open what the platform gave us instead of a path.
fn open_content_uri(
    app: &tauri::AppHandle,
    url: tauri::Url,
) -> Result<std::fs::File, String> {
    use tauri_plugin_fs::FsExt;
    app.fs()
        .open(
            tauri_plugin_fs::FilePath::Url(url),
            tauri_plugin_fs::OpenOptions::new().read(true).to_owned(),
        )
        .map_err(|e| format!("cannot open the file you picked: {e}"))
}

/// What to call a file we only ever saw as a descriptor.
///
/// The picker hands over a URI whose last segment is sometimes the name
/// (`primary:Download/report.pdf`) and sometimes an opaque row id
/// (`document:1000000033`). The descriptor knows better: on Linux — and Android
/// is Linux — `/proc/self/fd/N` links to the real file whenever the provider is
/// backed by one, which covers everything on the device itself. A provider that
/// streams (a cloud drive) leaves a pipe there, and then the URI's own guess is
/// all there is.
fn handed_name(file: &std::fs::File, url: &tauri::Url) -> String {
    #[cfg(target_os = "android")]
    {
        use std::os::fd::AsRawFd;
        if let Ok(real) = std::fs::read_link(format!("/proc/self/fd/{}", file.as_raw_fd())) {
            if let Some(n) = real.file_name() {
                let n = n.to_string_lossy();
                if !n.is_empty() && !n.starts_with("pipe:") && !n.starts_with("socket:") {
                    return n.into_owned();
                }
            }
        }
    }
    let _ = file;
    let guess = url
        .path_segments()
        .and_then(|s| s.last())
        .unwrap_or_default();
    let decoded = percent_decode(guess);
    let tail = decoded
        .rsplit(['/', ':'])
        .next()
        .unwrap_or_default()
        .trim();
    if tail.is_empty() {
        "file".to_string()
    } else {
        tail.to_string()
    }
}

/// Enough of one to read a picker's URI segment; the general case is a
/// dependency this crate does not need.
fn percent_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' && i + 2 < b.len() {
            if let Ok(v) = u8::from_str_radix(&s[i + 1..i + 3], 16) {
                out.push(v);
                i += 3;
                continue;
            }
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// The app is going to sleep, or has just woken.
///
/// A phone freezes an app it puts in the background, and a transfer frozen in
/// flight comes back as a failure with its resume record already dropped —
/// which is why the only thing that used to survive backgrounding was a
/// transfer that had finished first. Stopping cleanly on the way down keeps the
/// partial *and* the record, and marks who stopped it, so coming back restarts
/// exactly that and never what a person paused on purpose.
#[tauri::command]
pub async fn suspend_transfers() -> Result<(), String> {
    let mut c = client().await?;
    c.suspend().await.or_else(err)
}

#[tauri::command]
pub async fn resume_suspended_transfers() -> Result<(), String> {
    let mut c = client().await?;
    c.resume_suspended().await.or_else(err)
}

#[tauri::command]
pub async fn send_to(
    app: tauri::AppHandle,
    state: tauri::State<'_, PickedFiles>,
    to: String,
    items: Vec<String>,
    note: String,
    device: Option<String>,
) -> Result<u64, String> {
    let r = state.resolve(&items, spool(&app).as_deref())?;
    let mut c = client().await?;
    // The GUI's live send carries no mailbox options: its mailbox mode goes
    // through `deposit` below, which has always carried them.
    // On a desktop the daemon is a child of this app and inherits its
    // file-access grants, so there is nothing to hand over; on a phone the
    // payload IS a descriptor, and `r.fds` is how it travels.
    let out = c
        .push(to, r.paths, note, None, None, None, device, r.fds.clone())
        .await;
    if out.is_err() {
        Resolved::abandon(&r.fds);
    }
    out.or_else(err)
}

/// Where several picked descriptors are gathered so the archive packer can read
/// them as files. Inside the app's own storage, and only ever written by
/// [`PickedFiles::resolve`].
fn spool(app: &tauri::AppHandle) -> Option<PathBuf> {
    use tauri::Manager as _;
    app.path().app_cache_dir().ok().map(|d| d.join("outgoing"))
}

/// `send --deposit`: straight to the recipient's mailbox, with the options that
/// only mean anything there. Returns the `arvm…` ticket so the sender can also
/// hand it over themselves.
#[tauri::command]
#[allow(clippy::too_many_arguments)]
pub async fn deposit_to(
    app: tauri::AppHandle,
    state: tauri::State<'_, PickedFiles>,
    to: String,
    items: Vec<String>,
    note: String,
    ttl: Option<u64>,
    max: Option<u32>,
    password: Option<String>,
    device: Option<String>,
) -> Result<ServedDto, String> {
    let r = state.resolve(&items, spool(&app).as_deref())?;
    let mut c = client().await?;
    match c
        .deposit(to, r.paths, note, ttl, max, password, device, r.fds.clone())
        .await
    {
        Ok((id, ticket)) => Ok(ServedDto { id, ticket }),
        Err(e) => {
            Resolved::abandon(&r.fds);
            err(e)
        }
    }
}

/// The machines sharing this identity — see `Request::ListDevices` for what this
/// list is and, more to the point, what it is not.
#[tauri::command]
pub async fn list_devices() -> Result<Vec<DeviceDto>, String> {
    let mut c = client().await?;
    c.list_devices().await.or_else(err)
}

/// Rename the machine the daemon runs on. Only that one: a row belongs to the
/// device that wrote it, so the UI offers this on its own row and nowhere else.
#[tauri::command]
pub async fn rename_this_device(name: String) -> Result<(), String> {
    let mut c = client().await?;
    c.rename_this_device(name).await.or_else(err)
}

#[tauri::command]
pub async fn forget_device(who: String) -> Result<(), String> {
    let mut c = client().await?;
    c.forget_device(who).await.or_else(err)
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
    app: tauri::AppHandle,
    state: tauri::State<'_, PickedFiles>,
    items: Vec<String>,
    seed_relay: Option<String>,
) -> Result<ServedDto, String> {
    let r = state.resolve(&items, spool(&app).as_deref())?;
    let mut c = client().await?;
    match c.serve_ticket(r.paths, seed_relay, r.fds.clone()).await {
        Ok((id, ticket)) => Ok(ServedDto { id, ticket }),
        Err(e) => {
            Resolved::abandon(&r.fds);
            err(e)
        }
    }
}

#[tauri::command]
pub async fn create_link(
    app: tauri::AppHandle,
    state: tauri::State<'_, PickedFiles>,
    item: String,
    ttl: Option<u64>,
    max: Option<u32>,
) -> Result<String, String> {
    let mut r = state.resolve(std::slice::from_ref(&item), spool(&app).as_deref())?;
    let mut c = client().await?;
    let out = c
        .create_link(r.paths.remove(0), ttl, max, r.fds.clone())
        .await;
    if out.is_err() {
        Resolved::abandon(&r.fds);
    }
    out.or_else(err)
}

#[tauri::command]
pub async fn accept_offer(
    state: tauri::State<'_, PickedFiles>,
    offer_id: String,
    out_item: Option<String>,
    password: Option<String>,
) -> Result<u64, AcceptFailed> {
    let out = match out_item {
        Some(id) => Some(
            // A destination, not a source: always a real path, so no hand-off
            // and nowhere to spool.
            state
                .resolve(std::slice::from_ref(&id), None)?
                .paths
                .remove(0),
        ),
        None => None,
    };
    let mut c = client().await?;
    c.accept_with_password(offer_id, out.map(PathBuf::from), password)
        .await
        .map_err(AcceptFailed::from)
}

#[tauri::command]
pub async fn reject_offer(app: tauri::AppHandle, offer_id: String) -> Result<(), String> {
    let mut c = client().await?;
    let done = c.reject(offer_id).await.or_else(err);
    // Rejecting emits no engine event, so the pump never hears about it and the
    // mark would keep breathing over an offer that is gone. Accepting needs no such
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

/// Park a waiting send on the relay instead of waiting longer for an accept.
/// Errors when the send is not waiting: the daemon refuses, and the refusal is
/// the point — a bare `Ok` would tell the user their file is on its way by a
/// route it never took.
#[tauri::command]
pub async fn deposit_now(id: u64) -> Result<(), String> {
    let mut c = client().await?;
    c.deposit_now(id).await.or_else(err)
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
    app: tauri::AppHandle,
    state: tauri::State<'_, PickedFiles>,
    items: Vec<String>,
    relay: Option<String>,
    keep: bool,
) -> Result<CodeDto, String> {
    let r = state.resolve(&items, spool(&app).as_deref())?;
    let mut c = client().await?;
    match c.serve_code(r.paths, relay, keep, r.fds.clone()).await {
        Ok((id, code)) => Ok(CodeDto { id, code }),
        Err(e) => {
            Resolved::abandon(&r.fds);
            err(e)
        }
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
        Some(id) => Some(
            // A destination, not a source: always a real path, so no hand-off
            // and nowhere to spool.
            state
                .resolve(std::slice::from_ref(&id), None)?
                .paths
                .remove(0),
        ),
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

/// Point an existing contact name at a different key. Separate from
/// [`add_contact`] on purpose: that one is refused on a name collision, and this
/// is the only way past it — so nothing can re-key a contact, and drop the marks
/// that go with it, without having asked first. Returns the old and new
/// fingerprints when a key really did change.
#[tauri::command]
pub async fn rebind_contact(name: String, id: String) -> Result<Option<(String, String)>, String> {
    let mut c = client().await?;
    c.rebind_contact(name, id).await.or_else(err)
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
/// a terminal for. A leftover `daemon stop` marker is cleared first: an in-app
/// restart is the user asking for it to be up again.
#[cfg(desktop)]
#[tauri::command]
pub async fn restart_daemon() -> Result<(), String> {
    let _ = std::fs::remove_file(arvolo_ipc::stop_marker_path());
    crate::daemon::stop().await.map_err(|e| format!("{e:#}"))
}

/// There is no daemon here to restart: the engine is this process, and restarting
/// it means restarting the app.
#[cfg(mobile)]
#[tauri::command]
pub async fn restart_daemon() -> Result<(), String> {
    Err("the engine runs inside the app on this platform".into())
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

/// Hand a path to the system opener (`open` on macOS, `xdg-open` on Linux).
///
/// The opener plugin's own `open_path` command is not used: its ACL wants a
/// *static* scope in `capabilities/`, and none of the three paths this app opens
/// is knowable at build time — the download folder is configurable, a receive can
/// be saved anywhere the accept dialog pointed, and `config.toml` follows the
/// platform's config dir. `reveal_item_in_dir` (the "open folder" action) has no
/// scope at all, which is why only "open file" was failing.
///
/// So the check happens here instead, against the daemon rather than against a
/// glob: the path must be one the daemon itself reports — a completed receive's
/// resting place, the download folder, or `config.toml`. Same stance as
/// `PickedFiles`: the webview never gets to name a path of its own choosing.
#[tauri::command]
pub async fn open_path(app: tauri::AppHandle, path: String) -> Result<(), String> {
    let mut c = client().await?;
    let cfg = c.get_config().await.or_else(err)?;
    let known = path == cfg.download_dir
        || path == cfg.config_path
        || c.list()
            .await
            .or_else(err)?
            .iter()
            .any(|t| t.path.as_deref() == Some(path.as_str()));
    if !known {
        return Err(format!("path not open-able from here: {path}"));
    }
    tauri_plugin_opener::OpenerExt::opener(&app)
        .open_path(&path, None::<&str>)
        .map_err(|e| format!("{path}: {e}"))
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
        assert!(reg.resolve(std::slice::from_ref(&a.id), None).is_ok());
        assert!(
            reg.resolve(&[a.id, "no-such-id".into()], None).is_err(),
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
        assert!(reg.resolve(&[one.id, two.id], None).is_ok());
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
            reg.resolve(std::slice::from_ref(&first.id), None).is_err(),
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
