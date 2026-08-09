//! `arvolo cancel <id>` — take back anything `arvolo status` shows.
//!
//! The user sees one list, so they get one verb. Behind it are three kinds of
//! thing, told apart by the shape of their id and by which store holds them:
//!
//! * a **running transfer** — the daemon owns it, and its ids are plain numbers
//!   ([`arvolo_core::manager::Transfer::id`]);
//! * a **deposit** — a file left on a relay (a public link or a sealed mailbox
//!   send), whose id is 8 hex chars ([`crate::deposits::id_for`]);
//! * a **resumable send** — an interrupted P2P send saved locally, whose id is
//!   also 8 hex chars ([`crate::sessions::id_for`]);
//! * the **`arvm…` ticket or download link itself** — for withdrawing something
//!   sent from *another* machine, where there is no local record and so no local
//!   copy of the revoke token. That one case is why `--token` exists, and it is
//!   the only case that needs it. This used to be a separate `arvolo revoke`
//!   verb; it was never a separate intention.
//!
//! The ids don't fall into tidy disjoint shapes, so **presence in a store decides**
//! — the stores are asked first, and only an id nothing on disk claims is read as a
//! transfer number. The tempting order is the other way round, and it's wrong: an
//! 8-hex id is all digits about one time in forty (`(10/16)^8`), so `73406183` is a
//! perfectly ordinary deposit that `parse::<u64>()` would happily swallow, sending
//! the user to "no such transfer" for a file they can see in the list.
//!
//! Reading the other way is safe: a deposit id is always 8 chars, so a real
//! transfer number would have to reach ~73 million — from a per-process counter
//! that starts at 1 — before it could shadow one.
//!
//! Only a transfer needs a daemon. A deposit or a session can be taken back from a
//! bare CLI on any platform, which is why this module isn't unix-gated.

use anyhow::{bail, Result};

use crate::commands::offline;
use crate::{deposits, sessions};

pub(crate) async fn cancel_cmd(id: String, token: Option<String>) -> Result<()> {
    if deposits::load(&id).is_some() {
        return cancel_deposit(&id).await;
    }
    // `sessions::load` 404s with its own message; we only want to know if it's there.
    if sessions::load(&id).is_ok() {
        sessions::remove(&id)?;
        println!("Dropped resumable send '{id}' — the ticket you shared no longer works.");
        return Ok(());
    }
    if let Some((relay, claim)) = offline::withdrawal_target(&id) {
        // A ticket for something we deposited ourselves: the local record holds
        // the token already, so don't make the user find it — and the local path
        // also retracts the offer pointing at the blob, which a bare token
        // revocation cannot do.
        let local = deposits::id_for(&claim);
        if deposits::load(&local).is_some() {
            return cancel_deposit(&local).await;
        }
        let Some(token) = token else {
            bail!(
                "this machine has no record of that ticket, so withdrawing it needs \
                 `--token <t>` — the revoke token printed when you sent it. (From the \
                 machine that sent it, `arvolo cancel <id>` needs no token; the id is \
                 in `arvolo status`.)"
            )
        };
        return offline::revoke_by_token(&relay, &claim, &token).await;
    }
    if let Ok(tid) = id.parse::<u64>() {
        return cancel_transfer(tid).await;
    }
    bail!("no transfer, deposit or resumable send with id '{id}' — see `arvolo status`")
}

/// A live transfer: only the daemon can stop one, since only the daemon is running it.
#[cfg(unix)]
async fn cancel_transfer(id: u64) -> Result<()> {
    use anyhow::Context;

    let mut client = crate::commands::daemon::daemon_client()
        .await
        .context("no daemon running (start `arvolo daemon`)")?;
    client.cancel(id).await?;
    eprintln!("cancelled transfer {id}.");
    Ok(())
}

#[cfg(not(unix))]
async fn cancel_transfer(id: u64) -> Result<()> {
    bail!("transfer {id} needs a running daemon, which this platform doesn't support yet")
}

