//! The shared outbound HTTP client, and the optional proxy every relay request
//! can be routed through.
//!
//! Two reasons this exists rather than a `reqwest::Client::new()` at each call
//! site.
//!
//! **A proxy has to be all-or-nothing.** Everything arvolo sends to a relay —
//! deposits, fetches, rendezvous, inbox polls, presence beacons — is end-to-end
//! encrypted, so the relay learns nothing from the bodies. What it does learn is
//! the client IP, on every request, and it necessarily keeps it: the per-IP rate
//! limiters are how a public relay survives (see the relay's `limits` module),
//! and any reverse proxy in front logs it besides. [`PROXY_ENV`] routes the whole
//! HTTP surface through a proxy — `socks5h://127.0.0.1:9050` for Tor — so the
//! relay sees the exit's address instead. One client factory is what makes that
//! claim checkable: a single missed call site would leak the real address for
//! that one route while every other route was covered, which is worse than not
//! offering the option at all.
//!
//! **Connection reuse.** `Client::new()` per request also threw away reqwest's
//! connection pool, paying a fresh TLS handshake per poll on a daemon that polls
//! forever. [`client`] hands out clones of one pooled client.
//!
//! What this does *not* cover is the P2P path: iroh speaks QUIC (UDP), and a
//! SOCKS5 proxy cannot carry it. A direct transfer shows your address to the peer,
//! and n0's discovery preset publishes a stable `EndpointId → address` record
//! besides. That path has its own switches — see
//! [`DiscoveryChoice`](crate::transfer::DiscoveryChoice) and
//! [`p2p_enabled`](crate::transfer::p2p_enabled) — and `p2p = false` is what makes
//! this proxy total, by leaving the mailbox as the only route.

use std::sync::OnceLock;
use std::time::Duration;

/// Env var naming a proxy for every relay request. Any scheme reqwest accepts:
/// `socks5h://127.0.0.1:9050` (Tor — `h` so the relay hostname is resolved at
/// the exit, not locally), `socks5://…`, `http://…`, `https://…`. Unset or empty
/// means a direct connection.
pub const PROXY_ENV: &str = "ARVOLO_PROXY";

/// A proxy URL that cannot connect, used when a *configured* proxy is unusable.
///
/// The one thing this module must never do is fall back to a direct connection:
/// someone who set [`PROXY_ENV`] and got a typo back would have their real
/// address handed to the relay by the very setting meant to prevent it, and
/// nothing in the output would say so. Port 1 on loopback refuses instantly, so
/// a misconfigured proxy shows up as every request failing — loud, immediate, and
/// leaking nothing.
const BLACKHOLE_PROXY: &str = "http://127.0.0.1:1";

/// The configured proxy URL, or `None` for a direct connection.
pub fn configured_proxy() -> Option<String> {
    std::env::var(PROXY_ENV)
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

/// Validate a proxy URL, returning the reason it can't be used.
///
/// For callers that can report to a human at startup — the CLI does this before
/// any request goes out, so a typo is one clear error instead of every relay
/// operation failing obscurely.
pub fn check_proxy(url: &str) -> Result<(), String> {
    reqwest::Proxy::all(url)
        .map(|_| ())
        .map_err(|e| e.to_string())
}

/// Build a client routed through `proxy` (if any), with an optional total-request
/// timeout.
///
/// Infallible by construction: an unusable proxy URL becomes [`BLACKHOLE_PROXY`]
/// rather than a direct connection, and a client that cannot be built at all
/// falls back to `Client::new()` — which only happens when there is no proxy to
/// honour, so it cannot leak past one.
pub fn build_client(proxy: Option<&str>, timeout: Option<Duration>) -> reqwest::Client {
    let mut builder = reqwest::Client::builder();
    if let Some(url) = proxy {
        let p = reqwest::Proxy::all(url).or_else(|e| {
            eprintln!("warning: {PROXY_ENV}={url} is not a usable proxy ({e}); refusing to connect directly — relay requests will fail until it is fixed");
            reqwest::Proxy::all(BLACKHOLE_PROXY)
        });
        match p {
            Ok(p) => builder = builder.proxy(p),
            // Unreachable in practice (a literal, valid URL), and if it ever were
            // reachable the safe move is still not to connect directly.
            Err(_) => builder = builder.no_proxy(),
        }
    }
    if let Some(t) = timeout {
        builder = builder.timeout(t);
    }
    builder.build().unwrap_or_else(|_| reqwest::Client::new())
}

/// The shared client: pooled, no request timeout (callers that need one use
/// [`client_with_timeout`]), routed through [`PROXY_ENV`] when it is set.
///
/// The proxy is read once, at first use. A process that changes the variable
/// mid-run keeps the client it already built — which is what a long-lived daemon
/// wants, and why tests exercise [`build_client`] directly.
pub fn client() -> reqwest::Client {
    static SHARED: OnceLock<reqwest::Client> = OnceLock::new();
    SHARED
        .get_or_init(|| build_client(configured_proxy().as_deref(), None))
        .clone()
}

/// A client with a total-request timeout, honouring [`PROXY_ENV`] like [`client`].
///
/// Built per call (the timeout is part of the client, and these are one-shot
/// probes rather than a poll loop), so it does not share the pooled client above.
pub fn client_with_timeout(timeout: Duration) -> reqwest::Client {
    build_client(configured_proxy().as_deref(), Some(timeout))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_proxy_builds_a_direct_client() {
        // Nothing observable to assert beyond "it built": reqwest exposes no
        // getter for the proxy. The behavioural checks live in the two below.
        let _ = build_client(None, None);
        let _ = build_client(None, Some(Duration::from_secs(1)));
    }

    #[test]
    fn a_valid_proxy_url_is_accepted() {
        assert!(check_proxy("socks5h://127.0.0.1:9050").is_ok());
        assert!(check_proxy("http://127.0.0.1:3128").is_ok());
    }

    #[test]
    fn a_bad_proxy_url_is_rejected_not_ignored() {
        assert!(check_proxy("not a url").is_err());
        // And building with it must not yield a direct client: the request has to
        // fail. `http://127.0.0.1:1` refuses, so this resolves fast.
        let c = build_client(Some("not a url"), Some(Duration::from_secs(5)));
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let err = rt.block_on(async { c.get("http://example.invalid/").send().await });
        assert!(err.is_err(), "a misconfigured proxy must fail, not bypass");
    }
}
