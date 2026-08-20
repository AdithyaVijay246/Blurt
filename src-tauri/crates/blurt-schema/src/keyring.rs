//! Key-wrapping keyring — `MODULE_02_SCHEMA.md` §3.
//!
//! The SQLCipher master key is **random**, never derived from the passphrase.
//! It is stored only in wrapped form, across independent slots, any one of
//! which can unwrap it. This is the LUKS-keyslot pattern §3 names explicitly,
//! and it is what makes passphrase change, cross-device reset (§5), and
//! recovery possible without ever re-encrypting the database.
//!
//! Do not reintroduce `key = Argon2id(passphrase)` — that design was
//! deliberately superseded.
//!
//! ## Where this lives on disk
//!
//! The wrapped-key blobs cannot live inside the database they unlock, so the
//! keyring is a separate small file (`keyring.json`) beside `blurt.db`. It is
//! not secret: every slot is useless without the corresponding passphrase or
//! recovery key.
//!
//! ## Slot kinds
//!
//! - [`SlotKind::Passphrase`] — day-to-day unlock. Argon2id, because the input
//!   is a human-chosen passphrase and must be expensive to guess.
//! - [`SlotKind::Recovery`] — last-resort unlock. HKDF-SHA256, because the
//!   input is already 256 bits of CSPRNG output; stretching it would buy
//!   nothing and only make recovery slow.
//! - [`SlotKind::Sensitive`] — the §5 sensitive-info passphrase, which the
//!   user may set to the master passphrase or to something distinct. Modelling
//!   it as a slot means "is this the right passphrase" is answered by
//!   "does it unwrap", with no separate verifier to keep in sync. Each slot
//!   carries its own salt, so reuse is not detectable from the file.

use aes_gcm::{
    aead::{Aead, KeyInit},
    Aes256Gcm, Nonce,
};
use serde::{Deserialize, Serialize};
use zeroize::{Zeroize, ZeroizeOnDrop};

use crate::error::{Result, SchemaError};
use crate::recovery::RecoveryKey;

/// Length of the SQLCipher master key in bytes (AES-256).
pub const MASTER_KEY_LEN: usize = 32;

/// AES-GCM nonce length in bytes.
const NONCE_LEN: usize = 12;

/// Per-slot salt length in bytes.
const SALT_LEN: usize = 16;

/// Current on-disk keyring format version.
pub const KEYRING_VERSION: u32 = 1;

/// Argon2id cost parameters for passphrase slots.
///
/// Above the OWASP floor of 19 MiB / t=2: this key protects an entire
/// zero-knowledge store whose loss is unrecoverable, and unlock happens once
/// per session rather than per request, so a slower KDF is cheap here.
const ARGON2_M_COST: u32 = 65536; // 64 MiB
const ARGON2_T_COST: u32 = 3;
const ARGON2_P_COST: u32 = 1;

/// The unwrapped SQLCipher master key.
///
/// Exists in memory only while the app is unlocked, and is zeroized on drop.
#[derive(Clone, Zeroize, ZeroizeOnDrop)]
pub struct MasterKey([u8; MASTER_KEY_LEN]);

impl MasterKey {
    /// Generates a new master key from the OS CSPRNG.
    ///
    /// This is the *only* way a master key comes into existence. It is never
    /// derived from a passphrase (§3).
    pub fn generate() -> Self {
        let mut bytes = [0u8; MASTER_KEY_LEN];
        rand::fill(&mut bytes[..]);
        Self(bytes)
    }

    pub fn from_bytes(bytes: [u8; MASTER_KEY_LEN]) -> Self {
        Self(bytes)
    }

    pub fn as_bytes(&self) -> &[u8; MASTER_KEY_LEN] {
        &self.0
    }

    /// Lowercase hex, for SQLCipher's raw-key `PRAGMA key = "x'...'"` form.
    pub fn to_hex(&self) -> String {
        to_hex(&self.0)
    }
}

impl std::fmt::Debug for MasterKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("MasterKey(<redacted>)")
    }
}

