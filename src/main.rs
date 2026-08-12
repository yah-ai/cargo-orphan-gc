//! @yah:ticket(R748-T5, "Publish readiness: mirror, publish = true, and an honest README")
//! @yah:at(2026-08-11T07:49:46Z)
//! @yah:next("Both gates are now met (F1: chain landed + A/B/C verified; T3: 20 tests, invariant_a..g greppable) and the reclamation-zero gotcha is cleared: F2 landed and the kamaji-copy demo reclaimed 3 surplus sessions / 44 MB real (du) - 24% of that incremental tree - in one sweep at zero rebuild cost, with the edit loop at 0.95s after. README rewritten to lead with the gap (bins + incremental are refused by compiler caches and governed by nothing); Cargo.toml carries full publish metadata (license MIT, repository, keywords, categories). What remains is the operator-facing release sequence, in order: (1) gh repo create yah-ai/cargo-orphan-gc --public, (2) scripts/export-oss.sh orphan-gc [check the script knows this subtree - it is newer than the script's repo list], (3) flip publish = false to true, (4) cargo publish. Steps 1/2/4 are outward-facing; do not take them without operator confirmation.")
//! @yah:assignee(agent:bundle-anthropic-ashguard)
//! @yah:parent(R748)
//! @yah:next("Last, and gated on -F1 (it does not work alongside sccache yet) and -T3 (unproven deletion authority). Mechanics: Cargo.toml still carries publish = false and version 0.1.0; the crates.io name cargo-orphan-gc is unregistered (404, sanity-checked against cargo-sweep 200). oss/orphan-gc has no GitHub mirror, so its first scripts/export-oss.sh run needs `gh repo create yah-ai/cargo-orphan-gc --public` first — same first-time-export path srcgraph took. Follow the oss/ rule in CLAUDE.md: the monorepo is source of truth, changes flow outward through export-oss.sh, never edit the mirror.\n\nThe README should lead with the gap rather than the mechanism: cargo has no target-dir GC; sccache and friends cache what they can, and structurally REFUSE --crate-type bin and -C incremental; those refusals are the largest artifacts in a working target dir. State the safety invariants and their cost plainly, including that orphan-only mode cannot enforce a ceiling and that budget mode trades Invariant A for one.")
//! @yah:gotcha("Do not publish while the measured reclamation on realistic workloads is zero. Four scenarios today returned 0 bytes, and -F2 is the ticket that changes that. A GC tool whose headline number is 0 will be judged on that number, and the second impression is much more expensive than the first.")
//! @yah:depends_on(R748-F1)
//! @yah:depends_on(R748-T3)

mod artifacts;
mod bootstrap;
mod cli;
mod config;
mod family;
mod gc;
mod lease;
mod state;
mod wrapper;

use std::ffi::OsString;
use std::process;

use anyhow::{anyhow, Result};

fn main() {
    let code = match dispatch() {
        Ok(code) => code,
        Err(err) => {
            eprintln!("cargo-orphan-gc: {err:#}");
            2
        }
    };
    process::exit(code);
}

fn dispatch() -> Result<i32> {
    let mut args = std::env::args_os();
    let _exe = args.next();
    let first = args
        .next()
        .ok_or_else(|| anyhow!("expected Cargo subcommand invocation or rustc wrapper invocation"))?;

    // Cargo custom subcommands are invoked as:
    //   cargo-orphan-gc orphan-gc <args...>
    // Cargo rustc wrappers are invoked as:
    //   cargo-orphan-gc <path-to-real-rustc> <rustc-args...>
    if first == OsString::from("orphan-gc") {
        cli::run(args.collect())?;
        Ok(0)
    } else {
        wrapper::run(first, args.collect())
    }
}
