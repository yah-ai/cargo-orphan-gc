//! @yah:ticket(R748-F2, "Collect surplus rustc sessions inside tracked incremental dirs")
//! @yah:at(2026-08-11T07:48:53Z)
//! @yah:status(review)
//! @yah:handoff("Landed as collect_surplus_sessions + finalized_sessions here, wired into gc::sweep_locked (under the family lock) so both the wrapper's post-compile sweep and `cargo orphan-gc sweep` run it. Key discovery this ticket's framing missed: cargo passes -C incremental= the profile-wide SHARED root, not a per-crate dir - rustc creates <crate>-<hash> keys inside it. OwnedArtifact gains session_prefix (the crate name; serde-defaulted so old state files load) to scope collection to keys attributable to the compiled crate; prefix matching is safe because rustc crate names cannot contain '-'. ARCHITECTURE 6.3/6.4 documents the shape and the concurrency argument (same key = same family = serialized by the family lock).")
//! @yah:verify("Unit fixtures per the ticket verify: two finalized + one -working plans exactly one deletion, keeps newest, never touches -working or .lock; root-shape descent enters only matching keys; no-prefix root collects nothing; foo- does not cross-match foo_bar keys. Plus tests/live_cargo.rs end-to-end through real cargo: fabricated surplus collected by the CLI sweep, newest survives, and the tree still builds incrementally afterwards.")
//! @yah:verify("Kamaji-copy demo 2026-08-11: 3 rounds of concurrent unwrapped builds grew 7 sessions to 10; one sweep collected exactly the 3 surplus (reported 85.6 MB, real du delta 44 MB, tree 182 to 138 MB) at zero rebuild cost; wrapped edit-recheck after = 0.95s.")
//! @yah:gotcha("The reported byte count overstates real disk: rustc hard-links unchanged files between sessions, so path_size (file-length sum) counts shared blocks per session while du counts them once. The kamaji demo measured 85.6 MB reported vs 44 MB actually freed. If the report ever grows a bytes-freed headline, compute it from block counts, not lengths.")
//! @yah:assignee(agent:bundle-anthropic-ashguard)
//! @yah:parent(R748)
//! @yah:next("This is the reclamation the tool currently cannot do, and it is the dominant daily case. §6.3 records `-C incremental=<dir>` as ONE opaque directory-owned artifact and deliberately does not look inside. But a source edit leaves the artifact PATH SET unchanged, so §7 refreshes the generation rather than orphaning it — nothing is reclaimed — while the tree still grows, because growth is happening INSIDE that directory.\n\nrustc keeps one live session per key and deletes the prior one when it finalizes a new one, but that GC needs the key's directory lock and silently skips under concurrent builds. Measured spread across five real trees (W307): mesofact 0%, yubaba 1-5%, yah debug 16-21%, yah release-dev 38%, noisetable 35%. Where session dirs are large this is the dominant term in BYTES (release-dev: surplus sessions alone were 41% of the tree).\n\nDeleting a surplus finalized session is FREE — rustc already intended to. Reference implementation exists: finalized_sessions / plan_incremental in crates/yah/camp-service/src/sweep.rs, which sorts sessions newest-first and keeps only the newest per key.")
//! @yah:verify("A fixture key holding two finalized sessions plus one -working session must plan exactly one deletion (the older finalized), leave the newest usable, and leave the -working session and every .lock untouched. Then the tree must still build incrementally afterwards — the point is a smaller VALID cache, not an empty one.")
//! @yah:gotcha("Never touch a session dir named s-<ts>-<rand>-working, nor the s-<ts>-<rand>.lock files beside it — a -working session belongs to a rustc that is mid-compile, and removing it corrupts that build. Also never delete individual FILES inside a session: a session is a dep-graph plus a query cache that reference each other, so removing part yields a corrupt cache rather than a smaller one. The safe units are a whole session dir or a whole key.")
//!
//! @yah:ticket(R748-B7, "Incremental anchor was recorded as an owned artifact, so orphaning one would delete the whole shared incremental root")
//! @yah:at(2026-08-12T02:08:26Z)
//! @yah:status(review)
//! @yah:assignee(agent:bundle-anthropic-ashguard)
//! @yah:parent(R748)
//! @yah:severity(critical)
//! @yah:handoff("Found by installing on this camp and reading `cargo orphan-gc status`: one `cargo check -p yah-board` claimed 22.7 GB of current artifacts. The third recorded artifact was /Users/leif/ss/yah/target/debug/incremental - the profile-wide root shared by every crate, not this family's key dir. Cargo passes -C incremental=<root>; rustc creates <crate>-<hash> underneath.")
//! @yah:handoff("Severity: gc.rs's own comment said orphaned incremental dirs 'are deleted whole by the orphan pass above', and the IncrementalDir validation only asserted path == root, which the recorded root trivially satisfies. So a family that stopped listing the anchor would have deleted 17 GB of every other crate's incremental state. Invariant D did not cover it - D compares against the SAME family's current paths.")
//! @yah:handoff("Fix: an incremental record is now an anchor, never a deletable object. artifacts::remove hard-bails on the kind; the orphan pass skips it (dropped, not queued, so the generation can still retire); new artifacts::owned_size charges an anchor only for key dirs its session_prefix matches, so max-bytes and budget-mode retirement stop seeing one crate as 22.7 GB.")
//! @yah:verify("New test gc::tests::an_orphaned_incremental_anchor_never_deletes_the_shared_root: orphans an anchor whose root holds another crate's session, asserts zero deletions, the root surviving, the foreign session surviving, and the anchor dropped rather than queued forever.")
//! @yah:verify("invariant_f test updated to the corrected contract: two deletable kinds fail closed and stay queued, the anchor is a no-op. artifacts::remove still refuses all three kinds.")
//! @yah:verify("25 tests green (23 unit + 2 integration), clippy silent. Verified live: reinstalled, re-enabled, cargo check -p yah-board collected 6 surplus sessions / 142 MB and recorded 6667 artifacts across 3 families that are all genuine .rcgu.o codegen units.")
//! @yah:gotcha("The camp state dir was purged after the buggy run (13 families had already been learned by peers' builds, all recording the shared root). Nothing was deleted from target/ - the tool was disabled before any generation could be superseded. If another tree ran the pre-fix binary, purge $CARGO_HOME/orphan-gc/workspaces/<hash> there too rather than trusting the recorded artifact lists.")

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::config::Mode;
use crate::family::Invocation;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum ArtifactKind {
    OutDirEntry,
    IncrementalDir,
    ExplicitEmit,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct OwnedArtifact {
    pub path: PathBuf,
    pub root: PathBuf,
    pub kind: ArtifactKind,
    /// For [`ArtifactKind::IncrementalDir`]: the crate name whose key dirs
    /// (`<crate_name>-<hash>/`) inside the recorded directory this family may
    /// collect surplus sessions from. Cargo passes `-C incremental=` the
    /// profile-wide *root* shared by every crate, so session collection must
    /// not roam it freely — the prefix is what scopes deletion to key dirs
    /// attributable to a crate this wrapper actually compiled. Safe as a
    /// prefix because rustc crate names cannot contain `-`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_prefix: Option<String>,
}

/// What a [`collect`] call actually did, so the caller can decide when the next
/// full scan is due.
#[derive(Debug, PartialEq, Eq)]
pub enum Scan {
    /// The out-dir was walked.
    Full,
    /// The previous generation's out-dir entries were all still present and
    /// were reused instead.
    Reused,
}

/// Discover the artifacts this invocation owns.
///
/// `reuse` offers the previous generation's artifacts as a fast path. Walking
/// the out-dir is by far the most expensive thing this tool does per compile:
/// the dir is shared by the whole workspace, and the walk is linear in its size
/// — 116 ms at the 210,770 entries of the camp this was measured on, which is
/// the filesystem's floor (Rust's `read_dir` there is already 3.5x faster than a
/// C-level `scandir`), so it cannot be optimized away, only skipped.
///
/// The fast path is taken when every out-dir entry the previous generation
/// recorded still exists. Its cost is one `stat` per recorded artifact.
///
/// Why this is safe rather than merely fast: reusing a stale set can only
/// *under*-record ownership — a file the compile newly emitted goes unrecorded,
/// and an unrecorded file is one this tool will never delete (Invariant B).
/// The failure mode is a leak, which is the fallback the whole design already
/// takes whenever ownership is uncertain. It is bounded anyway: any recorded
/// path going missing forces a full scan, and the caller resyncs periodically
/// (`full-scan-every`).
pub fn collect(inv: &Invocation, reuse: Option<&[OwnedArtifact]>) -> Result<(Vec<OwnedArtifact>, Scan)> {
    let mut found = BTreeMap::<PathBuf, OwnedArtifact>::new();
    let mut scan = Scan::Full;

    if let Some(previous) = reuse {
        let out_dir_entries: Vec<&OwnedArtifact> = previous
            .iter()
            .filter(|a| a.kind == ArtifactKind::OutDirEntry)
            .collect();
        if !out_dir_entries.is_empty() && out_dir_entries.iter().all(|a| a.path.exists()) {
            for artifact in out_dir_entries {
                found.insert(artifact.path.clone(), (*artifact).clone());
            }
            scan = Scan::Reused;
        }
    }

    if let (Scan::Full, Some(out_dir), Some(crate_name), Some(extra)) =
        (&scan, &inv.out_dir, &inv.crate_name, &inv.extra_filename)
    {
        // The stems are built ONCE. This loop runs over every entry in the
        // out-dir, which is shared by the whole workspace and gets very large —
        // 210,770 entries on the camp this was measured on, where the previous
        // shape (a String per entry plus up to five `format!`s inside the
        // predicate) cost 145 ms of wrapper overhead on every single rustc
        // invocation, linear in directory size. Comparing raw bytes against
        // pre-built stems allocates nothing per entry.
        if out_dir.is_dir() && !extra.is_empty() {
            let stem = format!("{crate_name}{extra}").into_bytes();
            let lib_stem = format!("lib{crate_name}{extra}").into_bytes();
            for entry in fs::read_dir(out_dir)
                .with_context(|| format!("read rustc out-dir {}", out_dir.display()))?
            {
                let entry = entry?;
                let name = entry.file_name();
                if looks_owned(name.as_encoded_bytes(), &stem, &lib_stem) {
                    let artifact = OwnedArtifact {
                        path: entry.path(),
                        root: out_dir.clone(),
                        kind: ArtifactKind::OutDirEntry,
                        session_prefix: None,
                    };
                    found.insert(artifact.path.clone(), artifact);
                }
            }
        }
    }

    if let Some(out_dir) = &inv.out_dir {
        for p in &inv.explicit_emit_paths {
            // Explicit --emit paths are only tracked if they remain inside the
            // compiler out-dir. Refuse to learn arbitrary deletion paths.
            if p.starts_with(out_dir) && p.exists() {
                let artifact = OwnedArtifact {
                    path: p.clone(),
                    root: out_dir.clone(),
                    kind: ArtifactKind::ExplicitEmit,
                    session_prefix: None,
                };
                found.insert(artifact.path.clone(), artifact);
            }
        }
    }

    if let Some(p) = &inv.incremental_dir {
        if p.exists() {
            let artifact = OwnedArtifact {
                path: p.clone(),
                root: p.clone(),
                kind: ArtifactKind::IncrementalDir,
                session_prefix: inv.crate_name.clone(),
            };
            found.insert(artifact.path.clone(), artifact);
        }
    }

    Ok((found.into_values().collect(), scan))
}

#[derive(Default, Debug)]
pub struct SessionCollection {
    pub deleted_sessions: usize,
    pub deleted_bytes: u64,
}

/// Delete every finalized rustc session except the newest inside the key dirs
/// a tracked incremental artifact covers.
///
/// This is the reclamation generation tracking alone cannot reach: a source
/// edit leaves the family's artifact *path set* unchanged, so the generation
/// refreshes rather than orphans — while rustc grows the tree *inside* the
/// incremental directory. rustc itself keeps one live session per key and
/// deletes the prior one when finalizing, but that GC needs the key's
/// directory lock and silently skips under concurrent builds. A surplus
/// finalized session is therefore dead by rustc's own accounting, and
/// deleting it is free — no rebuild cost, ever.
///
/// The deletion authority here is rustc's session lifecycle, not learned
/// generation history, so it needs none: only whole `s-*` session dirs are
/// ever removed (a session is a dep-graph plus query cache that reference
/// each other; partial deletion corrupts rather than shrinks), never the
/// newest finalized one, never a `-working` session (a rustc is mid-compile
/// in it), and never the `.lock` files beside them.
pub fn collect_surplus_sessions(
    dir: &Path,
    session_prefix: Option<&str>,
    mode: Mode,
) -> SessionCollection {
    let mut report = SessionCollection::default();
    if !dir.is_dir() {
        return report;
    }

    // Two shapes: the recorded dir may itself be a key dir holding `s-*`
    // sessions (a hand-set `-C incremental=` pointing at one), or — the shape
    // cargo produces — the profile-wide root whose children are per-crate
    // key dirs. In the root shape, only keys attributable to this artifact's
    // crate are visited; everything else belongs to families (or crates) this
    // record knows nothing about.
    if has_sessions(dir) {
        collect_in_key(dir, mode, &mut report);
        return report;
    }
    let Some(prefix) = session_prefix else {
        return report;
    };
    let prefix = format!("{prefix}-");
    let Ok(entries) = fs::read_dir(dir) else {
        return report;
    };
    for entry in entries.filter_map(|e| e.ok()) {
        let name = entry.file_name();
        if !name.to_string_lossy().starts_with(&prefix) {
            continue;
        }
        if entry.file_type().map(|t| t.is_dir()).unwrap_or(false) {
            collect_in_key(&entry.path(), mode, &mut report);
        }
    }
    report
}

/// Finalized session dirs inside one key, newest first. A session in progress
/// is named `s-<ts>-<rand>-working` and is never returned; the `.lock` files
/// beside sessions are files, not dirs, and fall out of the dir filter.
pub fn finalized_sessions(key_dir: &Path) -> Vec<(PathBuf, SystemTime)> {
    let Ok(entries) = fs::read_dir(key_dir) else {
        return Vec::new();
    };
    let mut sessions: Vec<(PathBuf, SystemTime)> = entries
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().map(|t| t.is_dir()).unwrap_or(false))
        .filter(|e| {
            let name = e.file_name();
            let name = name.to_string_lossy();
            name.starts_with("s-") && !name.ends_with("-working")
        })
        .filter_map(|e| {
            let meta = e.metadata().ok()?;
            Some((e.path(), meta.modified().ok()?))
        })
        .collect();
    sessions.sort_by_key(|s| std::cmp::Reverse(s.1));
    sessions
}

