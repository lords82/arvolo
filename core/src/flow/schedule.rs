//! Pure scheduling decisions for the swarm receiver, factored out of
//! [`super::recv::recv_chunked`] so they can be unit-tested without a network.
//!
//! Three decisions live here:
//! - [`providers_for_piece`] — *who* can serve a given piece right now. This is the
//!   **fallback core**: the origin sender holds every piece while it is live, so a
//!   piece a departed peer used to serve still resolves to the sender. When the
//!   sender is offline only the relay/peers that hold it remain; if none do, the
//!   result is empty and the caller backs the piece off until a source returns.
//! - [`rarest_set`] — *which* piece to fetch next (the rarest — fewest providers),
//!   the deterministic half of rarest-first; the caller random-tie-breaks.
//! - [`choose_provider`] — *from which* of a piece's providers to fetch: prefer a
//!   peer/relay over the origin (offload the source) and, within a tier, the one
//!   with the lowest estimated time-to-serve (faster + less loaded first).
//!
//! All are generic over the provider address type `A` so tests can use plain
//! strings while `recv_chunked` uses `iroh::EndpointAddr`.

use std::collections::{HashMap, HashSet};
use std::sync::Mutex;
use std::time::Instant;

/// The providers that can serve `piece` right now, in preference order
/// (sender, then relay, then peers): each source that *holds* the piece and is
/// neither banned nor cooling down, decided by `is_eligible`.
///
/// - `sender` is `Some` **iff the origin is live**; when present it is a provider
///   for *every* piece (it holds the whole file). This is the fallback that makes
///   a piece resolve back to the origin when the peer that held it leaves.
/// - `relay` is `Some` when a relay is configured; it serves only the pieces in its
///   `on_relay` set (the ones it has backfilled).
/// - `peers` each serve the pieces marked in their bitfield.
///
/// Returns `(id, addr)` pairs. An empty result means no source can serve the piece
/// at the moment — the caller then waits/backs off rather than busy-looping.
pub(crate) fn providers_for_piece<A: Clone>(
    piece: usize,
    sender: Option<&(String, A)>,
    relay: Option<(&(String, A), &HashSet<u32>)>,
    peers: &[(String, A, Vec<u8>)],
    is_eligible: impl Fn(&str) -> bool,
) -> Vec<(String, A)> {
    let mut out: Vec<(String, A)> = Vec::new();
    // The origin sender: present here only while live, and holds every piece.
    if let Some((id, addr)) = sender {
        if is_eligible(id) {
            out.push((id.clone(), addr.clone()));
        }
    }
    // The relay: a provider only for the pieces it has backfilled.
    if let Some(((id, addr), on_relay)) = relay {
        if on_relay.contains(&(piece as u32)) && is_eligible(id) {
            out.push((id.clone(), addr.clone()));
        }
    }
    // Peers: each serves exactly the pieces its bitfield advertises.
    for (id, addr, bitfield) in peers {
        if crate::swarm::bitfield_has(bitfield, piece) && is_eligible(id) {
            out.push((id.clone(), addr.clone()));
        }
    }
    out
}

/// A convenience wrapper over [`providers_for_piece`]'s `is_eligible` argument: a
/// provider is eligible when it is not `banned` and any per-provider `cooldown` has
/// elapsed by `now`. (Kept separate so the eligibility rule is testable on its own.)
pub(crate) fn is_eligible(
    id: &str,
    banned: &HashSet<String>,
    cooldown: &HashMap<String, Instant>,
    now: Instant,
) -> bool {
    !banned.contains(id) && cooldown.get(id).map(|t| *t <= now).unwrap_or(true)
}

/// The subset of `candidates` tied for **rarest** — the fewest providers. This is
/// the deterministic core of rarest-first selection; the caller picks randomly
/// among the returned set. An empty input yields an empty output.
///
/// Because the origin holds every piece, a piece only the origin has (1 provider)
/// is rarer than one the origin *and* a peer/relay have (2+), so it is drained
/// first — automatically prioritising the scarce source before it can drop. Each
/// candidate is `(piece, providers)`; callers pass only candidates that already
/// have ≥1 provider.
pub(crate) fn rarest_set<A>(mut candidates: Vec<(usize, Vec<A>)>) -> Vec<(usize, Vec<A>)> {
    match candidates.iter().map(|(_, p)| p.len()).min() {
        Some(min) => {
            candidates.retain(|(_, p)| p.len() == min);
            candidates
        }
        None => candidates,
    }
}

