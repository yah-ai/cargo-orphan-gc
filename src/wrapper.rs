//! @yah:ticket(R748-F1, "Own rustc-wrapper and chain the inner wrapper, instead of nesting under it")
//! @yah:at(2026-08-11T07:48:43Z)
//! @yah:status(review)
//! @yah:handoff("Landed. bootstrap now writes rustc-wrapper = cargo-orphan-gc (outer slot), ADOPTS an existing rustc-wrapper as inner-wrapper in workspace metadata, migrates old rustc-workspace-wrapper installs, refuses a foreign rustc-workspace-wrapper (nesting would hand us that wrapper as the compiler), and warns when a RUSTC_WRAPPER env var would shadow the config. run_in chains the inner wrapper so it receives rustc as argv[1]; non-workspace units (crate root not a .rs under the workspace) pass through with zero bookkeeping.")
//! @yah:verify("A/B/C via scripts/wrapper-chain-ab.sh, 2026-08-11, private sccache server: A nested = 'multiple input files 2'; B shell-chain = crate-type 1 / incremental 1 only; C real binary outer + inner-wrapper=sccache = identical clean stats to B PLUS families: 2 / 15 artifacts recorded. The fix works and the tool still does its job in the same run.")
//! @yah:verify("At scale (kamaji copy, ~60 registry deps): cold build through the chain = 135 sccache misses / 0 'multiple input files' / 39 non-cacheable (the legitimate bin+incremental refusals). Before the [env] fix the same build showed 0 hits 0 misses - sccache silently absent.")
//! @yah:gotcha("Registry/git dependency units run with BOTH cwd and CARGO_MANIFEST_DIR inside the registry checkout, so workspace discovery structurally cannot reach the metadata for them - and they are exactly the units sccache serves. inner-wrapper therefore travels via cargo's [env] table (CARGO_ORPHAN_GC_INNER_WRAPPER, written by bootstrap); metadata wins when both are visible. Measured before the fix: kamaji cold build, 0 sccache requests from dep compiles.")
//! @yah:assignee(agent:bundle-anthropic-ashguard)
//! @yah:parent(R748)
//! @yah:next("Diagnosis (verified with shell stand-ins, /tmp/wrapchain/test.sh): cargo nests rustc-wrapper OUTSIDE rustc-workspace-wrapper, so sccache is handed argv[1] = the inner wrapper binary rather than rustc. It cannot recognise that as a compiler, falls off its rust-aware path, and reports 'multiple input files'. Reproduced with a PURE PASSTHROUGH inner wrapper, so the cause is cargo's nesting, not this tool.\n\nFix, measured to work: take the OUTER slot. bootstrap writes rustc-wrapper = cargo-orphan-gc (not rustc-workspace-wrapper), and the wrapper invokes a configured inner wrapper itself so sccache receives rustc as argv[1].\n\nA/B: nested => 'multiple input files' 2. Inverted => that reason GONE, leaving only sccache's legitimate refusals (crate-type 1, incremental 1).\n\nCost of the outer slot: rustc-wrapper applies to ALL units including registry deps, where rustc-workspace-wrapper applied only to workspace members. The wrapper must therefore cheaply identify non-workspace units and pass them straight through to the inner wrapper without doing family bookkeeping.")
//! @yah:verify("Reproduce the A/B in a scratch workspace containing one bin and one path-dep lib. A: rustc-wrapper = sccache + rustc-workspace-wrapper = <passthrough> must show 'multiple input files' in sccache --show-stats. B: rustc-wrapper = <tool that execs sccache \"$@\"> must NOT show it, leaving only crate-type / incremental. Zero stats between runs; the failure is invisible without reading them.")
//! @yah:gotcha("bootstrap must REFUSE to write rustc-workspace-wrapper when rustc-wrapper is already set by someone else, rather than silently producing the nested arrangement — the resulting cache collapse throws no error and shows up only in sccache stats. ARCHITECTURE.md §12 previously claimed the two slots compose; corrected in place with the measurement.")
//! @yah:gotcha("The repro now lives at oss/orphan-gc/scripts/wrapper-chain-ab.sh (the /tmp path named above is gone). CAVEAT: the A/B numbers recorded on this ticket come from running that logic verbatim from /tmp earlier; the landed copy has NOT been run end-to-end in place, because the camp was too contended to finish it. Two attempts stalled — first on `cargo clean`, which takes cargo's global ~/.cargo/.package-cache lock and blocks for as long as any other build on the machine holds it (fixed: the script now `rm -rf target` instead, since the target dir is inside its own mktemp), then on `cargo build` for the same reason. On a busy shared machine this script looks like it is hanging when it is only waiting. Run it when the camp is quiet, and confirm both halves before trusting the fix.")
//! @yah:gotcha("The nesting failure is WORSE than lost cache hits and this is now the strongest argument for the ticket. Cargo probes the toolchain with `rustc -vV` THROUGH the wrapper chain, so sccache is handed a shell script where it expects a compiler — and it HANGS rather than erroring. The hung client never exits; enough of them wedge the server for the whole machine, with `sccache --show-stats` still answering instantly while every real compile blocks forever and nothing logs an error. Observed taking out three unrelated sessions' builds at once (~15 sleeping clients each, 0.01s CPU apiece), including an orphan still wedged 16 minutes after its workspace was deleted. Recovery: reap the clients (pkill -f 'sccache .*<wrapper-path>') then sccache --stop-server && --start-server. Consequence for this ticket: bootstrap must refuse the nested arrangement outright rather than warn — it is not a degraded mode, it is a machine-wide denial of service. scripts/wrapper-chain-ab.sh now runs against a PRIVATE sccache server (own port + cache dir, reaped on exit) because inducing this against a shared server is unsafe.")

