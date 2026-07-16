//! Persisted relay-deposit sessions. Every mailbox/link send (`send --to` to an
//! offline recipient / `--ticket`, or `send --link`) — whether sealed to a
//! contact (`arvm` ticket) or a public browser `--link` — leaves the encrypted
//! file on a relay. We record it here, including the sender-only **revoke
//! token**, so the sender can later list and cancel it without having kept the
//! printed token. Removing a record revokes the blob on the relay (see `arvolo
//! cancel <id>`), so the file/link lives exactly as long as this local record.
//!
//! Deposits are listed by `arvolo transfers` (the "left on relay" section) and by
//! the GUI's deposits panel, both through [`list_dtos`].
//!
//! A record holds the revoke token (a capability secret), so its file is written
//! owner-only (0600), like the resumable-send [`crate::sessions`] store.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use arvolo_core::presence::PostedOffer;
use arvolo_core::reexport::Hash;
use arvolo_ipc::protocol::DepositDto;
use data_encoding::HEXLOWER;
use serde::{Deserialize, Serialize};

use crate::book;

/// Record kind: a public download link vs a recipient-sealed offline ticket.
pub const KIND_LINK: &str = "link";
pub const KIND_OFFLINE: &str = "offline";

/// Sentinel download cap meaning "no limit" (a link's default). Passed to the
/// relay as-is; the relay clamps it to its own maximum.
pub const UNLIMITED: u32 = u32::MAX;

/// How long past its expiry a record is kept before being swept.
///
/// Note what this is *not* insuring against: a clock offset. The TTL is a duration,
/// not an instant. We deadline at `created + ttl` off our clock; the relay deadlined
/// at `created + ttl` off its own, from the same physical moment. A clock an hour
/// fast puts `created` an hour ahead too, so both sides reach `now >= expires`
/// together — a standing offset cancels, and knowing the relay's wall time would
/// correct the one thing already correct. (The relay never reports its deadline
/// anyway: `/v1/entry/{claim}/status` answers presence and counts, nothing more.)
///
/// What survives the cancellation is our clock *changing* across the interval —
/// drift (minutes over a 7-day TTL, at worst), or a jump: an NTP step, a suspended
/// VM, a hand-set clock. Then `expires`, computed before the jump, is read after it,
/// and we could call a blob dead while the relay still serves it — binning the token
/// for something still fetchable. An hour covers any plausible drift and most jumps,
/// and costs only an hour of a dead row lingering.
const REAP_GRACE_SECS: u64 = 3600;

fn deposits_dir() -> PathBuf {
    book::config_dir().join("deposits")
}

fn record_path(id: &str) -> PathBuf {
    deposits_dir().join(format!("{id}.toml"))
}

/// A file left on a relay by a mailbox/link send.
#[derive(Serialize, Deserialize, Clone)]
pub struct DepositRecord {
    pub id: String,
    /// [`KIND_LINK`] (public download URL) or [`KIND_OFFLINE`] (sealed `arvm`).
    pub kind: String,
    pub relay: String,
    pub claim: String,
    /// Sender-only revoke secret. Secret — the file is 0600.
    pub revoke_token: String,
    pub name: String,
    pub size: u64,
    /// Download cap requested at deposit. `u32::MAX` means effectively unlimited
    /// (the relay clamps it to its own cap); a link defaults to this, a sealed
    /// offline deposit defaults to 1 (burn-after-read).
    pub max: u32,
    /// For a public link: the full browser URL. `None` for a sealed deposit.
    pub link: Option<String>,
    /// For a sealed deposit: the recipient's base32 id — always the resolved key,
    /// never the name that was typed, so a reader can decode it and a UI can match it
    /// to a contact. `None` for a link. (Records written before this was made true
    /// may hold a contact name; they decode to nothing, and fall back to being shown
    /// as-is.)
    pub recipient: Option<String>,
    pub created: u64,
    /// Unix seconds when the relay auto-expires the blob (`created + ttl`).
    pub expires: u64,
    /// The daemon transfer this deposit belongs to, when a daemon made it.
    ///
    /// It decides *how* the deposit is withdrawn. A deposit the daemon created also
    /// left an **offer in the recipient's inbox**; only the engine's `cancel` retracts
    /// that alongside revoking the blob. Revoking without it would delete the file and
    /// leave the recipient staring at an offer for something that is no longer there.
    /// `None` — a one-shot CLI deposit, or a record written before this field existed
    /// — means the engine has no row for it, and the withdrawal is done from here.
    #[serde(default)]
    pub transfer_id: Option<u64>,
    /// The offer left in the recipient's inbox pointing at this blob, and the token
    /// that retracts it. Empty for a link (nobody's inbox) or a record written before
    /// these existed.
    ///
    /// Withdrawing has to take *both* down. Revoking the blob alone leaves the
    /// recipient an offer for a file that is no longer there: their daemon keeps
    /// trying to fetch it, and a person sees an arrival that fails. The engine keeps
    /// its own copy of these for the deposits it makes ([`DepositedRecord`]); a
    /// one-shot `send --to` has no engine behind it, so it keeps them here.
    ///
    /// `poster_token` is a capability secret, like `revoke_token` — hence 0600, and
    /// hence never in `DepositDto`.
    #[serde(default)]
    pub offer_id: String,
    #[serde(default)]
    pub poster_token: String,
}

