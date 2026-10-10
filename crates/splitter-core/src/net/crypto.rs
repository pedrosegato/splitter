//! Pairing, session and media encryption.
//!
//! First-time pairing derives the token from an ephemeral X25519 exchange, so it never
//! crosses the wire; both screens show a code from the same secret to rule out a
//! man-in-the-middle. Paired peers prove the token with HMACs over the handshake
//! transcript, and every session gets fresh keys from that exchange and the token.

use base64::engine::general_purpose::STANDARD as B64;
use base64::Engine;
use chacha20poly1305::aead::{Aead, KeyInit, Payload};
use chacha20poly1305::{ChaCha20Poly1305, Nonce};
use constant_time_eq::constant_time_eq;
use hkdf::Hkdf;
use hmac::{Hmac, Mac};
use rand::RngCore;
use sha2::Sha256;
use uuid::Uuid;
use x25519_dalek::{EphemeralSecret, PublicKey};

pub const KEY_LEN: usize = 32;
pub type Key = [u8; KEY_LEN];

const COUNTER_LEN: usize = 8;
/// Bytes a sealed datagram adds: the counter in front and the tag behind.
pub const SEAL_OVERHEAD: usize = COUNTER_LEN + 16;
// Media packets can be reordered in flight; anything further behind is a replay.
const REPLAY_WINDOW: u64 = 1024;

/// Everything both sides saw during the handshake; proofs and keys are bound to it.
#[derive(Debug, Clone, Copy)]
pub struct Transcript<'a> {
    pub client_peer_id: &'a str,
    pub server_peer_id: &'a str,
    pub client_nonce: &'a str,
    pub server_nonce: &'a str,
    pub client_key: &'a str,
    pub server_key: &'a str,
}

impl Transcript<'_> {
    fn bytes(&self) -> Vec<u8> {
        let mut out = Vec::new();
        for part in [
            self.client_peer_id,
            self.server_peer_id,
            self.client_nonce,
            self.server_nonce,
            self.client_key,
            self.server_key,
        ] {
            out.extend_from_slice(&(part.len() as u32).to_be_bytes());
            out.extend_from_slice(part.as_bytes());
        }
        out
    }
}

pub fn new_nonce() -> String {
    let mut buf = [0u8; 16];
    rand::thread_rng().fill_bytes(&mut buf);
    B64.encode(buf)
}

pub fn new_salt() -> String {
    new_nonce()
}

pub struct KeyExchange {
    secret: EphemeralSecret,
    public: String,
}

impl Default for KeyExchange {
    fn default() -> Self {
        Self::new()
    }
}

impl KeyExchange {
    pub fn new() -> Self {
        let secret = EphemeralSecret::random_from_rng(rand::rngs::OsRng);
        let public = B64.encode(PublicKey::from(&secret).as_bytes());
        Self { secret, public }
    }

    pub fn public_key(&self) -> &str {
        &self.public
    }

    /// `None` for a malformed key or one that forces a predictable (low-order) result.
    pub fn agree(self, peer_public: &str) -> Option<Key> {
        let bytes: [u8; KEY_LEN] = B64.decode(peer_public).ok()?.try_into().ok()?;
        let shared = self.secret.diffie_hellman(&PublicKey::from(bytes));
        shared.was_contributory().then(|| *shared.as_bytes())
    }
}

pub fn server_proof(token: &str, transcript: &Transcript<'_>) -> String {
    proof(b"splitter-v2/server", token, transcript)
}

pub fn client_proof(token: &str, transcript: &Transcript<'_>) -> String {
    proof(b"splitter-v2/client", token, transcript)
}

pub fn proofs_match(expected: &str, got: &str) -> bool {
    constant_time_eq(expected.as_bytes(), got.as_bytes())
}

fn proof(label: &[u8], token: &str, transcript: &Transcript<'_>) -> String {
    let mut mac = <Hmac<Sha256> as Mac>::new_from_slice(token.as_bytes())
        .expect("HMAC accepts keys of any length");
    mac.update(label);
    mac.update(&transcript.bytes());
    B64.encode(mac.finalize().into_bytes())
}

fn expand<const N: usize>(ikm: &[u8], salt: &[u8], info: &[u8]) -> [u8; N] {
    let mut out = [0u8; N];
    Hkdf::<Sha256>::new(Some(salt), ikm)
        .expand(info, &mut out)
        .expect("output length is within HKDF limits");
    out
}

/// The token both sides store after a first-time pairing; derived, never sent.
pub fn pairing_token(dh: &Key, transcript: &Transcript<'_>) -> String {
    B64.encode(expand::<KEY_LEN>(
        dh,
        &transcript.bytes(),
        b"splitter-v2/pairing-token",
    ))
}

