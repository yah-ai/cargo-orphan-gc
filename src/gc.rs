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
//! @yah:notify_on(R742-F1, "Durable copy of a party.chat sent to @Ashguard:eclipse on 2026-08-11 (R742-F1 / @Ashguard:libra). cargo-orphan-gc deletes rmeta/rlib for a crate DURING the same cargo invocation about to link against it, so `cargo check --workspace --all-targets` in oss/yubaba fails camp-wide. Isolated, not inferred: WITH `rustc-wrapper = \\\"cargo-orphan-gc\\\"` (.cargo/config.toml:18) it exits 101 twice in a row on a DIFFERENT crate each time — run 1 `can't find crate for yah_scryer` (oss/qed/crates/velveteen-exec/src/integration.rs:38) plus `extern location for task_runs does not exist`, run 2 `can't find crate for velveteen_exec` (crates/cloud/src/reconciler/mesofact_static.rs:238) — each immediately after that crate's own `deleted 2 artifacts` log line. WITHOUT it (CARGO_BUILD_RUSTC_WRAPPER=/Users/leif/ss/yah/.cargo/rustc-wrapper.sh, the sccache shim alone) the same tree exits 0 with 0 errors. Every reaped crate lives in a SIBLING workspace (oss/qed, oss/yah-base, oss/kamaji) reached through [patch.crates-io] into oss/yubaba/target — my inference from log ordering, having not read gc.rs, is that a path-dep outside the current workspace reads as orphaned to the reachability walk while a live build still needs it. I touched nothing in oss/orphan-gc or .cargo/config.toml. Also an argument for R748-F6: shadow mode would have caught this for free, which is a case for it being the default on a shared tree rather than opt-in.")
//! @yah:handoff("R748-B10 and R748-F6 landed together and are in review (B10 first, because shadow mode's whole value is its report and the old report channel was cargo's replayed stderr). Shadow is now the DEFAULT: enabled = true alone learns and reports and deletes nothing; deletion needs an explicit dry-run = false. The camp's root Cargo.toml gained an explicit dry-run = true line next to enabled = false, so whoever re-enables gets the safe rung by default - that is the only file outside oss/orphan-gc this touched.")
//! @yah:handoff("DOGFOODING, operator-authorized 2026-08-12: the camp root Cargo.toml is now enabled = true + dry-run = true (shadow). State was purged first per R748-B9's gotcha, and the new binary was installed BEFORE enabling - the old one predates dry-run and would have deleted. Verified beforehand that replacing the wrapper binary at the same path does NOT invalidate cargo fingerprints, so no rebuild storm for peers. @Ashguard:eclipse, @Ashguard:libra and @Ashguard:spade were notified. Back out with enabled = false; nothing else needs touching.")
//! @yah:handoff("First camp-scale evidence, ~25 minutes of peers' real builds: 23 families learned, 30250 current artifacts (37 GB), and would-reclaim = 0 orphan artifacts + 58 surplus incremental sessions (7.19 GB). That CONFIRMS this relay's standing assumption at scale rather than refuting it: orphan-only supersession reclaims nothing here (every queued record is a path the current generation reuses, Invariant D), and 100% of the reclaimable value is R748-F2's surplus-session collector. Also fixed a label that was lying about exactly this - status said 'orphan artifacts pending deletion: 1.17 GB' for records whose real reclaimable value is zero; it now reads 'orphan records queued (not all reclaimable)'.")
//! @yah:gotcha("DO NOT ARCHIVE - operator decision 2026-08-12. R748 is the only relay holding cargo-orphan-gc, and the camp is now dogfooding it in deleting mode, so bugs and mistakes from real use land as children here. R748-T11 is the open child that keeps this relay alive; it closes when 0.8 ships. Ten children sitting in review is expected state, not a finished relay.")
//! @yah:gotcha("BYTE ACCOUNTING OVER-COUNTED 24x AND IT REACHED THE BUDGET CEILING (fixed 2026-08-14). status reported current artifacts of 1,800,626,861,376 bytes (1.8 TB) against a real 365 GB tree. Cause: owned_size for an IncrementalDir measures the key dirs under the profile-wide incremental ROOT matching session_prefix, and BOTH gc::budget_sweep and cli::status_cmd summed it once per family. Family identity is hash-sensitive (B9), so a crate forks a new family per unit-hash while session_prefix stays the crate name: 66 families all measuring the same yah key dirs, 3356 incremental-dir records collapsing to 380 distinct (path, prefix) pairs, 1978 GB counted for a real 81 GB. This was NOT cosmetic: max-bytes under budget-mode = lru-current-families is compared against exactly this total, so enabling budget mode would have read the ceiling as breached on a tree nowhere near it and retired cold families camp-wide for nothing. Fix is artifacts::size_identity + artifacts::SizeTotal (dedup by the identity owned_size actually measures), used at both totalling sites; budget_sweep charges each identity to the first family claiming it so per-family figures sum to bytes_before. Status now reads 180 GB, matching an independent census (95.4 GB out-dir + 81.2 GB incremental). Test: gc::tests::budget_total_charges_a_shared_incremental_root_once.")
//! @yah:handoff("DISK CRISIS WORK, 2026-08-14 (operator-directed, @Glimmerstone:spade). Rig was at 52 GiB free and falling ~30 GiB/day. Three independent silent failures found, all fixed; 186 GiB reclaimed in one pass (52 to 238 GiB free, 935,881 paths). (1) The launchd janitor dev.yah.cargo-sweep had NEVER reclaimed a byte since install: cargo is not on launchds PATH, so cargo-sweeps internal cargo metadata died with ENOENT and every project was skipped, logging swept 0/24 unreachable=24 on every 6h firing. Reproduced deterministically. Retired per operator direction (one janitor, camp-driven, no bespoke launchd) and both files removed. (2) camp_service::sweep gained the policy it was missing: file-granularity reclaim over deps/ at a 16h cutoff, which is where 190 of the 365 GB sat. Its two existing policies structurally never fire on a worked camp (21-day retention for an active camp; a census found NOTHING older than 7 days anywhere, so this is churn not stale accumulation). Excludes incremental/ deliberately, since deleting a subset of a sessions files yields a corrupt cache and plan_incremental already owns that subtree safely. Also added plan_camps to stop nested camps (oss/mesofact is a camp AND lives under yah) reporting the same target dir twice. (3) sccache was serving a DEAD cache: the one-server-per-user singleton was pinned to a qed worktrees TMPDIR at a 0.18 percent hit rate while the real 43 GB cache sat unused. Restarted pinned to the real dir, verified 74 hits / 104 on a controlled clean-and-rebuild. Structural fix belongs to R744-T2, which now carries the measured evidence. 47 sweep tests pass (7 new), 44 orphan-gc tests pass (1 new).")
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
//!
//! @yah:ticket(R748-B13, "orphan-gc tracked bytes run 1.7-1.8 TB against 150 GiB max-bytes with budget-mode still orphan-only, so nothing enforces the ceiling")
//! @yah:at(2026-08-16T01:25:26Z)
//! @yah:status(open)
//! @yah:assignee(agent:bundle-anthropic-miravel)
//! @yah:parent(R748)
//! @yah:next("Check whether the 1.7-1.8 TB tracked-bytes figure is real (independent census, e.g. du -sh target/) or another size_identity double-count -- the dedup fix already landed for the budget_sweep/status totalling sites (R748-B7/B9 handoffs in gc.rs), so if this total comes from a THIRD site that sums owned_size without SizeTotal's dedup, that is the likely bug, not an actual 11x-over tree.")
//! @yah:next("If the figure is real: decide whether to flip budget-mode to lru-current-families (starts retiring cold families under the ceiling) or raise max-bytes -- an operator call, not a code fix.")
//! @yah:gotcha("Noticed in passing during R770 (2026-08-15): every compile's tracked-bytes check (wrapper.rs run_in, budget_ceiling watermark warning) is reading 1.7-1.8 TB against the camp's 150 GiB max-bytes -- an 11x overage -- on every single compile, and nothing acts on it because [workspace.metadata.orphan-gc] budget-mode is still the default orphan-only, under which max-bytes is a watermark, not a ceiling (config.rs Policy::budget_ceiling returns None unless budget-mode = \"lru-current-families\"). Not chased further; not yet confirmed whether the 1.7-1.8 TB figure itself is real or another instance of the size_identity double-count class (R748-B9's gotcha: hash-sensitive family identity forks a new family per unit-hash while session_prefix stays the crate name, and an earlier occurrence of exactly this dedup bug read 1978 GB for a real 81 GB before the fix in artifacts::size_identity / SizeTotal).")