/// Which unlock method a slot represents.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SlotKind {
    Passphrase,
    Recovery,
    Sensitive,
}

impl SlotKind {
    fn label(self) -> &'static str {
        match self {
            SlotKind::Passphrase => "passphrase",
            SlotKind::Recovery => "recovery",
            SlotKind::Sensitive => "sensitive",
        }
    }
}

/// How a slot's wrapping key is derived from its secret.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kdf", rename_all = "lowercase")]
pub enum Kdf {
    /// For human-chosen passphrases. Parameters are stored per slot so they
    /// can be raised for new slots later without invalidating existing ones.
    Argon2id { m_cost: u32, t_cost: u32, p_cost: u32 },
    /// For the already-high-entropy recovery key.
    HkdfSha256,
}

/// One independently-unwrappable slot.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KeySlot {
    pub kind: SlotKind,
    #[serde(flatten)]
    pub kdf: Kdf,
    #[serde(with = "hex_bytes")]
    pub salt: Vec<u8>,
    #[serde(with = "hex_bytes")]
    pub nonce: Vec<u8>,
    /// AES-256-GCM ciphertext of the master key, with the auth tag appended.
    #[serde(with = "hex_bytes")]
    pub wrapped_key: Vec<u8>,
}

/// The set of slots that can unlock the database.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Keyring {
    pub version: u32,
    pub slots: Vec<KeySlot>,
}

impl Default for Keyring {
    fn default() -> Self {
        Self { version: KEYRING_VERSION, slots: Vec::new() }
    }
}

impl Keyring {
    pub fn new() -> Self {
        Self::default()
    }

    /// True if a slot of this kind exists.
    pub fn has_slot(&self, kind: SlotKind) -> bool {
        self.slots.iter().any(|s| s.kind == kind)
    }

    /// Adds a passphrase-derived slot ([`SlotKind::Passphrase`] or
    /// [`SlotKind::Sensitive`]).
    ///
    /// Wraps the *existing* master key — it never generates or changes one.
    /// That is the property cross-device reset depends on.
    pub fn add_passphrase_slot(
        &mut self,
        kind: SlotKind,
        master: &MasterKey,
        passphrase: &str,
    ) -> Result<()> {
        if self.has_slot(kind) {
            return Err(SchemaError::SlotExists(kind.label()));
        }
        let slot = build_slot(kind, argon2id_params(), passphrase.as_bytes(), master)?;
        self.slots.push(slot);
        Ok(())
    }

    /// Adds the recovery-key slot, wrapping the same master key.
    pub fn add_recovery_slot(&mut self, master: &MasterKey, recovery: &RecoveryKey) -> Result<()> {
        if self.has_slot(SlotKind::Recovery) {
            return Err(SchemaError::SlotExists(SlotKind::Recovery.label()));
        }
        let slot = build_slot(
            SlotKind::Recovery,
            Kdf::HkdfSha256,
            recovery.as_bytes(),
            master,
        )?;
        self.slots.push(slot);
        Ok(())
    }

    /// Unwraps the master key using a passphrase slot.
    pub fn unwrap_with_passphrase(&self, kind: SlotKind, passphrase: &str) -> Result<MasterKey> {
        self.unwrap_slot(kind, passphrase.as_bytes())
    }

    /// Unwraps the master key using the recovery slot.
    pub fn unwrap_with_recovery_key(&self, recovery: &RecoveryKey) -> Result<MasterKey> {
        self.unwrap_slot(SlotKind::Recovery, recovery.as_bytes())
    }

