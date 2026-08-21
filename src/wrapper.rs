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
//!
//! @yah:ticket(R748-B10, "Operational log goes to rustc stderr, so cargo caches it per-unit and replays it forever as apparent live activity")
//! @yah:at(2026-08-12T23:41:06Z)
//! @yah:status(review)
//! @yah:assignee(agent:claude)
//! @yah:parent(R748)
//! @yah:severity(high)
//! @yah:handoff("The verbose per-compile summary is written with eprintln, i.e. to the stderr of the rustc invocation. Cargo captures each unit's compiler stderr into target/<profile>/.fingerprint/<unit>/output-* and REPLAYS it verbatim on every later build where that unit is fresh. So the log is not a record of what happened in the invocation you are reading - it is a record of whatever happened the last time that unit actually compiled.")
//! @yah:handoff("Measured on this camp: 269 fingerprint files currently carry cargo-orphan-gc log lines. The schema-embed output file holds two of them, from two different builds.")
//! @yah:handoff("This has already produced three wrong conclusions in one day, by three different readers - twice by me (I read replayed lines as fresh deletions, then read cached no-op runs as a successful verification of R748-B9) and once by a peer who reported the kill switch leaking, on the reasoning that other units compiled in the same invocation so no replay could be involved. That reasoning is the trap: replay is PER-UNIT, so a fresh unit replays while its neighbours genuinely compile.")
//! @yah:handoff("Verified the kill switch is actually sound, which is what the peer report was really about: with enabled = false, `cargo check -p yah --lib` emits the line, but the family count in $CARGO_HOME/orphan-gc/workspaces/*/families is identical before and after, so no bookkeeping ran. wrapper.rs returns at the enabled check before any state I/O. The visible line is purely a replay from when the tool was enabled.")
//! @yah:handoff("Tree anchor at handoff: cf7a7291bd6c6b29528131c852002fcb6fee0d00 — the shared tree as I left it. Diff against it (`git diff cf7a7291bd6c6b29528131c852002fcb6fee0d00..HEAD`) to see what landed under you, and quote this SHA rather than 'HEAD' in any revert/restore instruction.")
//! @yah:next("Stop writing operational output to the compiler's stderr. Options in preference order: (a) a log file under the state dir, which is also what shadow mode's report wants; (b) stderr only when a TTY is attached, so cargo never captures it; (c) keep stderr but stamp each line with a process id and timestamp, which does not stop the replay but makes it self-evidently stale. Anything but (a) still pollutes cargo's cache.")
//! @yah:next("Clearing the stale lines already in the tree needs the fingerprints touched - they age out only when each unit next recompiles. Worth a note wherever this is announced, because the lines will keep appearing for days and will keep being misread.")
//! @yah:verify("Reproduce: with enabled = false, run `cargo check -p yah --lib` twice from the camp root; the same line appears with an identical byte count both times, and `cat target/debug/.fingerprint/*schema-embed*/output-lib-schema_embed` shows that exact text on disk.")
//! @yah:verify("Confirm no bookkeeping: family count under $CARGO_HOME/orphan-gc/workspaces/*/families is unchanged across the run.")
//! @yah:gotcha("This makes the verbose log actively misleading as evidence, which matters for R748-F6: shadow mode's entire value is its report, and a report that cargo replays out of context is worse than no report. Fix this one BEFORE building shadow mode on top of the same channel.")
//! @yah:handoff("Fixed. New src/log.rs is the operational channel: an append-only file at <state-dir>/log, each line stamped with an ISO-8601 UTC timestamp and the pid, bounded at 1 MiB with one rotation. Echoed to stderr ONLY when stderr is a terminal, which cargo's capture pipe by construction is not.")
//! @yah:verify("37 unit + 2 integration tests green; clippy --all-targets silent.")
//! @yah:handoff("All four operational eprintln! sites moved: the wrapper's per-compile summary and its max-bytes watermark warning (wrapper.rs), gc.rs's defer-deletion line, and gc.rs's budget-retire line. bootstrap's RUSTC_WRAPPER warning deliberately stays on stderr - it runs from the CLI, never inside a rustc invocation.")
//! @yah:verify("End-to-end on a freshly built binary against real cargo (/tmp/r748-e2e3.sh): a build that genuinely recompiled wrote a timestamped, pid-stamped line to <state-dir>/log, and grep -rl over that tree's target/debug/.fingerprint/ returned 0 files carrying it. Checked by side-effect (session count on disk went 3 to 1), not by reading stdout.")
//! @yah:handoff("Option (a) from the ticket, not (b) or (c): a file under the state dir, which is what R748-F6's report wanted anyway. The timestamp is hand-rolled (civil_from_days) rather than a new dependency - it is the only place this tool formats a time.")
//! @yah:gotcha("The stale lines already captured in this camp's ~269 fingerprint files do NOT disappear with this fix - they age out only as each unit next recompiles, so they will keep surfacing, and keep being misread, for days. Any cargo-orphan-gc line seen WITHOUT a leading timestamp+pid is by definition a replay from the old binary.")

