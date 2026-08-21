//! @yah:ticket(R748-B8, "Adoption bugs found by installing on a real camp: relative inner-wrapper path, and env leaking into the test seam")
//! @yah:at(2026-08-12T02:08:50Z)
//! @yah:status(review)
//! @yah:assignee(agent:bundle-anthropic-ashguard)
//! @yah:parent(R748)
//! @yah:severity(high)
//! @yah:handoff("Bug 2: run_in - the pub(crate) test seam - read the inner wrapper from ambient env. Installing the tool on the repo that develops it made the crate's own unit tests chain every fixture rustc through the camp's real sccache, which then rejected the fixture shell script ('Compiler not supported'). Four tests failed with no obvious cause. The env fallback now resolves in run() instead, so run_in is hermetic; production precedence (policy wins, env is fallback) is unchanged.")
//! @yah:handoff("Bug 3: tests/live_cargo.rs leaked three variables into its scratch cargo - CARGO_TARGET_DIR, CARGO_ORPHAN_GC_INNER_WRAPPER, and CARGO_MANIFEST_DIR. The last is the subtle one: the outer cargo test sets it to this crate, and config::discover_for_wrapper prefers it over cwd (deliberately - that is how registry-dep units find the policy), so the fixture build discovered the CAMP's policy including its inner-wrapper. All three are env_remove'd now with the reasoning recorded at the call site.")
//! @yah:handoff("Portability follow-up DONE (was the ticket's own next bullet). Neither committed file names a machine any more. bootstrap now preserves the adopted wrapper's relative form and writes the [env] transport as { value = \".cargo/rustc-wrapper.sh\", relative = true } so CARGO resolves it per-machine - the same mechanism SCCACHE_DIR uses two lines above it; a bare name like sccache is still written plainly, since relative = true on a PATH lookup would resolve it to a nonexistent file. The manifest keeps the relative string and wrapper::run resolves it against the discovered workspace root at spawn.")
//! @yah:verify("Two new tests: bootstrap_never_writes_a_machine_path_into_a_committed_file asserts neither written file contains the workspace path, and a_bare_inner_wrapper_is_written_plainly_without_the_relative_flag pins the PATH-lookup case. 26 tests green, clippy silent.")
//! @yah:verify("Verified live on the camp, both halves of the chain: a workspace unit (cargo check -p yah-board, resolved via the manifest) and a registry dependency (a scratch package pulling cfg-if, resolved via cargo's relative = true). grep '/Users/leif' over .cargo/config.toml and the root Cargo.toml is empty.")
//! @yah:verify("sccache delta-checked rather than read cumulatively: over 5 fresh compile requests 'multiple input files' stayed at 105 (pre-existing, predates this session), misses +1 for the registry dep, incremental +1 for the workspace unit. The chain still hands sccache rustc as argv[1].")
//! @yah:gotcha("Upgrade ordering trap, hit while doing this: the config format changed before the binary that understands it was installed, and the OLD installed binary spawned the relative path verbatim - every compile failed 'No such file or directory', including the cargo install that would have fixed it. Escape hatch is RUSTC_WRAPPER=<the inner shim> cargo install --path ., which takes precedence over build.rustc-wrapper and cuts the tool out of the loop. Worth knowing before any future change to how inner-wrapper is stored.")
//! @yah:handoff("Bug 1 (would have broken the camp on install): bootstrap copied the existing build.rustc-wrapper string verbatim into inner-wrapper. This camp's is the RELATIVE \".cargo/rustc-wrapper.sh\". Cargo resolves that against the parent of the .cargo dir, but this tool spawns the inner wrapper itself and Command::new resolves against the process CWD - which for registry and git dependency units is inside the registry checkout. Every dependency compile would have failed 'no such file'. Final shape after the portability follow-up below: the value stays relative in both files and is resolved per-machine at load.")

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use toml_edit::{value, DocumentMut, InlineTable, Item, Table};

pub struct Bootstrap {
    pub manifest_path: PathBuf,
    pub config_path: PathBuf,
    /// A pre-existing `build.rustc-wrapper` (e.g. sccache) that was moved to
    /// `inner-wrapper` so this tool could take the outer slot.
    pub adopted_inner: Option<String>,
    /// True when an older bootstrap's `rustc-workspace-wrapper` entry was
    /// removed in favour of the outer slot.
    pub migrated_workspace_wrapper: bool,
}

