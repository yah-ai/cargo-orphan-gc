//! @yah:ticket(R748-B9, "Hash-insensitive family identity deletes a live artifact mid-invocation when one crate has two live unit hashes")
//! @yah:at(2026-08-13T00:35:58Z)
//! @yah:status(review)
//! @yah:assignee(agent:claude)
//! @yah:parent(R748)
//! @yah:severity(critical)
//! @yah:handoff("Reported on R742-F1: `cargo check --workspace --all-targets` in oss/yubaba exits 101 with the wrapper installed and 0 without it, failing on a different crate each run, with errors of the form `extern location for task_runs does not exist: .../libtask_runs-226ccde4fd02e56f.rmeta` immediately after this tool logged deleting that crate's artifacts.")
//! @yah:handoff("Root cause (strong evidence, not yet proven end-to-end): family.rs::parse_codegen STRIPPED -C metadata and never pushed -C extra-filename into the family key, by design - 'so a rebuilt version maps back to the same logical family'. That is only sound if at most one unit hash of a crate is live at a time. Feature unification builds the same crate several ways inside ONE cargo invocation, so compile #2 superseded compile #1 and deleted a .rmeta that units still queued in that same build link against. Only cross-workspace path deps (oss/yah-base, oss/qed, oss/kamaji, reached via [patch.crates-io]) were hit; crates native to oss/yubaba deleted nothing, which fits - those are the ones built several ways.")
//! @yah:handoff("Fix landed: unit hash is now part of family identity. Supersession fires only when cargo genuinely rebuilds the same unit; a differently-configured build forks a family and leaks, which is the fallback this tool is supposed to take when liveness is ambiguous. Reclamation that actually pays is untouched - surplus-session collection keys on the incremental dir, and budget mode retires cold families by LRU, which is the safe way to reap a stale hash.")
//! @yah:handoff("IMPORTANT - the fix is unit-tested but NOT verified against the real failure. My three end-to-end runs in oss/yubaba all exited 0, but every one was a fully cached no-op (run 3: 0 compiles, 0.52s). The 'deleted N artifacts' lines in those logs are cargo REPLAYING each unit's cached stderr, not fresh deletions - byte counts were identical across runs for that reason. Treat verbose wrapper output as evidence only when the unit actually compiled.")
//! @yah:handoff("Tree anchor at handoff: cf7a7291bd6c6b29528131c852002fcb6fee0d00 — the shared tree as I left it. Diff against it (`git diff cf7a7291bd6c6b29528131c852002fcb6fee0d00..HEAD`) to see what landed under you, and quote this SHA rather than 'HEAD' in any revert/restore instruction.")
//! @yah:next("Verify against a real recompile, in a COPY rather than the shared camp tree: scripts/adoption-demo.sh copies a workspace, but oss/yubaba reaches siblings through [patch.crates-io] relative paths, so oss/{yah-base,qed,kamaji} must be copied alongside it first. Then force the cross-workspace crates to rebuild and re-run `cargo check --workspace --all-targets` twice.")
//! @yah:next("Do not re-enable on the camp until that passes AND R748-F6 shadow mode exists. This failure is the argument for shadow being the default on a shared tree rather than an opt-in - it would have surfaced the deletions with zero blast radius.")
//! @yah:verify("29 tests green, clippy silent. New: wrapper::tests::two_live_unit_hashes_of_one_crate_never_reclaim_each_other reproduces the shape (two hashes of one crate compiled in sequence, neither reclaims the other), family::tests::a_different_unit_hash_is_a_different_family, and a_identical_unit_recompiled_stays_one_family pins that supersession still fires for a genuine rebuild.")
//! @yah:verify("The old test volatile_codegen_fields_do_not_change_family asserted the exact behaviour that caused this and was deleted, not renamed - it encoded the bug as the contract.")
//! @yah:gotcha("Camp state is enabled = false in the root Cargo.toml and $CARGO_HOME/orphan-gc/workspaces was purged. Anyone re-enabling should purge again first: family records written by the pre-fix binary are keyed hash-insensitively and would resume superseding across live hashes.")
//! @yah:handoff("Attempted the end-to-end verification with a minimal live-cargo fixture instead of a yubaba copy, and it did NOT reproduce the failure - recording that so the next attempt does not repeat it. Under resolver 2 a dev-dependency's features are not unified into the non-dev build, so --workspace --all-targets does duplicate-build a crate; but the two compiles differ in argv beyond the unit hash (measured: one carries --crate-type lib, the other does not), so they forked families even under the PRE-fix hash-insensitive key. Verified by reverting the fix in family.rs and re-running: still green.")
//! @yah:next("R748-F6 shadow mode has landed, which removes the reason to be afraid of doing this on a real tree: enabled = true + dry-run = true learns and reports and cannot delete anything. That is the cheap way to get the camp-scale evidence W306 also wants.")
//! @yah:next("The collision needs two units whose argv differs ONLY in -C metadata / -C extra-filename and the --extern hashes this tool normalizes away - which is what crates reached through [patch.crates-io] into a SIBLING workspace's target dir produce, and is why only those crates were hit on the camp. A single-workspace fixture cannot make that shape; the reproducer needs two workspaces plus a patch bridge (or the yubaba + oss/{yah-base,qed,kamaji} copy the earlier next-step describes).")
//! @yah:verify("Landed anyway as a permanent guard: tests/live_cargo.rs::a_duplicate_build_of_one_crate_keeps_both_units_alive drives a real cargo duplicate build, asserts both units' rmeta survive the invocation that built them, and asserts >= 2 families are tracked for the duplicated crate so it cannot pass vacuously. Its doc comment states plainly that it does not fail against the pre-fix identity.")
//! @yah:handoff("END-TO-END PROOF LANDED. The failure reproduces on demand and the fix is verified against it, not just against unit tests.")
//! @yah:verify("Pre-fix binary (same tree, parse_codegen reverted to dropping the two hash arms) exits 101 on run 2 with `error: extern location for mid does not exist: .../libmid-44412f1f97d529e4.rmeta`, and that unit's rlib+rmeta are gone from target/debug/deps. Fixed binary: both runs exit 0, both units intact, run 2 recompiles nothing.")
//! @yah:handoff("Reproduction shape, which is the part three earlier attempts missed: put the feature divergence one level BELOW the crate under watch. Resolver 2 does not unify features between the build graph and the normal graph, so a dependency built two ways duplicates its DEPENDENTS without changing their argv - the dependent's two compiles then differ only in -C metadata / -C extra-filename and the --extern hash the wrapper normalizes to <artifact>. Make the watched crate itself differ (dev-dep feature, --all-targets, a --test harness) and cargo also changes --crate-type / --emit, which forks the family even under the OLD key. That is why the earlier single-workspace fixtures came back green. No [patch.crates-io] bridge and no second workspace are needed after all.")
//! @yah:handoff("Second discovery, and the answer to why the camp symptom was intermittent and named a different crate each run: the artifact-reuse fast path MASKS supersession. While a family is on it (full-scan-every, default 16) the compile re-records the previous generation's path list verbatim, so the generation never changes and nothing is ever orphaned. Supersession is only due on a full scan, i.e. when that family's compiles_since_scan happens to come due on the second unit. The regression test pins full-scan-every = 1 for exactly this reason; without it the fixture passes against the pre-fix binary and proves nothing.")
//! @yah:handoff("Files: tests/live_cargo.rs - the toothless a_duplicate_build_of_one_crate_keeps_both_units_alive is REPLACED by two_units_of_one_crate_differing_only_in_hash_do_not_reclaim_each_other, whose doc comment carries both findings above. ARCHITECTURE.md section 5 was still documenting the PRE-fix identity as canon (-C metadata and -C extra-filename listed under Stripped generation dimensions); corrected in place with the reason, the reproducing shape, and a pointer to the test. Section 11.3 gained the reuse-path/supersession consequence.")
//! @yah:verify("43 tests green (40 unit + 1 bootstrap + 2 live_cargo), clippy --all-targets silent.")
//! @yah:gotcha("The gotcha above (camp state is enabled = false) is STALE as of 2026-08-12 17:xx: the root Cargo.toml is enabled = true + dry-run = true (shadow), operator-authorized. The purge advice still stands for any OTHER tree that ran the pre-fix binary.")
//! @yah:next("Operator call, not mine: flip the camp from dry-run = true to dry-run = false, or hold. Shadow after ~2h of peers builds: 35 families, 30.2 GB current artifacts, 14952 orphan records queued (992 MB) but would-reclaim = 0 orphan artifacts + 2 surplus incremental sessions (4.5 MB). Orphan-only reclaims nothing here by Invariant D, as at every previous measurement. Holding is defensible: W306s other half (per-member contention at root-workspace scale under agent load) is still unmeasured, and 4.5 MB is not worth a live deletion mode.")
//! @yah:verify("The camp-installed binary (~/.cargo/bin/cargo-orphan-gc, mtime 2026-08-12 17:20) was checked FUNCTIONALLY, not by mtime: driven through the same fixture it passes both runs with both units intact, so the shadow-mode dogfooding is running hash-sensitive identity.")
//! @yah:handoff("Operator chose to flip the camp out of shadow: root Cargo.toml [workspace.metadata.orphan-gc] is now dry-run = false, i.e. deleting. State was NOT purged and must not be - every family in the store was learned by the fixed, hash-sensitive binary during the shadow soak, so purging would only discard learned ownership. At the flip: 58 families, 42.0 GB current artifacts, would-reclaim 0 orphan artifacts + 27 surplus incremental sessions (2.8 GB). Those get collected as peers compile (pending-sweeps-per-compile = 4); no manual sweep was run.")
//! @yah:gotcha("The root block governs the oss/* workspaces too, which is wider than it looks and was NOT obvious to the camp - @Ashguard:libra flagged it. config::discover_from walks ancestors until it finds a [workspace.metadata.orphan-gc], and no oss/<name>/Cargo.toml carries one, so a compile under oss/mesofact or oss/kamaji falls through to the camp root. Verified: 16 of 58 tracked families have out-dirs under oss/ (yah-base 7, kamaji 5, orphan-gc 4), all pooled in one store under $CARGO_HOME. Correctly separated (out-dir is part of the family key) but one knob moves all of them. Exempt a workspace by giving it its own block with enabled = false - the walk stops at the first one it finds. Documented at the site in the root Cargo.toml.")