/// A file left on a relay. With a daemon we hand this to it, because a deposit *the
/// engine* made is only fully withdrawn from inside it — the engine's `cancel` also
/// ends the live row it is still holding open. Without a daemon (or for a deposit no
/// engine ever knew about), [`deposits::withdraw`] does the same job from the record.
async fn cancel_deposit(id: &str) -> Result<()> {
    #[cfg(unix)]
    if let Some(mut client) = crate::commands::daemon::daemon_client().await {
        client.revoke_deposit(id.to_string()).await?;
        println!("Revoked on the relay — the link/ticket no longer works.");
        return Ok(());
    }

    let Some(rec) = deposits::load(id) else {
        bail!("no deposit '{id}'")
    };
    deposits::withdraw(&rec).await?;
    println!(
        "Revoked on the relay — '{}' is deleted; the link/ticket no longer works.",
        rec.name
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    //! Each test holds the process-global `testlock::ENV` guard across its awaits on
    //! purpose: it keeps `ARVOLO_CONFIG_DIR` — and so the stores these commands read
    //! — pointed at this test's temp dir for the whole body. One test holds it at a
    //! time and nothing awaited re-acquires it, so there's no deadlock.
    #![allow(clippy::await_holding_lock)]

    use super::*;

    /// The three id shapes reach the three stores. Deposit and session ids are both
    /// 8 hex chars, so only presence tells them apart — a session id must not be
    /// mistaken for a deposit, and an unknown id must point at the list rather than
    /// failing bare.
    #[tokio::test]
    async fn an_unknown_id_says_where_to_look() {
        let _guard = crate::testlock::ENV
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let dir = tempfile::tempdir().unwrap();
        std::env::set_var("ARVOLO_CONFIG_DIR", dir.path());

        let err = cancel_cmd("00ff00ff".into(), None)
            .await
            .unwrap_err()
            .to_string();
        assert!(err.contains("arvolo status"), "got: {err}");

        std::env::remove_var("ARVOLO_CONFIG_DIR");
    }

    /// An 8-hex deposit id is all digits about one time in forty, and `73406183` is
    /// a real one this code produced. Checking `parse::<u64>()` first would read it
    /// as a transfer number and send the user to "no such transfer" for a deposit
    /// sitting right there in the list — so the stores are asked first.
    ///
    /// Written straight to disk: `save` derives the id from the claim, so an
    /// all-digit one can't be requested, only stumbled into.
    #[tokio::test]
    async fn an_all_digit_deposit_id_is_not_mistaken_for_a_transfer() {
        let _guard = crate::testlock::ENV
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let dir = tempfile::tempdir().unwrap();
        std::env::set_var("ARVOLO_CONFIG_DIR", dir.path());

        let toml = r#"
id = "73406183"
kind = "link"
relay = "http://192.0.2.1:1"
claim = "someclaim"
revoke_token = "tok"
name = "f.txt"
size = 3
max = 4294967295
created = 1700000000
expires = 1700003600
"#;
        let deposits_dir = dir.path().join("deposits");
        std::fs::create_dir_all(&deposits_dir).unwrap();
        std::fs::write(deposits_dir.join("73406183.toml"), toml).unwrap();
        assert!(deposits::load("73406183").is_some());

        // Long expired, so the relay is never dialled: this returns on the deposit
        // path alone. Down the transfer path it would say "no daemon"/"no transfer".
        cancel_cmd("73406183".into(), None)
            .await
            .expect("an all-digit id must reach the deposit it names");
        assert!(deposits::load("73406183").is_none(), "must be withdrawn");

        std::env::remove_var("ARVOLO_CONFIG_DIR");
    }

    /// A resumable send is dropped locally: no relay is involved, so this works with
    /// no daemon and no network — and must not be routed down the deposit path.
    #[tokio::test]
    async fn a_resumable_send_id_drops_that_session() {
        let _guard = crate::testlock::ENV
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let dir = tempfile::tempdir().unwrap();
        std::env::set_var("ARVOLO_CONFIG_DIR", dir.path());

        let rec = sessions::save(
            [7u8; arvolo_core::crypto::CHUNK_KEY_LEN],
            [9u8; 32],
            &[std::path::PathBuf::from("/tmp/video.mp4")],
            "video.mp4",
            false,
            190,
            12,
            "arvc-some-ticket",
        )
        .unwrap();
        assert!(sessions::load(&rec.id).is_ok());
        assert!(deposits::load(&rec.id).is_none(), "not a deposit id");

        cancel_cmd(rec.id.clone(), None).await.unwrap();
        assert!(sessions::load(&rec.id).is_err(), "session must be gone");

        std::env::remove_var("ARVOLO_CONFIG_DIR");
    }

    /// A link this machine never sent can only be withdrawn with its token, and
    /// the error has to name the flag — this is the one case where `cancel` needs
    /// something the user has to go and find.
    #[tokio::test]
    async fn a_foreign_link_without_a_token_names_the_flag() {
        let _guard = crate::testlock::ENV
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let dir = tempfile::tempdir().unwrap();
        std::env::set_var("ARVOLO_CONFIG_DIR", dir.path());

        let err = cancel_cmd("https://relay.example.com/dl/someclaim#k".into(), None)
            .await
            .unwrap_err()
            .to_string();
        assert!(err.contains("--token"), "got: {err}");

        std::env::remove_var("ARVOLO_CONFIG_DIR");
    }

    /// The same link, but this machine *does* hold the record: the token is ours
    /// already, so it must take the local path instead of demanding one — and that
    /// path is also the only one that retracts the recipient's offer.
    #[tokio::test]
    async fn a_link_we_still_have_a_record_of_needs_no_token() {
        let _guard = crate::testlock::ENV
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let dir = tempfile::tempdir().unwrap();
        std::env::set_var("ARVOLO_CONFIG_DIR", dir.path());

        // Long expired, so the relay is never dialled: this returns on the local
        // deposit path alone.
        let id = deposits::id_for("someclaim");
        let toml = format!(
            r#"
id = "{id}"
kind = "link"
relay = "http://192.0.2.1:1"
claim = "someclaim"
revoke_token = "tok"
name = "f.txt"
size = 3
max = 4294967295
created = 1700000000
expires = 1700003600
"#
        );
        let deposits_dir = dir.path().join("deposits");
        std::fs::create_dir_all(&deposits_dir).unwrap();
        std::fs::write(deposits_dir.join(format!("{id}.toml")), toml).unwrap();

        cancel_cmd("https://192.0.2.1:1/dl/someclaim#k".into(), None)
            .await
            .expect("a ticket we hold the record for must withdraw without a token");
        assert!(deposits::load(&id).is_none(), "must be withdrawn");

        std::env::remove_var("ARVOLO_CONFIG_DIR");
    }
}