/// True when a wrapper value is a workspace-relative *path* rather than a bare
/// command name to look up on `PATH`.
///
/// This distinction decides how the value must be carried. A bare name
/// (`sccache`) is portable as-is. A relative path (`.cargo/rustc-wrapper.sh`,
/// the shape a repo-local shim takes) is resolved by cargo against the parent
/// of the `.cargo` directory — but this tool spawns the inner wrapper itself,
/// and `Command::new` resolves against the process CWD, which for registry and
/// git dependency units is inside the registry checkout. So a relative path has
/// to be made absolute *somewhere*.
///
/// It must not be made absolute in the files, though: `.cargo/config.toml` and
/// the root `Cargo.toml` are typically committed, and baking one machine's
/// paths into them breaks every other checkout. Both write sites therefore keep
/// the relative string, and resolution happens per-machine at load: cargo's own
/// `relative = true` handles the `[env]` transport, and [`crate::wrapper::run`]
/// resolves the manifest value against the discovered workspace root.
pub(crate) fn is_relative_path_wrapper(value: &str) -> bool {
    let path_shaped = value.contains(std::path::MAIN_SEPARATOR) || value.contains('/');
    path_shaped && Path::new(value).is_relative()
}

pub fn run(start: &Path) -> Result<Bootstrap> {
    let root = find_manifest_dir(start)?;
    let manifest_path = root.join("Cargo.toml");

    let cargo_dir = root.join(".cargo");
    fs::create_dir_all(&cargo_dir)?;
    let config_path = cargo_dir.join("config.toml");

    // Inspect .cargo/config.toml before touching anything. This tool must own
    // the OUTER `rustc-wrapper` slot: cargo nests `rustc-wrapper` outside
    // `rustc-workspace-wrapper`, so a caching wrapper in the outer slot is
    // handed the inner wrapper — not rustc — as argv[1] and silently stops
    // caching every workspace crate (`multiple input files`, visible only in
    // its stats). An existing outer wrapper is therefore adopted as
    // `inner-wrapper` and invoked by this tool with its argv shape intact.
    let text = if config_path.exists() {
        fs::read_to_string(&config_path)
            .with_context(|| format!("read {}", config_path.display()))?
    } else {
        String::new()
    };
    let mut doc = text
        .parse::<DocumentMut>()
        .with_context(|| format!("parse {}", config_path.display()))?;
    let build = ensure_table(doc.as_table_mut(), "build")?;

    let mut migrated_workspace_wrapper = false;
    match build.get("rustc-workspace-wrapper").and_then(Item::as_str) {
        Some("cargo-orphan-gc") => {
            // An earlier bootstrap wrote the inner slot; migrate to the outer.
            build.remove("rustc-workspace-wrapper");
            migrated_workspace_wrapper = true;
        }
        Some(existing) => bail!(
            "{} sets build.rustc-workspace-wrapper={existing:?}; refusing to install alongside \
             it. Cargo would nest cargo-orphan-gc OUTSIDE that wrapper and hand it {existing:?} \
             as the compiler, corrupting family identity. Remove that entry, or chain the tool \
             behind {existing:?} yourself.",
            config_path.display()
        ),
        None => {}
    }

    let adopted_inner = match build.get("rustc-wrapper").and_then(Item::as_str) {
        Some("cargo-orphan-gc") | None => None,
        Some(existing) => Some(existing.to_string()),
    };

    let inner = enable_manifest(&manifest_path, adopted_inner.as_deref())?;

    build["rustc-wrapper"] = value("cargo-orphan-gc");
    if let Some(inner) = &inner {
        // The [env] transport for the inner wrapper. Registry/git dependency
        // units run with cwd and CARGO_MANIFEST_DIR inside the registry
        // checkout, where workspace discovery cannot reach the metadata —
        // cargo's [env] table is the one channel that reaches every rustc it
        // spawns, and without it the chained cache silently stops covering
        // exactly the dependencies it serves best.
        let env_table = ensure_table(doc.as_table_mut(), "env")?;
        if is_relative_path_wrapper(inner) {
            // `relative = true` makes CARGO absolutize this per-machine,
            // against the parent of the .cargo dir — the same mechanism a
            // repo-local SCCACHE_DIR uses. That keeps the committed file
            // portable while still handing the wrapper an absolute path, which
            // is what registry-dependency units need (they run with cwd inside
            // the registry checkout, so a relative spawn would not resolve).
            let mut entry = InlineTable::new();
            entry.insert("value", inner.as_str().into());
            entry.insert("relative", true.into());
            env_table[crate::config::INNER_WRAPPER_ENV] = value(entry);
        } else {
            env_table[crate::config::INNER_WRAPPER_ENV] = value(inner.as_str());
        }
    }
    fs::write(&config_path, doc.to_string())
        .with_context(|| format!("write {}", config_path.display()))?;

    if std::env::var_os("RUSTC_WRAPPER").is_some() {
        eprintln!(
            "cargo-orphan-gc: warning: the RUSTC_WRAPPER environment variable is set and takes \
             precedence over .cargo/config.toml, so the installed wrapper will not run until it \
             is unset."
        );
    }

    Ok(Bootstrap {
        manifest_path,
        config_path,
        adopted_inner,
        migrated_workspace_wrapper,
    })
}