use std::collections::BTreeSet;
use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

#[derive(Clone, Debug)]
pub struct Invocation {
    pub crate_name: Option<String>,
    pub crate_root: Option<PathBuf>,
    pub out_dir: Option<PathBuf>,
    pub incremental_dir: Option<PathBuf>,
    pub extra_filename: Option<String>,
    pub explicit_emit_paths: Vec<PathBuf>,
    pub extern_paths: Vec<PathBuf>,
    pub family_key: Option<String>,
    pub family_label: String,
}

impl Invocation {
    /// Borrows the argv rather than taking it: the caller still needs both to
    /// actually spawn rustc, and this runs once per compile.
    pub fn parse(real_rustc: &OsStr, args: &[OsString]) -> Result<Self> {
        let cwd = std::env::current_dir().context("get current directory")?;
        let mut crate_name = None;
        let mut crate_root = None;
        let mut out_dir = None;
        let mut incremental_dir = None;
        let mut extra_filename = None;
        let mut explicit_emit_paths = Vec::new();
        let mut extern_paths = Vec::new();
        let mut normalized = Vec::<String>::new();

        let mut i = 0usize;
        while i < args.len() {
            let s = args[i].to_string_lossy();

            if s == "--crate-name" {
                if let Some(v) = args.get(i + 1) {
                    crate_name = Some(v.to_string_lossy().into_owned());
                    normalized.push("--crate-name".into());
                    normalized.push(v.to_string_lossy().into_owned());
                    i += 2;
                    continue;
                }
            }

            if s == "--out-dir" {
                if let Some(v) = args.get(i + 1) {
                    let p = absolutize(&cwd, Path::new(v));
                    out_dir = Some(p.clone());
                    // Deliberately keep the out-dir in the family identity. This means
                    // target/profile/platform layout changes form a new family and leak
                    // rather than cross-delete, which is the conservative policy.
                    normalized.push(format!("--out-dir={}", p.to_string_lossy()));
                    i += 2;
                    continue;
                }
            }

            if s == "--extern" {
                if let Some(v) = args.get(i + 1) {
                    let raw = v.to_string_lossy();
                    normalize_extern(&raw, &cwd, &mut normalized, &mut extern_paths);
                    i += 2;
                    continue;
                }
            }
            if let Some(raw) = s.strip_prefix("--extern=") {
                normalize_extern(raw, &cwd, &mut normalized, &mut extern_paths);
                i += 1;
                continue;
            }

            if s == "--emit" {
                if let Some(v) = args.get(i + 1) {
                    normalize_emit(&v.to_string_lossy(), &cwd, &mut normalized, &mut explicit_emit_paths);
                    i += 2;
                    continue;
                }
            }
            if let Some(raw) = s.strip_prefix("--emit=") {
                normalize_emit(raw, &cwd, &mut normalized, &mut explicit_emit_paths);
                i += 1;
                continue;
            }

            if s == "-L" {
                if let Some(v) = args.get(i + 1) {
                    normalized.push(normalize_search_path(&v.to_string_lossy()));
                    i += 2;
                    continue;
                }
            }
            if s.starts_with("-L") && s.len() > 2 {
                normalized.push(normalize_search_path(&s[2..]));
                i += 1;
                continue;
            }

            if s == "-C" {
                if let Some(v) = args.get(i + 1) {
                    parse_codegen(
                        &v.to_string_lossy(),
                        &cwd,
                        &mut incremental_dir,
                        &mut extra_filename,
                        &mut normalized,
                    );
                    i += 2;
                    continue;
                }
            }
            if let Some(raw) = s.strip_prefix("-C") {
                if !raw.is_empty() {
                    parse_codegen(raw, &cwd, &mut incremental_dir, &mut extra_filename, &mut normalized);
                    i += 1;
                    continue;
                }
            }

            if !s.starts_with('-') && s != "-" && crate_root.is_none() {
                let p = absolutize(&cwd, Path::new(&*s));
                crate_root = Some(p.clone());
                normalized.push(format!("crate-root={}", p.to_string_lossy()));
                i += 1;
                continue;
            }

            normalized.push(s.into_owned());
            i += 1;
        }

        if let Some(toolchain) = std::env::var_os("RUSTUP_TOOLCHAIN") {
            normalized.push(format!("env:RUSTUP_TOOLCHAIN={}", toolchain.to_string_lossy()));
        }
        if let Some(pkg) = std::env::var_os("CARGO_PKG_NAME") {
            normalized.push(format!("env:CARGO_PKG_NAME={}", pkg.to_string_lossy()));
        }
        if let Some(ver) = std::env::var_os("CARGO_PKG_VERSION") {
            normalized.push(format!("env:CARGO_PKG_VERSION={}", ver.to_string_lossy()));
        }
        normalized.push(format!("rustc={}", real_rustc.to_string_lossy()));

        let trackable = crate_name.is_some() && crate_root.is_some() && out_dir.is_some();
        let family_key = trackable.then(|| {
            let mut hasher = blake3::Hasher::new();
            for part in &normalized {
                hasher.update(part.as_bytes());
                hasher.update(&[0]);
            }
            hasher.finalize().to_hex().to_string()
        });

        let family_label = match (&crate_name, &crate_root) {
            (Some(name), Some(root)) => format!("{name} ({})", root.display()),
            (Some(name), None) => name.clone(),
            _ => "untracked rustc invocation".into(),
        };

        extern_paths.sort();
        extern_paths.dedup();
        explicit_emit_paths.sort();
        explicit_emit_paths.dedup();

        Ok(Self {
            crate_name,
            crate_root,
            out_dir,
            incremental_dir,
            extra_filename,
            explicit_emit_paths,
            extern_paths,
            family_key,
            family_label,
        })
    }
}