/// How strongly to prefer a peer/relay over the origin sender, so downloads offload
/// the source: the origin's estimated cost is multiplied by this. A peer wins unless
/// the origin is this many times cheaper (far faster and/or far less loaded) — so C
/// pulls a piece B already holds from B, not A, yet a hopelessly slow/overloaded
/// peer still yields back to the origin.
pub(crate) const ORIGIN_OFFLOAD_FACTOR: f64 = 4.0;

/// EWMA smoothing for per-provider throughput samples (higher = more reactive).
const RATE_EWMA_ALPHA: f64 = 0.3;

/// One provider of a specific piece, with the live signals used to rank it.
pub(crate) struct Candidate<A> {
    pub id: String,
    pub addr: A,
    /// The origin sender holds every piece, but we prefer to spare it (offload).
    pub is_origin: bool,
    /// Requests already outstanding to this provider (load).
    pub in_flight: usize,
    /// Measured throughput (bytes/sec, EWMA), or `None` until we've timed a fetch
    /// from it — an unmeasured provider is treated as fast as the fastest known one
    /// so it still gets tried rather than starved by a known-fast incumbent.
    pub rate_bps: Option<f64>,
}

/// Estimated time-to-serve one more piece: queue depth over throughput. Lower is
/// better, so a faster provider absorbs more load before a slower one is preferred.
/// The origin is inflated by [`ORIGIN_OFFLOAD_FACTOR`] to spare the source.
fn cost<A>(c: &Candidate<A>, prior_rate: f64) -> f64 {
    let rate = c.rate_bps.filter(|r| *r > 0.0).unwrap_or(prior_rate).max(1.0);
    let base = (c.in_flight as f64 + 1.0) / rate;
    if c.is_origin {
        base * ORIGIN_OFFLOAD_FACTOR
    } else {
        base
    }
}

/// Pick which provider of a piece to fetch from. Prefers peers/relay over the origin
/// (offload) and, within reach of that preference, the lowest estimated
/// time-to-serve — so a faster, less-loaded source wins and a slow/busy one is
/// spared. A much faster and idle origin can still beat a slow, overloaded peer, so
/// the preference bends to real speed rather than blindly always avoiding the
/// origin. Deterministic tie-break by id. `None` only for an empty candidate list.
pub(crate) fn choose_provider<A>(candidates: &[Candidate<A>]) -> Option<&Candidate<A>> {
    // Optimistic prior for unmeasured providers: the fastest rate we've measured
    // (or a neutral 1.0 if none yet), so a new provider is explored, not starved.
    let best_known = candidates
        .iter()
        .filter_map(|c| c.rate_bps)
        .filter(|r| *r > 0.0)
        .fold(f64::NAN, f64::max);
    let prior = if best_known.is_nan() { 1.0 } else { best_known };
    candidates
        .iter()
        .min_by(|a, b| cost(a, prior).total_cmp(&cost(b, prior)).then(a.id.cmp(&b.id)))
}

