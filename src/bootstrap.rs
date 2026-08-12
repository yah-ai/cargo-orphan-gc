use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use toml_edit::{value, DocumentMut, Item, Table};

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
        env_table[crate::config::INNER_WRAPPER_ENV] = value(inner.as_str());
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