fn find_manifest_dir(start: &Path) -> Result<PathBuf> {
    let start = if start.is_file() {
        start.parent().unwrap_or(start)
    } else {
        start
    };
    for dir in start.ancestors() {
        if dir.join("Cargo.toml").is_file() {
            return Ok(dir.to_path_buf());
        }
    }
    bail!("no Cargo.toml found from {} upward", start.display())
}

/// Returns the effective inner wrapper after the write — adopted, or already
/// present in the metadata — so bootstrap can mirror it into `[env]`.
fn enable_manifest(path: &Path, adopted_inner: Option<&str>) -> Result<Option<String>> {
    let text = fs::read_to_string(path).with_context(|| format!("read {}", path.display()))?;
    let mut doc = text.parse::<DocumentMut>()
        .with_context(|| format!("parse {}", path.display()))?;

    let root = doc.as_table_mut();
    let section_name = if root.contains_key("workspace") { "workspace" } else { "package" };
    let section = ensure_table(root, section_name)?;
    let metadata = ensure_table(section, "metadata")?;
    let gc = ensure_table(metadata, "orphan-gc")?;
    gc["enabled"] = value(true);
    // Shadow is the install default, and writing it explicitly rather than
    // leaning on the serde default is the point: the knob has to be visible in
    // the file the operator is looking at, because turning it off is the second
    // half of adopting this tool. An existing `dry-run = false` is a decision
    // someone already made — never revert it.
    if !gc.contains_key("dry-run") {
        gc["dry-run"] = value(true);
    }
    if !gc.contains_key("pending-sweeps-per-compile") {
        gc["pending-sweeps-per-compile"] = value(4);
    }
    if let Some(inner) = adopted_inner {
        match gc.get("inner-wrapper").and_then(Item::as_str) {
            None => {
                gc["inner-wrapper"] = value(inner);
            }
            Some(existing) if existing == inner => {}
            Some(existing) => bail!(
                "{} already sets inner-wrapper={existing:?} but .cargo/config.toml has \
                 build.rustc-wrapper={inner:?}; resolve the conflict by hand before \
                 bootstrapping.",
                path.display()
            ),
        }
    }
    let effective_inner = gc
        .get("inner-wrapper")
        .and_then(Item::as_str)
        .map(str::to_string);

    fs::write(path, doc.to_string()).with_context(|| format!("write {}", path.display()))?;
    Ok(effective_inner)
}

