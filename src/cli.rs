use std::ffi::OsString;
use std::path::Path;

use anyhow::{bail, Context, Result};

use crate::artifacts;
use crate::bootstrap;
use crate::config::{self, Policy};
use crate::gc;
use crate::log;
use crate::state::Store;

pub fn run(args: Vec<OsString>) -> Result<()> {
    let command = args.first().and_then(|s| s.to_str()).unwrap_or("help");
    let rest = &args[1.min(args.len())..];
    match command {
        "bootstrap" => bootstrap_cmd(),
        "status" => status_cmd(),
        "sweep" => sweep_cmd(rest),
        "log" => log_cmd(rest),
        "help" | "--help" | "-h" => {
            print_help();
            Ok(())
        }
        other => bail!("unknown command {other:?}; run `cargo orphan-gc --help`"),
    }
}

/// The policy with deletion forced off, for the surfaces whose whole job is to
/// answer "what would this take?" without taking it.
fn shadow(policy: &Policy) -> Policy {
    Policy { dry_run: true, ..policy.clone() }
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
    // Read the policy back rather than assuming what was written: a manifest
    // that already carried `dry-run = false` keeps it, and telling that
    // operator they are in shadow mode would be exactly the kind of lie this
    // tool cannot afford about deletion.
    let shadow_now = config::discover_from(&cwd)?
        .map(|w| w.policy.mode().is_shadow())
        .unwrap_or(true);
    if shadow_now {
        println!(
            "SHADOW MODE is on (dry-run = true): the tool will learn what it owns and report \
             what it would reclaim, and will delete nothing. Build as usual, then run \
             `cargo orphan-gc status` — when the numbers look right, set dry-run = false to \
             authorize deletion."
        );
    } else {
        println!("dry-run = false: this workspace has already authorized deletion.");
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
    // Deduped across families on purpose — see `artifacts::size_identity`.
    // These totals are read by a human deciding whether to authorize deletion,
    // and the incremental root is recorded once per family while measuring the
    // same bytes every time; summing naively reported 1978 GB for a real 81 GB
    // here.
    let mut current_total = artifacts::SizeTotal::new();
    let mut orphan_total = artifacts::SizeTotal::new();

    for key in store.family_keys()? {
        if let Some(state) = store.load_family(&key)? {
            families += 1;
            if let Some(current) = state.current {
                for artifact in current.artifacts {
                    current_artifacts += 1;
                    current_total.add(&artifact);
                }
            }
            for generation in state.orphans {
                for artifact in generation.artifacts {
                    orphan_artifacts += 1;
                    orphan_total.add(&artifact);
                }
            }
        }
    }
    let current_bytes = current_total.bytes();
    let orphan_bytes = orphan_total.bytes();

    println!("workspace: {}", workspace.root.display());
    println!("enabled: {}", workspace.policy.enabled);
    if workspace.policy.mode().is_shadow() {
        println!("mode: SHADOW — learning and reporting, deleting nothing (dry-run = true)");
    } else {
        println!("mode: deleting (dry-run = false)");
    }
    println!("state: {}", store.root.display());
    println!("log: {}", log::log_path(&store.root).display());
    println!("families: {families}");
    println!("current artifacts: {current_artifacts} ({current_bytes} bytes)");
    // Deliberately NOT called "pending deletion". Most of these records are
    // paths the current generation reuses, which Invariant D forbids deleting —
    // the queue is where supersession puts things, not a promise. Measured on
    // the camp this was dogfooded on: 14,952 queued records / 1.17 GB whose
    // actual reclaimable value was zero. The "would reclaim now" line below is
    // the one that answers the question, because it runs the real sweep.
    println!("orphan records queued: {orphan_artifacts} ({orphan_bytes} bytes, not all reclaimable)");

    // The number the operator actually needs before authorizing deletion, and
    // the one the counts above cannot give: it runs the real sweep's own
    // decision path — leases, current-path domination, per-kind validation —
    // and adds the surplus-session term, which dominates the bytes and is
    // invisible to the orphan queue entirely.
    let reclaimable = gc::sweep_all(&store, &shadow(&workspace.policy))?;
    println!(
        "would reclaim now: {} orphan artifacts ({} bytes) + {} surplus incremental sessions \
         ({} bytes)",
        reclaimable.deleted_artifacts,
        reclaimable.deleted_bytes,
        reclaimable.collected_sessions,
        reclaimable.collected_session_bytes
    );
    if reclaimable.already_gone_artifacts > 0 {
        println!(
            "  {} more orphan records are already gone from disk (stale bookkeeping, not a \
             reclaim)",
            reclaimable.already_gone_artifacts
        );
    }
    if reclaimable.deferred_artifacts > 0 {
        println!(
            "  {} artifacts would be deferred (active build, or a path that fails validation)",
            reclaimable.deferred_artifacts
        );
    }
    if workspace.policy.budget_ceiling().is_some() {
        let plan = gc::budget_sweep(&store, &shadow(&workspace.policy))?;
        if plan.retired_families > 0 {
            println!(
                "  budget would additionally retire {} cold families ({} -> {} bytes)",
                plan.retired_families, plan.bytes_before, plan.bytes_after
            );
        }
    }
    if workspace.policy.mode().is_shadow() && workspace.policy.enabled {
        println!();
        println!(
            "Nothing above has been deleted. When the numbers look right, set dry-run = false \
             in [workspace.metadata.orphan-gc]."
        );
    }
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

fn sweep_cmd(args: &[OsString]) -> Result<()> {
    let mut forced_shadow = false;
    for arg in args {
        match arg.to_str() {
            // Deliberately no `-n` alias: `log -n` takes a line count, and one
            // letter meaning two things in one CLI is how a reader ends up
            // authorizing a sweep they meant to preview.
            Some("--dry-run") => forced_shadow = true,
            other => bail!("unknown flag {:?} for `sweep`", other.unwrap_or("<non-utf8>")),
        }
    }

    let cwd = std::env::current_dir()?;
    let Some(workspace) = config::discover_from(&cwd)? else {
        bail!("no orphan-gc metadata found in this Cargo workspace");
    };
    // `--dry-run` can only ever make the sweep *less* destructive than the
    // policy: there is no flag that authorizes deletion the manifest has not.
    let policy = if forced_shadow { shadow(&workspace.policy) } else { workspace.policy.clone() };
    let mode = policy.mode();

    let store = Store::for_workspace(Path::new(&workspace.root))?;
    let report = gc::sweep_all(&store, &policy)?;
    println!(
        "{} {} orphan artifacts ({} bytes; {} already gone); deferred {} active/unsafe \
         artifacts; {} {} surplus incremental sessions ({} bytes)",
        mode.verb(),
        report.deleted_artifacts,
        report.deleted_bytes,
        report.already_gone_artifacts,
        report.deferred_artifacts,
        if mode.is_shadow() { "would collect" } else { "collected" },
        report.collected_sessions,
        report.collected_session_bytes
    );

    // Budget enforcement runs after the orphan pass, never before: everything
    // already superseded should be reclaimed for free before any *current*
    // family is considered for retirement.
    let budget = gc::budget_sweep(&store, &policy)?;
    if policy.budget_ceiling().is_some() {
        println!(
            "budget: {} -> {} bytes across {} families; {} {} cold families, {} raced",
            budget.bytes_before,
            budget.bytes_after,
            budget.families,
            if mode.is_shadow() { "would retire" } else { "retired" },
            budget.retired_families,
            budget.raced_families
        );
    }
    if mode.is_shadow() {
        println!();
        println!("Nothing was deleted — this sweep ran in shadow mode.");
    }
    Ok(())
}

fn log_cmd(args: &[OsString]) -> Result<()> {
    let mut lines = 50usize;
    let mut args = args.iter();
    while let Some(arg) = args.next() {
        match arg.to_str() {
            Some("-n") | Some("--lines") => {
                let value = args.next().context("-n takes a line count")?;
                lines = value
                    .to_str()
                    .and_then(|s| s.parse().ok())
                    .with_context(|| format!("not a line count: {value:?}"))?;
            }
            other => bail!("unknown flag {:?} for `log`", other.unwrap_or("<non-utf8>")),
        }
    }

    let cwd = std::env::current_dir()?;
    let Some(workspace) = config::discover_from(&cwd)? else {
        bail!("no orphan-gc metadata found in this Cargo workspace");
    };
    let store = Store::for_workspace(&workspace.root)?;
    let path = log::log_path(&store.root);
    let tail = log::tail(&store.root, lines);
    if tail.is_empty() {
        println!("no operational log yet at {}", path.display());
        println!(
            "(the wrapper only logs when verbose = true, or when it has something to report)"
        );
        return Ok(());
    }
    for line in tail {
        println!("{line}");
    }
    Ok(())
}

fn print_help() {
    println!(
        r#"cargo-orphan-gc

USAGE:
    cargo orphan-gc bootstrap
    cargo orphan-gc status
    cargo orphan-gc sweep [--dry-run]
    cargo orphan-gc log [-n LINES]

The same binary is also a rustc wrapper. `bootstrap` installs it as
build.rustc-wrapper in .cargo/config.toml (adopting any existing wrapper such
as sccache as `inner-wrapper`), while the enable/disable policy lives in
Cargo.toml metadata.

A fresh install is in SHADOW MODE (`dry-run = true`): it learns ownership and
reports what it would reclaim, and deletes nothing. Read `status`, then set
`dry-run = false` to authorize deletion.

`log` shows the operational log. The wrapper never writes to the compiler's
stderr — cargo caches each unit's stderr and replays it on later builds, so a
line printed there would describe a build that is no longer running.
"#
    );
}