/// Six digits shown on both machines; a mismatch means someone sits in between.
pub fn pairing_code(dh: &Key, transcript: &Transcript<'_>) -> String {
    let bytes = expand::<4>(dh, &transcript.bytes(), b"splitter-v2/pairing-code");
    let code = u32::from_be_bytes(bytes) % 1_000_000;
    format!("{:03} {:03}", code / 1000, code % 1000)
}

#[derive(Clone)]
pub struct SessionKeys {
    secret: Key,
    pub client_to_server: Key,
    pub server_to_client: Key,
}

impl std::fmt::Debug for SessionKeys {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SessionKeys").finish_non_exhaustive()
    }
}

pub fn session_keys(dh: &Key, token: &str, transcript: &Transcript<'_>) -> SessionKeys {
    let salt = transcript.bytes();
    let ikm = [dh.as_slice(), token.as_bytes()].concat();
    let secret = expand::<KEY_LEN>(&ikm, &salt, b"splitter-v2/session");
    SessionKeys {
        client_to_server: expand(&secret, &salt, b"splitter-v2/c2s"),
        server_to_client: expand(&secret, &salt, b"splitter-v2/s2c"),
        secret,
    }
}

#[doc(hidden)]
pub fn _session_for_tests() -> SessionKeys {
    let transcript = Transcript {
        client_peer_id: "client",
        server_peer_id: "server",
        client_nonce: "cn",
        server_nonce: "sn",
        client_key: "ck",
        server_key: "sk",
    };
    session_keys(&[1u8; KEY_LEN], "token", &transcript)
}

#[derive(Clone, Copy)]
pub struct StreamKeys {
    /// Source to sink audio.
    pub media: Key,
    /// Sink to source receiver reports.
    pub report: Key,
}

impl std::fmt::Debug for StreamKeys {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StreamKeys").finish_non_exhaustive()
    }
}

impl SessionKeys {
    /// The salt is fresh per StreamOpen, so a reused stream id never reuses a key.
    pub fn stream(&self, session_id: Uuid, stream_id: u8, salt: &str) -> StreamKeys {
        let info = |label: &[u8]| [label, session_id.as_bytes(), &[stream_id]].concat();
        StreamKeys {
            media: expand(&self.secret, salt.as_bytes(), &info(b"splitter-v2/media")),
            report: expand(&self.secret, salt.as_bytes(), &info(b"splitter-v2/report")),
        }
    }
}

fn nonce(counter: u64) -> Nonce {
    let mut bytes = [0u8; 12];
    bytes[4..].copy_from_slice(&counter.to_be_bytes());
    Nonce::from(bytes)
}

/// One direction of an ordered stream (TCP), so the nonce counter is implicit.
pub struct FrameCipher {
    aead: ChaCha20Poly1305,
    counter: u64,
}

impl FrameCipher {
    pub fn new(key: &Key) -> Self {
        Self {
            aead: ChaCha20Poly1305::new(key.into()),
            counter: 0,
        }
    }

    pub fn seal(&mut self, plaintext: &[u8]) -> Vec<u8> {
        let sealed = self
            .aead
            .encrypt(&nonce(self.counter), plaintext)
            .expect("ChaCha20-Poly1305 encryption cannot fail for in-memory buffers");
        self.counter += 1;
        sealed
    }

    pub fn open(&mut self, ciphertext: &[u8]) -> Option<Vec<u8>> {
        let opened = self.aead.decrypt(&nonce(self.counter), ciphertext).ok()?;
        self.counter += 1;
        Some(opened)
    }
}

/// One direction of datagrams, which can be lost or reordered: the counter travels in
/// front of the ciphertext and the caller's header is authenticated alongside it.
pub struct PacketSealer {
    aead: ChaCha20Poly1305,
    counter: u64,
}

impl PacketSealer {
    pub fn new(key: &Key) -> Self {
        Self {
            aead: ChaCha20Poly1305::new(key.into()),
            counter: 0,
        }
    }

    pub fn seal(&mut self, header: &[u8], plaintext: &[u8]) -> Vec<u8> {
        let counter = self.counter.to_be_bytes();
        let aad = [header, counter.as_slice()].concat();
        let ciphertext = self
            .aead
            .encrypt(
                &nonce(self.counter),
                Payload {
                    msg: plaintext,
                    aad: &aad,
                },
            )
            .expect("ChaCha20-Poly1305 encryption cannot fail for in-memory buffers");
        self.counter += 1;
        [counter.as_slice(), &ciphertext].concat()
    }
}