fn has_sessions(dir: &Path) -> bool {
    let Ok(entries) = fs::read_dir(dir) else {
        return false;
    };
    entries
        .filter_map(|e| e.ok())
        .any(|e| e.file_name().to_string_lossy().starts_with("s-"))
}

fn collect_in_key(key_dir: &Path, mode: Mode, report: &mut SessionCollection) {
    for (session, _) in finalized_sessions(key_dir).iter().skip(1) {
        let bytes = path_size(session).unwrap_or(0);
        // Shadow counts exactly what Delete would take — the surplus set is
        // decided by `finalized_sessions`, above, so the two modes cannot
        // disagree about which sessions are in it.
        if mode.is_shadow() || fs::remove_dir_all(session).is_ok() {
            report.deleted_sessions += 1;
            report.deleted_bytes = report.deleted_bytes.saturating_add(bytes);
        }
    }
}

/// Whether a directory entry is an output of the unit being compiled.
///
/// Byte-exact reimplementation of the original predicate, deliberately
/// including its asymmetry: `<stem>-…` counts (cargo's `-<hash>.rcgu.o`
/// codegen units hang off the bare stem) while `lib<stem>-…` does not. Widening
/// that here would widen what this tool is willing to delete, which is not a
/// performance decision to make in passing.
fn looks_owned(name: &[u8], stem: &[u8], lib_stem: &[u8]) -> bool {
    if let Some(rest) = name.strip_prefix(stem) {
        if rest.is_empty() || rest[0] == b'.' || rest[0] == b'-' {
            return true;
        }
    }
    if let Some(rest) = name.strip_prefix(lib_stem) {
        if rest.is_empty() || rest[0] == b'.' {
            return true;
        }
    }
    false
}