fn parse_codegen(
    raw: &str,
    cwd: &Path,
    incremental_dir: &mut Option<PathBuf>,
    extra_filename: &mut Option<String>,
    normalized: &mut Vec<String>,
) {
    let (key, value) = raw.split_once('=').unwrap_or((raw, ""));
    match key {
        // These two carry cargo's unit hash, and they are part of family
        // identity — deliberately, after R748-B9.
        //
        // The original model stripped them, so a recompile "mapped back to the
        // same logical family" and superseded it. That is only safe if at most
        // one hash of a crate is live at a time, and on a real workspace it is
        // not: feature unification builds the same crate several ways in ONE
        // cargo invocation (`--workspace --all-targets` over `[patch.crates-io]`
        // path deps is the reliable reproducer). The second compile then
        // orphaned the first's `.rmeta`/`.rlib` while the build still had units
        // queued that link against them, and cargo failed with `extern location
        // for <crate> does not exist` naming the deleted hash.
        //
        // Hash-exact families mean supersession only fires when cargo genuinely
        // rebuilds the same unit, and a differently-configured build forks a
        // family and leaks instead (Invariant C) — leaking is the behaviour this
        // tool is supposed to fall back on when liveness is ambiguous. The
        // reclamation that actually pays is unaffected: surplus-session
        // collection keys on the incremental dir, and budget mode retires cold
        // families by LRU, which is the safe way to reap a stale hash.
        "metadata" => normalized.push(format!("-C{raw}")),
        "extra-filename" => {
            *extra_filename = Some(value.to_string());
            normalized.push(format!("-C{raw}"));
        }
        "incremental" => {
            if !value.is_empty() {
                *incremental_dir = Some(absolutize(cwd, Path::new(value)));
            }
        }
        _ => normalized.push(format!("-C{raw}")),
    }
}

