//! @yah:relay(R748, "cargo-orphan-gc as adoption candidate: bounded GC for build artifacts that resist caching")
//! @yah:at(2026-08-11T07:48:26Z)
//! @yah:status(open)
//! @yah:handoff("F1, F2, T3, S4 all in review; T5 is the remaining open child and is operator-gated (mirror creation + crates.io publish). The standing assumption is RESOLVED: measured reclamation is no longer zero. Kamaji-copy adoption demo (scratchpad r748-demo.sh, shipped arrangement: tool outer + sccache inner): cold build 135 sccache misses / 0 'multiple input files'; three rounds of concurrent unwrapped builds manufactured the shared-tree surplus (7 to 10 sessions); ONE sweep collected all 3 (182 to 138 MB on disk, zero rebuild cost); wrapped edit-recheck 0.95s. Note the tool now PREVENTS same-key surplus while installed (family lock serializes same-family rustc, so rustc's own session GC stops skipping) - F2's collector matters for pre-adoption trees and for unwrapped writers sharing the tree, which is exactly the camp's situation.")
//! @yah:gotcha("W306's 'Rejected: adopting cargo-orphan-gc' verdict predates F1 and is now stale in one direction: the disqualifier (sccache collapse under nesting) is fixed by the chain inversion, and leg C proves the shipped arrangement preserves sccache. The OTHER half of that verdict stands: per-member contention overhead (~6.5ms linear, R745-S2) is unmeasured at root-workspace scale under real agent load, so re-opening camp adoption needs that number first. Do not treat this relay as overturning W306 for the camp; it makes the tool publishable for trees without ten concurrent agents.")
//! @yah:assignee(agent:bundle-anthropic-ashguard)
//! @yah:next("The thesis, and why this is worth finishing rather than shelving: sccache serves registry dependencies well (86.7% hit rate rebuilding a cleaned 3.5 GiB target, W307) but structurally REFUSES two classes — --crate-type bin and anything compiled with -C incremental. Those are the crates you actually work on, they are the largest objects in a target dir (yah 2131 MB, desktop 515 MB in this camp), and nothing governs them: cargo has no target-dir GC, and cargo-sweep --maxsize provably never walks incremental/ (identical output at --maxsize 6GB and --maxsize 1). A tool that bounds exactly the artifacts a compiler cache cannot touch is a gap in the whole Rust ecosystem, not just this camp. crates.io name cargo-orphan-gc is unregistered (404, sanity-checked against cargo-sweep 200).")
//! @yah:gotcha("The adoption blocker was NOT per-invocation overhead — that measured 2-5 ms (W307) and was the wrong thing to worry about. It is that nesting under sccache silently collapses the cache: 'multiple input files', 0 hits, 0 misses. Root cause is now diagnosed (see -F1) and reproduces with a pure passthrough script, so it is cargo's wrapper NESTING, not anything this tool does. Any future wrapper-chain work must re-run that A/B — the failure is invisible without reading sccache stats.")
//! @yah:assumes("That orphan-only deletion plus the LRU budget mode is enough policy. Measured today: orphan-only reclaimed ZERO bytes across four scenarios (source edit, feature change, rustflags change, rebuild) because a stable path set REFRESHES a generation rather than orphaning it, and feature/rustflags changes fork a NEW family that leaks by Invariant C. Budget mode covers the fork case. -F2 covers the stable-path-set case. If neither fires on real workloads, the lifetime policy itself is wrong and that is the thing to learn before publishing.")
//! @arch:see(oss/orphan-gc/ARCHITECTURE.md)
//!
//! @yah:ticket(R748-T3, "Cover invariants A-G with tests before this deletes files on anyone else's machine")
//! @yah:at(2026-08-11T07:49:10Z)
//! @yah:status(review)
//! @yah:handoff("Every invariant A-G now has a named test with the letter in the name (grep 'fn invariant_'). A/B/C drive the real wrapper path via a new pub(crate) run_in seam (store + workspace injected, rustc = a shell script whose BODY changes between generations - its PATH must stay fixed because the rustc executable string is part of the family key). D and F are gc-level; E and G were the existing budget-mode tests, renamed to carry their letters. Also added: outside-workspace units are never tracked, the inner-wrapper chain hands rustc as argv[1], and tests/live_cargo.rs runs the whole thing through a real cargo. 20 tests total, up from 8.")
//! @yah:verify("cargo test in oss/orphan-gc - 20/20: 18 unit (invariant_a/b/c in wrapper::tests, invariant_d/f + invariant_e/g in gc::tests, 4 session-collection fixtures in artifacts::tests, family identity, budget suite) + fixture-shape + live-cargo end-to-end.")
//! @yah:assignee(agent:bundle-anthropic-ashguard)
//! @yah:parent(R748)
//! @yah:next("8 tests for ~1400 lines, in a tool whose entire job is deleting build artifacts. The invariants are crisply stated in ARCHITECTURE.md §4 and individually testable; most are asserted only by prose. Missing coverage, by invariant: A (a FAILED rustc must not retire the previous generation — nothing tests the compile-error path); B (an artifact never recorded in a persisted Generation is never deletable, i.e. a pre-existing file in the out-dir survives); C (a build-grid change forks a family and leaks rather than cross-deleting); D (a path reused by the new generation is dropped from the orphan set WITHOUT deletion); F (each of the three kind-specific path validations fails CLOSED — out-dir entry outside its recorded parent, explicit emit outside the out-dir, incremental path != ownership root). E and G have one test each from the budget-mode work and should be kept.\n\nPublishing to crates.io without these means shipping unproven deletion authority to strangers' machines.")
//! @yah:verify("cargo test in oss/orphan-gc, with at least one named test per invariant A-G and the invariant letter in the test name so the mapping is greppable.")
//! @yah:gotcha("Invariant A's test needs a rustc that exits non-zero AFTER a successful prior generation exists, which the current fixtures never construct — they all drive gc:: directly rather than through wrapper::run. Testing the wrapper path may need the real-rustc invocation behind a seam.")

