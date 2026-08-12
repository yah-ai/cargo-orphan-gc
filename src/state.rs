//! @yah:ticket(R748-S4, "Reconcile mode: relearn ownership after state loss instead of leaking permanently")
//! @yah:at(2026-08-11T07:49:19Z)
//! @yah:kind(spike)
//! @yah:status(review)
//! @yah:handoff("Concluded: reconcile-as-deletion-authority is unsafe by construction, and the conclusion is written up as ARCHITECTURE.md 16. Deciding 'no current fingerprint references this entry => garbage' IS the Invariant B forbidden heuristic, and the reasons are structural: the name derivation is one-way (absence of evidence, not evidence of orphanhood), shared target dirs legitimately hold entries this workspace's view cannot account for, and a convention-keyed reconciler fails OPEN - toward deletion - on the next cargo version. The safe half (rebuilding current ownership from .fingerprint) authorizes deleting nothing and equals what the wrapper relearns on the next compile anyway, so reconcile would buy one generation of delta at the price of a second heuristic deletion authority. Not built; do not build it.")
//! @yah:verify("The leak decomposition that makes state loss survivable without reconcile, also in 16: families whose config still builds relearn with the SAME path set (leak ~0); the dominant byte term (F2's surplus sessions) derives authority from rustc session semantics, not history, so it resumes at full strength on the first post-loss build; only never-rebuilt families leak permanently, and their recovery is the user's own cargo clean, documented as the explicit path.")
//! @yah:assignee(agent:bundle-anthropic-ashguard)
//! @yah:parent(R748)
//! @yah:next("ARCHITECTURE.md §14 accepts that losing or corrupting the metadata under $CARGO_HOME/orphan-gc leaks: the tool stops recognising anything it learned and can only relearn going forward. That is the right SAFETY choice (unknown ownership must never become deletion authority) but it is a poor ADOPTION story — the state dir is disposable by design, sits outside the tree, and is trivially lost to a CARGO_HOME change, a machine move, or a cleanup script. A user who hits it sees the tool quietly stop reclaiming, with a target dir that only grows.\n\nSpike the question: can ownership be re-derived from the tree, safely? Cargo's own layout is highly structured — .fingerprint/<pkg>-<hash>/ names units, deps/ entries carry -<extra-filename> stems, incremental/<crate>-<hash>/ is self-describing. A conservative reconcile could rebuild families it can prove, and refuse the rest. If it cannot be made safe, say so and document recovery as 'delete the state and accept one generation of leak' rather than leaving it implicit.")
//! @yah:assumes("That reconcile can be made safe at all. It may not be: the whole design rests on deleting ONLY what was watched being superseded, and re-deriving ownership from the filesystem is exactly the 'everything old-looking in target/ is garbage' heuristic §4 Invariant B exists to forbid. If the spike concludes that, the conclusion IS the deliverable — do not weaken Invariant B to make reconcile work.")

use std::env;
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use fs2::FileExt;
use serde::{Deserialize, Serialize};