fn normalize_extern(
    raw: &str,
    cwd: &Path,
    normalized: &mut Vec<String>,
    extern_paths: &mut Vec<PathBuf>,
) {
    if let Some((name, path)) = raw.rsplit_once('=') {
        if !path.is_empty() {
            extern_paths.push(absolutize(cwd, Path::new(path)));
        }
        // Keep the extern logical name/modifiers, but intentionally discard the
        // dependency artifact filename hash. A dependency rebuild should not
        // manufacture a forever-new family for every downstream crate.
        normalized.push(format!("--extern={name}=<artifact>"));
    } else {
        normalized.push(format!("--extern={raw}"));
    }
}

fn normalize_emit(
    raw: &str,
    cwd: &Path,
    normalized: &mut Vec<String>,
    explicit_emit_paths: &mut Vec<PathBuf>,
) {
    let mut kinds = BTreeSet::new();
    for part in raw.split(',') {
        if let Some((kind, path)) = part.split_once('=') {
            kinds.insert(kind.to_string());
            if !path.is_empty() {
                explicit_emit_paths.push(absolutize(cwd, Path::new(path)));
            }
        } else {
            kinds.insert(part.to_string());
        }
    }
    normalized.push(format!("--emit={}", kinds.into_iter().collect::<Vec<_>>().join(",")));
}