impl DepositRecord {
    /// Whether the relay TTL has already elapsed (the blob is likely gone).
    pub fn expired(&self) -> bool {
        now_secs() >= self.expires
    }

    /// Whether this record can be swept: past its TTL by enough that the relay has
    /// certainly let the blob go, so the record can no longer do anything.
    ///
    /// **Expiry is the only trigger, on purpose.** A record is kept for exactly one
    /// reason — it holds the revoke token — so it dies when the blob does. The relay
    /// drops a blob on the first of: download cap reached, TTL lapsed, or revoked.
    /// Of those, only the TTL can be known here: from the clock, for free, and for
    /// certain. A *download* proves nothing (a `--link` is unlimited by default and
    /// survives being fetched fifty times; even a sealed send takes `--max 3`), and
    /// "the relay says it's gone" costs a round-trip that an unreachable relay answers
    /// with silence — which must never be read as absence.
    ///
    /// The asymmetry is what settles it: keeping a dead record costs an ugly line,
    /// while binning a live one costs the token, leaving a file on a relay with no way
    /// to take it back. So sweep only on the signal that can't be wrong, and let a
    /// downloaded-but-unexpired one sit there saying "gone" until its TTL — for a
    /// `--link`, which writes no history, that record is the only trace it ever existed.
    pub fn reapable(&self) -> bool {
        now_secs() >= self.expires.saturating_add(REAP_GRACE_SECS)
    }