use std::collections::HashSet;
use std::ffi::OsString;
use std::path::Path;
use std::process::Command;

use anyhow::{Context, Result};

use crate::artifacts;
use crate::bootstrap;
use crate::config::{self, WorkspaceConfig};
use crate::family::Invocation;
use crate::gc;
use crate::lease::Lease;
use crate::log::Log;
use crate::state::{generation_id, now_ms, FamilyState, Generation, Store};

pub fn run(real_rustc: OsString, args: Vec<OsString>) -> Result<i32> {
    let Some(mut workspace) = config::discover_for_wrapper()? else {
        // No discoverable workspace — a registry/git dependency unit, whose
        // manifest dir and cwd both sit inside the registry checkout. The
        // inner wrapper still applies (via the [env] transport); bookkeeping
        // does not.
        return exec_rustc(config::inner_wrapper_from_env().as_deref(), &real_rustc, &args);
    };
    // Resolve the env transport HERE, at the real entry point, rather than
    // inside `run_in`. `run_in` is the test seam: leaving the fallback there
    // makes it read ambient process state, so running this crate's own tests
    // inside a workspace where the tool is installed chains every fixture
    // "rustc" through that workspace's real cache — which then rejects the
    // fixture shell script ("Compiler not supported"). Production behaviour is
    // unchanged: the policy still wins, the env is still the fallback.
    if workspace.policy.inner_wrapper.is_none() {
        // Already absolute when it arrives: bootstrap writes the [env] entry
        // with cargo's `relative = true`, so cargo resolves it per-machine.
        workspace.policy.inner_wrapper = config::inner_wrapper_from_env();
    } else if let Some(inner) = workspace.policy.inner_wrapper.take() {
        // The manifest keeps the value relative so the committed file stays
        // portable; resolve it here, against the workspace root cargo would
        // have resolved it against. Workspace units only — dependency units
        // never reach this branch, since their policy comes from [env].
        workspace.policy.inner_wrapper = Some(if bootstrap::is_relative_path_wrapper(&inner) {
            workspace.root.join(&inner).to_string_lossy().into_owned()
        } else {
            inner
        });
    }
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
    let inner = workspace.policy.inner_wrapper.as_deref();
    if !workspace.policy.enabled {
        return exec_rustc(inner, &real_rustc, &args);
    }

    let inv = Invocation::parse(&real_rustc, &args)?;
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

    let mut state = store
        .load_family(key)?
        .unwrap_or_else(|| FamilyState::new(key, &inv.family_label));

    // Walking the shared out-dir is the dominant per-compile cost (116 ms at
    // this camp's 210k entries, and that is the filesystem floor). Offer the
    // previous generation's entries as a fast path, and force a full scan every
    // `full-scan-every` compiles so a newly-emitted artifact cannot go
    // unrecorded indefinitely.
    let due_for_scan = workspace.policy.full_scan_every == 0
        || state.compiles_since_scan + 1 >= workspace.policy.full_scan_every;
    let reuse = if due_for_scan {
        None
    } else {
        state.current.as_ref().map(|g| g.artifacts.as_slice())
    };
    let (current_artifacts, scan) = artifacts::collect(&inv, reuse)?;
    state.compiles_since_scan = match scan {
        artifacts::Scan::Full => 0,
        artifacts::Scan::Reused => state.compiles_since_scan.saturating_add(1),
    };
    let id = generation_id(&current_artifacts);

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
        if let Some(mut previous) = state.current.take() {
            previous.orphaned_unix_ms = Some(now_ms());
            state.orphans.push(previous);
            store.mark_pending(key)?;
        }
        state.current = Some(Generation {
            id,
            created_unix_ms: now_ms(),
            artifacts: current_artifacts,
            orphaned_unix_ms: None,
        });
    } else if let Some(current) = state.current.as_mut() {
        current.created_unix_ms = now_ms();
        current.artifacts = current_artifacts;
    }

    state.label = inv.family_label.clone();
    state.last_used_unix_ms = now_ms();
    store.save_family(&state)?;

    // Recording what was just built (above) must happen every compile — it's
    // the only place a newly-orphaned generation gets queued (`mark_pending`,
    // inside the `changed_generation` branch further up) — but actually
    // *sweeping* for grace-period-eligible deletions is a background-shaped
    // cost, not a per-compile one: it does not depend on what was just
    // compiled, so paying it every time taxes a fast incremental rebuild far
    // more than a slow one (20-77% measured live, yah chat 2026-08-20). Both
    // this family's own sweep and the cross-family retry below share one
    // throttle window: a family this compile doesn't sweep is still
    // discoverable by the other, since it was just marked pending above.
    let due = store.due_for_pending_sweep(workspace.policy.pending_sweep_min_interval_ms);
    if due {
        store.mark_pending_sweep_done();
        let report = gc::sweep_locked(store, key, &workspace.policy)?;
        if workspace.policy.verbose
            && (report.deleted_artifacts > 0
                || report.already_gone_artifacts > 0
                || report.deferred_artifacts > 0
                || report.collected_sessions > 0)
        {
            let mode = workspace.policy.mode();
            // Never `eprintln!` from inside a rustc invocation: cargo caches
            // this unit's stderr and replays it on every later build where
            // the unit is fresh, so the line would outlive the run it
            // describes (R748-B10).
            Log::for_store(store).write(&format!(
                "{}: {} {} artifacts ({} bytes; {} already gone), deferred {}, {} {} surplus \
                 incremental sessions ({} bytes)",
                inv.family_label,
                mode.verb(),
                report.deleted_artifacts,
                report.deleted_bytes,
                report.already_gone_artifacts,
                report.deferred_artifacts,
                if mode.is_shadow() { "would collect" } else { "collected" },
                report.collected_sessions,
                report.collected_session_bytes
            ));
        }
    }

    drop(lease);
    drop(family_lock);

    // Retry a few previously blocked orphan families on every successful
    // workspace compile. This is what turns one-shot sweeping into automatic
    // GC — gated on the same `due` window as this family's own sweep above.
    if due && workspace.policy.pending_sweeps_per_compile > 0 {
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
                    Log::for_store(store).write(&format!(
                        "tracked live+orphan bytes ({bytes}) exceed max-bytes ({max}); \
                         orphan-only mode will not evict current families. Set budget-mode = \
                         \"lru-current-families\" to authorize a real ceiling."
                    ));
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
            for generation in state.current.into_iter().chain(state.orphans) {
                for artifact in generation.artifacts {
                    total = total.saturating_add(artifacts::owned_size(&artifact).unwrap_or(0));
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
                root,
                // `dry-run` defaults to true; the invariant tests are about
                // what deletion does, so they opt in explicitly. `orphan_grace_ms`
                // is zeroed too: these fixtures supersede a generation and sweep
                // it within the same synchronous test, so the R770 grace period
                // (which needs real elapsed time) would defer every one of them —
                // it has its own dedicated test in gc.rs instead.
                // `pending_sweep_min_interval_ms` is zeroed for the same reason:
                // two `compile()` calls in one test happen well under a second
                // apart, and the throttle would otherwise skip the second
                // compile's own sweep — that throttle window has its own test
                // in state.rs.
                policy: Policy {
                    enabled: true,
                    dry_run: false,
                    orphan_grace_ms: 0,
                    pending_sweep_min_interval_ms: 0,
                    ..Policy::default()
                },
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

        // Same unit hash: a recompile of the SAME unit is what supersession is
        // for, and it is the only thing that can retire a generation now.
        set_rustc(&f.rustc, "exit 1");
        assert_eq!(compile(&f, "-aaa", &f.out_dir), 1, "exit status must propagate");

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
        let new = f.out_dir.join("libfoo-aaa.rmeta");

        // One unit, recompiled, emitting a different file the second time —
        // supersession within a single family, which is what may reclaim.
        set_rustc(&f.rustc, &format!("touch {}; exit 0", old.display()));
        assert_eq!(compile(&f, "-aaa", &f.out_dir), 0);
        set_rustc(
            &f.rustc,
            &format!("rm -f {}; touch {}; exit 0", old.display(), new.display()),
        );
        assert_eq!(compile(&f, "-aaa", &f.out_dir), 0);

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

    /// R748-B9 — two hashes of one crate can be live AT THE SAME TIME.
    ///
    /// Feature unification builds the same crate several ways inside a single
    /// cargo invocation; `cargo check --workspace --all-targets` over
    /// `[patch.crates-io]` path deps does it reliably. Both `.rmeta`s are
    /// referenced by units still queued in that same build, so neither may be
    /// reclaimed just because the other compiled second.
    ///
    /// This broke the camp: `extern location for task_runs does not exist:
    /// .../libtask_runs-226ccde4fd02e56f.rmeta`, immediately after this tool
    /// logged deleting that crate's artifacts.
    #[test]
    #[cfg(unix)]
    fn two_live_unit_hashes_of_one_crate_never_reclaim_each_other() {
        let tmp = tempfile::tempdir().unwrap();
        let f = fixture(tmp.path());
        let features_a = f.out_dir.join("libfoo-aaa.rmeta");
        let features_b = f.out_dir.join("libfoo-bbb.rmeta");

        set_rustc(&f.rustc, &format!("touch {}; exit 0", features_a.display()));
        assert_eq!(compile(&f, "-aaa", &f.out_dir), 0);
        set_rustc(&f.rustc, &format!("touch {}; exit 0", features_b.display()));
        assert_eq!(compile(&f, "-bbb", &f.out_dir), 0);

        assert!(
            features_a.exists(),
            "the first unit hash is still linked against by this very build"
        );
        assert!(features_b.exists());
        assert_eq!(
            f.store.family_keys().unwrap().len(),
            2,
            "distinct unit hashes are distinct families"
        );
    }

    /// R748-F6 — what shadow mode is for: the wrapper keeps LEARNING. A
    /// supersession in shadow records the new generation and queues the old
    /// one (that queue is what `status` reports), and reclaims nothing. Turning
    /// `dry-run` off later then acts on exactly the queue the operator read.
    #[test]
    #[cfg(unix)]
    fn shadow_mode_learns_and_queues_but_reclaims_nothing_until_authorized() {
        let tmp = tempfile::tempdir().unwrap();
        let mut f = fixture(tmp.path());
        f.workspace.policy.dry_run = true;
        let old = f.out_dir.join("libfoo-aaa.rlib");
        let new = f.out_dir.join("libfoo-aaa.rmeta");

        set_rustc(&f.rustc, &format!("touch {}; exit 0", old.display()));
        assert_eq!(compile(&f, "-aaa", &f.out_dir), 0);
        set_rustc(
            &f.rustc,
            &format!("rm -f {}; touch {}; exit 0", old.display(), new.display()),
        );
        assert_eq!(compile(&f, "-aaa", &f.out_dir), 0);

        let state = only_family(&f.store);
        assert!(state.current.is_some(), "shadow mode still learns ownership");
        assert_eq!(
            state.orphans.iter().flat_map(|g| g.artifacts.iter()).count(),
            1,
            "the superseded generation is queued, not swept: {state:?}"
        );

        // Authorize deletion and compile once more: the queue shadow reported
        // is the queue that gets reclaimed.
        f.workspace.policy.dry_run = false;
        assert_eq!(compile(&f, "-aaa", &f.out_dir), 0);
        assert!(only_family(&f.store).orphans.is_empty(), "the queue is now reclaimed");
        assert!(new.exists(), "the live artifact is untouched throughout");
    }

    /// The out-dir walk is the tool's dominant per-compile cost, so it is
    /// skipped while every recorded artifact is still present — but only for
    /// `full-scan-every` compiles, or a newly emitted artifact would stay
    /// unrecorded forever. Both halves are pinned here.
    #[test]
    #[cfg(unix)]
    fn a_newly_emitted_artifact_is_picked_up_at_the_next_full_scan() {
        let tmp = tempfile::tempdir().unwrap();
        let mut f = fixture(tmp.path());
        f.workspace.policy.full_scan_every = 2;
        let rlib = f.out_dir.join("libfoo-aaa.rlib");
        let extra = f.out_dir.join("libfoo-aaa.rmeta");

        set_rustc(&f.rustc, &format!("touch {}; exit 0", rlib.display()));
        assert_eq!(compile(&f, "-aaa", &f.out_dir), 0);
        assert_eq!(only_family(&f.store).compiles_since_scan, 0, "first compile scans");

        // Second compile emits an ADDITIONAL file. The fast path reuses the
        // recorded set, so the new file is not learned yet — it leaks, which is
        // the safe direction: an unrecorded file is one this tool never deletes.
        set_rustc(
            &f.rustc,
            &format!("touch {}; touch {}; exit 0", rlib.display(), extra.display()),
        );
        assert_eq!(compile(&f, "-aaa", &f.out_dir), 0);
        let state = only_family(&f.store);
        assert_eq!(state.compiles_since_scan, 1, "second compile reused the recorded set");
        assert_eq!(
            state.current.as_ref().unwrap().artifacts.len(),
            1,
            "the new artifact is not recorded yet: {state:?}"
        );

        // Third compile is due for a full scan, which finds it.
        assert_eq!(compile(&f, "-aaa", &f.out_dir), 0);
        let state = only_family(&f.store);
        assert_eq!(state.compiles_since_scan, 0, "the scan resets the counter");
        assert_eq!(
            state.current.as_ref().unwrap().artifacts.len(),
            2,
            "the full scan learns the artifact the fast path missed: {state:?}"
        );
        assert!(rlib.exists() && extra.exists(), "nothing was deleted along the way");
    }

    /// A recorded artifact that has gone missing invalidates the fast path
    /// immediately, whatever the counter says — otherwise the tool would keep
    /// asserting ownership of a file that is no longer there.
    #[test]
    #[cfg(unix)]
    fn a_missing_recorded_artifact_forces_a_walk_before_the_counter_is_due() {
        let tmp = tempfile::tempdir().unwrap();
        let mut f = fixture(tmp.path());
        f.workspace.policy.full_scan_every = 1000;
        let rlib = f.out_dir.join("libfoo-aaa.rlib");
        let rmeta = f.out_dir.join("libfoo-aaa.rmeta");

        set_rustc(&f.rustc, &format!("touch {}; exit 0", rlib.display()));
        assert_eq!(compile(&f, "-aaa", &f.out_dir), 0);

        set_rustc(
            &f.rustc,
            &format!("rm -f {}; touch {}; exit 0", rlib.display(), rmeta.display()),
        );
        assert_eq!(compile(&f, "-aaa", &f.out_dir), 0);

        let state = only_family(&f.store);
        assert_eq!(state.compiles_since_scan, 0, "a missing path forces a walk");
        let current: Vec<_> = state
            .current
            .as_ref()
            .unwrap()
            .artifacts
            .iter()
            .map(|a| a.path.clone())
            .collect();
        assert_eq!(current, vec![rmeta], "the walk re-learns the real set: {current:?}");
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