    fn unwrap_slot(&self, kind: SlotKind, secret: &[u8]) -> Result<MasterKey> {
        let slot = self
            .slots
            .iter()
            .find(|s| s.kind == kind)
            .ok_or(SchemaError::NoSuchSlot(kind.label()))?;

        let wrapping_key = derive_wrapping_key(slot.kdf, secret, &slot.salt)?;
        let cipher = Aes256Gcm::new((&wrapping_key).into());
        let nonce: [u8; NONCE_LEN] = slot
            .nonce
            .as_slice()
            .try_into()
            .map_err(|_| SchemaError::WrongSecret)?;

        // A wrong secret produces a failed GCM tag check, which is
        // indistinguishable here from a corrupt slot — deliberately so.
        let plaintext = cipher
            .decrypt(&Nonce::from(nonce), slot.wrapped_key.as_slice())
            .map_err(|_| SchemaError::WrongSecret)?;

        let bytes: [u8; MASTER_KEY_LEN] = plaintext
            .as_slice()
            .try_into()
            .map_err(|_| SchemaError::WrongSecret)?;

        Ok(MasterKey::from_bytes(bytes))
    }

    /// Replaces a slot's secret, leaving the master key untouched.
    ///
    /// This is both "change passphrase" (§5) and the cross-device reset path:
    /// a device that already holds the unwrapped master key can mint a new
    /// passphrase slot without ever knowing the old passphrase.
    pub fn replace_passphrase_slot(
        &mut self,
        kind: SlotKind,
        master: &MasterKey,
        new_passphrase: &str,
    ) -> Result<()> {
        let slot = build_slot(kind, argon2id_params(), new_passphrase.as_bytes(), master)?;
        match self.slots.iter_mut().find(|s| s.kind == kind) {
            Some(existing) => *existing = slot,
            None => self.slots.push(slot),
        }
        Ok(())
    }

    /// Removes a slot. Refuses to remove the last remaining one, which would
    /// make the database permanently unopenable.
    pub fn remove_slot(&mut self, kind: SlotKind) -> Result<()> {
        if !self.has_slot(kind) {
            return Err(SchemaError::NoSuchSlot(kind.label()));
        }
        if self.slots.len() == 1 {
            return Err(SchemaError::NoSuchSlot(
                "another unlock method — removing the last slot would orphan the database",
            ));
        }
        self.slots.retain(|s| s.kind != kind);
        Ok(())
    }

    pub fn save(&self, path: &std::path::Path) -> Result<()> {
        let json = serde_json::to_string_pretty(self)?;
        std::fs::write(path, json)?;
        Ok(())
    }

    pub fn load(path: &std::path::Path) -> Result<Self> {
        let json = std::fs::read_to_string(path)?;
        let ring: Keyring = serde_json::from_str(&json)?;
        if ring.version != KEYRING_VERSION {
            return Err(SchemaError::UnsupportedKeyringVersion(ring.version));
        }
        Ok(ring)
    }
}

fn argon2id_params() -> Kdf {
    Kdf::Argon2id {
        m_cost: ARGON2_M_COST,
        t_cost: ARGON2_T_COST,
        p_cost: ARGON2_P_COST,
    }
}

/// Derives a wrapping key, then encrypts the master key under it.
///
/// Each slot gets a fresh random salt and nonce, so two slots holding the same
/// passphrase still produce different bytes on disk — the file must not reveal
/// that a user reused their master passphrase for the sensitive slot.
fn build_slot(kind: SlotKind, kdf: Kdf, secret: &[u8], master: &MasterKey) -> Result<KeySlot> {
    let mut salt = vec![0u8; SALT_LEN];
    rand::fill(&mut salt[..]);
    let mut nonce_bytes = [0u8; NONCE_LEN];
    rand::fill(&mut nonce_bytes[..]);

    let wrapping_key = derive_wrapping_key(kdf, secret, &salt)?;
    let cipher = Aes256Gcm::new((&wrapping_key).into());
    let wrapped_key = cipher
        .encrypt(&Nonce::from(nonce_bytes), master.as_bytes().as_slice())
        .map_err(|e| SchemaError::Kdf(format!("key wrapping failed: {e}")))?;

    Ok(KeySlot {
        kind,
        kdf,
        salt,
        nonce: nonce_bytes.to_vec(),
        wrapped_key,
    })
}

