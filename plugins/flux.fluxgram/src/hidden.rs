//! The hidden chats' list (part 3+): the chats of each account the user hid, kept encrypted with a
//! key derived from the phrase set in Settings → FluxGram → Hidden Chats. The phrase is never
//! stored: PBKDF2-HMAC-SHA256 over it with a random salt gives the key ([`Key::derive`]), and
//! ChaCha20-Poly1305 seals the list with it ([`Sealed`], the plugin's storage). Without the phrase
//! the stored list says nothing — neither which chats nor how many (it is padded to a block) — and
//! nothing tells a phrase cheaper than the derivation: a wrong one is known only by the seal
//! failing to open. A forgotten phrase is a lost list (the chats stay in Telegram).
//!
//! No host here: tested natively.

use std::collections::BTreeMap;

use base64::Engine;
use base64::engine::general_purpose::STANDARD as BASE64;
use chacha20poly1305::aead::{Aead, KeyInit, Payload};
use chacha20poly1305::{ChaCha20Poly1305, Nonce};
use serde::{Deserialize, Serialize};
use zeroize::Zeroize;

/// PBKDF2's rounds for a new phrase: a try costs ~200 ms in Flux's WASM (measured on an Apple M5:
/// 200 000 rounds took 98 ms). The rounds are stored with the list, so a later change still reads
/// the lists sealed before it.
pub const ROUNDS: u32 = 400_000;
/// The shortest phrase: a shorter search text isn't tried — no derivation for every short search.
pub const MIN_PHRASE: usize = 6;
/// The sealed list's length is a multiple of this: how many chats are hidden doesn't show.
const BLOCK: usize = 1024;
/// What the seal binds the list to: this format of it.
const LABEL: &[u8] = b"fluxgram.hidden-chats.v1";

/// The hidden chats: their ids by account.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct HiddenList {
    pub accounts: BTreeMap<String, Vec<i64>>,
}

impl HiddenList {
    pub fn contains(&self, account: &str, chat: i64) -> bool {
        self.chats(account).contains(&chat)
    }

    /// The account's hidden chats, in the order hidden.
    pub fn chats(&self, account: &str) -> &[i64] {
        self.accounts.get(account).map_or(&[], Vec::as_slice)
    }

    /// Hides the chat; false — it was hidden already.
    pub fn hide(&mut self, account: &str, chat: i64) -> bool {
        let chats = self.accounts.entry(account.to_string()).or_default();
        if chats.contains(&chat) {
            return false;
        }
        chats.push(chat);
        true
    }

    /// Shows the chat again; false — it wasn't hidden.
    pub fn unhide(&mut self, account: &str, chat: i64) -> bool {
        let Some(chats) = self.accounts.get_mut(account) else {
            return false;
        };
        let before = chats.len();
        chats.retain(|hidden| *hidden != chat);
        let changed = chats.len() != before;
        if chats.is_empty() {
            self.accounts.remove(account);
        }
        changed
    }

    /// Forgets an account's chats (the account signed out); false — it had none.
    pub fn forget_account(&mut self, account: &str) -> bool {
        self.accounts.remove(account).is_some()
    }
}

/// The list as stored: what opens with the right phrase only.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Sealed {
    /// The derivation's salt and rounds.
    pub salt: String,
    pub rounds: u32,
    /// The seal's nonce: new each time the list is sealed.
    pub nonce: String,
    /// The padded list, encrypted, with its tag: base64.
    pub data: String,
}

/// A key derived from the phrase, with the salt and the rounds it was derived with.
#[derive(Clone)]
pub struct Key {
    key: [u8; 32],
    salt: [u8; 16],
    rounds: u32,
}

impl Key {
    /// Derives the key of `phrase` (as typed: [`normalize`] is applied here).
    pub fn derive(phrase: &str, salt: [u8; 16], rounds: u32) -> Key {
        let mut key = [0u8; 32];
        pbkdf2::pbkdf2_hmac::<sha2::Sha256>(normalize(phrase).as_bytes(), &salt, rounds, &mut key);
        Key { key, salt, rounds }
    }

    /// Seals the list with this key; `nonce` — never used with the key before (random).
    pub fn seal(&self, list: &HiddenList, nonce: [u8; 12]) -> Sealed {
        let mut plain = serde_json::to_vec(list).unwrap_or_default();
        // Spaces after the JSON (it ignores them) up to the next block.
        let padded = plain.len().div_ceil(BLOCK).max(1) * BLOCK;
        plain.resize(padded, b' ');
        let data = self
            .cipher()
            .encrypt(
                Nonce::from_slice(&nonce),
                Payload {
                    msg: &plain,
                    aad: LABEL,
                },
            )
            .unwrap_or_default();
        Sealed {
            salt: BASE64.encode(self.salt),
            rounds: self.rounds,
            nonce: BASE64.encode(nonce),
            data: BASE64.encode(data),
        }
    }

    /// Opens a list sealed with this key; none — another key's, or damaged.
    pub fn open(&self, sealed: &Sealed) -> Option<HiddenList> {
        let nonce: [u8; 12] = BASE64.decode(&sealed.nonce).ok()?.try_into().ok()?;
        let data = BASE64.decode(&sealed.data).ok()?;
        let plain = self
            .cipher()
            .decrypt(
                Nonce::from_slice(&nonce),
                Payload {
                    msg: &data,
                    aad: LABEL,
                },
            )
            .ok()?;
        serde_json::from_slice(&plain).ok()
    }

    fn cipher(&self) -> ChaCha20Poly1305 {
        ChaCha20Poly1305::new((&self.key).into())
    }
}

