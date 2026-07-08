//! Per-IP rate limiting for the rendezvous and unauthenticated write routes.

use std::collections::HashMap;
use std::sync::Mutex;

use axum::http::StatusCode;

use crate::state::env_usize;

// ---- rendezvous rate limiting ----------------------------------------------
//
// The rendezvous nameplate space is deliberately tiny (a 4-digit slot the user
// types), so an attacker can cheaply sweep every slot to find in-flight pairings
// and then grief them (claim `mr` first, or burn the ticket) — each hijack also
// buys one online SPAKE2 guess at the two code words. The PAKE already caps the
// damage at one guess per exchange with visible failure; per-IP rate limits on
// the rendezvous routes make the sweep itself slow and noisy.

/// Default per-IP cap on rendezvous **POSTs** per minute. A legitimate pairing
/// writes at most three values (`ms`, `mr`, `tkt`); the default leaves headroom
/// for several concurrent pairings behind one NAT. `0` disables the limit.
/// Override with `ARVOLO_RZ_POSTS_PER_MIN`.
pub const DEFAULT_RZ_POSTS_PER_MIN: u32 = 30;
/// Default per-IP cap on **distinct rendezvous slots** touched by GETs per
/// minute. A legitimate peer polls one slot (however frequently — polling is
/// never throttled); sweeping the 10k-nameplate space needs thousands. `0`
/// disables the limit. Override with `ARVOLO_RZ_SLOTS_PER_MIN`.
pub const DEFAULT_RZ_SLOTS_PER_MIN: u32 = 60;
/// Bound on tracked client IPs (memory guard for the limiter map).
const MAX_RZ_LIMITER_IPS: usize = 100_000;
/// The per-IP accounting window.
const RZ_LIMITER_WINDOW_SECS: u64 = 60;

pub(crate) fn rz_posts_per_min() -> u32 {
    env_usize("ARVOLO_RZ_POSTS_PER_MIN", DEFAULT_RZ_POSTS_PER_MIN as usize) as u32
}

pub(crate) fn rz_slots_per_min() -> u32 {
    env_usize("ARVOLO_RZ_SLOTS_PER_MIN", DEFAULT_RZ_SLOTS_PER_MIN as usize) as u32
}

/// Whether to trust `X-Forwarded-For` for the client IP (set `ARVOLO_TRUST_PROXY`
/// when the relay sits behind a reverse proxy such as nginx — and only then:
/// trusting it on a directly exposed relay lets clients spoof their IP).
fn trust_proxy() -> bool {
    matches!(
        std::env::var("ARVOLO_TRUST_PROXY")
            .unwrap_or_default()
            .trim()
            .to_ascii_lowercase()
            .as_str(),
        "1" | "true" | "yes" | "on"
    )
}

/// One IP's rendezvous activity inside the current window. Public only because
/// it appears in [`AppState`]'s limiter type; fields stay private.
#[derive(Default)]
pub struct RzIpWindow {
    window_start: u64,
    posts: u32,
    slots: std::collections::HashSet<String>,
}

/// Per-IP rendezvous limiter state (keyed by client IP).
pub type RzLimiter = Mutex<HashMap<std::net::IpAddr, RzIpWindow>>;

/// The client IP for rate-limiting: `X-Forwarded-For` when the administrator
/// opted in via `ARVOLO_TRUST_PROXY`, else the socket peer address. `None` when
/// neither is available (e.g. unit tests driving the router without connect
/// info), in which case the limiter is bypassed.
pub(crate) struct ClientIp(pub(crate) Option<std::net::IpAddr>);

impl<S: Send + Sync> axum::extract::FromRequestParts<S> for ClientIp {
    type Rejection = std::convert::Infallible;

    async fn from_request_parts(
        parts: &mut axum::http::request::Parts,
        _state: &S,
    ) -> Result<Self, Self::Rejection> {
        if trust_proxy() {
            if let Some(ip) = parts
                .headers
                .get("x-forwarded-for")
                .and_then(|v| v.to_str().ok())
                .and_then(|v| v.split(',').next())
                .and_then(|v| v.trim().parse().ok())
            {
                return Ok(ClientIp(Some(ip)));
            }
        }
        Ok(ClientIp(
            parts
                .extensions
                .get::<axum::extract::ConnectInfo<std::net::SocketAddr>>()
                .map(|c| c.0.ip()),
        ))
    }
}

