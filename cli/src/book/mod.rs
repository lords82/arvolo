//! Local config + contacts (address book), stored under ~/.config/arvolo.

mod blocked;
mod config;
mod contacts;
mod marks;
mod names;
mod paths;
mod seen;
mod sync_bridge;
mod trusted;
mod verified;

pub(crate) use blocked::*;
pub(crate) use config::*;
pub(crate) use contacts::*;
pub(crate) use marks::*;
pub(crate) use names::*;
pub(crate) use paths::*;
pub(crate) use seen::*;
pub(crate) use sync_bridge::*;
pub(crate) use trusted::*;
pub(crate) use verified::*;

#[cfg(test)]
mod tests {
    use super::*;

    use arvolo_core::crypto::PublicId;

    use arvolo_core::crypto::Identity;

    #[test]
    fn relay_scheme_defaults_to_https() {
        // Bare host → https, unless --use-http is asked for.
        assert_eq!(
            normalize_relay("relay.example.com", false),
            "https://relay.example.com"
        );
        assert_eq!(
            normalize_relay("relay.example.com", true),
            "http://relay.example.com"
        );
        assert_eq!(
            normalize_relay("  relay:8787 ", false),
            "https://relay:8787"
        );
        // An explicit scheme always wins over the flag.
        assert_eq!(
            normalize_relay("http://relay.local", false),
            "http://relay.local"
        );
        assert_eq!(
            normalize_relay("https://relay.example.com", true),
            "https://relay.example.com"
        );
    }

    #[test]
    fn contacts_and_config_roundtrip() {
        let _guard = crate::testlock::ENV
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let dir = tempfile::tempdir().unwrap();
        std::env::set_var("ARVOLO_CONFIG_DIR", dir.path());
        std::env::remove_var("ARVOLO_RELAY");

        // Config: default_relay reads the config.toml `relay`.
        std::fs::write(
            dir.path().join("config.toml"),
            "relay = \"https://relay.example.com\"\n",
        )
        .unwrap();
        assert_eq!(
            default_relay().as_deref(),
            Some("https://relay.example.com")
        );

        // Contacts: add, resolve by name, list, remove.
        let id = Identity::generate().public();
        let id_b32 = data_encoding::BASE32_NOPAD
            .encode(&id.to_bytes())
            .to_lowercase();
        contact_add("alice", &id_b32).unwrap();
        assert_eq!(
            resolve_recipient("alice").unwrap().to_bytes(),
            id.to_bytes()
        );
        // A raw id resolves too (not a contact name).
        assert_eq!(
            resolve_recipient(&id_b32).unwrap().to_bytes(),
            id.to_bytes()
        );

        // Reverse-lookup: id -> saved contact name.
        assert_eq!(resolve_name(&id_b32).as_deref(), Some("alice"));
        assert_eq!(resolve_name("nonexistentid"), None);

        // TOFU ledger: unseen at first, then seen after recording a receipt.
        let st = sender_status(&id_b32);
        assert_eq!(st.name.as_deref(), Some("alice"));
        assert!(!st.seen_before, "sender not seen before the first receipt");
        record_seen(&id_b32);
        assert!(
            sender_status(&id_b32).seen_before,
            "sender is seen after recording a receipt"
        );

        assert_eq!(contact_list(), vec![("alice".into(), id_b32.clone())]);

        // Trust ledger: default untrusted; mark/unmark round-trips.
        assert!(!sender_status(&id_b32).trusted, "untrusted by default");
        mark_trusted("alice").unwrap();
        assert!(is_trusted(&id_b32), "alice is trusted after marking");
        assert!(sender_status(&id_b32).trusted);
        unmark_trusted("alice").unwrap();
        assert!(!is_trusted(&id_b32), "trust cleared after unmark");
        // Re-trust for the key-change check below.
        mark_trusted("alice").unwrap();

        // Verify + key-change detection: re-adding the same name under a *new* id
        // reports a key change and drops the verified mark.
        mark_verified("alice").unwrap();
        assert!(is_verified(&id_b32), "alice is verified");
        // Same id again → no key change, verified preserved.
        assert!(contact_add("alice", &id_b32).unwrap().is_none());
        assert!(is_verified(&id_b32), "re-adding the same id keeps verified");
        // A different id → key change reported, verified cleared.
        let new_id = Identity::generate().public();
        let new_b32 = data_encoding::BASE32_NOPAD
            .encode(&new_id.to_bytes())
            .to_lowercase();
        let change = contact_add("alice", &new_b32).unwrap();
        assert!(change.is_some(), "key change is reported");
        let change = change.unwrap();
        assert_ne!(change.old_fingerprint, change.new_fingerprint);
        assert!(!is_verified(&id_b32), "old key's verified mark is cleared");
        assert!(!is_verified(&new_b32), "the new key is not auto-verified");
        assert!(
            !is_trusted(&id_b32),
            "old key's trust is cleared on key change"
        );
        assert!(!is_trusted(&new_b32), "the new key is not auto-trusted");

        assert!(contact_remove("alice").unwrap());
        assert!(contact_list().is_empty());

        std::env::remove_var("ARVOLO_CONFIG_DIR");
    }