/// Turns a secret into a 256-bit AES-GCM wrapping key.
///
/// Argon2id for passphrases, because the input is low-entropy and must be
/// expensive to guess. HKDF for the recovery key, because the input is already
/// 256 bits of CSPRNG output and stretching it would only slow recovery down.
fn derive_wrapping_key(kdf: Kdf, secret: &[u8], salt: &[u8]) -> Result<[u8; MASTER_KEY_LEN]> {
    let mut out = [0u8; MASTER_KEY_LEN];

    match kdf {
        Kdf::Argon2id {
            m_cost,
            t_cost,
            p_cost,
        } => {
            let params = argon2::Params::new(m_cost, t_cost, p_cost, Some(MASTER_KEY_LEN))
                .map_err(|e| SchemaError::Kdf(e.to_string()))?;
            let argon = argon2::Argon2::new(
                argon2::Algorithm::Argon2id,
                argon2::Version::V0x13,
                params,
            );
            argon
                .hash_password_into(secret, salt, &mut out)
                .map_err(|e| SchemaError::Kdf(e.to_string()))?;
        }
        Kdf::HkdfSha256 => {
            hkdf::Hkdf::<sha2::Sha256>::new(Some(salt), secret)
                .expand(b"blurt:master-key-wrap:v1", &mut out)
                .map_err(|e| SchemaError::Kdf(e.to_string()))?;
        }
    }

    Ok(out)
}

/// Hex serialization for the binary fields, so `keyring.json` stays
/// hand-inspectable during development.
mod hex_bytes {
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S: Serializer>(bytes: &Vec<u8>, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&super::to_hex(bytes))
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<u8>, D::Error> {
        let s = String::deserialize(d)?;
        super::from_hex(&s).map_err(serde::de::Error::custom)
    }
}

pub(crate) fn to_hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