/// The outcome of a single artifact removal, kept distinct from a plain byte
/// count so a caller summing a sweep cannot mistake "already gone by the time
/// we got here" (routine in a camp this concurrent — something else, or an
/// earlier sweep, won the race) for an actual reclaim of `0` bytes. Folding
/// both into one `u64` is what made every real deletion line in the
/// operational log read `(0 bytes)`: `AlreadyGone` hits vastly outnumber
/// `Freed` ones, and summed together they round every total down to zero.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Reclaim {
    Freed(u64),
    AlreadyGone,
}

pub fn remove(artifact: &OwnedArtifact) -> Result<Reclaim> {
    check_deletable(artifact)?;
    if !artifact.path.exists() {
        return Ok(Reclaim::AlreadyGone);
    }

    let bytes = path_size(&artifact.path).unwrap_or(0);
    let meta = fs::symlink_metadata(&artifact.path)
        .with_context(|| format!("stat {}", artifact.path.display()))?;

    if meta.file_type().is_symlink() || meta.is_file() {
        fs::remove_file(&artifact.path)
            .with_context(|| format!("remove {}", artifact.path.display()))?;
    } else if meta.is_dir() {
        fs::remove_dir_all(&artifact.path)
            .with_context(|| format!("remove directory {}", artifact.path.display()))?;
    } else {
        fs::remove_file(&artifact.path)
            .with_context(|| format!("remove special file {}", artifact.path.display()))?;
    }
    Ok(Reclaim::Freed(bytes))
}