    /// A human label for the download cap (`unlimited` for the link default).
    pub fn max_label(&self) -> String {
        if self.max == UNLIMITED {
            "unlimited".to_string()
        } else {
            self.max.to_string()
        }
    }
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// A short, stable id derived from the claim (first 4 bytes of its BLAKE3, hex).
pub fn id_for(claim: &str) -> String {
    HEXLOWER.encode(&Hash::new(claim.as_bytes()).as_bytes()[..4])
}

/// Build and persist a deposit record. Returns it (its `id` is what the user
/// passes to `arvolo cancel <id>`).
///
/// `expires` is absolute (unix seconds), not a TTL: a deposit the daemon restores
/// after a restart must keep the relay's original deadline, and computing one here
/// from "now" would silently extend it on every restart.
///
/// Writing the same `claim` twice overwrites the same file — `id` is derived from
/// the claim — so re-recording a restored deposit is idempotent by construction.
/// `offer` is the inbox offer posted alongside the blob, when there was one — it is
/// taken whole rather than as a loose id/token pair, which in this argument list
/// would sit next to `revoke_token` and be one transposition away from a silent
/// disaster.
#[allow(clippy::too_many_arguments)]
pub fn save(
    kind: &str,
    relay: &str,
    claim: &str,
    revoke_token: &str,
    name: &str,
    size: u64,
    max: u32,
    link: Option<String>,
    recipient: Option<String>,
    expires: u64,
    transfer_id: Option<u64>,
    offer: Option<&PostedOffer>,
) -> Result<DepositRecord> {
    let rec = DepositRecord {
        id: id_for(claim),
        kind: kind.to_string(),
        relay: relay.to_string(),
        claim: claim.to_string(),
        revoke_token: revoke_token.to_string(),
        name: name.to_string(),
        size,
        max,
        link,
        recipient,
        created: now_secs(),
        expires,
        transfer_id,
        offer_id: offer.map(|o| o.id.clone()).unwrap_or_default(),
        poster_token: offer.map(|o| o.poster_token.clone()).unwrap_or_default(),
    };
    let dir = deposits_dir();
    std::fs::create_dir_all(&dir).context("create deposits dir")?;
    let s = toml::to_string_pretty(&rec).context("serialize deposit")?;
    let path = record_path(&rec.id);
    std::fs::write(&path, s).with_context(|| format!("write deposit {}", path.display()))?;
    restrict(&path);
    Ok(rec)
}

/// File a receipt for a deposit the engine just made, from its `Deposited` event.
///
/// This is what makes a mailbox send look the same however it was sent. The
/// one-shot CLI path calls [`save`] itself; the engine can't — it lives in
/// `arvolo-core`, below this store — so its two front-ends (the daemon, and a
/// foreground `send --to`) call this on the event instead. Without it, a deposit
/// made through the engine was missing from `arvolo transfers`' "left on relay"
/// section and from the GUI's deposits panel; a foreground send dropped the revoke
/// token entirely, leaving the file on the relay until its TTL with no way back.
///
/// `transfer_id` is the engine row, when there is one to cancel through.
///
/// Best-effort: a receipt we failed to write must not take down the transfer it
/// describes. The deposit itself stands either way.
pub fn record_from_event(transfer_id: Option<u64>, info: &arvolo_core::manager::DepositInfo) {
    if let Err(e) = save(
        KIND_OFFLINE,
        &info.relay,
        &info.claim,
        &info.revoke_token,
        &info.name,
        info.size,
        info.max,
        None,
        info.recipient.as_ref().map(crate::util::encode_id),
        info.expires,
        transfer_id,
        // Kept even though the engine holds its own copy: the daemon may be gone by
        // the time this is withdrawn, and then this record is all there is.
        Some(&PostedOffer {
            id: info.offer_id.clone(),
            poster_token: info.poster_token.clone(),
        }),
    ) {
        tracing::warn!("could not record deposit '{}': {e:#}", info.name);
    }
}

/// Take a deposit back off the relay and forget it: revoke the blob, retract the
/// inbox offer that points at it, then drop the record.
///
/// **Both halves, or it isn't a withdrawal.** A `send --to` leaves two things on the
/// relay — the sealed blob and an offer in the recipient's inbox — and killing only
/// the blob leaves the recipient an arrival they can never fetch: their daemon keeps
/// retrying a claim that 404s, and a person sees a file that never lands. The engine
/// has always done both for the deposits it makes (`cancel_deposited`); this is the
/// same job for the ones made without it.
///
/// Best-effort against the relay, deliberately. Neither call can be retried into
/// success by the user, and both become moot at the TTL — so a failure is reported
/// and the record still goes, rather than stranding it as unwithdrawable forever. An
/// expired deposit is not called at all: there is nothing left on the relay to take.
pub async fn withdraw(rec: &DepositRecord) -> Result<()> {
    if !rec.expired() {
        if let Err(e) =
            arvolo_core::flow::revoke_offline(&rec.relay, &rec.claim, &rec.revoke_token).await
        {
            eprintln!("⚠ relay revoke failed ({e}); the blob lapses at its TTL.");
        }
        retract_offer(rec).await;
    }
    remove(&rec.id)
}

/// Retract the inbox offer pointing at this deposit, if it left one. Silent when it
/// didn't (a link has no recipient) or when the record predates the tokens being
/// kept — those offers can only lapse at their TTL, which is what they did before.
async fn retract_offer(rec: &DepositRecord) {
    if rec.offer_id.is_empty() || rec.poster_token.is_empty() {
        return;
    }
    let Some(Ok(recipient)) = rec.recipient.as_deref().map(crate::book::decode_id) else {
        return;
    };
    if let Err(e) = arvolo_core::presence::retract_offer(
        &reqwest::Client::new(),
        &rec.relay,
        &recipient,
        &rec.offer_id,
        &rec.poster_token,
    )
    .await
    {
        eprintln!("⚠ couldn't retract the inbox offer ({e}); it lapses at its TTL.");
    }
}

/// Load a deposit record by id (`None` if there is no such record).
pub fn load(id: &str) -> Option<DepositRecord> {
    let s = std::fs::read_to_string(record_path(id)).ok()?;
    toml::from_str(&s).ok()
}

/// All saved deposit sessions, newest first.
pub fn list() -> Vec<DepositRecord> {
    let mut out: Vec<DepositRecord> = std::fs::read_dir(deposits_dir())
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| e.path().extension().is_some_and(|x| x == "toml"))
        .filter_map(|e| std::fs::read_to_string(e.path()).ok())
        .filter_map(|s| toml::from_str::<DepositRecord>(&s).ok())
        .collect();
    out.sort_by_key(|r| std::cmp::Reverse(r.created));
    out
}