    fn b32(id: &PublicId) -> String {
        data_encoding::BASE32_NOPAD
            .encode(&id.to_bytes())
            .to_lowercase()
    }

    /// A block has to reach the user's other devices, or silencing someone on the
    /// laptop still lets them through on the desktop — which is not a block, just
    /// a preference. Unblocking has to propagate too, as a tombstone.
    #[test]
    fn sync_merge_propagates_block_and_unblock() {
        let _guard = crate::testlock::ENV
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let dir_a = tempfile::tempdir().unwrap();
        let dir_b = tempfile::tempdir().unwrap();
        let spammer = b32(&Identity::generate().public());

        std::env::set_var("ARVOLO_CONFIG_DIR", dir_a.path());
        mark_blocked(&spammer).unwrap();
        assert!(is_blocked(&spammer));
        let snap = build_local_snapshot().unwrap();

        std::env::set_var("ARVOLO_CONFIG_DIR", dir_b.path());
        apply_merged_state(&snap).unwrap();
        assert!(is_blocked(&spammer), "the block must reach device B");

        // And lifting it propagates as well.
        std::env::set_var("ARVOLO_CONFIG_DIR", dir_a.path());
        assert!(unmark_blocked(&spammer).unwrap());
        let snap = build_local_snapshot().unwrap();
        std::env::set_var("ARVOLO_CONFIG_DIR", dir_b.path());
        apply_merged_state(&snap).unwrap();
        assert!(!is_blocked(&spammer), "the unblock must reach device B too");

        std::env::remove_var("ARVOLO_CONFIG_DIR");
    }

    /// A ledger written before marks carried a timestamp must still read as
    /// *verified* — dropping the mark on upgrade would silently undo a security
    /// decision the user made deliberately. It comes back with no date, which is
    /// honestly different from "verified just now".
    #[test]
    fn a_ledger_without_timestamps_still_reads_as_verified() {
        let _guard = crate::testlock::ENV
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let dir = tempfile::tempdir().unwrap();
        std::env::set_var("ARVOLO_CONFIG_DIR", dir.path());
        let id = b32(&Identity::generate().public());

        // The original shape: a bare list of ids.
        std::fs::write(
            dir.path().join("verified.toml"),
            format!("verified = [\"{id}\"]\n"),
        )
        .unwrap();

        assert!(is_verified(&id), "the mark must survive the format change");
        assert!(
            verified_since(&id).is_none(),
            "but with no date, not a fabricated one"
        );

        // Re-verifying stamps it, and the file is rewritten in the new shape.
        mark_verified(&id).unwrap();
        assert!(verified_since(&id).is_some());
        let text = std::fs::read_to_string(dir.path().join("verified.toml")).unwrap();
        assert!(text.contains(&id) && text.contains('='));

        std::env::remove_var("ARVOLO_CONFIG_DIR");
    }

    /// The date has to reach the user's other devices, or "verified 2 years ago"
    /// would mean something different on each machine.
    #[test]
    fn sync_carries_when_a_mark_was_made() {
        let _guard = crate::testlock::ENV
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let dir_a = tempfile::tempdir().unwrap();
        let dir_b = tempfile::tempdir().unwrap();
        let id = b32(&Identity::generate().public());

        std::env::set_var("ARVOLO_CONFIG_DIR", dir_a.path());
        contact_add("alice", &id).unwrap();
        mark_verified("alice").unwrap();
        let when_a = verified_since(&id).expect("stamped on A");
        let snap = build_local_snapshot().unwrap();

        std::env::set_var("ARVOLO_CONFIG_DIR", dir_b.path());
        apply_merged_state(&snap).unwrap();
        assert_eq!(
            verified_since(&id),
            Some(when_a),
            "B must see the same date A recorded, not the moment it merged"
        );

        std::env::remove_var("ARVOLO_CONFIG_DIR");
    }