const WINDOW_WORDS: usize = (REPLAY_WINDOW / 64) as usize;

/// Bit `i` records whether counter `highest - i` has been accepted.
#[derive(Default)]
struct ReplayWindow {
    highest: Option<u64>,
    seen: [u64; WINDOW_WORDS],
}

impl ReplayWindow {
    fn is_fresh(&self, counter: u64) -> bool {
        let Some(highest) = self.highest else {
            return true;
        };
        if counter > highest {
            return true;
        }
        let age = highest - counter;
        age < REPLAY_WINDOW && self.seen[(age / 64) as usize] & (1 << (age % 64)) == 0
    }

    fn mark(&mut self, counter: u64) {
        match self.highest {
            Some(highest) if counter <= highest => {
                let age = highest - counter;
                self.seen[(age / 64) as usize] |= 1 << (age % 64);
            }
            highest => {
                self.shift(highest.map_or(REPLAY_WINDOW, |h| counter - h));
                self.highest = Some(counter);
                self.seen[0] |= 1;
            }
        }
    }

    fn shift(&mut self, by: u64) {
        if by >= REPLAY_WINDOW {
            self.seen = [0; WINDOW_WORDS];
            return;
        }
        let words = (by / 64) as usize;
        let bits = (by % 64) as u32;
        for i in (0..WINDOW_WORDS).rev() {
            let low = i.checked_sub(words).map_or(0, |j| self.seen[j]);
            let carry = match i.checked_sub(words + 1) {
                Some(j) if bits > 0 => self.seen[j] >> (64 - bits),
                _ => 0,
            };
            self.seen[i] = (low << bits) | carry;
        }
    }
}

pub struct PacketOpener {
    aead: ChaCha20Poly1305,
    window: ReplayWindow,
}

impl PacketOpener {
    pub fn new(key: &Key) -> Self {
        Self {
            aead: ChaCha20Poly1305::new(key.into()),
            window: ReplayWindow::default(),
        }
    }

