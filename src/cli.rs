use std::ffi::OsString;
use std::path::Path;

use anyhow::{bail, Result};

use crate::artifacts;
use crate::bootstrap;
use crate::config;
use crate::gc;
use crate::state::Store;

pub fn run(args: Vec<OsString>) -> Result<()> {
    let command = args.first().and_then(|s| s.to_str()).unwrap_or("help");
    match command {
        "bootstrap" => bootstrap_cmd(),
        "status" => status_cmd(),
        "sweep" => sweep_cmd(),
        "help" | "--help" | "-h" => {
            print_help();
            Ok(())
        }
        other => bail!("unknown command {other:?}; run `cargo orphan-gc --help`"),
    }
}

fn bootstrap_cmd() -> Result<()> {
    let cwd = std::env::current_dir()?;
    let outcome = bootstrap::run(&cwd)?;
    println!("Enabled cargo-orphan-gc in:");
    println!("  {}", outcome.manifest_path.display());
    println!("  {}", outcome.config_path.display());
    if let Some(inner) = &outcome.adopted_inner {
        println!();
        println!(
            "Adopted existing rustc-wrapper {inner:?} as inner-wrapper: cargo-orphan-gc now \
             holds the outer slot and invokes {inner} around rustc itself, so its cache keeps \
             seeing rustc as argv[1]."
        );
    }
    if outcome.migrated_workspace_wrapper {
        println!();
        println!(
            "Migrated from rustc-workspace-wrapper to rustc-wrapper (the outer slot); the old \
             entry was removed."
        );
    }
    println!();
    println!("Continue using normal Cargo commands: cargo build, cargo check, cargo test, ...");
    Ok(())
}

fn status_cmd() -> Result<()> {
    let cwd = std::env::current_dir()?;
    let Some(workspace) = config::discover_from(&cwd)? else {
        bail!("no [workspace.metadata.orphan-gc] or [package.metadata.orphan-gc] found");
    };
    let store = Store::for_workspace(&workspace.root)?;

    let mut families = 0usize;
    let mut current_artifacts = 0usize;
    let mut orphan_artifacts = 0usize;
    let mut current_bytes = 0u64;
    let mut orphan_bytes = 0u64;

    for key in store.family_keys()? {
        if let Some(state) = store.load_family(&key)? {
            families += 1;
            if let Some(current) = state.current {
                for artifact in current.artifacts {
                    current_artifacts += 1;
                    current_bytes = current_bytes.saturating_add(artifacts::path_size(&artifact.path).unwrap_or(0));
                }
            }
            for generation in state.orphans {
                for artifact in generation.artifacts {
                    orphan_artifacts += 1;
                    orphan_bytes = orphan_bytes.saturating_add(artifacts::path_size(&artifact.path).unwrap_or(0));
                }
            }
        }
    }

    println!("workspace: {}", workspace.root.display());
    println!("enabled: {}", workspace.policy.enabled);
    println!("state: {}", store.root.display());
    println!("families: {families}");
    println!("current artifacts: {current_artifacts} ({current_bytes} bytes)");
    println!("orphan artifacts pending deletion: {orphan_artifacts} ({orphan_bytes} bytes)");
    match (workspace.policy.max_bytes, workspace.policy.budget_ceiling()) {
        (Some(max), Some(_)) => {
            println!("max-bytes ceiling: {max} bytes (budget-mode = lru-current-families)");
            if current_bytes > max {
                println!(
                    "  OVER by {} bytes — next sweep retires coldest current families",
                    current_bytes - max
                );
            }
        }
        (Some(max), None) => {
            println!("max-bytes watermark: {max} bytes (warning-only in orphan-only mode)");
            if current_bytes > max {
                println!(
                    "  OVER by {} bytes — orphan-only mode cannot reclaim this; set \
                     budget-mode = \"lru-current-families\" to authorize it",
                    current_bytes - max
                );
            }
        }
        (None, _) => {}
    }
    Ok(())
}

fn sweep_cmd() -> Result<()> {
    let cwd = std::env::current_dir()?;
    let Some(workspace) = config::discover_from(&cwd)? else {
        bail!("no orphan-gc metadata found in this Cargo workspace");
    };
    let store = Store::for_workspace(Path::new(&workspace.root))?;
    let report = gc::sweep_all(&store, &workspace.policy)?;
    println!(
        "deleted {} orphan artifacts ({} bytes); deferred {} active/unsafe artifacts; collected \
         {} surplus incremental sessions ({} bytes)",
        report.deleted_artifacts,
        report.deleted_bytes,
        report.deferred_artifacts,
        report.collected_sessions,
        report.collected_session_bytes
    );

    // Budget enforcement runs after the orphan pass, never before: everything
    // already superseded should be reclaimed for free before any *current*
    // family is considered for retirement.
    let budget = gc::budget_sweep(&store, &workspace.policy)?;
    if workspace.policy.budget_ceiling().is_some() {
        println!(
            "budget: {} -> {} bytes across {} families; retired {} cold families, {} raced",
            budget.bytes_before,
            budget.bytes_after,
            budget.families,
            budget.retired_families,
            budget.raced_families
        );
    }
    Ok(())
}

fn print_help() {
    println!(
        r#"cargo-orphan-gc

USAGE:
    cargo orphan-gc bootstrap
    cargo orphan-gc status
    cargo orphan-gc sweep

The same binary is also a rustc wrapper. `bootstrap` installs it as
build.rustc-wrapper in .cargo/config.toml (adopting any existing wrapper such
as sccache as `inner-wrapper`), while the enable/disable policy lives in
Cargo.toml metadata.
"#
    );
}