use std::collections::HashSet;
use std::ffi::OsString;
use std::path::Path;
use std::process::Command;

use anyhow::{Context, Result};

use crate::artifacts;
use crate::config::{self, WorkspaceConfig};
use crate::family::Invocation;
use crate::gc;
use crate::lease::Lease;
use crate::state::{generation_id, now_ms, FamilyState, Generation, Store};

pub fn run(real_rustc: OsString, args: Vec<OsString>) -> Result<i32> {
    let Some(workspace) = config::discover_for_wrapper()? else {
        // No discoverable workspace — a registry/git dependency unit, whose
        // manifest dir and cwd both sit inside the registry checkout. The
        // inner wrapper still applies (via the [env] transport); bookkeeping
        // does not.
        return exec_rustc(config::inner_wrapper_from_env().as_deref(), &real_rustc, &args);
    };
    let store = Store::for_workspace(&workspace.root)?;
    run_in(&store, &workspace, real_rustc, args)
}

/// [`run`] with the workspace and state store injected, so the whole wrapper
/// path — including a failing rustc — is exercisable from tests without
/// touching the real `$CARGO_HOME` or a real compiler.
pub(crate) fn run_in(
    store: &Store,
    workspace: &WorkspaceConfig,
    real_rustc: OsString,
    args: Vec<OsString>,
) -> Result<i32> {
    let env_inner = config::inner_wrapper_from_env();
    let inner = workspace.policy.inner_wrapper.as_deref().or(env_inner.as_deref());
    if !workspace.policy.enabled {
        return exec_rustc(inner, &real_rustc, &args);
    }

    let inv = Invocation::parse(real_rustc.clone(), args.clone())?;
    let Some(key) = inv.family_key.as_deref() else {
        return exec_rustc(inner, &real_rustc, &args);
    };
    // The outer `rustc-wrapper` slot sees *every* unit, not just workspace
    // members. Registry and git dependencies get the chained inner wrapper
    // and nothing else — no locks, no leases, no state I/O. The `.rs` check
    // additionally refuses to learn ownership from an argv whose positional
    // argument is not a crate root (e.g. a hand-configured wrapper nested in
    // `rustc-workspace-wrapper`, where argv[1] is that wrapper's binary).
    if !is_workspace_unit(&inv, &workspace.root) {
        return exec_rustc(inner, &real_rustc, &args);
    }

    let family_lock = store.lock_family(key)?;
    let lease = Lease::create(store, &inv)?;

    let status = rustc_command(inner, &real_rustc, &args)
        .status()
        .with_context(|| format!("execute {}", real_rustc.to_string_lossy()))?;

    if !status.success() {
        drop(lease);
        drop(family_lock);
        return Ok(status.code().unwrap_or(1));
    }

    let current_artifacts = artifacts::collect(&inv)?;
    let id = generation_id(&current_artifacts);
    let mut state = store
        .load_family(key)?
        .unwrap_or_else(|| FamilyState::new(key, &inv.family_label));

    let current_paths = current_artifacts
        .iter()
        .map(|a| a.path.clone())
        .collect::<HashSet<_>>();

    let changed_generation = state
        .current
        .as_ref()
        .map(|g| g.id != id || g.artifacts.iter().any(|a| !current_paths.contains(&a.path)))
        .unwrap_or(true);

    if changed_generation {
        if let Some(previous) = state.current.take() {
            state.orphans.push(previous);
            store.mark_pending(key)?;
        }
        state.current = Some(Generation {
            id,
            created_unix_ms: now_ms(),
            artifacts: current_artifacts,
        });
    } else if let Some(current) = state.current.as_mut() {
        current.created_unix_ms = now_ms();
        current.artifacts = current_artifacts;
    }

    state.label = inv.family_label.clone();
    state.last_used_unix_ms = now_ms();
    store.save_family(&state)?;

    let report = gc::sweep_locked(store, key, &workspace.policy)?;
    if workspace.policy.verbose
        && (report.deleted_artifacts > 0
            || report.deferred_artifacts > 0
            || report.collected_sessions > 0)
    {
        eprintln!(
            "cargo-orphan-gc: {}: deleted {} artifacts ({} bytes), deferred {}, collected {} \
             surplus incremental sessions ({} bytes)",
            inv.family_label,
            report.deleted_artifacts,
            report.deleted_bytes,
            report.deferred_artifacts,
            report.collected_sessions,
            report.collected_session_bytes
        );
    }

    drop(lease);
    drop(family_lock);

    // Retry a few previously blocked orphan families on every successful
    // workspace compile. This is what turns one-shot sweeping into automatic GC.
    if workspace.policy.pending_sweeps_per_compile > 0 {
        let _ = gc::sweep_pending(
            store,
            &workspace.policy,
            workspace.policy.pending_sweeps_per_compile,
            Some(key),
        );
    }

    // In orphan-only mode `max-bytes` is a watermark, and this warning is the
    // only signal the operator gets that the tree has outgrown it — nothing
    // else will act on it. In budget mode the sweep enforces the ceiling, so
    // warning here would be noise; more to the point, `tracked_bytes` sizes
    // every artifact of every family, which is far too expensive to pay on
    // each rustc invocation. Budget mode therefore leaves the hot path alone
    // and lets `cargo orphan-gc sweep` do the accounting (ARCHITECTURE §10.1).
    if workspace.policy.budget_ceiling().is_none() {
        if let Some(max) = workspace.policy.max_bytes {
            if let Ok(bytes) = tracked_bytes(store) {
                if bytes > max {
                    eprintln!(
                        "cargo-orphan-gc: tracked live+orphan bytes ({bytes}) exceed max-bytes \
                         ({max}); orphan-only mode will not evict current families. Set \
                         budget-mode = \"lru-current-families\" to authorize a real ceiling."
                    );
                }
            }
        }
    }

    Ok(status.code().unwrap_or(0))
}

