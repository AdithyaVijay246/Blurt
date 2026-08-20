//! Recovery key generation, encoding, and parsing.
//!
//! `MODULE_02_SCHEMA.md` §3 specifies a randomly-generated recovery key that
//! independently wraps the master key, and `MODULE_06_UI_SHELL.md` §B4
//! specifies it is displayed "in readable groups (e.g. `XXXX-XXXX-XXXX-XXXX`)"
//! with a verification step that re-enters two of the groups.
//!
//! §B4's four-group example is illustrative. Four groups of four is 80 bits,
//! which would make the recovery slot the weakest link against a 256-bit
//! master key, so the key here is a full 256 bits encoded as 13 groups of 4.
//!
//! Encoding is Crockford base32, whose alphabet omits `I`, `L`, `O`, and `U`
//! precisely so a key transcribed onto paper and typed back in can't be
//! garbled. Parsing accepts lowercase, folds the ambiguous characters onto
//! their intended digits, and ignores separators.

use zeroize::{Zeroize, ZeroizeOnDrop};

use crate::error::{Result, SchemaError};

/// Crockford base32 alphabet — no `I`, `L`, `O`, or `U`.
const ALPHABET: &[u8; 32] = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ";

/// Length of the raw recovery key in bytes.
pub const RECOVERY_KEY_LEN: usize = 32;

/// Characters per display group.
pub const GROUP_LEN: usize = 4;

/// 32 bytes = 256 bits; at 5 bits per base32 character that is 52 characters.
pub const ENCODED_LEN: usize = 52;

/// Number of display groups (52 / 4).
pub const GROUP_COUNT: usize = ENCODED_LEN / GROUP_LEN;

/// A 256-bit recovery key.
///
/// Zeroized on drop. Never persisted in this form outside the encrypted
/// database — the keyring file stores only the wrapped master key it produces.
#[derive(Clone, Zeroize, ZeroizeOnDrop)]
pub struct RecoveryKey([u8; RECOVERY_KEY_LEN]);

impl RecoveryKey {
    /// Generates a new recovery key from the OS CSPRNG.
    pub fn generate() -> Self {
        let mut bytes = [0u8; RECOVERY_KEY_LEN];
        rand::fill(&mut bytes[..]);
        Self(bytes)
    }

    /// Builds a recovery key from raw bytes.
    pub fn from_bytes(bytes: [u8; RECOVERY_KEY_LEN]) -> Self {
        Self(bytes)
    }

    /// The raw key material.
    pub fn as_bytes(&self) -> &[u8; RECOVERY_KEY_LEN] {
        &self.0
    }

    /// Encodes as 13 dash-separated groups of 4, e.g. `A1B2-C3D4-...`.
    pub fn to_grouped_string(&self) -> String {
        self.groups().join("-")
    }

    /// The individual display groups, for the §B4 verification step that asks
    /// the user to re-enter two of them.
    pub fn groups(&self) -> Vec<String> {
        let encoded = encode(&self.0);
        encoded
            .as_bytes()
            .chunks(GROUP_LEN)
            .map(|c| String::from_utf8_lossy(c).into_owned())
            .collect()
    }

    /// Parses a user-entered recovery key.
    ///
    /// Tolerant of what a human actually types back from paper: any case,
    /// dashes or spaces anywhere or not at all, and the Crockford
    /// substitutions (`I`/`L` → `1`, `O` → `0`).
    pub fn parse(input: &str) -> Result<Self> {
        let cleaned: Vec<u8> = input
            .bytes()
            .filter(|b| !b.is_ascii_whitespace() && *b != b'-')
            .collect();

        if cleaned.len() != ENCODED_LEN {
            return Err(SchemaError::InvalidRecoveryKey("wrong length"));
        }

        Ok(Self(decode(&cleaned)?))
    }
}

/// Encodes 32 bytes as 52 Crockford base32 characters.
///
/// 256 bits is not a multiple of 5, so the final character carries the last
/// bit left-aligned with 4 zero bits of padding.
fn encode(bytes: &[u8; RECOVERY_KEY_LEN]) -> String {
    let mut out = String::with_capacity(ENCODED_LEN);
    let mut buffer: u32 = 0;
    let mut bits: u32 = 0;

    for &byte in bytes {
        buffer = (buffer << 8) | u32::from(byte);
        bits += 8;
        while bits >= 5 {
            let index = (buffer >> (bits - 5)) & 0x1f;
            out.push(ALPHABET[index as usize] as char);
            bits -= 5;
        }
    }

    if bits > 0 {
        let index = (buffer << (5 - bits)) & 0x1f;
        out.push(ALPHABET[index as usize] as char);
    }

    out
}

/// Decodes 52 Crockford base32 characters back into 32 bytes.
fn decode(chars: &[u8]) -> Result<[u8; RECOVERY_KEY_LEN]> {
    let mut out = [0u8; RECOVERY_KEY_LEN];
    let mut buffer: u32 = 0;
    let mut bits: u32 = 0;
    let mut written = 0usize;

    for &ch in chars {
        buffer = (buffer << 5) | u32::from(decode_char(ch)?);
        bits += 5;
        if bits >= 8 {
            if written == RECOVERY_KEY_LEN {
                return Err(SchemaError::InvalidRecoveryKey("too much data"));
            }
            out[written] = (buffer >> (bits - 8)) as u8;
            written += 1;
            bits -= 8;
        }
    }

    if written != RECOVERY_KEY_LEN {
        return Err(SchemaError::InvalidRecoveryKey("truncated"));
    }

    Ok(out)
}