/// The gate every deletion passes, split out of [`remove`] so shadow mode can
/// ask the same question without answering it destructively. A would-delete
/// figure computed any other way could promise reclamation the real sweep would
/// refuse — which is the one way a report can be worse than no report.
pub fn check_deletable(artifact: &OwnedArtifact) -> Result<()> {
    match artifact.kind {
        ArtifactKind::OutDirEntry => {
            anyhow::ensure!(
                artifact.path.parent() == Some(artifact.root.as_path()),
                "refusing out-dir deletion outside recorded direct parent: {}",
                artifact.path.display()
            );
        }
        ArtifactKind::ExplicitEmit => {
            anyhow::ensure!(
                artifact.path.starts_with(&artifact.root),
                "refusing explicit emit deletion outside recorded out-dir: {}",
                artifact.path.display()
            );
        }
        ArtifactKind::IncrementalDir => {
            // Never, under any caller. Cargo passes `-C incremental=<root>`
            // where <root> is the PROFILE-WIDE directory shared by every crate
            // in the workspace — rustc creates the per-crate key dir beneath
            // it. So this record is an *anchor* for prefix-scoped session
            // collection, not an owned object, and the family that recorded it
            // never owned the root: deleting it whole would take every other
            // crate's incremental state with it (17 GB across ten agents, in
            // the tree this was found on). Reclamation inside the anchor goes
            // through `collect_surplus_sessions`, which is prefix-scoped and
            // only ever removes whole superseded `s-*` sessions.
            anyhow::bail!(
                "refusing to delete a shared incremental root: {} (anchor, not an owned artifact)",
                artifact.path.display()
            );
        }
    }
    Ok(())
}