/// A unit this tool does family bookkeeping for: its crate root is a source
/// file inside the workspace. Everything else — registry deps, git deps,
/// probe invocations — flows through [`exec_rustc`] untracked.
fn is_workspace_unit(inv: &Invocation, workspace_root: &Path) -> bool {
    inv.crate_root
        .as_deref()
        .map(|root| {
            root.starts_with(workspace_root)
                && root.extension().map(|e| e == "rs").unwrap_or(false)
        })
        .unwrap_or(false)
}

/// The rustc invocation, chained through the configured inner wrapper so a
/// compiler cache holding that slot still sees rustc as its argv[1].
fn rustc_command(inner: Option<&str>, real_rustc: &OsString, args: &[OsString]) -> Command {
    match inner {
        Some(wrapper) if !wrapper.is_empty() => {
            let mut command = Command::new(wrapper);
            command.arg(real_rustc).args(args);
            command
        }
        _ => {
            let mut command = Command::new(real_rustc);
            command.args(args);
            command
        }
    }
}

fn exec_rustc(inner: Option<&str>, real_rustc: &OsString, args: &[OsString]) -> Result<i32> {
    let status = rustc_command(inner, real_rustc, args)
        .status()
        .with_context(|| format!("execute {}", real_rustc.to_string_lossy()))?;
    Ok(status.code().unwrap_or(1))
}