    /// Renaming keeps the marks, which is the entire reason it exists: doing it as
    /// remove + add silently drops them.
    #[test]
    fn rename_keeps_verified_and_trusted() {
        let _guard = crate::testlock::ENV
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let dir = tempfile::tempdir().unwrap();
        std::env::set_var("ARVOLO_CONFIG_DIR", dir.path());
        let id = b32(&Identity::generate().public());

        contact_add("alice", &id).unwrap();
        mark_verified("alice").unwrap();
        mark_trusted("alice").unwrap();

        contact_rename("alice", "alessandra").unwrap();
        assert!(contact_list().iter().any(|(n, _)| n == "alessandra"));
        assert!(!contact_list().iter().any(|(n, _)| n == "alice"));
        assert!(
            is_verified(&id) && is_trusted(&id),
            "marks survive a rename"
        );

        // A name already in use is refused rather than silently merging two people.
        contact_add("bob", &b32(&Identity::generate().public())).unwrap();
        assert!(contact_rename("alessandra", "bob").is_err());

        std::env::remove_var("ARVOLO_CONFIG_DIR");
    }

    /// Removing a contact takes its advertised-name row with it (that row would
    /// otherwise sync forever with nobody to own it) but keeps the `seen` counter,
    /// which is TOFU evidence: forgetting it would make a known sender look new.
    #[test]
    fn remove_forgets_the_name_row_but_not_that_we_have_seen_them() {
        let _guard = crate::testlock::ENV
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let dir = tempfile::tempdir().unwrap();
        std::env::set_var("ARVOLO_CONFIG_DIR", dir.path());
        let id = b32(&Identity::generate().public());

        contact_add("alice", &id).unwrap();
        observe_advertised_name(&id, "Alice A.");
        accept_name("alice").unwrap();
        record_seen(&id);
        assert!(display_name_of(&id).is_some());

        assert!(contact_remove("alice").unwrap());
        assert!(
            display_name_of(&id).is_none(),
            "the name row goes with them"
        );
        assert!(
            sender_status(&id).seen_before,
            "but we still remember having received from this key"
        );

        std::env::remove_var("ARVOLO_CONFIG_DIR");
    }

    #[test]
    fn sync_merge_propagates_add_and_key_change() {
        let _guard = crate::testlock::ENV
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let dir_a = tempfile::tempdir().unwrap();
        let dir_b = tempfile::tempdir().unwrap();
        let id1 = b32(&Identity::generate().public());
        let id2 = b32(&Identity::generate().public());

        // Device A: alice=id1, verified + trusted; publish snapshot.
        std::env::set_var("ARVOLO_CONFIG_DIR", dir_a.path());
        contact_add("alice", &id1).unwrap();
        mark_verified("alice").unwrap();
        mark_trusted("alice").unwrap();
        let snap1 = build_local_snapshot().unwrap();

        // Device B: apply → alice=id1 with both marks.
        std::env::set_var("ARVOLO_CONFIG_DIR", dir_b.path());
        apply_merged_state(&snap1).unwrap();
        assert_eq!(
            resolve_recipient("alice").unwrap().to_bytes(),
            decode_id(&id1).unwrap().to_bytes()
        );
        assert!(is_verified(&id1) && is_trusted(&id1));

        // Device A: alice's key changes to id2 (clears id1's marks locally).
        std::env::set_var("ARVOLO_CONFIG_DIR", dir_a.path());
        assert!(contact_add("alice", &id2).unwrap().is_some());
        assert!(!is_verified(&id1));
        let snap2 = build_local_snapshot().unwrap();

        // Device B: apply → alice=id2, id1's marks cleared, id2 not auto-verified.
        std::env::set_var("ARVOLO_CONFIG_DIR", dir_b.path());
        apply_merged_state(&snap2).unwrap();
        assert_eq!(
            resolve_recipient("alice").unwrap().to_bytes(),
            decode_id(&id2).unwrap().to_bytes()
        );
        assert!(!is_verified(&id1), "old key's verified mark cleared on B");
        assert!(!is_trusted(&id1), "old key's trust cleared on B");
        assert!(!is_verified(&id2), "new key is not auto-verified");

        std::env::remove_var("ARVOLO_CONFIG_DIR");
    }

