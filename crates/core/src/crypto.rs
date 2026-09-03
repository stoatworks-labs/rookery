//! Encrypts instance credentials before they touch disk.
//!
//! A WebLinked `--token` is not a login password, but it is the only thing
//! standing between a network and full control of what is on air, and
//! `registry.json` is exactly the sort of file that ends up in a backup, a
//! synced folder or a support bundle. The key lives in its own file next to
//! the registry, generated on first run, and never leaves this process (the
//! API layer separately redacts tokens in responses - see
//! `Instance::redacted`; this is the distinct concern of what sits on disk).
//!
//! Adapted from flock's `crates/core/src/crypto.rs`, same scheme.

use aes_gcm::aead::{Aead, AeadCore, Generate, KeyInit};
use aes_gcm::{Aes256Gcm, Key, Nonce};
use std::path::Path;

const NONCE_LEN: usize = 12;
/// Every encrypted value is stored with this prefix so decryption can tell
/// it apart from a token an operator typed straight into `registry.json` by
/// hand - no real WebLinked token will legitimately start with it.
const PREFIX: &str = "rookery-enc-v1:";

pub(crate) struct CredentialCipher {
    cipher: Aes256Gcm,
}

impl CredentialCipher {
    /// Loads the key from `key_path`, generating and persisting a new random
    /// one on first run. The file is chmod 600 on unix - it's the only thing
    /// standing between `registry.json` and every instance's plaintext
    /// token.
    pub(crate) fn load_or_create(key_path: &Path) -> anyhow::Result<Self> {
        let key = if key_path.exists() {
            let hex = std::fs::read_to_string(key_path)?;
            let bytes = decode_hex(hex.trim())?;
            anyhow::ensure!(
                bytes.len() == 32,
                "{} does not contain a valid 32-byte key (found {} bytes) - delete it to \
                 generate a new one, but note this makes existing stored tokens unreadable",
                key_path.display(),
                bytes.len()
            );
            Key::<Aes256Gcm>::try_from(bytes.as_slice())
                .map_err(|_| anyhow::anyhow!("{} is not a valid 32-byte key", key_path.display()))?
        } else {
            let key = Key::<Aes256Gcm>::generate();
            if let Some(parent) = key_path.parent() {
                std::fs::create_dir_all(parent)?;
            }
            std::fs::write(key_path, encode_hex(&key))?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                std::fs::set_permissions(key_path, std::fs::Permissions::from_mode(0o600))?;
            }
            key
        };
        Ok(Self {
            cipher: Aes256Gcm::new(&key),
        })
    }

    /// Encrypts `plaintext` into a self-describing string safe to embed in
    /// JSON.
    pub(crate) fn encrypt(&self, plaintext: &str) -> anyhow::Result<String> {
        let nonce = Nonce::<<Aes256Gcm as AeadCore>::NonceSize>::generate();
        let ciphertext = self
            .cipher
            .encrypt(&nonce, plaintext.as_bytes())
            .map_err(|e| anyhow::anyhow!("failed to encrypt token: {e}"))?;
        Ok(format!(
            "{PREFIX}{}:{}",
            encode_hex(&nonce),
            encode_hex(&ciphertext)
        ))
    }

    /// Decrypts a value produced by `encrypt`. Passes through anything
    /// without the `rookery-enc-v1:` prefix unchanged, which is what lets an
    /// operator hand-write `registry.json` with a plaintext token and have it
    /// work: it loads as typed, and the very next save encrypts it going
    /// forward. Refusing it instead would mean the only way to seed a fleet
    /// from a config-management system is through the UI, one instance at a
    /// time.
    pub(crate) fn decrypt_or_pass_through(&self, stored: &str) -> anyhow::Result<String> {
        let Some(rest) = stored.strip_prefix(PREFIX) else {
            return Ok(stored.to_string());
        };
        let (nonce_hex, ct_hex) = rest
            .split_once(':')
            .ok_or_else(|| anyhow::anyhow!("malformed encrypted token"))?;
        let nonce_bytes = decode_hex(nonce_hex)?;
        anyhow::ensure!(
            nonce_bytes.len() == NONCE_LEN,
            "malformed encrypted token: wrong nonce length"
        );
        let ciphertext = decode_hex(ct_hex)?;
        let nonce = Nonce::try_from(nonce_bytes.as_slice())
            .map_err(|_| anyhow::anyhow!("malformed encrypted token: wrong nonce length"))?;
        let plaintext = self
            .cipher
            .decrypt(&nonce, ciphertext.as_ref())
            .map_err(|_| {
                anyhow::anyhow!(
                    "failed to decrypt a stored token - wrong or missing credentials.key?"
                )
            })?;
        Ok(String::from_utf8(plaintext)?)
    }
}