/// Delete a deposit record locally (does not touch the relay).
pub fn remove(id: &str) -> Result<()> {
    std::fs::remove_file(record_path(id)).with_context(|| format!("remove deposit '{id}'"))?;
    Ok(())
}

/// How long the relay gets to answer one status query while the deposit list is
/// being built. The list must open promptly even when the relay is down, so a slow
/// answer degrades to "unknown" instead of hanging whoever asked.
const CLAIM_STATUS_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(4);

/// Everything this client has left on a relay and could still take back, newest
/// first (the order [`list`] already guarantees). The revoke token stays behind:
/// a caller needs the id, never the secret.
///
/// The local record is a *receipt*, not a status: it is written once, at deposit
/// time, and nothing ever updates it. A one-shot link that has been downloaded and
/// a sealed deposit the recipient collected both leave it untouched — the relay
/// never reports back. Listing records alone would therefore present dead entries
/// as alive, which is worse than saying nothing. So ask the relay, concurrently and
/// best-effort: unreachable leaves the live fields `None`, and the caller says it
/// does not know rather than inventing an answer.
///
/// Shared by the daemon's `ListDeposits` and by `arvolo transfers` without one, so
/// both render the same view of the same store.
///
/// Building the list also **sweeps** the records whose TTL is well past (see
/// [`DepositRecord::reapable`]): the blob is gone, so the record can neither be
/// listed honestly under "left on relay" nor withdraw anything, and nothing else
/// would ever tidy it — a link deposited once would leave its receipt on disk for
/// good. Listing is the only moment every record is looked at, so it's where the
/// sweep goes; it is idempotent and needs no relay. The engine reaps its own expired
/// records at startup for the same reason (see `restore_deposited`).
pub async fn list_dtos() -> Vec<DepositDto> {
    let recs: Vec<DepositRecord> = list()
        .into_iter()
        .filter(|d| {
            if d.reapable() {
                // Best-effort: a sweep we couldn't finish just happens next time.
                let _ = remove(&d.id);
                return false;
            }
            true
        })
        .collect();

    let mut set = tokio::task::JoinSet::new();
    for (i, d) in recs.iter().enumerate() {
        // An expired record has nothing left on the relay by definition — `expired`
        // already says so, and a request could only confirm it. Don't spend one.
        if d.expired() {
            continue;
        }
        let (relay, claim) = (d.relay.clone(), d.claim.clone());
        set.spawn(async move {
            let info = tokio::time::timeout(
                CLAIM_STATUS_TIMEOUT,
                arvolo_core::flow::claim_info(&relay, &claim),
            )
            .await
            .ok()
            .and_then(|r| r.ok());
            (i, info)
        });
    }
    let mut live: HashMap<usize, arvolo_core::flow::ClaimInfo> = HashMap::new();
    while let Some(joined) = set.join_next().await {
        if let Ok((i, Some(info))) = joined {
            live.insert(i, info);
        }
    }

    recs.into_iter()
        .enumerate()
        .map(|(i, d)| {
            let info = live.get(&i);
            DepositDto {
                expired: d.expired(),
                max_label: d.max_label(),
                present: info.map(|l| l.present),
                downloads: info.and_then(|l| l.downloads),
                max_downloads: info.and_then(|l| l.max_downloads),
                id: d.id,
                kind: d.kind,
                name: d.name,
                size: d.size,
                link: d.link.unwrap_or_default(),
                recipient: d.recipient.unwrap_or_default(),
                created: d.created,
                expires: d.expires,
            }
        })
        .collect()
}