impl Drop for Key {
    fn drop(&mut self) {
        // The key doesn't outlive its use in the instance's memory.
        self.key.zeroize();
    }
}

/// Opens the stored list with `phrase`: the key (to seal it again) and the list; none — a wrong
/// phrase, or a damaged list.
pub fn open(sealed: &Sealed, phrase: &str) -> Option<(Key, HiddenList)> {
    let salt: [u8; 16] = BASE64.decode(&sealed.salt).ok()?.try_into().ok()?;
    let key = Key::derive(phrase, salt, sealed.rounds);
    let list = key.open(sealed)?;
    Some((key, list))
}

/// A new key for `phrase`: a fresh salt, today's rounds.
pub fn new_key(phrase: &str) -> Key {
    Key::derive(phrase, random(), ROUNDS)
}

/// The phrase as it is derived: without the spaces around it.
pub fn normalize(phrase: &str) -> &str {
    phrase.trim()
}

/// The text is long enough to be a phrase: only such search texts are tried.
pub fn can_be_phrase(text: &str) -> bool {
    normalize(text).chars().count() >= MIN_PHRASE
}

/// `N` random bytes: a salt, a nonce.
pub fn random<const N: usize>() -> [u8; N] {
    let mut bytes = [0u8; N];
    // No randomness is no seal: zeros would repeat a nonce, so this stops loudly.
    getrandom::fill(&mut bytes).expect("the system's randomness");
    bytes
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Few rounds: the tests check the seal, not the cost.
    const FAST: u32 = 1_000;

    fn list(chats: &[i64]) -> HiddenList {
        let mut list = HiddenList::default();
        for chat in chats {
            list.hide("a1ec86805", *chat);
        }
        list
    }

    #[test]
    fn the_right_phrase_opens_and_a_wrong_one_does_not() {
        let key = Key::derive("correct horse", random(), FAST);
        let hidden = list(&[-1001234567890, 777000]);
        let sealed = key.seal(&hidden, random());
        let (again, opened) = open(&sealed, "correct horse").expect("opens");
        assert_eq!(opened, hidden);
        // The spaces around a typed phrase don't matter; anything else does.
        assert_eq!(
            open(&sealed, "  correct horse ").map(|(_, l)| l),
            Some(hidden)
        );
        assert!(open(&sealed, "Correct horse").is_none());
        assert!(open(&sealed, "correct hors").is_none());
        assert!(open(&sealed, "").is_none());
        // The key opened sealed again opens what it seals.
        let resealed = again.seal(&list(&[1]), random());
        assert_eq!(again.open(&resealed), Some(list(&[1])));
        assert!(
            Key::derive("correct horse", random(), FAST)
                .open(&sealed)
                .is_none()
        );
    }

    #[test]
    fn the_stored_list_reveals_nothing() {
        let key = Key::derive("correct horse", random(), FAST);
        let one = key.seal(&list(&[-1001234567890]), random());
        let ten = key.seal(
            &list(&(1..=10).map(|n| -1001234567800 - n).collect::<Vec<_>>()),
            random(),
        );
        let none = key.seal(&HiddenList::default(), random());
        // The same length whatever the count.
        assert_eq!(one.data.len(), ten.data.len());
        assert_eq!(one.data.len(), none.data.len());
        assert_eq!(BASE64.decode(&one.data).unwrap().len(), BLOCK + 16);
        // No ids or accounts in what is stored, in any form.
        let stored = serde_json::to_string(&one).unwrap();
        assert!(!stored.contains("1234567890") && !stored.contains("a1ec86805"));
        let raw = BASE64.decode(&one.data).unwrap();
        assert!(!raw.windows(10).any(|w| w == b"1234567890"));
        // The same list sealed twice looks different: a fresh nonce each time.
        let again = key.seal(&list(&[-1001234567890]), random());
        assert_ne!(again.data, one.data);
        // A tampered list doesn't open.
        let mut tampered = one.clone();
        let mut bytes = BASE64.decode(&tampered.data).unwrap();
        bytes[3] ^= 1;
        tampered.data = BASE64.encode(bytes);
        assert!(key.open(&tampered).is_none());
        // Many chats take more blocks.
        let many: Vec<i64> = (1..=200).map(|n| -1001234567000 - n).collect();
        let many = key.seal(&list(&many), random());
        assert_eq!((BASE64.decode(&many.data).unwrap().len() - 16) % BLOCK, 0);
        assert!(many.data.len() > one.data.len());
    }

    /// The cost of a try natively (`cargo test --release -- --ignored`); Flux's WASM logs its own
    /// ("A phrase tried in … ms").
    #[test]
    #[ignore]
    fn derivation_cost() {
        let started = std::time::Instant::now();
        let _ = Key::derive("correct horse", random(), ROUNDS);
        println!("{ROUNDS} rounds: {} ms", started.elapsed().as_millis());
    }

    #[test]
    fn keeps_the_list() {
        let mut hidden = list(&[5]);
        assert!(!hidden.hide("a1ec86805", 5));
        assert!(hidden.hide("adb1e492b", 5));
        assert!(hidden.contains("adb1e492b", 5) && hidden.contains("a1ec86805", 5));
        assert!(hidden.unhide("a1ec86805", 5));
        assert!(!hidden.unhide("a1ec86805", 5));
        assert!(
            !hidden.accounts.contains_key("a1ec86805"),
            "no empty accounts"
        );
        assert!(hidden.forget_account("adb1e492b"));
        assert_eq!(hidden, HiddenList::default());
        assert!(can_be_phrase(" secret ") && !can_be_phrase("  short "));
        assert!(can_be_phrase("тайное"), "letters, not bytes");
    }
}
