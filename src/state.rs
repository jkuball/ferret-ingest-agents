//! What earlier runs sent, so unchanged files and documents are skipped and
//! documents a session no longer produces can be deleted.

use std::collections::BTreeMap;
use std::path::Path;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

#[derive(Default, Serialize, Deserialize)]
pub struct State {
    /// What the documents were rendered with: version, options and templates.
    /// When it changes, every file is rendered again; unchanged documents
    /// still aren't sent.
    #[serde(default)]
    pub renderer: Option<String>,
    pub files: BTreeMap<String, FileState>,
}

#[derive(Default, Clone, Serialize, Deserialize)]
pub struct FileState {
    /// The size and modification time the file had when it was fully sent.
    /// None makes the next run read it again.
    pub seen: Option<(u64, i128)>,
    /// Document external ids and the hash of what was sent for each.
    pub docs: BTreeMap<String, String>,
}

impl State {
    pub fn load(path: &Path) -> Result<State> {
        match std::fs::read_to_string(path) {
            Ok(text) => {
                serde_json::from_str(&text).with_context(|| format!("parsing {}", path.display()))
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(State::default()),
            Err(error) => Err(error).with_context(|| format!("reading {}", path.display())),
        }
    }

    /// Written to a temporary file first, so a crash never leaves half a state.
    pub fn save(&self, path: &Path) -> Result<()> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
        }
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, serde_json::to_vec(self)?)
            .with_context(|| format!("writing {}", tmp.display()))?;
        std::fs::rename(&tmp, path).with_context(|| format!("replacing {}", path.display()))?;
        Ok(())
    }
}

pub fn hash(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}
