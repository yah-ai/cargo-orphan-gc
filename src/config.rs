use std::env;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::Deserialize;

/// What `max-bytes` is allowed to authorize.
///
/// Orphan GC alone cannot enforce a ceiling: if four current families occupy
/// 80 GiB each and none has been superseded, no orphan-only policy can get
/// under 200 GiB without deleting a *current* family. That is a different
/// authority, and ARCHITECTURE.md §10 is explicit that it must never be
/// smuggled into orphan GC silently — hence an opt-in mode rather than a
/// behaviour change to `max-bytes`.
#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum BudgetMode {
    /// `max-bytes` is a warning watermark. Only superseded generations are
    /// ever deleted. The default, and the only mode with the strong
    /// no-current-deletion invariant.
    #[default]
    OrphanOnly,
    /// `max-bytes` is a ceiling. When the total exceeds it, the least
    /// recently used *current* families are retired oldest-first until it
    /// fits.
    ///
    /// This buys a hard bound at the cost of Invariant A: a retired family
    /// was not superseded, so rebuilding it costs a cold compile. LRU is what
    /// keeps that cost off the hot path — a family in active use is touched
    /// on every build that needs it and so is never the coldest.
    LruCurrentFamilies,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(default, rename_all = "kebab-case")]
pub struct Policy {
    pub enabled: bool,
    pub verbose: bool,
    pub pending_sweeps_per_compile: usize,
    pub max_bytes: Option<u64>,
    pub budget_mode: BudgetMode,
    /// A compiler wrapper (e.g. `sccache`) this tool invokes *around* rustc.
    ///
    /// This tool must own Cargo's outer `build.rustc-wrapper` slot: when it
    /// instead nests inside another wrapper via `rustc-workspace-wrapper`,
    /// that wrapper receives this binary — not rustc — as its first argument,
    /// and a caching wrapper like sccache silently stops caching every
    /// wrapped crate (`multiple input files`). Chaining the cache *inside*
    /// keeps its argv shape intact: it is handed rustc as argv[1] exactly as
    /// if this tool were not present.
    pub inner_wrapper: Option<String>,
}

impl Default for Policy {
    fn default() -> Self {
        Self {
            enabled: false,
            verbose: false,
            pending_sweeps_per_compile: 4,
            max_bytes: None,
            budget_mode: BudgetMode::OrphanOnly,
            inner_wrapper: None,
        }
    }
}

impl Policy {
    /// The ceiling, when one is both configured and authorized. `None` in
    /// orphan-only mode even if `max-bytes` is set — that combination is a
    /// watermark, not a budget.
    pub fn budget_ceiling(&self) -> Option<u64> {
        match self.budget_mode {
            BudgetMode::OrphanOnly => None,
            BudgetMode::LruCurrentFamilies => self.max_bytes,
        }
    }
}

/// The env-var transport for `inner-wrapper`, written into `[env]` by
/// bootstrap. Needed because workspace discovery is impossible for registry
/// and git dependency units: cargo runs their rustc with both
/// `CARGO_MANIFEST_DIR` *and* the working directory inside the registry
/// checkout, so no ancestor walk from either can reach the workspace's
/// metadata. Cargo's `[env]` table applies to every rustc it spawns,
/// which makes it the one channel that reaches those units — and losing the
/// chained cache for exactly the dependencies it serves best would be the
/// kind of silent failure this tool exists to avoid.
pub const INNER_WRAPPER_ENV: &str = "CARGO_ORPHAN_GC_INNER_WRAPPER";

pub fn inner_wrapper_from_env() -> Option<String> {
    env::var(INNER_WRAPPER_ENV).ok().filter(|s| !s.is_empty())
}

#[derive(Clone, Debug)]
pub struct WorkspaceConfig {
    pub root: PathBuf,
    pub manifest_path: PathBuf,
    pub policy: Policy,
}

pub fn discover_for_wrapper() -> Result<Option<WorkspaceConfig>> {
    // CARGO_MANIFEST_DIR points at the package being compiled. For registry
    // and git dependencies that is inside $CARGO_HOME, far from the workspace
    // — but those units still need the policy (specifically `inner-wrapper`:
    // dropping the chained cache for exactly the dependencies it serves would
    // be the silent-failure this tool exists to avoid). Cargo runs rustc from
    // the workspace root, so fall back to the current directory.
    if let Some(dir) = env::var_os("CARGO_MANIFEST_DIR").map(PathBuf::from) {
        if let Some(found) = discover_from(&dir)? {
            return Ok(Some(found));
        }
    }
    discover_from(&env::current_dir().context("get current directory")?)
}

pub fn discover_from(start: &Path) -> Result<Option<WorkspaceConfig>> {
    let start = if start.is_file() {
        start.parent().unwrap_or(start)
    } else {
        start
    };

    let mut package_fallback = None;

    for dir in start.ancestors() {
        let manifest = dir.join("Cargo.toml");
        if !manifest.is_file() {
            continue;
        }

        let text = fs::read_to_string(&manifest)
            .with_context(|| format!("read {}", manifest.display()))?;
        let value: toml::Value = toml::from_str(&text)
            .with_context(|| format!("parse {}", manifest.display()))?;

        if let Some(gc) = value
            .get("workspace")
            .and_then(|v| v.get("metadata"))
            .and_then(|v| v.get("orphan-gc"))
        {
            let policy: Policy = gc
                .clone()
                .try_into()
                .context("parse [workspace.metadata.orphan-gc]")?;
            return Ok(Some(WorkspaceConfig {
                root: dir.to_path_buf(),
                manifest_path: manifest,
                policy,
            }));
        }

        if package_fallback.is_none() {
            if let Some(gc) = value
                .get("package")
                .and_then(|v| v.get("metadata"))
                .and_then(|v| v.get("orphan-gc"))
            {
                let policy: Policy = gc
                    .clone()
                    .try_into()
                    .context("parse [package.metadata.orphan-gc]")?;
                package_fallback = Some(WorkspaceConfig {
                    root: dir.to_path_buf(),
                    manifest_path: manifest.clone(),
                    policy,
                });
            }
        }
    }

    Ok(package_fallback)
}
