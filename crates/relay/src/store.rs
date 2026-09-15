//! JSON state file: allowed users, agent token hashes, thread bindings (spec 11).

use std::collections::BTreeMap;
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq;

#[derive(Serialize, Deserialize, Default, Debug, Clone, PartialEq)]
pub struct Store {
    #[serde(default)]
    pub users: Vec<String>,
    #[serde(default)]
    pub tokens: Vec<TokenRecord>,
    #[serde(default)]
    pub bindings: BTreeMap<String, Binding>,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct TokenRecord {
    pub sha256: String,
    pub label: String,
    pub created: String,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct Binding {
    pub session: String,
    pub user: String,
    #[serde(default)]
    pub created: i64,
}

impl Store {
    /// Reads the state file. A missing file yields an empty store.
    pub fn load(path: &Path) -> io::Result<Store> {
        match fs::read(path) {
            Ok(bytes) => serde_json::from_slice(&bytes)
                .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e)),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(Store::default()),
            Err(e) => Err(e),
        }
    }

    /// Writes `{path}.tmp` (mode 0600 on Unix) and renames it over `path`.
    pub fn save(&self, path: &Path) -> io::Result<()> {
        let bytes = serde_json::to_vec_pretty(self)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
        let mut tmp = path.as_os_str().to_owned();
        tmp.push(".tmp");
        let tmp = PathBuf::from(tmp);

        let mut options = fs::OpenOptions::new();
        options.write(true).create(true).truncate(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options.open(&tmp)?;
        file.write_all(&bytes)?;
        file.sync_all()?;
        drop(file);
        fs::rename(&tmp, path)
    }

    /// True when SHA-256(raw) matches a stored token hash.
    pub fn token_ok(&self, raw: &str) -> bool {
        let digest = hex::encode(Sha256::digest(raw.as_bytes()));
        self.tokens
            .iter()
            .any(|t| bool::from(t.sha256.as_bytes().ct_eq(digest.as_bytes())))
    }
}

#[cfg(test)]
mod tests;
