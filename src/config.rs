//! @yah:ticket(R748-F6, "Shadow mode: learn and report without deleting, so a shared tree can evaluate before authorizing")
//! @yah:at(2026-08-12T23:41:24Z)
//! @yah:status(review)
//! @yah:assignee(agent:bundle-anthropic-ashguard)
//! @yah:parent(R748)
//! @yah:next("Tier: Warrior — the flag itself is small, but it has to reach EVERY deletion site or it lies: gc::sweep_locked, budget retirement, and the F2 surplus-session collector are three separate paths.")
//! @yah:next("Add dry-run = false to Policy (config.rs). When true: learn ownership, record generations, mark orphans, and report — delete nothing. This is the missing rung between enabled = false (fully transparent, learns nothing, wrapper.rs:53) and enabled = true (deletes on a ten-agent shared tree).")
//! @yah:next("Report through the existing surfaces: cargo orphan-gc status should show what a real sweep WOULD reclaim, and the verbose per-compile line should say 'would delete' rather than 'deleted'. A number the operator can read before authorizing is the whole point.")
//! @yah:next("Test it the way the invariants are tested: a dry-run sweep over a fixture with known orphans deletes zero bytes and reports the same non-zero figure a real sweep would.")
//! @yah:verify("A dry-run sweep on a tree with known surplus reports non-zero reclaimable bytes and leaves every file in place (assert by file count and by du, not just by the report).")
//! @yah:verify("Grep the three deletion paths and confirm each is gated: sweep_locked, budget retirement, surplus-session collection.")
//! @yah:gotcha("Motivating numbers, measured 2026-08-11: yah carries 185 surplus incremental sessions across 612 keys (24 GB incremental of a 51 GB target); noisetable carries 329 across 605 keys (28 GB of 47 GB). Both are candidate adopters and both are shared trees, which is exactly why authorizing deletion needs a look-first mode.")
//! @yah:assumes("That the adoption order is yah first, then noisetable — chosen because yah's ten concurrent agents generate the load that finds bugs fastest, and because W306's one surviving objection (per-family contention at root-workspace scale) can only be measured there.")
//! @yah:next("RE-SCOPED after R748-B9: shadow must be the DEFAULT on first install, not an opt-in flag. B9 was a live-artifact deletion that broke a peer's whole-workspace check on a shared tree, and shadow would have surfaced it with zero blast radius. Concretely: bootstrap should write dry-run = true, and turning it off should be a deliberate second step taken after reading a report.")
//! @yah:next("ORDER OF WORK: land R748-B10 FIRST. Shadow mode's entire value is its report, and today's report channel is the compiler's stderr, which cargo caches per-unit and replays out of context - it has already misled three readers in one day. Building shadow mode on that channel would make a tool whose only output is a lie about which invocation it describes.")
//! @yah:gotcha("Do not verify shadow mode (or anything else on this crate) with a cached build. Three consecutive `cargo check --workspace --all-targets` runs in oss/yubaba exited 0 while compiling NOTHING, and their orphan-gc output was entirely replayed - it looked like a clean verification and proved nothing. The reliable discriminator is side-effect state: family count under $CARGO_HOME/orphan-gc/workspaces/*/families before and after, not text on stdout.")
//! @yah:handoff("Landed, and as the DEFAULT rather than a flag: Policy::dry_run defaults to true, so a manifest that says only enabled = true learns and reports and deletes nothing. bootstrap writes dry-run = true explicitly (and never reverts an existing dry-run = false).")
//! @yah:verify("37 unit + 2 integration tests green; clippy --all-targets silent.")
//! @yah:handoff("All three deletion paths are gated, per the ticket's own warning that a flag reaching only one would lie: gc::sweep_locked_protecting (orphan supersession), artifacts::collect_surplus_sessions (now takes a Mode), and gc::budget_sweep (plans retirements, takes none).")
//! @yah:verify("Shadow reports the SAME figure the real sweep then delivers, because it reuses the real decision path: artifacts::validate is now pub check_deletable and shadow calls exactly it, and the surplus term reuses finalized_sessions. Tests assert the equality both ways round (gc::a_shadow_sweep_reports_what_a_real_sweep_would_delete_and_deletes_nothing, artifacts::shadow_reports_the_same_surplus_it_would_have_collected).")
//! @yah:handoff("A shadow sweep is a PURE READ: no save_family, no mark_pending/clear_pending, no anchor drop, and it takes no family lock (so status never blocks behind a live compile). That is what makes the report honest - two shadow sweeps report the same thing, and the queue the operator read about is the queue that gets reclaimed when they flip the flag.")
//! @yah:verify("End-to-end on the shipped binary through real cargo (/tmp/r748-e2e.sh): fresh install reported 'would reclaim now: 0 orphan artifacts + 1 surplus incremental sessions (25753 bytes)' with the session count on disk unchanged at 2 across both status and sweep; after setting dry-run = false the real sweep collected exactly that one session (2 to 1) for the identical byte figure.")
//! @yah:handoff("Surfaces: status prints the mode, the log path, a 'would reclaim now' line and (in budget mode) which families a ceiling would retire; sweep gained --dry-run, which can only ever make a sweep less destructive than the policy; the per-compile line says 'would delete' via Mode::verb so no formatting site can claim a deletion that did not happen. Docs updated: ARCHITECTURE 11.1 (three-rung table + why the report is trustworthy) and 11.2, README setup step 4, examples/workspace-Cargo.toml.")
//! @yah:cleanup("Shadow's budget-retirement byte figure is an optimistic bound - it names exactly which families a ceiling would retire but assumes their bytes are then fully reclaimable, where a real run may defer some behind an active lease. Stated in ARCHITECTURE 11.1 and README known-limitations. The orphan and surplus-session figures are exact.")
//! @yah:handoff("OVERHEAD (operator asked to measure and optimize, 2026-08-12). Measured per rustc invocation against a no-op compiler, side-effect verified: the dominant cost was artifacts::collect walking the out-dir, which is shared by the whole workspace and therefore linear in everyone's accumulated output. 1k entries 7.1ms / 50k 33.5ms / 200k 145ms. The camp's target/debug/deps holds 210,770 entries, so it was paying ~145ms on EVERY tracked compile.")
//! @yah:handoff("Two findings from attacking it. (1) The loop's allocations were NOT the problem: removing them (a String per entry plus up to five format!s in the predicate, ~1M allocations per compile at that scale) bought only 145 to 117ms. (2) The walk is at the FILESYSTEM's floor - Rust's fs::read_dir costs 551 ns/entry on that directory, 3.5x FASTER than a C-level scandir at 1934 ns/entry. There is no implementation left to improve; the walk can only be skipped.")
//! @yah:verify("After: flat ~6ms regardless of out-dir size (1k 5.8 / 50k 5.6 / 200k 6.7), and 14.1ms amortized over 48 compiles at 200k entries including the periodic full scans. 145ms to 14ms, a 10x cut, and no longer scaling with tree age. 43 tests green, clippy silent.")
//! @yah:handoff("The fix: artifacts::collect now takes the previous generation's artifact list and reuses it when every one of those files still exists (one stat each), instead of walking. New policy knob full-scan-every (default 16) forces a real walk periodically; a missing recorded path forces one immediately. Safety argument, which is what makes it a perf knob rather than a risk: reuse can only UNDER-record ownership, and an unrecorded file is one this tool never deletes (Invariant B) - the failure mode is a leak, which is the fallback the design already takes whenever ownership is uncertain. FamilyState gained compiles_since_scan (serde-defaulted, old state files load unchanged). Documented in ARCHITECTURE 11.3.")
//! @yah:verify("New tests pin both halves: wrapper::a_newly_emitted_artifact_is_picked_up_at_the_next_full_scan (the fast path misses it, the scheduled scan learns it, nothing is deleted meanwhile) and wrapper::a_missing_recorded_artifact_forces_a_walk_before_the_counter_is_due. artifacts::ownership_matching_is_byte_exact_and_keeps_its_asymmetry pins the rewritten predicate, including that lib<stem>-... is deliberately NOT owned - widening that would widen what the tool deletes.")

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