/// What a rendezvous request is asking the limiter to account for.
pub(crate) enum RzAction<'a> {
    /// One write (`POST /v1/rz/…`).
    Post,
    /// One read touching this slot (`GET /v1/rz/{slot}/…`).
    GetSlot(&'a str),
}

/// Enforce the per-IP rendezvous limits. `Ok(())` admits the request; `Err` is
/// the 429 to return. An unknown client IP (no connect info, no trusted XFF)
/// bypasses the limiter — `main` serves with connect info, so that only happens
/// in tests.
pub(crate) fn rz_rate_limit(
    limiter: &RzLimiter,
    ip: Option<std::net::IpAddr>,
    action: RzAction<'_>,
    now: u64,
    post_cap: u32,
    slot_cap: u32,
) -> Result<(), (StatusCode, String)> {
    let Some(ip) = ip else { return Ok(()) };
    let mut map = limiter.lock().unwrap();
    // Memory guard: on pressure, drop windows that have already elapsed; if the
    // map is still saturated (an active flood from many IPs), fail open rather
    // than letting the limiter itself become the DoS.
    if map.len() >= MAX_RZ_LIMITER_IPS && !map.contains_key(&ip) {
        map.retain(|_, w| now < w.window_start + RZ_LIMITER_WINDOW_SECS);
        if map.len() >= MAX_RZ_LIMITER_IPS {
            return Ok(());
        }
    }
    let w = map.entry(ip).or_default();
    if now >= w.window_start + RZ_LIMITER_WINDOW_SECS {
        w.window_start = now;
        w.posts = 0;
        w.slots.clear();
    }
    match action {
        RzAction::Post => {
            if post_cap > 0 && w.posts >= post_cap {
                return Err((
                    StatusCode::TOO_MANY_REQUESTS,
                    "rendezvous rate limit exceeded".into(),
                ));
            }
            w.posts += 1;
        }
        RzAction::GetSlot(slot) => {
            if !w.slots.contains(slot) {
                if slot_cap > 0 && w.slots.len() as u32 >= slot_cap {
                    return Err((
                        StatusCode::TOO_MANY_REQUESTS,
                        "rendezvous rate limit exceeded".into(),
                    ));
                }
                w.slots.insert(slot.to_string());
            }
        }
    }
    Ok(())
}

// ---- write-route rate limiting ---------------------------------------------
//
// The unauthenticated write routes (deposit, seed, inbox-post, swarm-announce,
// presence) are each individually bounded (size caps, per-slot caps, row caps),
// but nothing stopped one source from churning them as fast as it liked —
// poisoning peer lists, replaying offers, and racing toward the disk-fill caps.
// A single per-IP request budget across all of them makes that abuse slow and
// noisy without affecting a normal client (which writes a handful of times).

/// Default per-IP cap on writes per minute across all unauthenticated write
/// routes. Generous enough for concurrent transfers behind one NAT; `0` disables
/// the limit. Override with `ARVOLO_WRITES_PER_MIN`.
pub const DEFAULT_WRITES_PER_MIN: u32 = 240;

pub(crate) fn writes_per_min() -> u32 {
    env_usize("ARVOLO_WRITES_PER_MIN", DEFAULT_WRITES_PER_MIN as usize) as u32
}

/// One IP's write count inside the current window.
#[derive(Default)]
pub struct WriteIpWindow {
    window_start: u64,
    writes: u32,
}

/// Per-IP write-limiter state (keyed by client IP). Public only because it
/// appears in [`AppState`]'s type; fields stay private.
pub type WriteLimiter = Mutex<HashMap<std::net::IpAddr, WriteIpWindow>>;