use std::collections::HashSet;

use anyhow::Result;

use crate::artifacts;
use crate::config::{Mode, Policy};
#[cfg(test)]
use crate::config::BudgetMode;
use crate::lease;
use crate::log::Log;
use crate::state::{now_ms, Store};

#[derive(Default, Debug)]
pub struct SweepReport {
    pub deleted_artifacts: usize,
    pub deleted_bytes: u64,
    /// Records dropped because the path was already gone by the time the
    /// sweep reached it — a race with something else on the same tree, not a
    /// reclaim this sweep can take credit for. Counted separately so
    /// `deleted_bytes` stays a real bytes-freed figure instead of being
    /// diluted toward zero by these no-op hits.
    pub already_gone_artifacts: usize,
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
        self.already_gone_artifacts += other.already_gone_artifacts;
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
    let mode = policy.mode();
    let Some(mut state) = store.load_family(key)? else {
        if mode == Mode::Delete {
            store.clear_pending(key);
        }
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

    let now = now_ms();
    for mut generation in state.orphans.drain(..) {
        // R770: a Lease only protects the extern inputs of a rustc invocation
        // currently running — it has no view of cargo's build plan, so a unit
        // that will need this generation's artifact but hasn't started yet is
        // invisible to `lease::active_inputs`. Give the generation a grace
        // window before any sweep (this family's own, or an unrelated
        // family's `sweep_pending` retry) may delete out of it, so cargo has
        // time to either start that downstream unit — which then leases the
        // path itself — or the window closes without incident. Every artifact
        // defers together, same bucket as an active-lease hit, so a young
        // generation is retried rather than dropped.
        let age = now.saturating_sub(generation.orphaned_unix_ms.unwrap_or(0));
        if age < policy.orphan_grace_ms {
            report.deferred_artifacts += generation.artifacts.len();
            retained_generations.push(generation);
            continue;
        }

        let mut retained_artifacts = Vec::new();
        for artifact in generation.artifacts.drain(..) {
            if current_paths.contains(&artifact.path) {
                // Same path was overwritten/reused by the current generation.
                // It is not an orphan anymore and must never be deleted.
                continue;
            }

            if artifact.kind == artifacts::ArtifactKind::IncrementalDir {
                // An incremental record anchors prefix-scoped session
                // collection on a root shared by the whole workspace; the
                // family never owned that root, so orphaning one must not
                // delete anything. Dropping it (rather than retaining it)
                // keeps the generation able to empty out and retire.
                continue;
            }

            if active.unknown_active_lease || lease::is_active_input(&active, &artifact.path) {
                report.deferred_artifacts += 1;
                retained_artifacts.push(artifact);
                continue;
            }

            // Shadow asks the deletion gate the same question and stops short
            // of the answer, so a would-delete figure can never promise
            // reclamation the real sweep would refuse.
            let outcome = match mode {
                Mode::Shadow => artifacts::check_deletable(&artifact).map(|()| {
                    if artifact.path.exists() {
                        artifacts::Reclaim::Freed(artifacts::path_size(&artifact.path).unwrap_or(0))
                    } else {
                        artifacts::Reclaim::AlreadyGone
                    }
                }),
                Mode::Delete => artifacts::remove(&artifact),
            };
            match outcome {
                Ok(artifacts::Reclaim::Freed(bytes)) => {
                    report.deleted_artifacts += 1;
                    report.deleted_bytes = report.deleted_bytes.saturating_add(bytes);
                    if mode.is_shadow() {
                        retained_artifacts.push(artifact);
                    }
                }
                Ok(artifacts::Reclaim::AlreadyGone) => {
                    report.already_gone_artifacts += 1;
                    if mode.is_shadow() {
                        retained_artifacts.push(artifact);
                    }
                }
                Err(err) => {
                    if policy.verbose {
                        Log::for_store(store).write(&format!(
                            "defer deletion of {}: {err:#}",
                            artifact.path.display()
                        ));
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
    // everything else here. This is the ONLY path that reclaims incremental
    // space — the orphan pass above deliberately skips incremental anchors,
    // because the path they record is the profile-wide root shared by every
    // crate rather than anything this family owns.
    if let Some(current) = state.current.as_ref() {
        for artifact in &current.artifacts {
            if artifact.kind == artifacts::ArtifactKind::IncrementalDir {
                let collected = artifacts::collect_surplus_sessions(
                    &artifact.path,
                    artifact.session_prefix.as_deref(),
                    mode,
                );
                report.collected_sessions += collected.deleted_sessions;
                report.collected_session_bytes = report
                    .collected_session_bytes
                    .saturating_add(collected.deleted_bytes);
            }
        }
    }

    // A shadow sweep is a pure read, and deliberately so: it must not advance
    // the bookkeeping either. Dropping an anchor or clearing `pending` here
    // would mean the first real sweep after `dry-run = false` acted on a
    // different queue than the one the operator read the report for.
    if mode.is_shadow() {
        return Ok(report);
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
        let report = sweep_locked(store, &key, policy)?;
        // This is the ONLY deletion path the wrapper's own per-compile log
        // line never covers (wrapper::run_in discards this function's return
        // value with `let _ =`), so without per-family logging here a
        // background sweep triggered by an unrelated crate's compile deletes
        // silently — no line in `cargo orphan-gc log` ever names the family
        // it touched. Found the hard way (R770): an rmeta went missing for a
        // crate whose own compiles never logged a nonzero deletion all day.
        if policy.verbose
            && (report.deleted_artifacts > 0
                || report.already_gone_artifacts > 0
                || report.deferred_artifacts > 0)
        {
            let label = store
                .load_family(&key)?
                .map(|s| s.label)
                .unwrap_or_else(|| key.clone());
            Log::for_store(store).write(&format!(
                "background sweep of pending family {label}: {} {} artifacts ({} bytes; {} \
                 already gone), deferred {}",
                policy.mode().verb(),
                report.deleted_artifacts,
                report.deleted_bytes,
                report.already_gone_artifacts,
                report.deferred_artifacts,
            ));
        }
        total.absorb(report);
    }
    Ok(total)
}

pub fn sweep_all(store: &Store, policy: &Policy) -> Result<SweepReport> {
    let mut total = SweepReport::default();
    for key in store.family_keys()? {
        // A shadow sweep writes nothing, so it does not take the family lock:
        // `status` reports through this path and must never block behind a
        // live compile holding the lock for a whole rustc invocation.
        let _lock = match policy.mode() {
            Mode::Shadow => None,
            Mode::Delete => Some(store.lock_family(&key)?),
        };
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
    // Size identities already charged to some family, so no byte is counted
    // twice across the whole tree. See the dedup note in the loop below.
    let mut charged: HashSet<(std::path::PathBuf, Option<String>)> = HashSet::new();
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
        // Charge each distinct `size_identity` to the FIRST family that claims
        // it, so the per-family figures sum to exactly `bytes_before` and the
        // `running` arithmetic below stays consistent with the ceiling it is
        // compared against.
        //
        // This dedup is load-bearing, not tidiness. Family identity is
        // hash-sensitive, so a crate forks a new family on every unit-hash
        // change while its `session_prefix` stays the crate name — this camp
        // reached 66 families all measuring the same incremental key dirs, and
        // an undeduped total read 1978 GB for a real 81 GB. Comparing a 24x
        // over-count against `max-bytes` would report the ceiling breached on
        // a tree nowhere near it and retire cold families for nothing.
        //
        // The attribution is first-claim and therefore arbitrary between two
        // families sharing an incremental root: a family charged 0 bytes for a
        // shared dir sorts as cheap to retire, and retiring it frees nothing
        // because the other owner still holds it. That is the safe direction
        // (under-reclaim, never over-delete) and it is bounded by the
        // `leftover` recount after each sweep below.
        let bytes = {
            let mut t = artifacts::SizeTotal::new();
            for a in &current.artifacts {
                if charged.insert(artifacts::size_identity(a)) {
                    t.add(a);
                }
            }
            t.bytes()
        };
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

    // Retirement is the third deletion path, and the only one that can take a
    // generation nothing superseded. In shadow it stops at the plan: which
    // families the LRU order would reach, and how far under the ceiling that
    // would get. Deliberately not simulated further than that — the byte figure
    // assumes each retired family's current artifacts are then fully
    // reclaimable, where a real run may defer some behind an active lease or a
    // path another family still holds, so treat it as the optimistic bound it
    // is. What matters before authorizing is *which* families go, and that is
    // exact.
    if policy.mode().is_shadow() {
        for (_, _, bytes) in &families {
            if running <= ceiling {
                break;
            }
            running = running.saturating_sub(*bytes);
            report.retired_families += 1;
        }
        report.bytes_after = running;
        return Ok(report);
    }

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
        let Some(mut current) = state.current.take() else {
            continue;
        };

        if policy.verbose {
            Log::for_store(store).write(&format!(
                "budget retire {} ({bytes} bytes, idle since {}ms)",
                state.label, state.last_used_unix_ms
            ));
        }

        // Not `now_ms()`: the grace period below exists for the cross-family
        // `sweep_pending` race (an unrelated compile's background retry
        // outrunning cargo's build plan by minutes). Budget retirement has no
        // such gap — it sweeps the SAME generation it just orphaned, under
        // the SAME lock, in the SAME call, re-checking active leases fresh —
        // so stamp it pre-aged and let it reclaim on the ceiling's schedule
        // rather than the race mitigation's.
        current.orphaned_unix_ms = Some(0);
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
                    .map(|a| artifacts::owned_size(a).unwrap_or(0))
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
            orphaned_unix_ms: None,
        });
        store.save_family(&state).unwrap();
    }

    /// A policy that actually deletes. `dry-run` defaults to true (shadow), so
    /// every test that asserts reclamation has to say so — which is the
    /// behaviour R748-F6 is buying, spelled out at each fixture.
    fn policy(max_bytes: Option<u64>, mode: BudgetMode) -> Policy {
        Policy {
            enabled: true,
            dry_run: false,
            max_bytes,
            budget_mode: mode,
            ..Policy::default()
        }
    }

    fn shadow_of(policy: &Policy) -> Policy {
        Policy { dry_run: true, ..policy.clone() }
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

    /// Cargo passes `-C incremental=<profile-wide root>`, not a per-crate
    /// directory — rustc creates the key dir underneath. So an incremental
    /// record names a path shared by every crate in the workspace, and
    /// orphaning one must reclaim nothing.
    ///
    /// Found live: installing on a ten-agent camp recorded
    /// `target/debug/incremental` as an owned artifact of a single crate, and
    /// the orphan pass would have deleted 17 GB of everyone else's incremental
    /// state whole. Invariant D did not cover it — D compares against the same
    /// family's current paths, and a family that stops listing the anchor
    /// hands it straight to the deletion path.
    #[test]
    fn an_orphaned_incremental_anchor_never_deletes_the_shared_root() {
        let tmp = tempfile::tempdir().unwrap();
        let store = store_in(tmp.path());
        let incr = tmp.path().join("incremental");
        // Another crate's key dir, of the kind the camp had 611 of.
        let foreign_key = incr.join("someone-else-abc123");
        fs::create_dir_all(foreign_key.join("s-100-aaa")).unwrap();
        fs::write(foreign_key.join("s-100-aaa/dep-graph.bin"), b"not yours").unwrap();

        let key = blake3::hash(b"anchor").to_hex().to_string();
        let mut state = FamilyState::new(&key, "mycrate");
        // Orphaned, and the current generation does not list it — exactly the
        // shape that reaches `artifacts::remove`.
        state.orphans.push(Generation {
            id: "old".into(),
            created_unix_ms: now_ms(),
            artifacts: vec![OwnedArtifact {
                path: incr.clone(),
                root: incr.clone(),
                kind: ArtifactKind::IncrementalDir,
                session_prefix: Some("mycrate".into()),
            }],
            orphaned_unix_ms: Some(0),
        });
        store.save_family(&state).unwrap();

        let report = {
            let _lock = store.lock_family(&key).unwrap();
            sweep_locked(&store, &key, &policy(None, BudgetMode::OrphanOnly)).unwrap()
        };

        assert_eq!(report.deleted_artifacts, 0, "{report:?}");
        assert!(incr.is_dir(), "the shared incremental root must survive");
        assert!(
            foreign_key.join("s-100-aaa/dep-graph.bin").exists(),
            "another crate's session must survive"
        );
        // And the anchor is dropped rather than queued forever.
        let after = store.load_family(&key).unwrap().unwrap();
        assert!(after.orphans.iter().all(|g| g.artifacts.is_empty()));
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
                orphaned_unix_ms: None,
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
            orphaned_unix_ms: Some(0),
        });
        state.current = Some(Generation {
            id: "new".into(),
            created_unix_ms: now_ms(),
            artifacts: vec![artifact(&reused)],
            orphaned_unix_ms: None,
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

    /// R770 — a Lease only protects the `--extern` inputs of a rustc
    /// invocation that is *currently running*; it has no view of cargo's
    /// build plan, so `sweep_pending`'s cross-family background retries can
    /// reach a generation minutes before the downstream unit that still needs
    /// it has even started (measured on this camp: a 6-9 minute vanish
    /// window). A freshly-orphaned generation must defer whole, not just its
    /// individual artifacts, until `orphan_grace_ms` has elapsed — and an aged
    /// one must sweep exactly as before.
    #[test]
    fn a_generation_orphaned_within_the_grace_period_is_deferred_whole() {
        let tmp = tempfile::tempdir().unwrap();
        let store = store_in(tmp.path());
        let deps = tmp.path().join("deps");
        fs::create_dir_all(&deps).unwrap();
        let doomed = deps.join("libfoo-abc.rlib");
        fs::write(&doomed, vec![0u8; 100]).unwrap();

        let artifact = OwnedArtifact {
            path: doomed.clone(),
            root: deps.clone(),
            kind: ArtifactKind::OutDirEntry,
            session_prefix: None,
        };
        let key = blake3::hash(b"grace").to_hex().to_string();
        let mut state = FamilyState::new(&key, "foo");
        state.orphans.push(Generation {
            id: "old".into(),
            created_unix_ms: now_ms(),
            artifacts: vec![artifact],
            orphaned_unix_ms: Some(now_ms()), // orphaned just now
        });
        store.save_family(&state).unwrap();

        let graced_policy = Policy { orphan_grace_ms: 60_000, ..policy(None, BudgetMode::OrphanOnly) };
        let report = {
            let _lock = store.lock_family(&key).unwrap();
            sweep_locked(&store, &key, &graced_policy).unwrap()
        };
        assert_eq!(report.deleted_artifacts, 0, "{report:?}");
        assert_eq!(report.deferred_artifacts, 1, "{report:?}");
        assert!(doomed.exists(), "a generation inside its grace window must survive");
        let after = store.load_family(&key).unwrap().unwrap();
        assert_eq!(
            after.orphans.iter().flat_map(|g| g.artifacts.iter()).count(),
            1,
            "still queued for retry once the grace period elapses"
        );

        // Same fixture, aged past the grace period: sweeps exactly as an
        // ungraced orphan would.
        let mut state = store.load_family(&key).unwrap().unwrap();
        for generation in &mut state.orphans {
            generation.orphaned_unix_ms = Some(0);
        }
        store.save_family(&state).unwrap();
        let report = {
            let _lock = store.lock_family(&key).unwrap();
            sweep_locked(&store, &key, &graced_policy).unwrap()
        };
        assert_eq!(report.deleted_artifacts, 1, "{report:?}");
        assert!(!doomed.exists(), "an aged-out generation sweeps normally");
    }

    /// An orphan whose file is already gone by the time the sweep reaches it
    /// (something else won the race — routine in a concurrent camp) must not
    /// be counted as a `deleted_bytes`-bearing reclaim: that conflation is
    /// what made every real deletion line in the operational log read
    /// `(0 bytes)`, because `AlreadyGone` hits vastly outnumber real `Freed`
    /// ones and summed together as one figure they round to zero.
    #[test]
    fn an_orphan_already_gone_from_disk_is_counted_separately_from_a_real_reclaim() {
        let tmp = tempfile::tempdir().unwrap();
        let store = store_in(tmp.path());
        let deps = tmp.path().join("deps");
        fs::create_dir_all(&deps).unwrap();
        let real = deps.join("libfoo-real.rlib");
        let already_gone = deps.join("libfoo-gone.rlib");
        fs::write(&real, vec![0u8; 4096]).unwrap();
        // `already_gone` is recorded but never written — simulates a path
        // some other process (or an earlier sweep) already removed.

        let artifact = |path: &std::path::Path| OwnedArtifact {
            path: path.to_path_buf(),
            root: deps.clone(),
            kind: ArtifactKind::OutDirEntry,
            session_prefix: None,
        };
        let key = blake3::hash(b"already-gone").to_hex().to_string();
        let mut state = FamilyState::new(&key, "foo");
        state.orphans.push(Generation {
            id: "old".into(),
            created_unix_ms: now_ms(),
            artifacts: vec![artifact(&real), artifact(&already_gone)],
            orphaned_unix_ms: Some(0),
        });
        store.save_family(&state).unwrap();

        let report = {
            let _lock = store.lock_family(&key).unwrap();
            sweep_locked(&store, &key, &policy(None, BudgetMode::OrphanOnly)).unwrap()
        };

        assert_eq!(report.deleted_artifacts, 1, "{report:?}");
        assert_eq!(report.deleted_bytes, 4096, "the real reclaim, not diluted to zero");
        assert_eq!(report.already_gone_artifacts, 1, "{report:?}");
        assert!(!real.exists(), "the real artifact was actually removed");

        // Shadow mode must draw the same distinction in its preview.
        let store2 = store_in(&tmp.path().join("shadow"));
        let mut state2 = FamilyState::new(&key, "foo");
        fs::write(&real, vec![0u8; 4096]).unwrap();
        state2.orphans.push(Generation {
            id: "old".into(),
            created_unix_ms: now_ms(),
            artifacts: vec![artifact(&real), artifact(&already_gone)],
            orphaned_unix_ms: Some(0),
        });
        store2.save_family(&state2).unwrap();
        let shadow_report = {
            let _lock = store2.lock_family(&key).unwrap();
            sweep_locked(&store2, &key, &shadow_of(&policy(None, BudgetMode::OrphanOnly))).unwrap()
        };
        assert_eq!(shadow_report.deleted_bytes, 4096, "{shadow_report:?}");
        assert_eq!(shadow_report.already_gone_artifacts, 1, "{shadow_report:?}");
        assert!(real.exists(), "shadow mode must not delete anything");
    }

    /// R748-F6 — deletion path 1 of 3. A shadow sweep must report the figure a
    /// real sweep would produce and leave the tree byte-identical, including
    /// the bookkeeping: the operator has to be able to read the report, decide,
    /// and then get exactly what was promised.
    #[test]
    fn a_shadow_sweep_reports_what_a_real_sweep_would_delete_and_deletes_nothing() {
        let tmp = tempfile::tempdir().unwrap();
        let store = store_in(tmp.path());
        let deps = tmp.path().join("deps");
        fs::create_dir_all(&deps).unwrap();
        let doomed = deps.join("libfoo-abc.rlib");
        let reused = deps.join("libfoo-abc.d");
        fs::write(&doomed, vec![0u8; 4096]).unwrap();
        fs::write(&reused, vec![0u8; 100]).unwrap();

        let artifact = |path: &std::path::Path| OwnedArtifact {
            path: path.to_path_buf(),
            root: deps.clone(),
            kind: ArtifactKind::OutDirEntry,
            session_prefix: None,
        };
        let key = blake3::hash(b"shadow").to_hex().to_string();
        let mut state = FamilyState::new(&key, "foo");
        state.orphans.push(Generation {
            id: "old".into(),
            created_unix_ms: now_ms(),
            artifacts: vec![artifact(&doomed), artifact(&reused)],
            orphaned_unix_ms: Some(0),
        });
        state.current = Some(Generation {
            id: "new".into(),
            created_unix_ms: now_ms(),
            artifacts: vec![artifact(&reused)],
            orphaned_unix_ms: None,
        });
        store.save_family(&state).unwrap();
        let real_policy = policy(None, BudgetMode::OrphanOnly);

        let before = fs::read_dir(&deps).unwrap().count();
        let shadow = sweep_locked(&store, &key, &shadow_of(&real_policy)).unwrap();

        assert_eq!(shadow.deleted_artifacts, 1, "{shadow:?}");
        assert_eq!(shadow.deleted_bytes, 4096, "the real byte figure, not an estimate");
        assert_eq!(fs::read_dir(&deps).unwrap().count(), before, "no file left the tree");
        assert!(doomed.exists());
        // State must not advance either: a second shadow sweep has to report
        // the same thing, and the queue must be intact when deletion is
        // authorized later.
        let after_state = store.load_family(&key).unwrap().unwrap();
        assert_eq!(
            after_state.orphans.iter().flat_map(|g| g.artifacts.iter()).count(),
            2,
            "shadow mode moves nothing in the orphan queue"
        );
        assert_eq!(
            sweep_locked(&store, &key, &shadow_of(&real_policy)).unwrap().deleted_artifacts,
            1,
            "a shadow sweep is idempotent"
        );

        // The promise, kept: the same fixture swept for real.
        let real = {
            let _lock = store.lock_family(&key).unwrap();
            sweep_locked(&store, &key, &real_policy).unwrap()
        };
        assert_eq!(real.deleted_artifacts, shadow.deleted_artifacts);
        assert_eq!(real.deleted_bytes, shadow.deleted_bytes);
        assert!(!doomed.exists());
        assert!(reused.exists(), "a reused path is still never deleted");
    }

    /// R748-F6 — deletion path 3 of 3. Budget retirement is the only authority
    /// that can take a generation nothing superseded, so shadow has to reach it
    /// too: report which families the ceiling would cost, retire none of them,
    /// and leave every current generation in place.
    #[test]
    fn a_shadow_budget_sweep_plans_retirements_without_taking_any() {
        let tmp = tempfile::tempdir().unwrap();
        let store = store_in(tmp.path());
        let deps = tmp.path().join("deps");
        family(&store, &deps, "hot", 1000, 1_000);
        family(&store, &deps, "warm", 1000, 60_000);
        family(&store, &deps, "cold", 1000, 900_000);
        let real_policy = policy(Some(2400), BudgetMode::LruCurrentFamilies);

        let plan = budget_sweep(&store, &shadow_of(&real_policy)).unwrap();

        assert_eq!(plan.retired_families, 1, "{plan:?}");
        assert!(plan.bytes_after <= 2400);
        assert!(deps.join("libcold-abc123.rlib").exists(), "shadow retires nothing");
        for name in ["hot", "warm", "cold"] {
            let key = blake3::hash(name.as_bytes()).to_hex().to_string();
            let state = store.load_family(&key).unwrap().unwrap();
            assert!(state.current.is_some(), "{name} must still hold its generation");
            assert!(state.orphans.is_empty(), "{name} must not be queued for deletion");
        }

        // And the plan is what the real run then executes.
        let real = budget_sweep(&store, &real_policy).unwrap();
        assert_eq!(real.retired_families, plan.retired_families);
        assert!(!deps.join("libcold-abc123.rlib").exists());
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
            orphaned_unix_ms: Some(0),
        });
        store.save_family(&state).unwrap();

        let report = {
            let _lock = store.lock_family(&key).unwrap();
            sweep_locked(&store, &key, &policy(None, BudgetMode::OrphanOnly)).unwrap()
        };

        assert_eq!(report.deleted_artifacts, 0, "{report:?}");
        // Only the two *deletable* kinds queue for retry. An incremental record
        // is an anchor on a root shared by the whole workspace, never an owned
        // object, so orphaning one is a no-op rather than a deferred deletion —
        // queuing it would retry forever against something that can never
        // become deletable.
        assert_eq!(report.deferred_artifacts, 2, "both deletable kinds stay queued");
        assert!(deep.exists());
        assert!(stray_emit.exists());
        assert!(incr.exists(), "the shared incremental root must survive");
        let after = store.load_family(&key).unwrap().unwrap();
        assert_eq!(
            after.orphans.iter().flat_map(|g| g.artifacts.iter()).count(),
            2,
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

    /// Two families that both record the SAME incremental root with the SAME
    /// session prefix measure the same bytes; the budget total must charge
    /// those bytes once.
    ///
    /// This is the shape hash-sensitive family identity produces constantly —
    /// a crate forks a family on every unit-hash change while its
    /// `session_prefix` stays the crate name. Measured on the dogfooding camp
    /// 2026-08-14: 66 families for `yah` alone, 3356 incremental-dir records
    /// collapsing to 380 distinct (path, prefix) pairs, and a naive total of
    /// 1978 GB against a real 81 GB. Undeduped, `max-bytes` reads as breached
    /// on a tree nowhere near the ceiling and retires cold families for
    /// nothing.
    #[test]
    fn budget_total_charges_a_shared_incremental_root_once() {
        let tmp = tempfile::tempdir().unwrap();
        let store = store_in(tmp.path());

        // One incremental root with a single `demo-*` key dir of known size,
        // recorded by two separate families exactly as the wrapper does.
        let incremental = tmp.path().join("incremental");
        let key_dir = incremental.join("demo-abc123");
        fs::create_dir_all(&key_dir).unwrap();
        fs::write(key_dir.join("dep-graph.bin"), vec![0u8; 8192]).unwrap();

        for (n, name) in ["one", "two"].iter().enumerate() {
            let artifacts = vec![OwnedArtifact {
                path: incremental.clone(),
                root: incremental.clone(),
                kind: ArtifactKind::IncrementalDir,
                session_prefix: Some("demo".to_string()),
            }];
            let key = blake3::hash(name.as_bytes()).to_hex().to_string();
            let mut state = FamilyState::new(&key, name);
            state.last_used_unix_ms = now_ms() - (n as u128 + 1) * 1000;
            state.current = Some(Generation {
                id: crate::state::generation_id(&artifacts),
                created_unix_ms: now_ms(),
                artifacts,
                orphaned_unix_ms: None,
            });
            store.save_family(&state).unwrap();
        }

        // A ceiling far above the real 8 KiB, but below what a double count
        // would report. Deduped this is inert; undeduped it retires a family.
        let report =
            budget_sweep(&store, &policy(Some(12_288), BudgetMode::LruCurrentFamilies)).unwrap();

        assert_eq!(
            report.bytes_before, 8192,
            "the shared key dir must be charged once, not once per family"
        );
        assert_eq!(
            report.retired_families, 0,
            "a tree under the ceiling must retire nothing"
        );
    }
}
