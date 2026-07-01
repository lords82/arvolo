//! Short-code pairing (magic-wormhole / croc style).
//!
//! Instead of copying a ~1000-char `arvc` ticket, the sender shows a short human
//! code like `4821-crater-mango`; the receiver types it. The ticket is exchanged
//! over a relay **rendezvous** and protected by a **SPAKE2** PAKE keyed on the
//! code, so the relay stays zero-knowledge (it only sees PAKE messages and the
//! **encrypted** ticket) and a short code is safe (no offline dictionary attack).
//!
//! The code may embed the sender's relay (`code@https://relay…`) so it works even
//! when the two sides use different relays; without it, the receiver's configured
//! default relay is used.

use std::time::{Duration, Instant};

use anyhow::{anyhow, bail, Context, Result};
use rand::Rng;

use crate::crypto::{open_chunk, seal_chunk, CHUNK_KEY_LEN};
use crate::pairing;

/// Sender's SPAKE2 message / slot-claim key.
const K_MS: &str = "ms";
/// Receiver's SPAKE2 message key.
const K_MR: &str = "mr";
/// Encrypted-ticket key (fetching it burns the slot).
const K_TKT: &str = "tkt";
/// Total time to wait for the peer at each rendezvous step.
const POLL_TIMEOUT: Duration = Duration::from_secs(120);

/// Parse a code into `(nameplate/slot, pake_secret, optional_relay_url)`.
/// Accepts `N-word-word` and `N-word-word@relay-url`.
pub fn parse_code(code: &str) -> Result<(String, String, Option<String>)> {
    let code = code.trim();
    let (secret, relay) = match code.split_once('@') {
        Some((s, r)) if !r.is_empty() => (s.to_string(), Some(r.to_string())),
        _ => (code.to_string(), None),
    };
    let nameplate = secret.split('-').next().unwrap_or("").to_string();
    if nameplate.is_empty() || secret.matches('-').count() < 2 {
        bail!("invalid code (expected N-word-word[@relay])");
    }
    Ok((nameplate, secret, relay))
}

/// `true` if `s` looks like a pairing code (vs. an `arvc…`/`arvm…` ticket).
pub fn looks_like_code(s: &str) -> bool {
    let head = s.split_once('@').map(|(l, _)| l).unwrap_or(s);
    let parts: Vec<&str> = head.split('-').collect();
    parts.len() >= 3 && parts[0].chars().all(|c| c.is_ascii_digit()) && !parts[0].is_empty()
}

/// A fresh `(nameplate, secret)`, e.g. `("4821", "4821-crater-mango")`.
fn gen_secret() -> (String, String) {
    let mut rng = rand::rng();
    let nameplate = rng.random_range(0u32..10_000).to_string();
    let w1 = WORDS[rng.random_range(0..WORDS.len())];
    let w2 = WORDS[rng.random_range(0..WORDS.len())];
    let secret = format!("{nameplate}-{w1}-{w2}");
    (nameplate, secret)
}

/// Derive the 32-byte ticket-encryption key from the SPAKE2 shared secret.
fn key32(pake_key: &[u8]) -> [u8; CHUNK_KEY_LEN] {
    let mut k = [0u8; CHUNK_KEY_LEN];
    let n = pake_key.len().min(CHUNK_KEY_LEN);
    k[..n].copy_from_slice(&pake_key[..n]);
    k
}

fn rz_url(relay: &str, slot: &str, key: &str) -> String {
    format!("{}/v1/rz/{slot}/{key}", relay.trim_end_matches('/'))
}