/// Bytes an artifact record actually accounts for.
///
/// For everything but an incremental anchor this is just the path's size. An
/// anchor's path is the profile-wide incremental root, so sizing it naively
/// charges one crate for the entire workspace's incremental tree — 22.7 GB
/// against a single `cargo check -p yah-board`, when this was found. That
/// number is not cosmetic: it feeds `max-bytes` and, in budget mode, decides
/// which families get retired. Charge the anchor only for the key dirs its
/// `session_prefix` actually matches.
/// The identity of the *bytes* [`owned_size`] measures for an artifact — which
/// is not the same thing as the identity of the artifact record.
///
/// For an [`ArtifactKind::IncrementalDir`] the recorded `path` is the
/// profile-wide incremental **root**, shared by every crate in the workspace,
/// and `owned_size` scopes its measurement to the key dirs matching
/// `session_prefix`. Two records with the same (path, prefix) therefore
/// measure exactly the same bytes, and summing both double-counts them.
///
/// That is not a hypothetical. Family identity is hash-sensitive (R748-B9), so
/// every unit-hash change forks a *new* family, while `session_prefix` stays
/// the crate name — this camp reached **66 families** all measuring the same
/// `yah` key dirs. Measured 2026-08-14: 3356 incremental-dir records collapse
/// to 380 distinct (path, prefix) pairs, and totalling without this dedup
/// reported **1978 GB against a real 81 GB** — a 24x over-count.
///
/// Why that matters beyond a cosmetic status line: `max-bytes` in
/// `budget-mode = lru-current-families` is compared against exactly this
/// total. An inflated total means the ceiling reads as breached when the tree
/// is nowhere near it, and the sweep retires cold families — a camp-wide
/// cold-rebuild storm sourced entirely from a counting bug.
pub fn size_identity(artifact: &OwnedArtifact) -> (PathBuf, Option<String>) {
    match artifact.kind {
        ArtifactKind::IncrementalDir => {
            (artifact.path.clone(), artifact.session_prefix.clone())
        }
        // Out-dir entries and explicit emits are concrete files owned by one
        // family; the path alone identifies the bytes.
        _ => (artifact.path.clone(), None),
    }
}