use std::collections::HashSet;

use anyhow::Result;

use crate::artifacts;
use crate::config::Policy;
#[cfg(test)]
use crate::config::BudgetMode;
use crate::lease;
use crate::state::Store;

#[derive(Default, Debug)]
pub struct SweepReport {
    pub deleted_artifacts: usize,
    pub deleted_bytes: u64,
    pub deferred_artifacts: usize,
    /// Surplus finalized rustc sessions collected inside tracked incremental
    /// dirs — reclamation that generation supersession cannot see, because a
    /// source edit refreshes the family's path set while the tree grows
    /// *inside* the recorded directory.
    pub collected_sessions: usize,
    pub collected_session_bytes: u64,
}

impl SweepReport {
    fn absorb(&mut self, other: SweepReport) {
        self.deleted_artifacts += other.deleted_artifacts;
        self.deleted_bytes = self.deleted_bytes.saturating_add(other.deleted_bytes);
        self.deferred_artifacts += other.deferred_artifacts;
        self.collected_sessions += other.collected_sessions;
        self.collected_session_bytes = self
            .collected_session_bytes
            .saturating_add(other.collected_session_bytes);
    }
}

pub fn sweep_locked(store: &Store, key: &str, policy: &Policy) -> Result<SweepReport> {
    sweep_locked_protecting(store, key, policy, &HashSet::new())
}