/// Poll a rendezvous key until it's posted (or time out).
async fn poll_get(client: &reqwest::Client, url: &str, what: &str) -> Result<Vec<u8>> {
    let start = Instant::now();
    loop {
        let resp = client.get(url).send().await.context("rendezvous poll")?;
        if resp.status().is_success() {
            return Ok(resp
                .bytes()
                .await
                .context("read rendezvous value")?
                .to_vec());
        }
        if resp.status() != reqwest::StatusCode::NOT_FOUND {
            resp.error_for_status().context("rendezvous poll")?;
        }
        if start.elapsed() > POLL_TIMEOUT {
            bail!("timed out waiting for {what}");
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
}

/// The sender's in-flight pairing: it has claimed a slot and posted its message;
/// [`PairComplete::run`] finishes the handshake and publishes the encrypted
/// ticket once the receiver shows up.
pub struct PairComplete {
    slot: String,
    relay: String,
    ticket: String,
    pairing: pairing::Pairing,
    client: reqwest::Client,
}

/// Claim a rendezvous slot and post the sender's SPAKE2 message. Returns the code
/// to display (with `@relay` appended when `embed_relay`) and a handle to finish
/// the exchange. Retries on a slot collision.
pub async fn publish_ticket(
    ticket: &str,
    relay: &str,
    embed_relay: bool,
) -> Result<(String, PairComplete)> {
    let relay = relay.trim_end_matches('/').to_string();
    let client = reqwest::Client::new();

    let (slot, secret, pairing) = loop {
        let (slot, secret) = gen_secret();
        let (p, ms) = pairing::start(&secret);
        let resp = client
            .post(rz_url(&relay, &slot, K_MS))
            .body(ms)
            .send()
            .await
            .context("rendezvous claim")?;
        if resp.status() == reqwest::StatusCode::CONFLICT {
            continue; // slot taken, pick a new nameplate
        }
        resp.error_for_status().context("rendezvous claim")?;
        break (slot, secret, p);
    };

    let shown = if embed_relay {
        format!("{secret}@{relay}")
    } else {
        secret
    };
    Ok((
        shown,
        PairComplete {
            slot,
            relay,
            ticket: ticket.to_string(),
            pairing,
            client,
        },
    ))
}

impl PairComplete {
    /// Wait for the receiver, derive the shared key, and publish the encrypted
    /// ticket. Completes once the receiver has shown up.
    pub async fn run(self) -> Result<()> {
        let mr = poll_get(
            &self.client,
            &rz_url(&self.relay, &self.slot, K_MR),
            "the receiver",
        )
        .await?;
        let key = key32(&self.pairing.finish(&mr)?);
        let ct = seal_chunk(&key, 0, 1, self.ticket.as_bytes())?;
        self.client
            .post(rz_url(&self.relay, &self.slot, K_TKT))
            .body(ct)
            .send()
            .await
            .context("publish ticket")?
            .error_for_status()
            .context("publish ticket")?;
        Ok(())
    }
}

/// Resolve a pairing code to its `arvc` ticket via the relay rendezvous. Uses the
/// relay embedded in the code, else `default_relay`.
pub async fn resolve_code(code: &str, default_relay: Option<&str>) -> Result<String> {
    let (slot, secret, relay_in_code) = parse_code(code)?;
    let relay = relay_in_code
        .or_else(|| default_relay.map(|s| s.to_string()))
        .ok_or_else(|| {
            anyhow!("no relay: the code has no @relay and no default relay is configured")
        })?;
    let relay = relay.trim_end_matches('/').to_string();
    let client = reqwest::Client::new();

    // Wait for the sender's message, then post ours and derive the key.
    let ms = poll_get(&client, &rz_url(&relay, &slot, K_MS), "the sender").await?;
    let (pairing, mr) = pairing::start(&secret);
    client
        .post(rz_url(&relay, &slot, K_MR))
        .body(mr)
        .send()
        .await
        .context("post pairing message")?
        .error_for_status()
        .context("post pairing message")?;
    let key = key32(&pairing.finish(&ms)?);

    // Fetch and decrypt the ticket (wrong code -> decrypt fails).
    let ct = poll_get(&client, &rz_url(&relay, &slot, K_TKT), "the ticket").await?;
    let ticket = open_chunk(&key, 0, 1, &ct).context("decrypt ticket (wrong code?)")?;
    String::from_utf8(ticket).context("ticket is not valid UTF-8")
}

/// 256 short words for pairing codes (16 bits of PAKE secret across two words).
#[rustfmt::skip]
const WORDS: [&str; 256] = [
    "acid","acorn","album","amber","anvil","apple","apron","arch","arena","armor",
    "ash","aspen","atlas","attic","axle","bacon","badge","bagel","bamboo","banjo",
    "barn","basil","bay","beacon","beam","bean","bear","beetle","bell","berry",
    "birch","bison","blade","blaze","bloom","board","boat","bolt","bongo","bonus",
    "boot","boulder","brave","bread","brick","bridge","broom","brush","bubble","bucket",
    "buffalo","bugle","bulb","bundle","cabin","cable","cactus","camel","candle","canoe",
    "canvas","canyon","cape","cargo","carol","carrot","castle","cave","cedar","cell",
    "chalk","cherry","chess","chime","cider","cinder","cliff","cloak","clover","cluster",
    "coal","cobra","cocoa","comet","copper","coral","cotton","cove","crane","crater",
    "crayon","creek","crest","crow","crown","cube","dagger","daisy","dawn","delta",
    "denim","desk","diamond","dingo","dock","dolphin","donut","dove","dragon","drum",
    "dune","eagle","ember","emu","engine","fable","falcon","fang","fern","ferry",
    "fiber","field","fig","finch","flame","flask","flint","flute","forest","fox",
    "frost","garlic","gecko","ginger","glacier","globe","glove","gnome","goat","gold",
    "grape","grotto","guitar","hammer","harbor","hawk","hazel","hedge","helm","heron",
    "hive","honey","horn","hut","igloo","indigo","ivory","ivy","jaguar","jasmine",
    "jelly","jet","jewel","kayak","kelp","kettle","key","kiwi","koala","lagoon",
    "lantern","lark","laurel","leaf","ledger","lemon","lentil","lily","lime","linen",
    "lion","llama","lobster","locket","lotus","lynx","mango","maple","marble","marsh",
    "meadow","melon","mesa","meteor","mint","mist","moss","moth","mule","nectar",
    "needle","nest","nettle","nickel","noble","nomad","oak","oasis","ocean","olive",
    "onyx","opal","orbit","otter","owl","oxide","paddle","palm","panda","papaya",
    "parrot","peach","pearl","pebble","pepper","phoenix","pigeon","pillow","pine","piston",
    "plum","pond","poppy","prairie","puma","quartz","quill","quilt","radish","raft",
    "rapid","raven","reef","ribbon","ridge","river","robin","rocket","rose","rubble",
    "ruby","sable","saffron","sage","salmon","sand",
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_plain_and_self_contained() {
        let (np, secret, relay) = parse_code("4821-crater-mango").unwrap();
        assert_eq!(np, "4821");
        assert_eq!(secret, "4821-crater-mango");
        assert_eq!(relay, None);

        let (np, secret, relay) = parse_code("7-fox-oak@https://relay.example.com:8787").unwrap();
        assert_eq!(np, "7");
        assert_eq!(secret, "7-fox-oak");
        assert_eq!(relay.as_deref(), Some("https://relay.example.com:8787"));
    }

    #[test]
    fn parse_rejects_junk() {
        assert!(parse_code("nope").is_err());
        assert!(parse_code("12-onlyone").is_err());
    }

    #[test]
    fn discriminates_code_from_ticket() {
        assert!(looks_like_code("4821-crater-mango"));
        assert!(looks_like_code("7-fox-oak@http://127.0.0.1:8787"));
        assert!(!looks_like_code("arvcQCAIAEEAQCAAQAUZLBT2")); // ticket
        assert!(!looks_like_code("word-word-word")); // non-digit nameplate
    }

    #[test]
    fn gen_secret_shape() {
        let (np, secret) = gen_secret();
        assert!(np.chars().all(|c| c.is_ascii_digit()));
        assert!(secret.starts_with(&format!("{np}-")));
        assert_eq!(secret.matches('-').count(), 2);
    }

    #[test]
    fn matching_code_decrypts_wrong_code_does_not() {
        let ticket = b"arvc-the-real-ticket";
        // Both sides run SPAKE2; matching secret -> same key -> decrypt works.
        let (ps, ms) = pairing::start("4821-crater-mango");
        let (pr, mr) = pairing::start("4821-crater-mango");
        let ks = key32(&ps.finish(&mr).unwrap());
        let kr = key32(&pr.finish(&ms).unwrap());
        let ct = seal_chunk(&ks, 0, 1, ticket).unwrap();
        assert_eq!(open_chunk(&kr, 0, 1, &ct).unwrap(), ticket);

        // Wrong secret on the receiver -> different key -> cannot decrypt.
        let (ps, ms) = pairing::start("4821-crater-mango");
        let (pw, mw) = pairing::start("4821-wrong-word");
        let ks = key32(&ps.finish(&mw).unwrap());
        let kw = key32(&pw.finish(&ms).unwrap());
        let ct = seal_chunk(&ks, 0, 1, ticket).unwrap();
        assert!(open_chunk(&kw, 0, 1, &ct).is_err());
    }
}
