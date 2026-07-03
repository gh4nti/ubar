use base64::engine::general_purpose::STANDARD as B64;
use base64::Engine;
use chacha20poly1305::aead::{Aead, AeadCore, KeyInit, OsRng};
use chacha20poly1305::{ChaCha20Poly1305, Key, Nonce};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;

#[derive(Clone, Serialize, Deserialize)]
pub struct Credential {
    pub origin: String,
    pub username: String,
    // base64(nonce || ciphertext)
    secret: String,
}

#[derive(Serialize, Deserialize, Default)]
struct VaultFile {
    entries: Vec<Credential>,
    never: Vec<String>,
}

pub struct Vault {
    path: PathBuf,
    key: [u8; 32],
    file: VaultFile,
}

fn data_dir() -> PathBuf {
    dirs::data_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("ubar")
}

// ponytail: key sits next to the vault; protects the file at rest, not
// against an attacker with the same user account. OS keychain if that matters.
fn load_key(path: &PathBuf) -> [u8; 32] {
    if let Ok(bytes) = fs::read(path)
        && bytes.len() == 32
    {
        let mut key = [0u8; 32];
        key.copy_from_slice(&bytes);
        return key;
    }

    let key: [u8; 32] = ChaCha20Poly1305::generate_key(&mut OsRng).into();
    let _ = fs::write(path, key);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(path, fs::Permissions::from_mode(0o600));
    }
    key
}

impl Vault {
    pub fn load() -> Self {
        let dir = data_dir();
        let _ = fs::create_dir_all(&dir);
        let key = load_key(&dir.join("vault.key"));
        let path = dir.join("vault.json");
        let file = fs::read_to_string(&path)
            .ok()
            .and_then(|text| serde_json::from_str(&text).ok())
            .unwrap_or_default();
        Self { path, key, file }
    }

    fn persist(&self) {
        if let Ok(data) = serde_json::to_vec_pretty(&self.file) {
            let tmp = self.path.with_extension("json.tmp");
            let _ = fs::write(&tmp, data);
            let _ = fs::rename(&tmp, &self.path);
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let _ = fs::set_permissions(&self.path, fs::Permissions::from_mode(0o600));
            }
        }
    }

    pub fn entries(&self) -> &[Credential] {
        &self.file.entries
    }

    pub fn is_never(&self, origin: &str) -> bool {
        self.file.never.iter().any(|entry| entry == origin)
    }

    pub fn set_never(&mut self, origin: &str) {
        if !self.is_never(origin) {
            self.file.never.push(origin.to_string());
            self.persist();
        }
    }

    pub fn get(&self, origin: &str) -> Option<(String, String)> {
        let entry = self.file.entries.iter().find(|entry| entry.origin == origin)?;
        let blob = B64.decode(&entry.secret).ok()?;
        if blob.len() < 12 {
            return None;
        }
        let cipher = ChaCha20Poly1305::new(Key::from_slice(&self.key));
        let plain = cipher
            .decrypt(Nonce::from_slice(&blob[..12]), &blob[12..])
            .ok()?;
        Some((entry.username.clone(), String::from_utf8(plain).ok()?))
    }

    pub fn save(&mut self, origin: &str, username: &str, password: &str) {
        let cipher = ChaCha20Poly1305::new(Key::from_slice(&self.key));
        let nonce = ChaCha20Poly1305::generate_nonce(&mut OsRng);
        let Ok(ciphertext) = cipher.encrypt(&nonce, password.as_bytes()) else {
            return;
        };
        let mut blob = nonce.to_vec();
        blob.extend_from_slice(&ciphertext);
        let secret = B64.encode(blob);

        if let Some(entry) = self
            .file
            .entries
            .iter_mut()
            .find(|entry| entry.origin == origin)
        {
            entry.username = username.to_string();
            entry.secret = secret;
        } else {
            self.file.entries.push(Credential {
                origin: origin.to_string(),
                username: username.to_string(),
                secret,
            });
        }
        self.persist();
    }

    pub fn remove(&mut self, origin: &str) -> bool {
        let before = self.file.entries.len();
        self.file.entries.retain(|entry| entry.origin != origin);
        self.file.never.retain(|entry| entry != origin);
        let changed = self.file.entries.len() != before;
        if changed {
            self.persist();
        }
        changed
    }
}