/// Enforce the per-IP write budget. `Ok(())` admits the request; `Err` is the 429
/// to return. An unknown client IP (no connect info, no trusted XFF) bypasses the
/// limiter — that only happens in tests, since `main` serves with connect info.
pub(crate) fn write_rate_limit(
    limiter: &WriteLimiter,
    ip: Option<std::net::IpAddr>,
    now: u64,
    cap: u32,
) -> Result<(), (StatusCode, String)> {
    if cap == 0 {
        return Ok(());
    }
    let Some(ip) = ip else { return Ok(()) };
    let mut map = limiter.lock().unwrap();
    // Same memory guard as the rendezvous limiter: shed elapsed windows under
    // pressure, and fail open rather than letting the limiter be the DoS.
    if map.len() >= MAX_RZ_LIMITER_IPS && !map.contains_key(&ip) {
        map.retain(|_, w| now < w.window_start + RZ_LIMITER_WINDOW_SECS);
        if map.len() >= MAX_RZ_LIMITER_IPS {
            return Ok(());
        }
    }
    let w = map.entry(ip).or_default();
    if now >= w.window_start + RZ_LIMITER_WINDOW_SECS {
        w.window_start = now;
        w.writes = 0;
    }
    if w.writes >= cap {
        return Err((
            StatusCode::TOO_MANY_REQUESTS,
            "write rate limit exceeded".into(),
        ));
    }
    w.writes += 1;
    Ok(())
}

#[cfg(test)]
mod rz_limiter_tests {
    use super::*;
    use std::net::{IpAddr, Ipv4Addr};

    fn ip(last: u8) -> Option<IpAddr> {
        Some(IpAddr::V4(Ipv4Addr::new(10, 0, 0, last)))
    }

    #[test]
    fn posts_are_capped_per_ip_and_window_resets() {
        let lim: RzLimiter = Mutex::new(HashMap::new());
        for _ in 0..3 {
            assert!(rz_rate_limit(&lim, ip(1), RzAction::Post, 100, 3, 10).is_ok());
        }
        // Fourth post inside the window is refused; another IP is unaffected.
        assert!(rz_rate_limit(&lim, ip(1), RzAction::Post, 100, 3, 10).is_err());
        assert!(rz_rate_limit(&lim, ip(2), RzAction::Post, 100, 3, 10).is_ok());
        // A new window admits again.
        assert!(rz_rate_limit(
            &lim,
            ip(1),
            RzAction::Post,
            100 + RZ_LIMITER_WINDOW_SECS,
            3,
            10
        )
        .is_ok());
    }

    #[test]
    fn distinct_slot_sweep_is_capped_but_polling_one_slot_is_not() {
        let lim: RzLimiter = Mutex::new(HashMap::new());
        // Polling the same slot any number of times is fine.
        for _ in 0..100 {
            assert!(rz_rate_limit(&lim, ip(1), RzAction::GetSlot("42"), 100, 10, 2).is_ok());
        }
        // A second distinct slot is fine, a third (over the cap of 2) is not.
        assert!(rz_rate_limit(&lim, ip(1), RzAction::GetSlot("43"), 100, 10, 2).is_ok());
        assert!(rz_rate_limit(&lim, ip(1), RzAction::GetSlot("44"), 100, 10, 2).is_err());
        // Already-counted slots keep working (the legit peer is never cut off).
        assert!(rz_rate_limit(&lim, ip(1), RzAction::GetSlot("42"), 100, 10, 2).is_ok());
    }

    #[test]
    fn zero_cap_disables_and_unknown_ip_bypasses() {
        let lim: RzLimiter = Mutex::new(HashMap::new());
        for _ in 0..1000 {
            assert!(rz_rate_limit(&lim, ip(1), RzAction::Post, 100, 0, 0).is_ok());
            assert!(rz_rate_limit(&lim, None, RzAction::Post, 100, 1, 1).is_ok());
        }
    }

    #[test]
    fn write_limit_caps_per_ip_resets_and_respects_zero_and_unknown() {
        let lim: WriteLimiter = Mutex::new(HashMap::new());
        // Up to `cap` writes admitted in a window; the next is refused.
        for _ in 0..3 {
            assert!(write_rate_limit(&lim, ip(1), 100, 3).is_ok());
        }
        assert!(write_rate_limit(&lim, ip(1), 100, 3).is_err());
        // A different IP has its own budget.
        assert!(write_rate_limit(&lim, ip(2), 100, 3).is_ok());
        // A new window admits again.
        assert!(write_rate_limit(&lim, ip(1), 100 + RZ_LIMITER_WINDOW_SECS, 3).is_ok());
        // cap == 0 disables; an unknown IP (no connect info) bypasses.
        for _ in 0..1000 {
            assert!(write_rate_limit(&lim, ip(3), 100, 0).is_ok());
            assert!(write_rate_limit(&lim, None, 100, 1).is_ok());
        }
    }
}