fn tracked_bytes(store: &Store) -> Result<u64> {
    let mut total = 0u64;
    for key in store.family_keys()? {
        if let Some(state) = store.load_family(&key)? {
            for generation in state.current.into_iter().chain(state.orphans.into_iter()) {
                for artifact in generation.artifacts {
                    total = total.saturating_add(artifacts::path_size(&artifact.path).unwrap_or(0));
                }
            }
        }
    }
    Ok(total)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Policy;
    use std::fs;
    use std::path::PathBuf;

    struct Fixture {
        store: Store,
        workspace: WorkspaceConfig,
        rustc: PathBuf,
        src: PathBuf,
        out_dir: PathBuf,
    }

    fn fixture(tmp: &std::path::Path) -> Fixture {
        let root = tmp.join("ws");
        let src = root.join("src/lib.rs");
        fs::create_dir_all(src.parent().unwrap()).unwrap();
        fs::write(&src, "pub fn f() {}\n").unwrap();
        let out_dir = root.join("target/debug/deps");
        fs::create_dir_all(&out_dir).unwrap();

        let store = Store { root: tmp.join("state") };
        store.ensure_layout().unwrap();
        Fixture {
            store,
            workspace: WorkspaceConfig {
                manifest_path: root.join("Cargo.toml"),
                root,
                policy: Policy { enabled: true, ..Policy::default() },
            },
            // One fixed path for every generation: the rustc executable string
            // is part of the family identity, so tests swap the script's BODY,
            // never its location.
            rustc: tmp.join("fake-rustc.sh"),
            src,
            out_dir,
        }
    }

    #[cfg(unix)]
    fn set_rustc(path: &std::path::Path, body: &str) {
        use std::os::unix::fs::PermissionsExt;
        fs::write(path, format!("#!/bin/sh\n{body}\n")).unwrap();
        fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
    }

    fn compile(f: &Fixture, extra_filename: &str, out_dir: &std::path::Path) -> i32 {
        let args = vec![
            OsString::from("--crate-name"),
            OsString::from("foo"),
            f.src.clone().into_os_string(),
            OsString::from("--out-dir"),
            out_dir.as_os_str().to_os_string(),
            OsString::from("-C"),
            OsString::from(format!("extra-filename={extra_filename}")),
        ];
        run_in(&f.store, &f.workspace, f.rustc.clone().into_os_string(), args).unwrap()
    }

    fn only_family(store: &Store) -> crate::state::FamilyState {
        let keys = store.family_keys().unwrap();
        assert_eq!(keys.len(), 1, "expected exactly one family, got {keys:?}");
        store.load_family(&keys[0]).unwrap().unwrap()
    }

    /// Invariant A — no successful replacement, no retirement. A rustc that
    /// FAILS after a successful prior generation exists must leave that
    /// generation current, orphan nothing, and delete nothing.
    #[test]
    #[cfg(unix)]
    fn invariant_a_failed_rustc_does_not_retire_the_previous_generation() {
        let tmp = tempfile::tempdir().unwrap();
        let f = fixture(tmp.path());
        let rlib = f.out_dir.join("libfoo-aaa.rlib");

        set_rustc(&f.rustc, &format!("touch {}; exit 0", rlib.display()));
        assert_eq!(compile(&f, "-aaa", &f.out_dir), 0);
        let before = only_family(&f.store);
        let current_id = before.current.as_ref().unwrap().id.clone();

        set_rustc(&f.rustc, "exit 1");
        assert_eq!(compile(&f, "-bbb", &f.out_dir), 1, "exit status must propagate");

        let after = only_family(&f.store);
        assert_eq!(after.current.as_ref().unwrap().id, current_id, "G17 remains current");
        assert!(after.orphans.is_empty(), "a failed compile orphans nothing");
        assert!(rlib.exists(), "the previous generation's artifact survives");
    }

    /// Invariant B — delete only learned ownership. A file that was never
    /// recorded in a persisted generation (here: a stranger in the out-dir)
    /// survives a supersession sweep that reclaims the recorded artifact
    /// beside it.
    #[test]
    #[cfg(unix)]
    fn invariant_b_a_file_never_recorded_in_a_generation_is_never_deleted() {
        let tmp = tempfile::tempdir().unwrap();
        let f = fixture(tmp.path());
        let stranger = f.out_dir.join("stranger.txt");
        fs::write(&stranger, "not yours").unwrap();
        let old = f.out_dir.join("libfoo-aaa.rlib");
        let new = f.out_dir.join("libfoo-bbb.rlib");

        set_rustc(&f.rustc, &format!("touch {}; exit 0", old.display()));
        assert_eq!(compile(&f, "-aaa", &f.out_dir), 0);
        set_rustc(&f.rustc, &format!("touch {}; exit 0", new.display()));
        assert_eq!(compile(&f, "-bbb", &f.out_dir), 0);

        assert!(!old.exists(), "the superseded recorded artifact is reclaimed");
        assert!(new.exists());
        assert!(stranger.exists(), "unknown ownership must leak, never delete");
    }

    /// Invariant C — family identity is conservative. A build-grid change
    /// (here: a different out-dir) forks a new family; the old family's
    /// artifacts leak rather than being cross-deleted.
    #[test]
    #[cfg(unix)]
    fn invariant_c_a_build_grid_change_forks_a_family_and_leaks() {
        let tmp = tempfile::tempdir().unwrap();
        let f = fixture(tmp.path());
        let debug_rlib = f.out_dir.join("libfoo-aaa.rlib");
        let release_dir = f.workspace.root.join("target/release/deps");
        fs::create_dir_all(&release_dir).unwrap();
        let release_rlib = release_dir.join("libfoo-aaa.rlib");

        set_rustc(&f.rustc, &format!("touch {}; exit 0", debug_rlib.display()));
        assert_eq!(compile(&f, "-aaa", &f.out_dir), 0);
        set_rustc(&f.rustc, &format!("touch {}; exit 0", release_rlib.display()));
        assert_eq!(compile(&f, "-aaa", &release_dir), 0);

        assert_eq!(f.store.family_keys().unwrap().len(), 2, "grid change = new family");
        assert!(debug_rlib.exists(), "the old grid's artifact leaks, by design");
        assert!(release_rlib.exists());
    }

    /// Registry/git dependencies — any unit whose crate root is outside the
    /// workspace — get no family bookkeeping at all from the outer wrapper
    /// slot.
    #[test]
    #[cfg(unix)]
    fn a_unit_outside_the_workspace_is_never_tracked() {
        let tmp = tempfile::tempdir().unwrap();
        let f = fixture(tmp.path());
        let registry_src = tmp.path().join("registry/dep-1.0.0/src/lib.rs");
        fs::create_dir_all(registry_src.parent().unwrap()).unwrap();
        fs::write(&registry_src, "pub fn g() {}\n").unwrap();

        set_rustc(&f.rustc, "exit 0");
        let args = vec![
            OsString::from("--crate-name"),
            OsString::from("dep"),
            registry_src.into_os_string(),
            OsString::from("--out-dir"),
            f.out_dir.as_os_str().to_os_string(),
            OsString::from("-C"),
            OsString::from("extra-filename=-ccc"),
        ];
        let code =
            run_in(&f.store, &f.workspace, f.rustc.clone().into_os_string(), args).unwrap();

        assert_eq!(code, 0);
        assert!(f.store.family_keys().unwrap().is_empty(), "no state for foreign units");
    }

    /// The inner-wrapper chain: the configured wrapper receives the real
    /// rustc as its argv[1], which is the entire point of owning the outer
    /// slot.
    #[test]
    #[cfg(unix)]
    fn the_inner_wrapper_receives_rustc_as_its_first_argument() {
        let tmp = tempfile::tempdir().unwrap();
        let mut f = fixture(tmp.path());
        let log = tmp.path().join("inner.log");
        let inner = tmp.path().join("inner.sh");
        set_rustc(&inner, &format!("echo \"$1\" > {}; exec \"$@\"", log.display()));
        set_rustc(&f.rustc, "exit 0");
        f.workspace.policy.inner_wrapper = Some(inner.to_string_lossy().into_owned());

        assert_eq!(compile(&f, "-aaa", &f.out_dir.clone()), 0);

        let seen = fs::read_to_string(&log).unwrap();
        assert_eq!(seen.trim(), f.rustc.to_string_lossy(), "argv[1] must be rustc");
    }
}