/// [`sweep_locked`], additionally treating `protected` as live paths.
///
/// Invariant D — current paths dominate orphan paths — is stated per family,
/// and that is sufficient while orphans only ever arise from a family
/// superseding itself. Budget mode breaks that assumption: it retires one
/// family's current generation while a *different* family may still own some
/// of the same paths. Cargo emits unhashed outputs (the final binary, its
/// `.d`) that several families legitimately share, so without this the sweep
/// would delete artifacts the surviving family still needs.
pub fn sweep_locked_protecting(
    store: &Store,
    key: &str,
    policy: &Policy,
    protected: &HashSet<std::path::PathBuf>,
) -> Result<SweepReport> {
    let Some(mut state) = store.load_family(key)? else {
        store.clear_pending(key);
        return Ok(SweepReport::default());
    };

    let active = lease::active_inputs(store)?;
    let mut current_paths = state
        .current
        .as_ref()
        .map(|g| g.artifacts.iter().map(|a| a.path.clone()).collect::<HashSet<_>>())
        .unwrap_or_default();
    current_paths.extend(protected.iter().cloned());

    let mut report = SweepReport::default();
    let mut retained_generations = Vec::new();

    for mut generation in state.orphans.drain(..) {
        let mut retained_artifacts = Vec::new();
        for artifact in generation.artifacts.drain(..) {
            if current_paths.contains(&artifact.path) {
                // Same path was overwritten/reused by the current generation.
                // It is not an orphan anymore and must never be deleted.
                continue;
            }

            if active.unknown_active_lease || lease::is_active_input(&active, &artifact.path) {
                report.deferred_artifacts += 1;
                retained_artifacts.push(artifact);
                continue;
            }

            match artifacts::remove(&artifact) {
                Ok(bytes) => {
                    report.deleted_artifacts += 1;
                    report.deleted_bytes = report.deleted_bytes.saturating_add(bytes);
                }
                Err(err) => {
                    if policy.verbose {
                        eprintln!(
                            "cargo-orphan-gc: defer deletion of {}: {err:#}",
                            artifact.path.display()
                        );
                    }
                    report.deferred_artifacts += 1;
                    retained_artifacts.push(artifact);
                }
            }
        }

        if !retained_artifacts.is_empty() {
            generation.artifacts = retained_artifacts;
            retained_generations.push(generation);
        }
    }

    state.orphans = retained_generations;

    // The free tier: surplus finalized sessions inside the *current*
    // generation's incremental dirs. Runs under the same family lock as
    // everything else here. Orphaned incremental dirs need no per-session
    // treatment — they are deleted whole by the orphan pass above.
    if let Some(current) = state.current.as_ref() {
        for artifact in &current.artifacts {
            if artifact.kind == artifacts::ArtifactKind::IncrementalDir {
                let collected = artifacts::collect_surplus_sessions(
                    &artifact.path,
                    artifact.session_prefix.as_deref(),
                );
                report.collected_sessions += collected.deleted_sessions;
                report.collected_session_bytes = report
                    .collected_session_bytes
                    .saturating_add(collected.deleted_bytes);
            }
        }
    }

    store.save_family(&state)?;
    if state.orphans.is_empty() {
        store.clear_pending(key);
    } else {
        store.mark_pending(key)?;
    }
    Ok(report)
}

pub fn sweep_pending(store: &Store, policy: &Policy, limit: usize, skip: Option<&str>) -> Result<SweepReport> {
    let mut total = SweepReport::default();
    for key in store.pending_keys(limit.saturating_add(1))? {
        if skip == Some(key.as_str()) {
            continue;
        }
        let _lock = store.lock_family(&key)?;
        total.absorb(sweep_locked(store, &key, policy)?);
    }
    Ok(total)
}

pub fn sweep_all(store: &Store, policy: &Policy) -> Result<SweepReport> {
    let mut total = SweepReport::default();
    for key in store.family_keys()? {
        let _lock = store.lock_family(&key)?;
        total.absorb(sweep_locked(store, &key, policy)?);
    }
    Ok(total)
}

// ---------------------------------------------------------------------------
// Budget mode — the ceiling orphan GC cannot supply on its own
// ---------------------------------------------------------------------------

#[derive(Default, Debug)]
pub struct BudgetReport {
    pub families: usize,
    /// Families whose current generation was retired into the orphan queue.
    pub retired_families: usize,
    /// Families skipped because a build touched them between measuring and
    /// locking them.
    pub raced_families: usize,
    pub bytes_before: u64,
    pub bytes_after: u64,
    pub sweep: SweepReport,
}