fn encode_hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn decode_hex(s: &str) -> anyhow::Result<Vec<u8>> {
    anyhow::ensure!(s.len().is_multiple_of(2), "invalid hex string (odd length)");
    // Over BYTES, not over &str slices. `&s[i..i + 2]` panics outright when a
    // multibyte character starts at an even offset ("byte index N is not a char
    // boundary"), and a hand-edited credentials.key or a token pasted into
    // registry.json by hand is exactly where that happens — pre-empting the
    // clear "delete it and restart" error this is supposed to return.
    s.as_bytes()
        .as_chunks::<2>()
        .0
        .iter()
        .map(|pair| {
            let hex = std::str::from_utf8(pair)
                .map_err(|_| anyhow::anyhow!("invalid hex string: not ASCII"))?;
            u8::from_str_radix(hex, 16).map_err(|e| anyhow::anyhow!("invalid hex string: {e}"))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cipher_at(dir: &Path) -> CredentialCipher {
        CredentialCipher::load_or_create(&dir.join("credentials.key")).unwrap()
    }

    fn tempdir() -> std::path::PathBuf {
        let dir =
            std::env::temp_dir().join(format!("rookery-crypto-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn round_trips_a_token() {
        let cipher = cipher_at(&tempdir());
        let encrypted = cipher.encrypt("show-token").unwrap();
        assert_ne!(encrypted, "show-token");
        assert!(encrypted.starts_with(PREFIX));
        assert_eq!(
            cipher.decrypt_or_pass_through(&encrypted).unwrap(),
            "show-token"
        );
    }

    #[test]
    fn passes_through_a_hand_written_plaintext_token_unchanged() {
        let cipher = cipher_at(&tempdir());
        assert_eq!(
            cipher.decrypt_or_pass_through("show-token").unwrap(),
            "show-token"
        );
    }

    #[test]
    fn a_multibyte_character_is_an_error_not_a_panic() {
        // A hand-edited credentials.key or a token pasted into registry.json is
        // the realistic source. Slicing &str by byte offset used to panic here
        // ("byte index 2 is not a char boundary"), which killed the process
        // before load_or_create could tell the operator to delete the key.
        // The cases that matter are EVEN byte length — an odd one is caught by
        // the ensure! above before any slicing happens. These get past it and
        // then cut a multibyte character in half.
        assert!(decode_hex("ab€x").is_err()); // 6 bytes; [2..4] splits the €
        assert!(decode_hex("€€").is_err()); // 6 bytes; [0..2] splits the first €
        assert!(decode_hex("zz").is_err()); // still rejects plain non-hex
        assert!(decode_hex("abc").is_err()); // still rejects odd length
        assert_eq!(decode_hex("00ff").unwrap(), vec![0x00, 0xff]);
    }

    #[test]
    fn a_multibyte_token_is_refused_rather_than_crashing_the_process() {
        let cipher = cipher_at(&tempdir());
        let token = format!("{PREFIX}ab€x");
        assert!(cipher.decrypt_or_pass_through(&token).is_err());
    }

    #[test]
    fn key_file_persists_across_reload() {
        let dir = tempdir();
        let key_path = dir.join("credentials.key");
        let encrypted = CredentialCipher::load_or_create(&key_path)
            .unwrap()
            .encrypt("secret")
            .unwrap();
        let reloaded = CredentialCipher::load_or_create(&key_path).unwrap();
        assert_eq!(
            reloaded.decrypt_or_pass_through(&encrypted).unwrap(),
            "secret"
        );
    }
}