fn ensure_table<'a>(parent: &'a mut Table, key: &str) -> Result<&'a mut Table> {
    if !parent.contains_key(key) {
        parent.insert(key, Item::Table(Table::new()));
    }
    parent
        .get_mut(key)
        .and_then(Item::as_table_mut)
        .with_context(|| format!("TOML key {key:?} exists but is not a table"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_bare_wrapper_name_is_not_treated_as_a_path() {
        assert!(!is_relative_path_wrapper("sccache"));
    }

    #[test]
    fn an_absolute_wrapper_path_needs_no_resolution() {
        assert!(!is_relative_path_wrapper("/opt/bin/sccache"));
    }

    #[test]
    fn a_repo_local_shim_is_a_relative_path() {
        // Exactly the yah camp's shape, and the one that needs per-machine
        // resolution: registry-dependency units run with cwd inside the
        // registry checkout, so spawning this relative would not resolve.
        assert!(is_relative_path_wrapper(".cargo/rustc-wrapper.sh"));
        assert!(is_relative_path_wrapper("tools/wrap.sh"));
    }

    #[test]
    fn bootstrap_never_writes_a_machine_path_into_a_committed_file() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        fs::create_dir_all(root.join(".cargo")).unwrap();
        fs::write(root.join(".cargo/rustc-wrapper.sh"), "#!/bin/sh\nexec sccache \"$@\"\n").unwrap();
        fs::write(
            root.join("Cargo.toml"),
            "[workspace]\nmembers = []\nresolver = \"2\"\n",
        )
        .unwrap();
        fs::write(
            root.join(".cargo/config.toml"),
            "[build]\nrustc-wrapper = \".cargo/rustc-wrapper.sh\"\n",
        )
        .unwrap();

        let outcome = run(root).unwrap();
        let adopted = outcome.adopted_inner.expect("the existing wrapper is adopted");
        assert_eq!(adopted, ".cargo/rustc-wrapper.sh", "the relative form is preserved");

        let manifest = fs::read_to_string(root.join("Cargo.toml")).unwrap();
        let config = fs::read_to_string(root.join(".cargo/config.toml")).unwrap();

        // The whole point: neither committed file may name this machine.
        let machine = root.to_string_lossy().into_owned();
        assert!(!manifest.contains(&machine), "manifest leaked a machine path:\n{manifest}");
        assert!(!config.contains(&machine), "config leaked a machine path:\n{config}");

        // The manifest keeps it relative (resolved against workspace.root at
        // spawn); the [env] transport delegates resolution to cargo.
        assert!(
            manifest.contains("inner-wrapper = \".cargo/rustc-wrapper.sh\""),
            "{manifest}"
        );
        assert!(config.contains("relative = true"), "{config}");
        assert!(config.contains(".cargo/rustc-wrapper.sh"), "{config}");
        assert!(config.contains("rustc-wrapper = \"cargo-orphan-gc\""), "{config}");
    }

    /// R748-F6 — bootstrap installs a tool that deletes nothing yet. The flag
    /// is written explicitly rather than left to the serde default, because the
    /// operator has to be able to *see* the knob they will later turn off; and
    /// an existing decision to delete is never reverted.
    #[test]
    fn bootstrap_installs_in_shadow_mode_and_never_reverts_an_authorized_one() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        fs::write(root.join("Cargo.toml"), "[workspace]\nmembers = []\n").unwrap();

        run(root).unwrap();
        let manifest = fs::read_to_string(root.join("Cargo.toml")).unwrap();
        assert!(manifest.contains("dry-run = true"), "{manifest}");

        // Someone authorizes deletion, then re-bootstraps (an upgrade, a second
        // `bootstrap` run). Their decision stands.
        let authorized = manifest.replace("dry-run = true", "dry-run = false");
        fs::write(root.join("Cargo.toml"), &authorized).unwrap();
        run(root).unwrap();
        let after = fs::read_to_string(root.join("Cargo.toml")).unwrap();
        assert!(after.contains("dry-run = false"), "{after}");
    }

    #[test]
    fn a_bare_inner_wrapper_is_written_plainly_without_the_relative_flag() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        fs::create_dir_all(root.join(".cargo")).unwrap();
        fs::write(root.join("Cargo.toml"), "[workspace]\nmembers = []\n").unwrap();
        fs::write(
            root.join(".cargo/config.toml"),
            "[build]\nrustc-wrapper = \"sccache\"\n",
        )
        .unwrap();

        run(root).unwrap();

        let config = fs::read_to_string(root.join(".cargo/config.toml")).unwrap();
        // `relative = true` on a PATH lookup would make cargo resolve "sccache"
        // into a nonexistent file next to the workspace root.
        assert!(!config.contains("relative = true"), "{config}");
        assert!(
            config.contains("CARGO_ORPHAN_GC_INNER_WRAPPER = \"sccache\""),
            "{config}"
        );
    }
}