use crate::artifacts::OwnedArtifact;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Generation {
    pub id: String,
    pub created_unix_ms: u128,
    pub artifacts: Vec<OwnedArtifact>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct FamilyState {
    pub schema: u32,
    pub family_key: String,
    pub label: String,
    pub last_used_unix_ms: u128,
    pub current: Option<Generation>,
    pub orphans: Vec<Generation>,
}

impl FamilyState {
    pub fn new(key: &str, label: &str) -> Self {
        Self {
            schema: 1,
            family_key: key.to_string(),
            label: label.to_string(),
            last_used_unix_ms: now_ms(),
            current: None,
            orphans: Vec::new(),
        }
    }
}

#[derive(Clone, Debug)]
pub struct Store {
    pub root: PathBuf,
}

impl Store {
    pub fn for_workspace(workspace_root: &Path) -> Result<Self> {
        let cargo_home = env::var_os("CARGO_HOME")
            .map(PathBuf::from)
            .or_else(|| dirs::home_dir().map(|p| p.join(".cargo")))
            .context("cannot determine CARGO_HOME")?;

        let mut hasher = blake3::Hasher::new();
        hasher.update(workspace_root.to_string_lossy().as_bytes());
        let workspace_id = hasher.finalize().to_hex().to_string();
        let root = cargo_home
            .join("orphan-gc")
            .join("workspaces")
            .join(workspace_id);

        let store = Self { root };
        store.ensure_layout()?;
        Ok(store)
    }

    pub fn ensure_layout(&self) -> Result<()> {
        for child in ["families", "locks", "leases", "pending"] {
            fs::create_dir_all(self.root.join(child))
                .with_context(|| format!("create state directory {child}"))?;
        }
        Ok(())
    }

    pub fn lock_family(&self, key: &str) -> Result<File> {
        let path = self.root.join("locks").join(format!("{key}.lock"));
        let file = OpenOptions::new().create(true).read(true).write(true).open(&path)?;
        file.lock_exclusive()
            .with_context(|| format!("lock family {key}"))?;
        Ok(file)
    }

    pub fn load_family(&self, key: &str) -> Result<Option<FamilyState>> {
        let path = self.family_path(key);
        if !path.exists() {
            return Ok(None);
        }
        let bytes = fs::read(&path).with_context(|| format!("read {}", path.display()))?;
        let state = serde_json::from_slice(&bytes)
            .with_context(|| format!("parse {}", path.display()))?;
        Ok(Some(state))
    }

    pub fn save_family(&self, state: &FamilyState) -> Result<()> {
        let path = self.family_path(&state.family_key);
        let tmp = path.with_extension(format!("json.tmp.{}", std::process::id()));
        let bytes = serde_json::to_vec_pretty(state)?;
        {
            let mut file = File::create(&tmp)?;
            file.write_all(&bytes)?;
            file.sync_all().ok();
        }
        if path.exists() {
            // Metadata is disposable. This Windows-compatible replace is not a
            // transaction; a crash here causes a leak, not unsafe deletion.
            fs::remove_file(&path).ok();
        }
        fs::rename(&tmp, &path)
            .with_context(|| format!("publish family state {}", path.display()))?;
        Ok(())
    }

    pub fn mark_pending(&self, key: &str) -> Result<()> {
        let path = self.root.join("pending").join(key);
        if !path.exists() {
            File::create(path)?;
        }
        Ok(())
    }

    pub fn clear_pending(&self, key: &str) {
        let _ = fs::remove_file(self.root.join("pending").join(key));
    }

    pub fn pending_keys(&self, limit: usize) -> Result<Vec<String>> {
        let mut keys = Vec::new();
        for entry in fs::read_dir(self.root.join("pending"))? {
            let entry = entry?;
            if let Some(name) = entry.file_name().to_str() {
                if name.len() == 64 && name.chars().all(|c| c.is_ascii_hexdigit()) {
                    keys.push(name.to_string());
                    if keys.len() >= limit {
                        break;
                    }
                }
            }
        }
        Ok(keys)
    }

    pub fn family_keys(&self) -> Result<Vec<String>> {
        let mut keys = Vec::new();
        for entry in fs::read_dir(self.root.join("families"))? {
            let entry = entry?;
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if let Some(key) = name.strip_suffix(".json") {
                if key.len() == 64 && key.chars().all(|c| c.is_ascii_hexdigit()) {
                    keys.push(key.to_string());
                }
            }
        }
        Ok(keys)
    }

    pub fn family_path(&self, key: &str) -> PathBuf {
        self.root.join("families").join(format!("{key}.json"))
    }
}

pub fn now_ms() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
}

pub fn generation_id(artifacts: &[OwnedArtifact]) -> String {
    let mut paths = artifacts
        .iter()
        .map(|a| a.path.to_string_lossy().into_owned())
        .collect::<Vec<_>>();
    paths.sort();
    let mut hasher = blake3::Hasher::new();
    for path in paths {
        hasher.update(path.as_bytes());
        hasher.update(&[0]);
    }
    hasher.finalize().to_hex().to_string()
}