fn normalize_search_path(raw: &str) -> String {
    if let Some((kind, _path)) = raw.split_once('=') {
        format!("-L{kind}=<path>")
    } else {
        "-L<path>".into()
    }
}

fn absolutize(cwd: &Path, path: &Path) -> PathBuf {
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        cwd.join(path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// R748-B9 inverted this. It used to assert that two different unit hashes
    /// were the SAME family, which is what let one compile delete another's
    /// live artifacts inside a single cargo invocation. They must be distinct.
    #[test]
    fn a_different_unit_hash_is_a_different_family() {
        let base = vec![
            "--crate-name", "foo", "src/lib.rs", "--out-dir", "target/debug/deps",
            "--extern", "bar=target/debug/deps/libbar-111.rlib",
            "-C", "metadata=aaa", "-C", "extra-filename=-aaa",
            "-C", "incremental=target/debug/incremental/foo-aaa",
        ];
        let next = vec![
            "--crate-name", "foo", "src/lib.rs", "--out-dir", "target/debug/deps",
            "--extern", "bar=target/debug/deps/libbar-222.rlib",
            "-C", "metadata=bbb", "-C", "extra-filename=-bbb",
            "-C", "incremental=target/debug/incremental/foo-bbb",
        ];
        let make = |xs: Vec<&str>| {
            let args: Vec<OsString> = xs.into_iter().map(OsString::from).collect();
            Invocation::parse(OsStr::new("rustc"), &args).unwrap()
        };
        assert_ne!(make(base).family_key, make(next).family_key);
    }

    /// The flip side: an identical unit recompiled — same hash, same flags, only
    /// the source changed — must still map to one family, or supersession never
    /// fires and nothing is ever reclaimed.
    #[test]
    fn an_identical_unit_recompiled_stays_one_family() {
        let args = vec![
            "--crate-name", "foo", "src/lib.rs", "--out-dir", "target/debug/deps",
            "--extern", "bar=target/debug/deps/libbar-111.rlib",
            "-C", "metadata=aaa", "-C", "extra-filename=-aaa",
            "-C", "incremental=target/debug/incremental/foo-aaa",
        ];
        let make = || {
            let xs: Vec<OsString> = args.iter().map(|s| OsString::from(*s)).collect();
            Invocation::parse(OsStr::new("rustc"), &xs).unwrap()
        };
        assert_eq!(make().family_key, make().family_key);
        assert!(make().family_key.is_some());
    }

}