    pub fn open(&mut self, header: &[u8], sealed: &[u8]) -> Option<Vec<u8>> {
        let (counter_bytes, ciphertext) = sealed.split_at_checked(COUNTER_LEN)?;
        let counter = u64::from_be_bytes(counter_bytes.try_into().ok()?);
        if !self.window.is_fresh(counter) {
            return None;
        }
        let aad = [header, counter_bytes].concat();
        let plaintext = self
            .aead
            .decrypt(
                &nonce(counter),
                Payload {
                    msg: ciphertext,
                    aad: &aad,
                },
            )
            .ok()?;
        self.window.mark(counter);
        Some(plaintext)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn transcript<'a>(client_key: &'a str, server_key: &'a str) -> Transcript<'a> {
        Transcript {
            client_peer_id: "client",
            server_peer_id: "server",
            client_nonce: "cn",
            server_nonce: "sn",
            client_key,
            server_key,
        }
    }

    fn exchange() -> (Key, Key, String, String) {
        let client = KeyExchange::new();
        let server = KeyExchange::new();
        let (cpub, spub) = (
            client.public_key().to_string(),
            server.public_key().to_string(),
        );
        let c = client.agree(&spub).unwrap();
        let s = server.agree(&cpub).unwrap();
        (c, s, cpub, spub)
    }

    #[test]
    fn both_sides_derive_the_same_pairing_token_code_and_keys() {
        let (c, s, cpub, spub) = exchange();
        let t = transcript(&cpub, &spub);
        assert_eq!(pairing_token(&c, &t), pairing_token(&s, &t));
        assert_eq!(pairing_code(&c, &t), pairing_code(&s, &t));
        let token = pairing_token(&c, &t);
        assert!(token.len() >= crate::net::trust::MIN_AUTH_TOKEN_LEN);
        let (kc, ks) = (session_keys(&c, &token, &t), session_keys(&s, &token, &t));
        assert_eq!(kc.client_to_server, ks.client_to_server);
        assert_ne!(kc.client_to_server, kc.server_to_client);
    }

    #[test]
    fn a_substituted_key_changes_the_pairing_code() {
        let (c, _s, cpub, spub) = exchange();
        let mitm = KeyExchange::new();
        let mitm_pub = mitm.public_key().to_string();
        let real = pairing_code(&c, &transcript(&cpub, &spub));
        let forged = pairing_code(&c, &transcript(&cpub, &mitm_pub));
        assert_ne!(real, forged);
        assert_eq!(real.len(), 7, "{real}");
    }

    #[test]
    fn agree_rejects_malformed_and_low_order_keys() {
        assert!(KeyExchange::new().agree("not base64!").is_none());
        assert!(KeyExchange::new().agree(&B64.encode([0u8; 16])).is_none());
        assert!(KeyExchange::new().agree(&B64.encode([0u8; 32])).is_none());
    }

    #[test]
    fn proofs_are_bound_to_role_token_and_transcript() {
        let t = transcript("ck", "sk");
        let base = client_proof("tok", &t);
        assert_ne!(base, server_proof("tok", &t));
        assert_ne!(base, client_proof("other", &t));
        assert_ne!(base, client_proof("tok", &transcript("ck", "forged")));
        assert!(proofs_match(&base, &client_proof("tok", &t)));
        assert!(!base.contains("tok"));
    }

    #[test]
    fn frame_cipher_round_trips_and_rejects_tampering_and_reordering() {
        let key = [7u8; KEY_LEN];
        let (mut tx, mut rx) = (FrameCipher::new(&key), FrameCipher::new(&key));
        let first = tx.seal(b"hello");
        let second = tx.seal(b"world");
        assert_eq!(rx.open(&first).as_deref(), Some(&b"hello"[..]));
        let mut tampered = second.clone();
        tampered[0] ^= 1;
        assert!(FrameCipher::new(&key).open(&tampered).is_none());
        let mut skipped = FrameCipher::new(&key);
        assert!(
            skipped.open(&second).is_none(),
            "frames must arrive in order"
        );
        assert_eq!(rx.open(&second).as_deref(), Some(&b"world"[..]));
    }

    #[test]
    fn packet_opener_authenticates_the_header_and_rejects_old_replays() {
        let key = [9u8; KEY_LEN];
        let mut sealer = PacketSealer::new(&key);
        let mut opener = PacketOpener::new(&key);
        let first = sealer.seal(b"hdr", b"frame");
        assert!(PacketOpener::new(&key).open(b"other-hdr", &first).is_none());
        assert_eq!(opener.open(b"hdr", &first).as_deref(), Some(&b"frame"[..]));
        let mut last = Vec::new();
        for _ in 0..=REPLAY_WINDOW {
            last = sealer.seal(b"hdr", b"frame");
        }
        assert!(opener.open(b"hdr", &last).is_some());
        assert!(
            opener.open(b"hdr", &first).is_none(),
            "too old to be reordering"
        );
        assert!(PacketOpener::new(&[1u8; KEY_LEN])
            .open(b"hdr", &last)
            .is_none());
    }

    #[test]
    fn packet_opener_rejects_duplicates_but_accepts_reordering() {
        let key = [4u8; KEY_LEN];
        let mut sealer = PacketSealer::new(&key);
        let mut opener = PacketOpener::new(&key);
        let packets: Vec<_> = (0..200).map(|_| sealer.seal(b"h", b"p")).collect();
        assert!(opener.open(b"h", &packets[0]).is_some());
        assert!(opener.open(b"h", &packets[0]).is_none(), "replayed");
        assert!(opener.open(b"h", &packets[150]).is_some());
        assert!(opener.open(b"h", &packets[70]).is_some(), "late but new");
        assert!(
            opener.open(b"h", &packets[70]).is_none(),
            "replayed late packet"
        );
        assert!(opener.open(b"h", &packets[150]).is_none());
        assert!(opener.open(b"h", &packets[149]).is_some());
    }

    #[test]
    fn replay_window_survives_shifts_across_word_boundaries() {
        let mut window = ReplayWindow::default();
        for counter in [0, 63, 64, 65, 130, 1000, 1023, 1024, 1090] {
            assert!(window.is_fresh(counter), "{counter}");
            window.mark(counter);
            assert!(!window.is_fresh(counter), "{counter}");
        }
        for seen in [130, 1000, 1023, 1024] {
            assert!(!window.is_fresh(seen), "{seen} must still be remembered");
        }
        for unseen in [129, 131, 999, 1089] {
            assert!(window.is_fresh(unseen), "{unseen}");
        }
        assert!(!window.is_fresh(65), "older than the window");

        window.mark(5000);
        assert!(window.is_fresh(4999));
        assert!(!window.is_fresh(5000 - REPLAY_WINDOW));
        assert!(!window.is_fresh(1090), "outside the window now");
    }

    #[test]
    fn stream_keys_differ_per_salt_and_direction() {
        let (c, _s, cpub, spub) = exchange();
        let t = transcript(&cpub, &spub);
        let session = session_keys(&c, "tok", &t);
        let sid = Uuid::new_v4();
        let a = session.stream(sid, 0, "salt-a");
        let b = session.stream(sid, 0, "salt-b");
        assert_ne!(a.media, b.media);
        assert_ne!(a.media, a.report);
    }
}