#[cfg(unix)]
fn restrict(path: &Path) {
    use std::os::unix::fs::PermissionsExt;
    let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
}

#[cfg(not(unix))]
fn restrict(_path: &Path) {}

#[cfg(test)]
mod tests {
    //! The async test holds the process-global `testlock::ENV` guard across its
    //! awaits on purpose: it keeps `ARVOLO_CONFIG_DIR` — and so `deposits_dir()` —
    //! pointed at this test's temp dir for the whole body. One test holds it at a
    //! time and nothing awaited re-acquires it, so there's no deadlock.
    #![allow(clippy::await_holding_lock)]

    use super::*;

    #[test]
    fn deposit_record_roundtrips_and_lists() {
        let _guard = crate::testlock::ENV
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let dir = tempfile::tempdir().unwrap();
        std::env::set_var("ARVOLO_CONFIG_DIR", dir.path());

        let expires = now_secs() + 3600;
        let rec = save(
            KIND_LINK,
            "https://relay.example",
            "abc123claim",
            "revoketoken",
            "photo.jpg",
            4242,
            UNLIMITED,
            Some("https://relay.example/dl/abc123claim#key".into()),
            None,
            expires,
            None,
            None,
        )
        .unwrap();
        assert_eq!(rec.kind, KIND_LINK);
        assert_eq!(rec.expires, expires);
        assert_eq!(rec.max_label(), "unlimited");
        assert!(!rec.expired());

        let loaded = load(&rec.id).expect("load");
        assert_eq!(loaded.claim, "abc123claim");
        assert_eq!(loaded.revoke_token, "revoketoken");
        assert_eq!(list().len(), 1);

        remove(&rec.id).unwrap();
        assert!(load(&rec.id).is_none());
        assert!(list().is_empty());

        std::env::remove_var("ARVOLO_CONFIG_DIR");
    }

    /// `transfer_id` was added after release, so records already on users' disks
    /// don't have the key. They must keep loading — a deposit that stopped being
    /// listed would be a file left on a relay with no way to take it back.
    #[test]
    fn a_record_written_before_transfer_id_existed_still_loads() {
        let _guard = crate::testlock::ENV
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let dir = tempfile::tempdir().unwrap();
        std::env::set_var("ARVOLO_CONFIG_DIR", dir.path());

        // Exactly what `save` wrote before the field existed.
        let old = r#"
id = "deadbeef"
kind = "offline"
relay = "https://relay.example"
claim = "oldclaim"
revoke_token = "oldtoken"
name = "report.pdf"
size = 99
max = 1
created = 1700000000
expires = 1700003600
"#;
        std::fs::create_dir_all(deposits_dir()).unwrap();
        std::fs::write(record_path("deadbeef"), old).unwrap();

        let loaded = load("deadbeef").expect("an older record must still load");
        assert_eq!(loaded.claim, "oldclaim");
        // No engine row is claimed, so it withdraws by revoking the blob — the only
        // thing that was ever true for these.
        assert_eq!(loaded.transfer_id, None);
        // Nor any offer tokens: these records predate them being kept, so their offer
        // can only lapse at its TTL. Withdrawing must skip the retract, not guess.
        assert!(loaded.offer_id.is_empty());
        assert!(loaded.poster_token.is_empty());
        assert_eq!(list().len(), 1);

        std::env::remove_var("ARVOLO_CONFIG_DIR");
    }

    /// The offer's id and token are kept so a withdrawal can retract it later — the
    /// half that used to be dropped on the floor, leaving the recipient an arrival
    /// pointing at a blob that was already deleted.
    #[test]
    fn a_sealed_deposit_keeps_what_retracts_its_offer() {
        let _guard = crate::testlock::ENV
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let dir = tempfile::tempdir().unwrap();
        std::env::set_var("ARVOLO_CONFIG_DIR", dir.path());

        let rec = save(
            KIND_OFFLINE,
            "https://relay.example",
            "sealedclaim",
            "revoke-me",
            "budget.xlsx",
            10,
            1,
            None,
            Some("k7x2".into()),
            now_secs() + 3600,
            None,
            Some(&PostedOffer {
                id: "offer-1".into(),
                poster_token: "poster-1".into(),
            }),
        )
        .unwrap();

        let loaded = load(&rec.id).expect("load");
        assert_eq!(loaded.offer_id, "offer-1");
        assert_eq!(loaded.poster_token, "poster-1");
        // (That neither reaches a UI is settled by `DepositDto` having nowhere to put
        // them — a capability the compiler keeps off the wire needs no test.)

        std::env::remove_var("ARVOLO_CONFIG_DIR");
    }