/// Total [`owned_size`] over artifacts, charging each distinct
/// [`size_identity`] exactly once.
///
/// Use this for any figure that will be *compared against a threshold* or
/// shown to a human as "how much is on disk". Summing `owned_size` directly
/// across families is only correct when the artifacts are known to be
/// disjoint, which across families they are not — see [`size_identity`].
#[derive(Debug, Default)]
pub struct SizeTotal {
    seen: std::collections::HashSet<(PathBuf, Option<String>)>,
    bytes: u64,
}

impl SizeTotal {
    pub fn new() -> Self {
        Self::default()
    }

    /// Add one artifact, or skip it if its bytes were already counted.
    pub fn add(&mut self, artifact: &OwnedArtifact) {
        if self.seen.insert(size_identity(artifact)) {
            self.bytes = self.bytes.saturating_add(owned_size(artifact).unwrap_or(0));
        }
    }

    pub fn bytes(&self) -> u64 {
        self.bytes
    }
}

pub fn owned_size(artifact: &OwnedArtifact) -> Result<u64> {
    if artifact.kind != ArtifactKind::IncrementalDir {
        return path_size(&artifact.path);
    }
    let dir = &artifact.path;
    if !dir.is_dir() {
        return Ok(0);
    }
    // The recorded dir may itself be a key dir (a hand-set `-C incremental=`),
    // matching `collect_surplus_sessions`'s two shapes.
    if has_sessions(dir) {
        return path_size(dir);
    }
    let Some(prefix) = artifact.session_prefix.as_deref() else {
        return Ok(0);
    };
    let prefix = format!("{prefix}-");
    let mut total = 0u64;
    let Ok(entries) = fs::read_dir(dir) else {
        return Ok(0);
    };
    for entry in entries.filter_map(|e| e.ok()) {
        if !entry.file_name().to_string_lossy().starts_with(&prefix) {
            continue;
        }
        if entry.file_type().map(|t| t.is_dir()).unwrap_or(false) {
            total = total.saturating_add(path_size(&entry.path()).unwrap_or(0));
        }
    }
    Ok(total)
}