    #[test]
    fn sync_merge_propagates_removal() {
        let _guard = crate::testlock::ENV
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let dir_a = tempfile::tempdir().unwrap();
        let dir_b = tempfile::tempdir().unwrap();
        let id = b32(&Identity::generate().public());

        std::env::set_var("ARVOLO_CONFIG_DIR", dir_a.path());
        contact_add("bob", &id).unwrap();
        let s1 = build_local_snapshot().unwrap();
        std::env::set_var("ARVOLO_CONFIG_DIR", dir_b.path());
        apply_merged_state(&s1).unwrap();
        assert_eq!(contact_list().len(), 1);

        // A removes bob → tombstone propagates → B drops it.
        std::env::set_var("ARVOLO_CONFIG_DIR", dir_a.path());
        assert!(contact_remove("bob").unwrap());
        let s2 = build_local_snapshot().unwrap();
        std::env::set_var("ARVOLO_CONFIG_DIR", dir_b.path());
        apply_merged_state(&s2).unwrap();
        assert!(contact_list().is_empty(), "removal propagated to B");

        std::env::remove_var("ARVOLO_CONFIG_DIR");
    }

    #[test]
    fn advertised_name_tofu_flow() {
        let _guard = crate::testlock::ENV
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let dir = tempfile::tempdir().unwrap();
        std::env::set_var("ARVOLO_CONFIG_DIR", dir.path());
        let id = b32(&Identity::generate().public());

        // Empty advertised name → nothing recorded.
        assert!(matches!(observe_advertised_name(&id, ""), NameStatus::None));
        assert_eq!(display_name_of(&id), None);

        // First real name → New, quarantined as pending (not auto-pinned).
        assert!(matches!(
            observe_advertised_name(&id, "Lorenzo"),
            NameStatus::New(_)
        ));
        assert_eq!(display_name_of(&id), None, "first name is not auto-pinned");
        assert_eq!(pending_name_of(&id).as_deref(), Some("Lorenzo"));

        // Approve by raw id → pinned, pending cleared.
        assert_eq!(accept_name(&id).unwrap(), "Lorenzo");
        assert_eq!(display_name_of(&id).as_deref(), Some("Lorenzo"));
        assert_eq!(pending_name_of(&id), None);

        // Same name again → Unchanged, still no pending.
        assert!(matches!(
            observe_advertised_name(&id, "Lorenzo"),
            NameStatus::Unchanged(_)
        ));
        assert_eq!(pending_name_of(&id), None);

        // A changed name → Changed, old kept pinned until approved.
        match observe_advertised_name(&id, "Lore") {
            NameStatus::Changed { old, new } => {
                assert_eq!(old, "Lorenzo");
                assert_eq!(new, "Lore");
            }
            _ => panic!("expected a Changed status"),
        }
        assert_eq!(
            display_name_of(&id).as_deref(),
            Some("Lorenzo"),
            "pinned name unchanged until approval"
        );
        assert_eq!(pending_name_of(&id).as_deref(), Some("Lore"));

        // Approve the change via a contact alias resolving to the same id.
        contact_add("lorenzo", &id).unwrap();
        assert_eq!(accept_name("lorenzo").unwrap(), "Lore");
        assert_eq!(display_name_of(&id).as_deref(), Some("Lore"));
        assert_eq!(pending_name_of(&id), None);

        // Nothing pending → accept_name errors.
        assert!(accept_name("lorenzo").is_err());

        std::env::remove_var("ARVOLO_CONFIG_DIR");
    }

    #[test]
    fn sync_propagates_approved_name() {
        let _guard = crate::testlock::ENV
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let dir_a = tempfile::tempdir().unwrap();
        let dir_b = tempfile::tempdir().unwrap();
        let id = b32(&Identity::generate().public());

        // Device A: observe + approve a name, publish snapshot.
        std::env::set_var("ARVOLO_CONFIG_DIR", dir_a.path());
        observe_advertised_name(&id, "Lorenzo");
        accept_name(&id).unwrap();
        let snap = build_local_snapshot().unwrap();

        // Device B: applying the snapshot pins the same approved name.
        std::env::set_var("ARVOLO_CONFIG_DIR", dir_b.path());
        apply_merged_state(&snap).unwrap();
        assert_eq!(display_name_of(&id).as_deref(), Some("Lorenzo"));
        assert_eq!(pending_name_of(&id), None);

        std::env::remove_var("ARVOLO_CONFIG_DIR");
    }