    /// A record whose TTL is long past is swept: the blob is gone, so it can't be
    /// listed honestly under "left on relay" and can't withdraw anything. Nothing
    /// else would ever tidy it — a link's receipt would sit on disk for good.
    #[tokio::test]
    async fn a_long_expired_record_is_swept_when_the_list_is_built() {
        let _guard = crate::testlock::ENV
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let dir = tempfile::tempdir().unwrap();
        std::env::set_var("ARVOLO_CONFIG_DIR", dir.path());

        let rec = save(
            KIND_LINK,
            "http://192.0.2.1:1",
            "deadclaim",
            "tok",
            "old.zip",
            10,
            UNLIMITED,
            None,
            None,
            now_secs().saturating_sub(REAP_GRACE_SECS + 60),
            None,
            None,
        )
        .unwrap();
        assert!(rec.reapable());

        assert!(list_dtos().await.is_empty(), "must not be listed");
        assert!(load(&rec.id).is_none(), "the record must be gone from disk");

        std::env::remove_var("ARVOLO_CONFIG_DIR");
    }

    /// Just-expired is **not** swept. `expires` is our own arithmetic off our own
    /// clock; if it runs fast we'd bin the revoke token for a blob the relay is still
    /// serving — the one mistake here that actually costs something. The grace buys
    /// that back, and the row stays visible saying so in the meantime.
    #[tokio::test]
    async fn a_just_expired_record_survives_the_grace_window() {
        let _guard = crate::testlock::ENV
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let dir = tempfile::tempdir().unwrap();
        std::env::set_var("ARVOLO_CONFIG_DIR", dir.path());

        let rec = save(
            KIND_OFFLINE,
            "http://192.0.2.1:1",
            "freshclaim",
            "tok",
            "recent.zip",
            10,
            1,
            None,
            None,
            now_secs().saturating_sub(60),
            None,
            None,
        )
        .unwrap();
        assert!(rec.expired(), "past its TTL");
        assert!(!rec.reapable(), "but not by enough to be sure");

        let dtos = list_dtos().await;
        assert_eq!(dtos.len(), 1, "must still be listed");
        assert!(dtos[0].expired);
        assert!(load(&rec.id).is_some(), "and still withdrawable");

        std::env::remove_var("ARVOLO_CONFIG_DIR");
    }

    /// An expired deposit is gone from the relay by definition, so `list_dtos` must
    /// not spend a round-trip asking about it — it reports `expired` and leaves the
    /// live fields `None`. The unreachable relay URL here is the assertion: if the
    /// code queried it, the test would sit through the timeout.
    #[tokio::test]
    async fn list_dtos_does_not_query_the_relay_for_expired_deposits() {
        let _guard = crate::testlock::ENV
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let dir = tempfile::tempdir().unwrap();
        std::env::set_var("ARVOLO_CONFIG_DIR", dir.path());

        let rec = save(
            KIND_OFFLINE,
            // Reserved for documentation, and refused fast if it were ever dialled.
            "http://192.0.2.1:1",
            "expiredclaim",
            "tok",
            "old.zip",
            10,
            1,
            None,
            None,
            now_secs().saturating_sub(60),
            None,
            None,
        )
        .unwrap();
        assert!(rec.expired());

        let started = std::time::Instant::now();
        let dtos = list_dtos().await;
        assert!(
            started.elapsed() < CLAIM_STATUS_TIMEOUT,
            "an expired deposit must not be queried"
        );
        assert_eq!(dtos.len(), 1);
        assert!(dtos[0].expired);
        assert_eq!(dtos[0].present, None);

        std::env::remove_var("ARVOLO_CONFIG_DIR");
    }
}