/// Maps one input character to its 5-bit value.
///
/// Applies the Crockford substitutions so a key read off paper survives the
/// usual transcription slips: `O` is a zero, `I` and `L` are ones. `U` is not
/// in the alphabet at all and is rejected rather than guessed at.
fn decode_char(ch: u8) -> Result<u8> {
    let upper = ch.to_ascii_uppercase();
    let folded = match upper {
        b'O' => b'0',
        b'I' | b'L' => b'1',
        other => other,
    };

    ALPHABET
        .iter()
        .position(|c| *c == folded)
        .map(|p| p as u8)
        .ok_or(SchemaError::InvalidRecoveryKey("invalid character"))
}

impl std::fmt::Debug for RecoveryKey {
    /// Never renders key material, so it can't leak into a log or panic message.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("RecoveryKey(<redacted>)")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generated_keys_are_full_length_and_not_all_zero() {
        let key = RecoveryKey::generate();
        assert_eq!(key.as_bytes().len(), RECOVERY_KEY_LEN);
        assert_ne!(key.as_bytes(), &[0u8; RECOVERY_KEY_LEN]);
    }

    #[test]
    fn generated_keys_differ() {
        let a = RecoveryKey::generate();
        let b = RecoveryKey::generate();
        assert_ne!(a.as_bytes(), b.as_bytes(), "two generated keys collided");
    }

    #[test]
    fn encodes_to_thirteen_groups_of_four() {
        let key = RecoveryKey::from_bytes([0x42; RECOVERY_KEY_LEN]);
        let encoded = key.to_grouped_string();
        let groups: Vec<&str> = encoded.split('-').collect();
        assert_eq!(groups.len(), GROUP_COUNT, "expected {GROUP_COUNT} groups");
        assert!(groups.iter().all(|g| g.len() == GROUP_LEN));
        assert_eq!(key.groups().len(), GROUP_COUNT);
    }

    #[test]
    fn encoding_uses_only_crockford_characters() {
        let key = RecoveryKey::generate();
        let encoded = key.to_grouped_string();
        for ch in encoded.chars().filter(|c| *c != '-') {
            assert!(
                ALPHABET.contains(&(ch as u8)),
                "character {ch:?} is outside the Crockford alphabet"
            );
        }
    }

    #[test]
    fn round_trips_through_encoding() {
        let key = RecoveryKey::generate();
        let parsed = RecoveryKey::parse(&key.to_grouped_string()).expect("should parse");
        assert_eq!(key.as_bytes(), parsed.as_bytes());
    }

    #[test]
    fn parsing_tolerates_how_people_actually_type() {
        let key = RecoveryKey::from_bytes([0x9c; RECOVERY_KEY_LEN]);
        let canonical = key.to_grouped_string();

        let lowercase = canonical.to_lowercase();
        let no_dashes = canonical.replace('-', "");
        let spaces = canonical.replace('-', " ");
        let padded = format!("  {canonical}  ");

        for variant in [lowercase, no_dashes, spaces, padded] {
            let parsed = RecoveryKey::parse(&variant)
                .unwrap_or_else(|e| panic!("failed to parse {variant:?}: {e}"));
            assert_eq!(key.as_bytes(), parsed.as_bytes(), "variant {variant:?}");
        }
    }

    #[test]
    fn parsing_folds_ambiguous_characters() {
        // A key whose encoding contains 0 and 1, so the O/0 and I/L/1
        // substitutions have something to act on.
        let key = RecoveryKey::from_bytes([0x00; RECOVERY_KEY_LEN]);
        let canonical = key.to_grouped_string();
        assert!(canonical.contains('0'), "fixture should contain a zero");

        let with_letter_o = canonical.replace('0', "O");
        let parsed = RecoveryKey::parse(&with_letter_o).expect("O should fold to 0");
        assert_eq!(key.as_bytes(), parsed.as_bytes());
    }

    #[test]
    fn rejects_wrong_length() {
        assert!(matches!(
            RecoveryKey::parse("ABCD-EFGH"),
            Err(SchemaError::InvalidRecoveryKey(_))
        ));
    }

    #[test]
    fn rejects_characters_outside_the_alphabet() {
        let key = RecoveryKey::generate();
        let mut bad = key.to_grouped_string();
        bad.replace_range(0..1, "!");
        assert!(matches!(
            RecoveryKey::parse(&bad),
            Err(SchemaError::InvalidRecoveryKey(_))
        ));
    }

    #[test]
    fn debug_never_leaks_key_material() {
        let key = RecoveryKey::from_bytes([0xAB; RECOVERY_KEY_LEN]);
        let rendered = format!("{key:?}");
        assert!(!rendered.contains("AB"), "Debug leaked key bytes");
        assert!(rendered.contains("redacted"));
    }
}