    #[test]
    fn display_name_config_roundtrip() {
        let _guard = crate::testlock::ENV
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let dir = tempfile::tempdir().unwrap();
        std::env::set_var("ARVOLO_CONFIG_DIR", dir.path());

        assert_eq!(my_display_name(), "", "unset by default");
        set_my_display_name("Lorenzo").unwrap();
        assert_eq!(my_display_name(), "Lorenzo");
        // Setting another key-free config value keeps the name (preserves the file).
        set_my_display_name("  Lore  ").unwrap();
        assert_eq!(my_display_name(), "Lore", "trimmed on set");
        set_my_display_name("").unwrap();
        assert_eq!(my_display_name(), "", "cleared");

        std::env::remove_var("ARVOLO_CONFIG_DIR");
    }

    #[test]
    fn observe_advertised_name_edge_cases() {
        let _guard = crate::testlock::ENV
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let dir = tempfile::tempdir().unwrap();
        std::env::set_var("ARVOLO_CONFIG_DIR", dir.path());
        let id = b32(&Identity::generate().public());

        // Whitespace-only advertised name is treated as "none" (trimmed away).
        assert!(matches!(
            observe_advertised_name(&id, "   "),
            NameStatus::None
        ));
        assert_eq!(pending_name_of(&id), None);

        // Two different names arrive before any approval: pending tracks the LATEST.
        assert!(matches!(
            observe_advertised_name(&id, "First"),
            NameStatus::New(_)
        ));
        assert!(matches!(
            observe_advertised_name(&id, "Second"),
            NameStatus::New(_)
        ));
        assert_eq!(
            pending_name_of(&id).as_deref(),
            Some("Second"),
            "pending follows the most recent advertised name"
        );

        // Approve, then the sender advertises a change, then reverts to the pinned
        // name → the pending change is cleared (nothing to approve anymore).
        accept_name(&id).unwrap();
        assert_eq!(display_name_of(&id).as_deref(), Some("Second"));
        assert!(matches!(
            observe_advertised_name(&id, "Third"),
            NameStatus::Changed { .. }
        ));
        assert_eq!(pending_name_of(&id).as_deref(), Some("Third"));
        assert!(matches!(
            observe_advertised_name(&id, "Second"),
            NameStatus::Unchanged(_)
        ));
        assert_eq!(
            pending_name_of(&id),
            None,
            "reverting to the pinned name clears the pending change"
        );

        // A now-empty advertised name never disturbs the pinned name.
        assert!(matches!(observe_advertised_name(&id, ""), NameStatus::None));
        assert_eq!(display_name_of(&id).as_deref(), Some("Second"));

        std::env::remove_var("ARVOLO_CONFIG_DIR");
    }

    #[test]
    fn accept_name_variants() {
        let _guard = crate::testlock::ENV
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let dir = tempfile::tempdir().unwrap();
        std::env::set_var("ARVOLO_CONFIG_DIR", dir.path());
        let id1 = b32(&Identity::generate().public());
        let id2 = b32(&Identity::generate().public());

        // Unknown alias / invalid id → a clear error (not a silent no-op).
        assert!(accept_name("nobody").is_err());
        assert!(accept_name("not-valid-base32!!").is_err());

        // Two pending names → accept_all approves both at once.
        observe_advertised_name(&id1, "Alice");
        observe_advertised_name(&id2, "Bob");
        assert_eq!(accept_all_names().unwrap(), 2);
        assert_eq!(display_name_of(&id1).as_deref(), Some("Alice"));
        assert_eq!(display_name_of(&id2).as_deref(), Some("Bob"));
        // Nothing left pending → accept_all is a no-op returning 0.
        assert_eq!(accept_all_names().unwrap(), 0);

        std::env::remove_var("ARVOLO_CONFIG_DIR");
    }

    #[test]
    fn contact_key_change_does_not_leak_advertised_name() {
        // A contact's advertised name is keyed by identity, so re-pointing a contact
        // alias at a NEW id must not carry the old id's name onto the new key.
        let _guard = crate::testlock::ENV
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let dir = tempfile::tempdir().unwrap();
        std::env::set_var("ARVOLO_CONFIG_DIR", dir.path());
        let old_id = b32(&Identity::generate().public());
        let new_id = b32(&Identity::generate().public());

        contact_add("boss", &old_id).unwrap();
        observe_advertised_name(&old_id, "Lorenzo");
        accept_name("boss").unwrap();
        assert_eq!(display_name_of(&old_id).as_deref(), Some("Lorenzo"));

        // The contact's key changes (a new identity under the same alias).
        assert!(contact_add("boss", &new_id).unwrap().is_some());
        assert_eq!(
            display_name_of(&new_id),
            None,
            "the new key has no advertised name of its own"
        );
        // The old id still carries its own record — it's a different identity.
        assert_eq!(display_name_of(&old_id).as_deref(), Some("Lorenzo"));

        std::env::remove_var("ARVOLO_CONFIG_DIR");
    }