pub(crate) fn from_hex(s: &str) -> std::result::Result<Vec<u8>, &'static str> {
    if s.len() % 2 != 0 {
        return Err("odd-length hex string");
    }
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).map_err(|_| "invalid hex digit"))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const PASSPHRASE: &str = "correct horse battery staple";

    fn keyring_with_passphrase() -> (Keyring, MasterKey) {
        let master = MasterKey::generate();
        let mut ring = Keyring::new();
        ring.add_passphrase_slot(SlotKind::Passphrase, &master, PASSPHRASE)
            .expect("add slot");
        (ring, master)
    }

    #[test]
    fn master_key_is_random_not_derived() {
        let a = MasterKey::generate();
        let b = MasterKey::generate();
        assert_eq!(a.as_bytes().len(), MASTER_KEY_LEN);
        assert_ne!(a.as_bytes(), &[0u8; MASTER_KEY_LEN]);
        assert_ne!(a.as_bytes(), b.as_bytes(), "two master keys collided");
    }

    #[test]
    fn master_key_hex_is_64_lowercase_chars() {
        let key = MasterKey::from_bytes([0xAB; MASTER_KEY_LEN]);
        let hex = key.to_hex();
        assert_eq!(hex.len(), 64);
        assert_eq!(hex, "ab".repeat(32));
    }

    #[test]
    fn debug_never_leaks_key_material() {
        let key = MasterKey::from_bytes([0xAB; MASTER_KEY_LEN]);
        let rendered = format!("{key:?}");
        assert!(!rendered.contains("ab") && !rendered.contains("AB"));
        assert!(rendered.contains("redacted"));
    }

    #[test]
    fn passphrase_slot_round_trips() {
        let (ring, master) = keyring_with_passphrase();
        let unwrapped = ring
            .unwrap_with_passphrase(SlotKind::Passphrase, PASSPHRASE)
            .expect("unwrap");
        assert_eq!(master.as_bytes(), unwrapped.as_bytes());
    }

    #[test]
    fn recovery_slot_round_trips() {
        let master = MasterKey::generate();
        let recovery = RecoveryKey::generate();
        let mut ring = Keyring::new();
        ring.add_recovery_slot(&master, &recovery).expect("add");
        let unwrapped = ring.unwrap_with_recovery_key(&recovery).expect("unwrap");
        assert_eq!(master.as_bytes(), unwrapped.as_bytes());
    }

    #[test]
    fn sensitive_slot_round_trips_independently() {
        let master = MasterKey::generate();
        let mut ring = Keyring::new();
        ring.add_passphrase_slot(SlotKind::Passphrase, &master, PASSPHRASE)
            .unwrap();
        ring.add_passphrase_slot(SlotKind::Sensitive, &master, "a different one")
            .unwrap();

        assert_eq!(
            ring.unwrap_with_passphrase(SlotKind::Sensitive, "a different one")
                .unwrap()
                .as_bytes(),
            master.as_bytes()
        );
        // The master passphrase must not open the sensitive slot.
        assert!(matches!(
            ring.unwrap_with_passphrase(SlotKind::Sensitive, PASSPHRASE),
            Err(SchemaError::WrongSecret)
        ));
    }

    #[test]
    fn sensitive_slot_may_reuse_the_master_passphrase() {
        // §5 explicitly permits this. Distinct salts mean the two slots must
        // still differ on disk, so the file doesn't reveal that they match.
        let master = MasterKey::generate();
        let mut ring = Keyring::new();
        ring.add_passphrase_slot(SlotKind::Passphrase, &master, PASSPHRASE)
            .unwrap();
        ring.add_passphrase_slot(SlotKind::Sensitive, &master, PASSPHRASE)
            .unwrap();

        let a = &ring.slots[0];
        let b = &ring.slots[1];
        assert_ne!(a.salt, b.salt, "slots must not share a salt");
        assert_ne!(a.wrapped_key, b.wrapped_key, "reuse is visible on disk");

        assert_eq!(
            ring.unwrap_with_passphrase(SlotKind::Sensitive, PASSPHRASE)
                .unwrap()
                .as_bytes(),
            master.as_bytes()
        );
    }

    #[test]
    fn wrong_passphrase_fails_cleanly() {
        let (ring, _) = keyring_with_passphrase();
        let err = ring
            .unwrap_with_passphrase(SlotKind::Passphrase, "not the passphrase")
            .unwrap_err();
        assert!(matches!(err, SchemaError::WrongSecret));
    }

    #[test]
    fn wrong_recovery_key_fails_cleanly() {
        let master = MasterKey::generate();
        let mut ring = Keyring::new();
        ring.add_recovery_slot(&master, &RecoveryKey::generate())
            .unwrap();
        let err = ring
            .unwrap_with_recovery_key(&RecoveryKey::generate())
            .unwrap_err();
        assert!(matches!(err, SchemaError::WrongSecret));
    }

    #[test]
    fn unwrapping_a_missing_slot_is_an_error_not_a_panic() {
        let ring = Keyring::new();
        assert!(matches!(
            ring.unwrap_with_passphrase(SlotKind::Passphrase, PASSPHRASE),
            Err(SchemaError::NoSuchSlot(_))
        ));
    }

    /// The property the whole key-wrapping design exists for: adding an unlock
    /// method must not disturb the master key, or every §5 recovery path
    /// would require re-encrypting the database.
    #[test]
    fn adding_a_slot_leaves_the_master_key_byte_identical() {
        let (mut ring, master) = keyring_with_passphrase();
        let recovery = RecoveryKey::generate();
        ring.add_recovery_slot(&master, &recovery).unwrap();
        ring.add_passphrase_slot(SlotKind::Sensitive, &master, "sensitive one")
            .unwrap();

        for unwrapped in [
            ring.unwrap_with_passphrase(SlotKind::Passphrase, PASSPHRASE).unwrap(),
            ring.unwrap_with_passphrase(SlotKind::Sensitive, "sensitive one").unwrap(),
            ring.unwrap_with_recovery_key(&recovery).unwrap(),
        ] {
            assert_eq!(
                master.as_bytes(),
                unwrapped.as_bytes(),
                "a slot unwrapped to a different master key"
            );
        }
    }

    #[test]
    fn adding_a_duplicate_slot_kind_is_rejected() {
        let (mut ring, master) = keyring_with_passphrase();
        assert!(matches!(
            ring.add_passphrase_slot(SlotKind::Passphrase, &master, "another"),
            Err(SchemaError::SlotExists(_))
        ));
    }

    /// Cross-device reset (§5): a device holding the unwrapped master key can
    /// mint a new passphrase slot without knowing the old passphrase.
    #[test]
    fn replacing_a_passphrase_preserves_the_master_key_and_other_slots() {
        let (mut ring, master) = keyring_with_passphrase();
        let recovery = RecoveryKey::generate();
        ring.add_recovery_slot(&master, &recovery).unwrap();

        ring.replace_passphrase_slot(SlotKind::Passphrase, &master, "brand new passphrase")
            .unwrap();

        assert_eq!(
            ring.unwrap_with_passphrase(SlotKind::Passphrase, "brand new passphrase")
                .unwrap()
                .as_bytes(),
            master.as_bytes()
        );
        assert!(matches!(
            ring.unwrap_with_passphrase(SlotKind::Passphrase, PASSPHRASE),
            Err(SchemaError::WrongSecret),
        ));
        // The recovery slot is untouched.
        assert_eq!(
            ring.unwrap_with_recovery_key(&recovery).unwrap().as_bytes(),
            master.as_bytes()
        );
    }

    #[test]
    fn removing_a_slot_leaves_the_others_working() {
        let (mut ring, master) = keyring_with_passphrase();
        let recovery = RecoveryKey::generate();
        ring.add_recovery_slot(&master, &recovery).unwrap();

        ring.remove_slot(SlotKind::Recovery).unwrap();

        assert!(!ring.has_slot(SlotKind::Recovery));
        assert_eq!(
            ring.unwrap_with_passphrase(SlotKind::Passphrase, PASSPHRASE)
                .unwrap()
                .as_bytes(),
            master.as_bytes()
        );
    }

    #[test]
    fn removing_the_last_slot_is_refused() {
        let (mut ring, _) = keyring_with_passphrase();
        assert!(
            ring.remove_slot(SlotKind::Passphrase).is_err(),
            "removing the last slot would orphan the database forever"
        );
    }

    #[test]
    fn survives_a_save_load_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("keyring.json");

        let (mut ring, master) = keyring_with_passphrase();
        let recovery = RecoveryKey::generate();
        ring.add_recovery_slot(&master, &recovery).unwrap();
        ring.save(&path).unwrap();

        let loaded = Keyring::load(&path).unwrap();
        assert_eq!(loaded.version, KEYRING_VERSION);
        assert_eq!(loaded.slots.len(), 2);
        assert_eq!(
            loaded
                .unwrap_with_passphrase(SlotKind::Passphrase, PASSPHRASE)
                .unwrap()
                .as_bytes(),
            master.as_bytes()
        );
        assert_eq!(
            loaded.unwrap_with_recovery_key(&recovery).unwrap().as_bytes(),
            master.as_bytes()
        );
    }

    #[test]
    fn keyring_file_never_contains_the_master_key() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("keyring.json");
        let (ring, master) = keyring_with_passphrase();
        ring.save(&path).unwrap();

        let contents = std::fs::read_to_string(&path).unwrap();
        assert!(
            !contents.contains(&master.to_hex()),
            "the master key was written to disk in the clear"
        );
        assert!(!contents.contains(PASSPHRASE), "passphrase written to disk");
    }

    #[test]
    fn hex_helpers_round_trip() {
        let bytes = vec![0x00, 0x0f, 0xa5, 0xff];
        assert_eq!(to_hex(&bytes), "000fa5ff");
        assert_eq!(from_hex("000fa5ff").unwrap(), bytes);
        assert!(from_hex("abc").is_err());
        assert!(from_hex("zz").is_err());
    }
}