/// Whether a sweep is allowed to delete, or only to report what it would.
///
/// Shadow is a third rung between the two the tool used to have. `enabled =
/// false` is fully transparent and learns nothing, so it can never tell you
/// what adopting the tool would cost you; `enabled = true` deletes, which on a
/// shared tree is a decision that has to be made *before* any evidence exists.
/// Shadow learns ownership, records generations, marks orphans and reports —
/// and deletes nothing, so the evidence arrives with zero blast radius.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    Shadow,
    Delete,
}

impl Mode {
    pub fn is_shadow(self) -> bool {
        self == Mode::Shadow
    }

    /// The verb for a report line, so one formatting site cannot claim a
    /// deletion that did not happen.
    pub fn verb(self) -> &'static str {
        match self {
            Mode::Shadow => "would delete",
            Mode::Delete => "deleted",
        }
    }
}

#[derive(Clone, Debug, Deserialize)]
#[serde(default, rename_all = "kebab-case")]
pub struct Policy {
    pub enabled: bool,
    /// Report what a sweep would reclaim; delete nothing. **Defaults to true**,
    /// including for a manifest that sets `enabled = true` and says nothing
    /// else: turning deletion on is a second, deliberate edit made after
    /// reading a report. R748-B9 is why — a live-artifact deletion broke every
    /// agent's build on this camp, and shadow would have surfaced it for free.
    pub dry_run: bool,
    pub verbose: bool,
    /// Force a full out-dir walk every N compiles of a family; in between, the
    /// previous generation's artifact list is reused when every one of its
    /// files is still present.
    ///
    /// This exists because the walk is the tool's dominant per-compile cost and
    /// is linear in the size of a directory shared by the whole workspace —
    /// 116 ms at 210,770 entries, measured, and that is the filesystem's floor
    /// rather than something implementable away. Reuse can only under-record
    /// ownership, and an unrecorded file is one this tool never deletes, so the
    /// tradeoff is reclamation latency against per-compile cost, never safety.
    /// `0` disables the fast path entirely (walk every time).
    pub full_scan_every: u32,
    pub pending_sweeps_per_compile: usize,
    /// Minimum time between `sweep_pending` runs, in milliseconds, shared
    /// across every concurrent compile in the workspace via a marker file —
    /// see [`Store::due_for_pending_sweep`](crate::state::Store::due_for_pending_sweep).
    /// `sweep_pending` is a fixed cost (35-90ms measured live, independent of
    /// the unit being compiled) that ran on every single successful compile
    /// before this existed, which is a 20-77% latency tax on fast/incremental
    /// compiles specifically — the common case in an active edit-check loop.
    /// Throttling it costs reclamation latency, not safety: the mechanism
    /// exists to close a 6-9 minute vanish window (R770), so a few seconds of
    /// delay spends a small fraction of that margin. `0` disables throttling
    /// (run on every compile, the old behavior).
    pub pending_sweep_min_interval_ms: u128,
    /// How long a generation must sit in the orphan queue before a sweep may
    /// delete it, in milliseconds.
    ///
    /// A [`Lease`](crate::lease::Lease) only protects the `--extern` inputs of
    /// a rustc invocation that is *currently running*; it has no visibility
    /// into cargo's build plan, so a unit that will need this generation's
    /// `.rmeta` as `--extern` but has not started yet is invisible to
    /// `lease::active_inputs`. `sweep_pending` (R770) closes exactly that gap:
    /// it retries orphaned families from *unrelated* compiles, so a family can
    /// be swept seconds after its own compile queued it, well before cargo
    /// schedules the downstream unit that still needs it — a live rmeta
    /// vanishes with `extern location ... does not exist` naming a unit whose
    /// own compile logged zero deletions. Measured on this camp (R770,
    /// 2026-08-15): a 6-9 minute window from write to vanish. This grace
    /// period is the mitigation: a generation younger than it is treated as
    /// deferred (same bucket as an active-lease hit), not deletable, giving
    /// cargo's own scheduler time to either start the downstream unit (which
    /// then leases the path itself) or move on. It narrows the race, it does
    /// not close it — a build stalled longer than the grace period is still
    /// exposed.
    pub orphan_grace_ms: u128,
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
            dry_run: true,
            verbose: false,
            full_scan_every: 16,
            pending_sweeps_per_compile: 4,
            // 5s: comfortably inside an edit-check loop's compile cadence, and
            // orders of magnitude under the 30-minute grace period below.
            pending_sweep_min_interval_ms: 5_000,
            // 30 minutes: comfortably past the 6-9 minute vanish window R770
            // measured, and short enough that a real leak still ages out of
            // the queue in one dev session rather than accumulating forever.
            orphan_grace_ms: 30 * 60 * 1000,
            max_bytes: None,
            budget_mode: BudgetMode::OrphanOnly,
            inner_wrapper: None,
        }
    }
}

impl Policy {
    pub fn mode(&self) -> Mode {
        if self.dry_run {
            Mode::Shadow
        } else {
            Mode::Delete
        }
    }

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
            return Ok(Some(WorkspaceConfig { root: dir.to_path_buf(), policy }));
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
                package_fallback = Some(WorkspaceConfig { root: dir.to_path_buf(), policy });
            }
        }
    }

    Ok(package_fallback)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn policy_from(toml_src: &str) -> Policy {
        toml_src.parse::<toml::Value>().unwrap().try_into().unwrap()
    }

    /// The R748-F6 contract, and the reason it is a *default* rather than a
    /// flag: turning the tool on must not, by itself, authorize deletion.
    #[test]
    fn enabling_the_tool_does_not_by_itself_authorize_deletion() {
        let policy = policy_from("enabled = true\n");
        assert!(policy.enabled);
        assert_eq!(policy.mode(), Mode::Shadow);
    }

    #[test]
    fn deletion_is_authorized_by_an_explicit_dry_run_false() {
        assert_eq!(policy_from("enabled = true\ndry-run = false\n").mode(), Mode::Delete);
    }
}