    #[test]
    fn build_local_snapshot_is_stable_for_names() {
        // Re-publishing without any change must not keep bumping the Lamport clock
        // (which would make every sync look like a fresh edit and never converge).
        let _guard = crate::testlock::ENV
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let dir = tempfile::tempdir().unwrap();
        std::env::set_var("ARVOLO_CONFIG_DIR", dir.path());
        let id = b32(&Identity::generate().public());

        observe_advertised_name(&id, "Lorenzo");
        accept_name(&id).unwrap();
        let s1 = build_local_snapshot().unwrap();
        let s2 = build_local_snapshot().unwrap();
        let n1 = s1.names.iter().find(|n| n.pubkey == id).unwrap();
        let n2 = s2.names.iter().find(|n| n.pubkey == id).unwrap();
        assert_eq!(
            n1.clock, n2.clock,
            "an unchanged name keeps its clock across snapshots"
        );

        std::env::remove_var("ARVOLO_CONFIG_DIR");
    }

    #[test]
    fn sync_propagates_pending_and_tombstone() {
        let _guard = crate::testlock::ENV
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let dir_a = tempfile::tempdir().unwrap();
        let dir_b = tempfile::tempdir().unwrap();
        let id = b32(&Identity::generate().public());

        // A observes a name (pending, not yet approved) and publishes.
        std::env::set_var("ARVOLO_CONFIG_DIR", dir_a.path());
        observe_advertised_name(&id, "Lorenzo");
        let s1 = build_local_snapshot().unwrap();

        // B sees the SAME pending name → a change is surfaced on every device.
        std::env::set_var("ARVOLO_CONFIG_DIR", dir_b.path());
        apply_merged_state(&s1).unwrap();
        assert_eq!(pending_name_of(&id).as_deref(), Some("Lorenzo"));
        assert_eq!(display_name_of(&id), None, "pending is not yet pinned on B");

        // A approves; B converges to the pinned name with no pending.
        std::env::set_var("ARVOLO_CONFIG_DIR", dir_a.path());
        accept_name(&id).unwrap();
        let s2 = build_local_snapshot().unwrap();
        std::env::set_var("ARVOLO_CONFIG_DIR", dir_b.path());
        apply_merged_state(&s2).unwrap();
        assert_eq!(display_name_of(&id).as_deref(), Some("Lorenzo"));
        assert_eq!(pending_name_of(&id), None);

        std::env::remove_var("ARVOLO_CONFIG_DIR");
    }

    #[test]
    fn display_name_config_escapes_special_chars() {
        // A name with quotes / unicode / a leading '#' must round-trip through the
        // config file intact (TOML-escaped, not truncated or misparsed) and must not
        // corrupt the file into an unreadable state.
        let _guard = crate::testlock::ENV
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let dir = tempfile::tempdir().unwrap();
        std::env::set_var("ARVOLO_CONFIG_DIR", dir.path());

        for name in [
            "O'Brien \"the Boss\"",
            "名前",
            "# not a comment",
            "a = b",
            "line\ttab",
        ] {
            set_my_display_name(name).unwrap();
            assert_eq!(my_display_name(), name, "round-trips: {name:?}");
        }

        std::env::remove_var("ARVOLO_CONFIG_DIR");
    }

    #[test]
    fn set_display_name_replaces_not_duplicates() {
        // Repeated sets must edit the single line in place, never accumulate.
        let _guard = crate::testlock::ENV
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let dir = tempfile::tempdir().unwrap();
        std::env::set_var("ARVOLO_CONFIG_DIR", dir.path());

        set_my_display_name("One").unwrap();
        set_my_display_name("Two").unwrap();
        set_my_display_name("Three").unwrap();
        let text = std::fs::read_to_string(config_path()).unwrap();
        let occurrences = text
            .lines()
            .filter(|l| {
                let t = l.trim_start().trim_start_matches('#').trim_start();
                t.starts_with("display_name")
            })
            .count();
        assert_eq!(occurrences, 1, "exactly one display_name line: {text:?}");
        assert_eq!(my_display_name(), "Three");

        std::env::remove_var("ARVOLO_CONFIG_DIR");
    }
}