/// Fold a fresh throughput sample (bytes/sec) into a provider's EWMA estimate, used
/// by [`choose_provider`] to prefer faster sources. The first sample seeds the
/// estimate; later ones smooth it.
pub(crate) fn update_rate(rates: &Mutex<HashMap<String, f64>>, id: &str, sample_bps: f64) {
    if !(sample_bps > 0.0) {
        return;
    }
    let mut r = rates.lock().unwrap();
    let e = r.entry(id.to_string()).or_insert(sample_bps);
    *e = RATE_EWMA_ALPHA * sample_bps + (1.0 - RATE_EWMA_ALPHA) * *e;
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    /// A peer entry `(id, addr, bitfield)` with `present` pieces marked. Address is
    /// a `&'static str` here — the scheduler is generic over the address type.
    fn peer(id: &str, n_chunks: usize, present: &[usize]) -> (String, &'static str, Vec<u8>) {
        let mut bf = crate::swarm::bitfield_new(n_chunks);
        for &i in present {
            crate::swarm::bitfield_set(&mut bf, i);
        }
        // Leak a small label so the address type is `&'static str` in tests.
        let addr: &'static str = Box::leak(format!("addr-{id}").into_boxed_str());
        (id.to_string(), addr, bf)
    }

    fn sender() -> (String, &'static str) {
        ("origin".to_string(), "addr-origin")
    }

    fn relay() -> (String, &'static str) {
        ("relay".to_string(), "addr-relay")
    }

    fn ids(v: &[(String, &'static str)]) -> Vec<String> {
        v.iter().map(|(id, _)| id.clone()).collect()
    }

    fn always_ok(_: &str) -> bool {
        true
    }

    // ---- providers_for_piece: the fallback core --------------------------------

    /// The headline guarantee behind the user's question: while the origin (A) is
    /// live it is a provider for *every* piece, with no peers or relay at all. So C
    /// can always fetch any chunk straight from A — a peer having downloaded it
    /// first neither adds a precondition nor removes A from the list.
    #[test]
    fn the_live_origin_is_a_provider_for_every_piece() {
        let s = sender();
        for piece in [0usize, 1, 5, 63, 1000] {
            let got = providers_for_piece(piece, Some(&s), None, &[], always_ok);
            assert_eq!(ids(&got), vec!["origin"], "origin serves piece {piece}");
        }
    }

    /// A present peer (B) that holds the piece is offered *alongside* the origin, so
    /// C can pull it from B (load-balanced) — but B is never listed for a piece it
    /// does not hold. This is "if B stays, does C try B? yes".
    #[test]
    fn a_present_peer_holding_the_piece_is_offered_next_to_the_origin() {
        let s = sender();
        let b = peer("B", 4, &[2]); // B only has piece 2
        let peers = [b];

        let for_2 = providers_for_piece(2, Some(&s), None, &peers, always_ok);
        assert_eq!(ids(&for_2), vec!["origin", "B"], "both A and B can serve piece 2");

        let for_0 = providers_for_piece(0, Some(&s), None, &peers, always_ok);
        assert_eq!(ids(&for_0), vec!["origin"], "B lacks piece 0, only A serves it");
    }

    /// B downloads a chunk then disconnects: the tracker drops it, so it is simply
    /// absent from `peers`. The piece it held still resolves to the origin — the
    /// fallback. Contrast the two calls: same piece, B present vs. B gone.
    #[test]
    fn a_departed_peer_falls_back_to_the_origin() {
        let s = sender();
        let with_b = [peer("B", 4, &[2])];
        let before = providers_for_piece(2, Some(&s), None, &with_b, always_ok);
        assert_eq!(ids(&before), vec!["origin", "B"]);

        // B is gone from the peer list; the origin is still a provider for piece 2.
        let after = providers_for_piece(2, Some(&s), None, &[], always_ok);
        assert_eq!(ids(&after), vec!["origin"], "piece 2 falls back to the origin");
    }

    /// When the origin is offline, the piece falls back to the relay — but only for
    /// pieces the relay has actually backfilled.
    #[test]
    fn an_offline_origin_falls_back_to_the_relay_for_backfilled_pieces() {
        let r = relay();
        let on_relay: HashSet<u32> = [1u32, 3].into_iter().collect();

        let has = providers_for_piece(1, None, Some((&r, &on_relay)), &[], always_ok);
        assert_eq!(ids(&has), vec!["relay"], "relay has piece 1");

        let lacks = providers_for_piece(2, None, Some((&r, &on_relay)), &[], always_ok);
        assert!(lacks.is_empty(), "relay never backfilled piece 2 → no provider");
    }

    /// With the origin gone and nothing on the relay, a peer that holds the piece is
    /// still a valid source — the swarm keeps flowing peer-to-peer.
    #[test]
    fn an_offline_origin_can_still_fall_back_to_a_peer() {
        let peers = [peer("B", 4, &[3])];
        let got = providers_for_piece(3, None, None, &peers, always_ok);
        assert_eq!(ids(&got), vec!["B"]);
    }

    /// Every source gone (origin offline, nothing on the relay, no peer holds it) →
    /// no provider. The caller must then wait/back off, not busy-loop — precisely the
    /// "missing everywhere" case in SWARM-DESIGN §9.
    #[test]
    fn a_piece_no_one_holds_has_no_provider() {
        let r = relay();
        let on_relay: HashSet<u32> = HashSet::new();
        let peers = [peer("B", 4, &[0])]; // B has piece 0, not piece 2
        let got = providers_for_piece(2, None, Some((&r, &on_relay)), &peers, always_ok);
        assert!(got.is_empty(), "no source for piece 2 → empty (caller waits)");
    }

    /// A peer that served corrupt bytes is banned and excluded even though it still
    /// advertises the piece; the origin remains a provider, so the fetch is retried
    /// from a trustworthy source.
    #[test]
    fn a_banned_peer_is_excluded_but_the_origin_remains() {
        let s = sender();
        let peers = [peer("B", 4, &[2])];
        let banned: HashSet<String> = ["B".to_string()].into_iter().collect();
        let cooldown = HashMap::new();
        let now = Instant::now();

        let got = providers_for_piece(2, Some(&s), None, &peers, |id| {
            is_eligible(id, &banned, &cooldown, now)
        });
        assert_eq!(ids(&got), vec!["origin"], "banned B dropped, origin kept");
    }

    /// A provider that just failed a fetch is cooled down and skipped until its
    /// cooldown elapses — then it is eligible again.
    #[test]
    fn a_cooling_down_provider_is_skipped_until_its_cooldown_elapses() {
        let s = sender();
        let banned = HashSet::new();
        let now = Instant::now();
        // Origin cools down for 3s.
        let cooldown: HashMap<String, Instant> =
            [("origin".to_string(), now + Duration::from_secs(3))]
                .into_iter()
                .collect();

        let during = providers_for_piece(0, Some(&s), None, &[], |id| {
            is_eligible(id, &banned, &cooldown, now)
        });
        assert!(during.is_empty(), "origin is cooling down right now");

        let after = providers_for_piece(0, Some(&s), None, &[], |id| {
            is_eligible(id, &banned, &cooldown, now + Duration::from_secs(4))
        });
        assert_eq!(ids(&after), vec!["origin"], "cooldown elapsed → eligible again");
    }

    /// Provider order is deterministic: origin first, then relay, then peers — the
    /// order the caller's load-balancer expects.
    #[test]
    fn providers_are_ordered_origin_then_relay_then_peers() {
        let s = sender();
        let r = relay();
        let on_relay: HashSet<u32> = [2u32].into_iter().collect();
        let peers = [peer("B", 4, &[2]), peer("C", 4, &[2])];
        let got = providers_for_piece(2, Some(&s), Some((&r, &on_relay)), &peers, always_ok);
        assert_eq!(ids(&got), vec!["origin", "relay", "B", "C"]);
    }

    // ---- rarest_set: rarest-first selection ------------------------------------

    /// The scarce piece is picked first: with the origin holding everything, a piece
    /// only the origin has (1 provider) is rarer than one the origin *and* a peer
    /// both have (2), so it is drained before the abundant one.
    #[test]
    fn rarest_prefers_the_piece_with_the_fewest_providers() {
        // piece 0: origin only (1). piece 1: origin + peer (2). piece 2: origin only (1).
        let cands = vec![
            (0usize, vec!["origin"]),
            (1, vec!["origin", "B"]),
            (2, vec!["origin"]),
        ];
        let rare = rarest_set(cands);
        let pieces: HashSet<usize> = rare.iter().map(|(i, _)| *i).collect();
        assert_eq!(
            pieces,
            [0, 2].into_iter().collect::<HashSet<_>>(),
            "the two 1-provider pieces are the rarest; the 2-provider piece is not"
        );
    }

    /// All pieces tied for rarest are returned (the caller then random-tie-breaks so
    /// two identical peers don't grab the same piece).
    #[test]
    fn rarest_returns_every_tie() {
        let cands = vec![
            (0usize, vec!["a", "b"]),
            (1, vec!["c", "d"]),
            (2, vec!["e", "f"]),
        ];
        assert_eq!(rarest_set(cands).len(), 3, "all three are equally rare");
    }

    /// A single clear winner is returned alone.
    #[test]
    fn rarest_returns_the_sole_scarcest() {
        let cands = vec![(0usize, vec!["a", "b", "c"]), (1, vec!["a"])];
        let rare = rarest_set(cands);
        assert_eq!(rare.len(), 1);
        assert_eq!(rare[0].0, 1);
    }

    /// No candidates → nothing to pick (the caller then waits).
    #[test]
    fn rarest_of_nothing_is_nothing() {
        let cands: Vec<(usize, Vec<&str>)> = Vec::new();
        assert!(rarest_set(cands).is_empty());
    }

    // ---- choose_provider: offload the origin, weigh speed & load ---------------

    fn cand(id: &str, is_origin: bool, in_flight: usize, rate_bps: Option<f64>) -> Candidate<()> {
        Candidate {
            id: id.to_string(),
            addr: (),
            is_origin,
            in_flight,
            rate_bps,
        }
    }

    /// The headline of the request: when a peer (B) holds the piece, fetch it from B,
    /// not the origin (A) — even at equal load and with no speed data yet — so A is
    /// offloaded. A is used only as the fallback.
    #[test]
    fn a_peer_is_preferred_over_the_origin_to_offload_the_source() {
        let cands = vec![cand("A", true, 0, None), cand("B", false, 0, None)];
        assert_eq!(choose_provider(&cands).unwrap().id, "B");
    }

    /// But the origin is still chosen when it is the only source — the fallback that
    /// makes a departed-peer piece resolve back to A.
    #[test]
    fn the_origin_is_used_when_it_is_the_only_provider() {
        let cands = vec![cand("A", true, 3, None)];
        assert_eq!(choose_provider(&cands).unwrap().id, "A");
    }

    /// Between two peers, the faster one (higher measured throughput) is preferred at
    /// equal load — "verify which is more convenient by speed".
    #[test]
    fn the_faster_peer_is_preferred_at_equal_load() {
        let cands = vec![
            cand("slow", false, 0, Some(1_000_000.0)),
            cand("fast", false, 0, Some(8_000_000.0)),
        ];
        assert_eq!(choose_provider(&cands).unwrap().id, "fast");
    }

    /// At equal speed, load balances: the peer with fewer requests in flight wins, so
    /// one source isn't hammered while another sits idle.
    #[test]
    fn load_is_spread_when_speeds_are_equal() {
        let cands = vec![
            cand("busy", false, 4, Some(5_000_000.0)),
            cand("idle", false, 0, Some(5_000_000.0)),
        ];
        assert_eq!(choose_provider(&cands).unwrap().id, "idle");
    }

    /// The preference bends to reality: a dramatically faster, idle origin still beats
    /// a slow, overloaded peer. So the scheduler offloads the origin *by default* but
    /// won't cripple the download to do it.
    #[test]
    fn a_much_faster_idle_origin_can_still_win() {
        let cands = vec![
            cand("A", true, 0, Some(50_000_000.0)), // origin: idle, very fast
            cand("B", false, 6, Some(200_000.0)),   // peer: overloaded, very slow
        ];
        // A cost = (0+1)/50e6 * 4 = 8e-8 ; B cost = (6+1)/2e5 = 3.5e-5 → A wins.
        assert_eq!(choose_provider(&cands).unwrap().id, "A");
    }

    /// An unmeasured provider is explored, not starved: it inherits the fastest known
    /// rate, so it ties the best peer (and the deterministic id tie-break picks one) —
    /// it isn't buried behind a provider we happen to have timed already.
    #[test]
    fn an_unmeasured_peer_is_explored_optimistically() {
        let cands = vec![
            cand("known", false, 0, Some(4_000_000.0)),
            cand("fresh", false, 0, None),
        ];
        // Equal cost (fresh inherits known's rate) → id tie-break → "fresh".
        assert_eq!(choose_provider(&cands).unwrap().id, "fresh");
    }

    /// No providers → nothing to choose.
    #[test]
    fn choose_of_nothing_is_none() {
        let cands: Vec<Candidate<()>> = Vec::new();
        assert!(choose_provider(&cands).is_none());
    }

    // ---- update_rate: throughput EWMA ------------------------------------------

    /// The first sample seeds the estimate; a later sample smooths toward it (EWMA),
    /// so a provider's rate tracks its recent throughput without lurching.
    #[test]
    fn update_rate_seeds_then_smooths() {
        let rates = Mutex::new(HashMap::new());
        update_rate(&rates, "P", 1_000_000.0);
        assert_eq!(*rates.lock().unwrap().get("P").unwrap(), 1_000_000.0);

        update_rate(&rates, "P", 2_000_000.0);
        let v = *rates.lock().unwrap().get("P").unwrap();
        // 0.3*2e6 + 0.7*1e6 = 1.3e6 — between the two, nearer the older value.
        assert!((v - 1_300_000.0).abs() < 1.0, "EWMA blends samples, got {v}");
    }

    /// Non-positive samples (a zero-time or empty read) are ignored, never poisoning
    /// the estimate with a zero/negative rate.
    #[test]
    fn update_rate_ignores_nonpositive_samples() {
        let rates = Mutex::new(HashMap::new());
        update_rate(&rates, "P", 0.0);
        update_rate(&rates, "P", -5.0);
        assert!(rates.lock().unwrap().get("P").is_none());
    }
}