pub fn path_size(path: &Path) -> Result<u64> {
    let meta = fs::symlink_metadata(path)?;
    if meta.file_type().is_symlink() || meta.is_file() {
        return Ok(meta.len());
    }
    if !meta.is_dir() {
        return Ok(meta.len());
    }

    let mut total = 0u64;
    for entry in fs::read_dir(path)? {
        let entry = entry?;
        total = total.saturating_add(path_size(&entry.path()).unwrap_or(0));
    }
    Ok(total)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::thread::sleep;
    use std::time::Duration;

    /// A fixture key dir holding sessions created oldest-first, so mtimes
    /// order the same way the names imply.
    fn key_with_sessions(key: &Path, names: &[&str]) {
        for name in names {
            let session = key.join(name);
            fs::create_dir_all(&session).unwrap();
            fs::write(session.join("dep-graph.bin"), b"x".repeat(64)).unwrap();
            // mtime granularity guard: each session strictly newer than the last.
            sleep(Duration::from_millis(15));
        }
    }

    /// Ownership matching decides what may later be deleted, and it was
    /// rewritten to byte comparisons for speed (R748-F6 overhead work). These
    /// pin the exact predicate, asymmetry included, so the optimization cannot
    /// quietly widen it.
    #[test]
    fn ownership_matching_is_byte_exact_and_keeps_its_asymmetry() {
        let stem = b"foo-abc".as_slice();
        let lib_stem = b"libfoo-abc".as_slice();
        let owned = |n: &str| looks_owned(n.as_bytes(), stem, lib_stem);

        // Exact stems, and the extensions hanging off them.
        assert!(owned("foo-abc"));
        assert!(owned("libfoo-abc"));
        assert!(owned("foo-abc.d"));
        assert!(owned("libfoo-abc.rlib"));
        assert!(owned("libfoo-abc.rmeta"));
        // Codegen units hang off the BARE stem with a dash.
        assert!(owned("foo-abc-1a2b3c.rcgu.o"));

        // The asymmetry: `lib<stem>-…` is deliberately NOT owned.
        assert!(!owned("libfoo-abc-1a2b3c.rcgu.o"));
        // A different unit hash of the same crate is a different unit (R748-B9).
        assert!(!owned("libfoo-def.rlib"));
        // A longer crate name that merely starts the same way.
        assert!(!owned("foo-abcd.rlib"));
        assert!(!owned("libfoo-abcd.rlib"));
        assert!(!owned("other-abc.rlib"));
    }

    /// The R748-F2 verify fixture: two finalized sessions plus one `-working`
    /// session must plan exactly one deletion — the older finalized — and
    /// leave the newest, the `-working` session, and every `.lock` untouched.
    #[test]
    fn surplus_collection_takes_exactly_the_older_finalized_session() {
        let tmp = tempfile::tempdir().unwrap();
        let key = tmp.path().join("foo-abc123");
        key_with_sessions(&key, &["s-aaa-1", "s-bbb-2"]);
        fs::create_dir_all(key.join("s-ccc-3-working")).unwrap();
        fs::write(key.join("s-aaa-1.lock"), b"").unwrap();
        fs::write(key.join("s-bbb-2.lock"), b"").unwrap();

        let report = collect_surplus_sessions(&key, None, Mode::Delete);

        assert_eq!(report.deleted_sessions, 1, "{report:?}");
        assert!(report.deleted_bytes > 0);
        assert!(!key.join("s-aaa-1").exists(), "older finalized goes");
        assert!(key.join("s-bbb-2").exists(), "newest finalized stays");
        assert!(key.join("s-ccc-3-working").exists(), "-working is untouchable");
        assert!(key.join("s-aaa-1.lock").exists(), ".lock files are untouchable");
        assert!(key.join("s-bbb-2.lock").exists());
    }

    /// Cargo hands `-C incremental=` the profile-wide root shared by every
    /// crate. Collection must only descend into key dirs attributable to the
    /// recorded crate; other crates' keys are not this family's to touch.
    #[test]
    fn surplus_collection_descends_only_into_matching_key_dirs() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("incremental");
        key_with_sessions(&root.join("foo-abc"), &["s-a-1", "s-b-2"]);
        key_with_sessions(&root.join("other_crate-def"), &["s-c-1", "s-d-2"]);

        let report = collect_surplus_sessions(&root, Some("foo"), Mode::Delete);

        assert_eq!(report.deleted_sessions, 1, "{report:?}");
        assert!(!root.join("foo-abc/s-a-1").exists());
        assert!(root.join("foo-abc/s-b-2").exists());
        assert!(
            root.join("other_crate-def/s-c-1").exists(),
            "another crate's key dir must not be entered"
        );
    }

    /// A root-shaped dir with no prefix recorded (a pre-upgrade state file)
    /// must collect nothing rather than roam the shared tree.
    #[test]
    fn surplus_collection_without_prefix_leaves_a_root_untouched() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("incremental");
        key_with_sessions(&root.join("foo-abc"), &["s-a-1", "s-b-2"]);

        let report = collect_surplus_sessions(&root, None, Mode::Delete);

        assert_eq!(report.deleted_sessions, 0);
        assert!(root.join("foo-abc/s-a-1").exists());
    }

    /// R748-F6 — the surplus-session collector is the third deletion path, and
    /// the one that reclaims the most bytes. Shadow must report the identical
    /// figure and leave every session on disk.
    #[test]
    fn shadow_reports_the_same_surplus_it_would_have_collected() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("incremental");
        key_with_sessions(&root.join("foo-abc"), &["s-a-1", "s-b-2", "s-c-3"]);

        let shadow = collect_surplus_sessions(&root, Some("foo"), Mode::Shadow);
        assert_eq!(shadow.deleted_sessions, 2, "{shadow:?}");
        assert!(shadow.deleted_bytes > 0);
        assert!(root.join("foo-abc/s-a-1").exists(), "shadow deletes nothing");
        assert!(root.join("foo-abc/s-b-2").exists());

        // Same fixture, now for real: the promise the shadow report made.
        let real = collect_surplus_sessions(&root, Some("foo"), Mode::Delete);
        assert_eq!(real.deleted_sessions, shadow.deleted_sessions);
        assert_eq!(real.deleted_bytes, shadow.deleted_bytes);
        assert!(!root.join("foo-abc/s-a-1").exists());
        assert!(root.join("foo-abc/s-c-3").exists(), "the newest still survives");
    }

    /// `foo-` must not cross-match a key belonging to a crate whose name
    /// merely starts with `foo` (e.g. `foo_bar`). Underscore vs dash is what
    /// makes the prefix safe; this pins it.
    #[test]
    fn surplus_collection_prefix_does_not_cross_match_longer_crate_names() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("incremental");
        key_with_sessions(&root.join("foo_bar-abc"), &["s-a-1", "s-b-2"]);

        let report = collect_surplus_sessions(&root, Some("foo"), Mode::Delete);

        assert_eq!(report.deleted_sessions, 0);
        assert!(root.join("foo_bar-abc/s-a-1").exists());
    }
}
