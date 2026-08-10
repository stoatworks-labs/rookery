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

use aes_gcm::aead::{Aead, AeadCore, KeyInit, OsRng};
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
            *Key::<Aes256Gcm>::from_slice(&bytes)
        } else {
            let key = Aes256Gcm::generate_key(OsRng);
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
        let nonce = Aes256Gcm::generate_nonce(OsRng);
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
        let plaintext = self
            .cipher
            .decrypt(Nonce::from_slice(&nonce_bytes), ciphertext.as_ref())
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
    (0..s.len())
        .step_by(2)
        .map(|i| {
            u8::from_str_radix(&s[i..i + 2], 16)
                .map_err(|e| anyhow::anyhow!("invalid hex string: {e}"))
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