/// Enforce `max-bytes` by retiring the least recently used current families.
///
/// The important property is what this function does *not* do: it never
/// deletes anything itself. Retiring a family moves its current generation
/// into the same orphan queue a supersession would have, and deletion then
/// goes through `sweep_locked` exactly as before — active-input leases,
/// current-path domination, and the Invariant F path validation all still
/// apply unchanged. Budget mode adds an *authority to orphan*, not a second
/// deletion path.
///
/// Deliberately not called from the wrapper. Costing the budget means sizing
/// every artifact of every family, which is far too expensive to run on each
/// rustc invocation — the amortized `pending_sweeps_per_compile` mechanism
/// (§9) exists precisely so the hot path stays cheap. This belongs on a timer
/// or an explicit `cargo orphan-gc sweep`.
pub fn budget_sweep(store: &Store, policy: &Policy) -> Result<BudgetReport> {
    let mut report = BudgetReport::default();
    let Some(ceiling) = policy.budget_ceiling() else {
        return Ok(report);
    };

    // (key, last_used_at_measure_time, current bytes)
    let mut families: Vec<(String, u128, u64)> = Vec::new();
    // Every path any family currently owns, keyed by owner. Retiring one
    // family must not delete a path another family still holds current.
    let mut owned: Vec<(String, HashSet<std::path::PathBuf>)> = Vec::new();
    for key in store.family_keys()? {
        let Some(state) = store.load_family(&key)? else {
            continue;
        };
        let Some(current) = state.current.as_ref() else {
            continue;
        };
        let bytes = current
            .artifacts
            .iter()
            .map(|a| artifacts::path_size(&a.path).unwrap_or(0))
            .fold(0u64, |acc, b| acc.saturating_add(b));
        report.bytes_before = report.bytes_before.saturating_add(bytes);
        owned.push((
            key.clone(),
            current.artifacts.iter().map(|a| a.path.clone()).collect(),
        ));
        families.push((key, state.last_used_unix_ms, bytes));
    }
    report.families = families.len();

    let mut running = report.bytes_before;
    if running <= ceiling {
        report.bytes_after = running;
        return Ok(report);
    }

    // Coldest first. A family in active use is touched by every build that
    // needs it, so it sorts last and survives.
    families.sort_by_key(|(_, last_used, _)| *last_used);

    // Never retire the most recently used family, even when the ceiling
    // cannot otherwise be met. Retiring the family currently being built is
    // guaranteed-useless churn — the next build recreates it immediately, so
    // it cannot improve steady state, and a ceiling set below the size of one
    // family would otherwise empty the tree on every sweep. An unsatisfiable
    // budget is reported (`bytes_after > ceiling`) rather than obeyed.
    families.pop();

    for (key, measured_last_used, bytes) in families {
        if running <= ceiling {
            break;
        }

        let _lock = store.lock_family(&key)?;

        // Re-read under the lock. A build may have used this family between
        // the measurement pass and now, which would make it the hottest
        // family rather than the coldest — retiring it then would delete
        // exactly what is about to be needed.
        let Some(mut state) = store.load_family(&key)? else {
            continue;
        };
        if state.last_used_unix_ms != measured_last_used {
            report.raced_families += 1;
            continue;
        }
        let Some(current) = state.current.take() else {
            continue;
        };

        if policy.verbose {
            eprintln!(
                "cargo-orphan-gc: budget retire {} ({bytes} bytes, idle since {}ms)",
                state.label, state.last_used_unix_ms
            );
        }

        state.orphans.push(current);
        store.save_family(&state)?;
        store.mark_pending(&key)?;

        // Everything still held current by some *other* family is off limits.
        let protected: HashSet<std::path::PathBuf> = owned
            .iter()
            .filter(|(owner, _)| owner != &key)
            .flat_map(|(_, paths)| paths.iter().cloned())
            .collect();
        let swept = sweep_locked_protecting(store, &key, policy, &protected)?;

        // Recompute from what actually survived rather than trusting
        // `deleted_bytes`. Two families can own overlapping paths, and the
        // second `remove` of an already-deleted path reports 0 bytes — which
        // would leave `running` overstated and retire more families than the
        // ceiling requires. Whatever is still queued in orphans is what this
        // family still costs; usually nothing.
        let leftover = store
            .load_family(&key)?
            .map(|s| {
                s.orphans
                    .iter()
                    .flat_map(|g| g.artifacts.iter())
                    .map(|a| artifacts::path_size(&a.path).unwrap_or(0))
                    .fold(0u64, |acc, b| acc.saturating_add(b))
            })
            .unwrap_or(0);
        running = running.saturating_sub(bytes).saturating_add(leftover);
        report.retired_families += 1;
        report.sweep.absorb(swept);
    }

    report.bytes_after = running;
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::artifacts::{ArtifactKind, OwnedArtifact};
    use crate::state::{now_ms, FamilyState, Generation};
    use std::fs;

    /// A family whose current generation owns one out-dir file of `bytes`,
    /// last used `age_ms` ago.
    fn family(store: &Store, out_dir: &std::path::Path, name: &str, bytes: usize, age_ms: u128) {
        fs::create_dir_all(out_dir).unwrap();
        let path = out_dir.join(format!("lib{name}-abc123.rlib"));
        fs::write(&path, vec![0u8; bytes]).unwrap();

        let key = blake3::hash(name.as_bytes()).to_hex().to_string();
        let artifacts = vec![OwnedArtifact {
            path,
            root: out_dir.to_path_buf(),
            kind: ArtifactKind::OutDirEntry,
            session_prefix: None,
        }];
        let mut state = FamilyState::new(&key, name);
        state.last_used_unix_ms = now_ms() - age_ms;
        state.current = Some(Generation {
            id: crate::state::generation_id(&artifacts),
            created_unix_ms: now_ms(),
            artifacts,
        });
        store.save_family(&state).unwrap();
    }

    fn policy(max_bytes: Option<u64>, mode: BudgetMode) -> Policy {
        Policy {
            enabled: true,
            max_bytes,
            budget_mode: mode,
            ..Policy::default()
        }
    }

    fn store_in(dir: &std::path::Path) -> Store {
        let store = Store { root: dir.join("state") };
        store.ensure_layout().unwrap();
        store
    }

    /// Orphan-only is the default and must stay inert no matter how far over
    /// `max-bytes` the tree is. This is the invariant ARCHITECTURE.md §10
    /// insists on: a ceiling must never be smuggled in silently.
    #[test]
    fn orphan_only_mode_never_retires_a_current_family() {
        let tmp = tempfile::tempdir().unwrap();
        let store = store_in(tmp.path());
        family(&store, &tmp.path().join("deps"), "cold", 4096, 900_000);

        let report = budget_sweep(&store, &policy(Some(1), BudgetMode::OrphanOnly)).unwrap();

        assert_eq!(report.retired_families, 0);
        assert_eq!(report.bytes_before, 0, "measurement is skipped entirely");
    }

    /// Under the ceiling, nothing is retired even in budget mode — the
    /// authority exists but is not exercised.
    #[test]
    fn budget_mode_is_inert_while_the_tree_fits() {
        let tmp = tempfile::tempdir().unwrap();
        let store = store_in(tmp.path());
        family(&store, &tmp.path().join("deps"), "a", 1000, 900_000);

        let report =
            budget_sweep(&store, &policy(Some(1_000_000), BudgetMode::LruCurrentFamilies)).unwrap();

        assert_eq!(report.retired_families, 0);
        assert_eq!(report.bytes_before, report.bytes_after);
    }

    /// The ceiling orphan GC could not supply: over budget, coldest family
    /// goes first and the hot one survives.
    #[test]
    fn budget_mode_retires_coldest_family_first_until_under_ceiling() {
        let tmp = tempfile::tempdir().unwrap();
        let store = store_in(tmp.path());
        let deps = tmp.path().join("deps");
        family(&store, &deps, "hot", 1000, 1_000);
        family(&store, &deps, "warm", 1000, 60_000);
        family(&store, &deps, "cold", 1000, 900_000);

        // Fits two of the three.
        let report =
            budget_sweep(&store, &policy(Some(2400), BudgetMode::LruCurrentFamilies)).unwrap();

        assert_eq!(report.retired_families, 1, "{report:?}");
        assert!(report.bytes_after <= 2400);
        assert!(
            !deps.join("libcold-abc123.rlib").exists(),
            "coldest family's artifact should be gone"
        );
        assert!(
            deps.join("libhot-abc123.rlib").exists(),
            "hot family must survive"
        );
    }

    /// Cargo emits unhashed outputs (the final binary and its `.d`) that
    /// several families legitimately share. Retiring a cold family must not
    /// delete a path a surviving family still holds current — Invariant D is
    /// stated per family, and budget mode is what makes cross-family
    /// collisions reachable.
    #[test]
    fn invariant_g_retiring_a_cold_family_spares_paths_another_family_still_owns() {
        let tmp = tempfile::tempdir().unwrap();
        let store = store_in(tmp.path());
        let deps = tmp.path().join("deps");
        fs::create_dir_all(&deps).unwrap();

        // One unhashed path both families claim, as cargo does for `app`.
        let shared = deps.join("app");
        fs::write(&shared, vec![0u8; 5000]).unwrap();

        for (name, age) in [("hot", 1_000u128), ("cold", 900_000)] {
            let key = blake3::hash(name.as_bytes()).to_hex().to_string();
            let own = deps.join(format!("lib{name}-abc123.rlib"));
            fs::write(&own, vec![0u8; 1000]).unwrap();
            let artifacts = vec![
                OwnedArtifact {
                    path: own,
                    root: deps.clone(),
                    kind: ArtifactKind::OutDirEntry,
                    session_prefix: None,
                },
                OwnedArtifact {
                    path: shared.clone(),
                    root: deps.clone(),
                    kind: ArtifactKind::OutDirEntry,
                    session_prefix: None,
                },
            ];
            let mut state = FamilyState::new(&key, name);
            state.last_used_unix_ms = now_ms() - age;
            state.current = Some(Generation {
                id: crate::state::generation_id(&artifacts),
                created_unix_ms: now_ms(),
                artifacts,
            });
            store.save_family(&state).unwrap();
        }

        budget_sweep(&store, &policy(Some(1), BudgetMode::LruCurrentFamilies)).unwrap();

        assert!(
            !deps.join("libcold-abc123.rlib").exists(),
            "the cold family's own artifact should still be reclaimed"
        );
        assert!(
            shared.exists(),
            "a path the surviving family still holds current must not be deleted"
        );
    }

    /// A ceiling smaller than a single family cannot be met. The tool must
    /// report that rather than empty the tree: retiring the family being
    /// built right now is churn that cannot improve steady state, because the
    /// next build recreates it.
    #[test]
    fn an_unsatisfiable_ceiling_still_leaves_the_hottest_family_intact() {
        let tmp = tempfile::tempdir().unwrap();
        let store = store_in(tmp.path());
        let deps = tmp.path().join("deps");
        family(&store, &deps, "hot", 1000, 1_000);
        family(&store, &deps, "cold", 1000, 900_000);

        let report = budget_sweep(&store, &policy(Some(1), BudgetMode::LruCurrentFamilies)).unwrap();

        assert_eq!(report.retired_families, 1, "{report:?}");
        assert!(
            report.bytes_after > 1,
            "an unmet ceiling must be visible in the report, got {report:?}"
        );
        assert!(
            deps.join("libhot-abc123.rlib").exists(),
            "hottest family survives an unsatisfiable budget"
        );
    }

    /// Invariant D — current paths dominate orphan paths. A path the new
    /// generation reuses is dropped from the orphan set WITHOUT deletion;
    /// only the genuinely superseded path is reclaimed.
    #[test]
    fn invariant_d_a_path_reused_by_the_current_generation_is_kept() {
        let tmp = tempfile::tempdir().unwrap();
        let store = store_in(tmp.path());
        let deps = tmp.path().join("deps");
        fs::create_dir_all(&deps).unwrap();
        let reused = deps.join("libfoo-abc.rlib");
        let superseded = deps.join("libfoo-abc.d");
        fs::write(&reused, vec![0u8; 100]).unwrap();
        fs::write(&superseded, vec![0u8; 100]).unwrap();

        let artifact = |path: &std::path::Path| OwnedArtifact {
            path: path.to_path_buf(),
            root: deps.clone(),
            kind: ArtifactKind::OutDirEntry,
            session_prefix: None,
        };
        let key = blake3::hash(b"d").to_hex().to_string();
        let mut state = FamilyState::new(&key, "foo");
        state.orphans.push(Generation {
            id: "old".into(),
            created_unix_ms: now_ms(),
            artifacts: vec![artifact(&reused), artifact(&superseded)],
        });
        state.current = Some(Generation {
            id: "new".into(),
            created_unix_ms: now_ms(),
            artifacts: vec![artifact(&reused)],
        });
        store.save_family(&state).unwrap();

        let report = {
            let _lock = store.lock_family(&key).unwrap();
            sweep_locked(&store, &key, &policy(None, BudgetMode::OrphanOnly)).unwrap()
        };

        assert_eq!(report.deleted_artifacts, 1, "{report:?}");
        assert!(reused.exists(), "a reused path must never be deleted");
        assert!(!superseded.exists());
        let after = store.load_family(&key).unwrap().unwrap();
        assert!(after.orphans.is_empty(), "the reused path leaves the orphan set");
    }

    /// Invariant F — unsafe path validation fails closed, per artifact kind:
    /// an out-dir entry that is not a direct child of its recorded parent, an
    /// explicit emit outside the recorded out-dir, and an incremental path
    /// that is not exactly its ownership root must all refuse deletion and
    /// stay queued rather than be deleted.
    #[test]
    fn invariant_f_unsafe_path_validation_fails_closed_for_every_kind() {
        let tmp = tempfile::tempdir().unwrap();
        let store = store_in(tmp.path());
        let deps = tmp.path().join("deps");
        let nested = deps.join("sub");
        fs::create_dir_all(&nested).unwrap();
        let outside = tmp.path().join("elsewhere");
        fs::create_dir_all(&outside).unwrap();

        // Kind 1: out-dir entry whose recorded parent is NOT its direct parent.
        let deep = nested.join("libfoo-abc.rlib");
        fs::write(&deep, b"x").unwrap();
        // Kind 2: explicit emit outside the recorded out-dir.
        let stray_emit = outside.join("foo.rmeta");
        fs::write(&stray_emit, b"x").unwrap();
        // Kind 3: incremental dir whose path and ownership root disagree.
        let incr = tmp.path().join("incremental");
        fs::create_dir_all(&incr).unwrap();

        let bad = vec![
            OwnedArtifact {
                path: deep.clone(),
                root: deps.clone(),
                kind: ArtifactKind::OutDirEntry,
                session_prefix: None,
            },
            OwnedArtifact {
                path: stray_emit.clone(),
                root: deps.clone(),
                kind: ArtifactKind::ExplicitEmit,
                session_prefix: None,
            },
            OwnedArtifact {
                path: incr.clone(),
                root: deps.clone(),
                kind: ArtifactKind::IncrementalDir,
                session_prefix: None,
            },
        ];
        for artifact in &bad {
            assert!(
                artifacts::remove(artifact).is_err(),
                "remove must refuse {:?}",
                artifact.kind
            );
        }

        let key = blake3::hash(b"f").to_hex().to_string();
        let mut state = FamilyState::new(&key, "foo");
        state.orphans.push(Generation {
            id: "old".into(),
            created_unix_ms: now_ms(),
            artifacts: bad,
        });
        store.save_family(&state).unwrap();

        let report = {
            let _lock = store.lock_family(&key).unwrap();
            sweep_locked(&store, &key, &policy(None, BudgetMode::OrphanOnly)).unwrap()
        };

        assert_eq!(report.deleted_artifacts, 0, "{report:?}");
        assert_eq!(report.deferred_artifacts, 3, "all three kinds stay queued");
        assert!(deep.exists());
        assert!(stray_emit.exists());
        assert!(incr.exists());
        let after = store.load_family(&key).unwrap().unwrap();
        assert_eq!(
            after.orphans.iter().flat_map(|g| g.artifacts.iter()).count(),
            3,
            "queued for retry, not forgotten"
        );
    }

    /// Retirement is not a new deletion path — it routes through the orphan
    /// queue, so every existing safety check still runs. An active lease
    /// therefore defers a budget retirement exactly as it defers a
    /// supersession.
    #[test]
    fn invariant_e_an_active_lease_defers_budget_retirement() {
        let tmp = tempfile::tempdir().unwrap();
        let store = store_in(tmp.path());
        let deps = tmp.path().join("deps");
        // Two families: the hottest is spared by the floor regardless, so the
        // cold one is what the lease has to protect.
        family(&store, &deps, "hot", 4096, 1_000);
        family(&store, &deps, "cold", 4096, 900_000);

        let _held =
            crate::lease::Lease::create_with_inputs(&store, &[deps.join("libcold-abc123.rlib")])
                .unwrap();

        let report =
            budget_sweep(&store, &policy(Some(1), BudgetMode::LruCurrentFamilies)).unwrap();

        assert_eq!(report.sweep.deleted_artifacts, 0, "{report:?}");
        assert!(report.sweep.deferred_artifacts > 0);
        assert!(
            deps.join("libcold-abc123.rlib").exists(),
            "an artifact under an active lease must survive"
        );
    }
}
