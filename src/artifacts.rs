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

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

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

pub fn collect(inv: &Invocation) -> Result<Vec<OwnedArtifact>> {
    let mut found = BTreeMap::<PathBuf, OwnedArtifact>::new();

    if let (Some(out_dir), Some(crate_name), Some(extra)) =
        (&inv.out_dir, &inv.crate_name, &inv.extra_filename)
    {
        if out_dir.is_dir() {
            for entry in fs::read_dir(out_dir)
                .with_context(|| format!("read rustc out-dir {}", out_dir.display()))?
            {
                let entry = entry?;
                let name = entry.file_name().to_string_lossy().into_owned();
                if looks_owned(&name, crate_name, extra) {
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

    Ok(found.into_values().collect())
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
pub fn collect_surplus_sessions(dir: &Path, session_prefix: Option<&str>) -> SessionCollection {
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
        collect_in_key(dir, &mut report);
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
            collect_in_key(&entry.path(), &mut report);
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
    sessions.sort_by(|a, b| b.1.cmp(&a.1));
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

fn collect_in_key(key_dir: &Path, report: &mut SessionCollection) {
    for (session, _) in finalized_sessions(key_dir).iter().skip(1) {
        let bytes = path_size(session).unwrap_or(0);
        if fs::remove_dir_all(session).is_ok() {
            report.deleted_sessions += 1;
            report.deleted_bytes = report.deleted_bytes.saturating_add(bytes);
        }
    }
}

fn looks_owned(name: &str, crate_name: &str, extra: &str) -> bool {
    if extra.is_empty() {
        return false;
    }
    let stem = format!("{crate_name}{extra}");
    let lib_stem = format!("lib{crate_name}{extra}");
    name == stem
        || name == lib_stem
        || name.starts_with(&(stem.clone() + "."))
        || name.starts_with(&(lib_stem + "."))
        || name.starts_with(&(stem + "-"))
}

pub fn remove(artifact: &OwnedArtifact) -> Result<u64> {
    validate(artifact)?;
    if !artifact.path.exists() {
        return Ok(0);
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
    Ok(bytes)
}

fn validate(artifact: &OwnedArtifact) -> Result<()> {
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
            anyhow::ensure!(
                artifact.path == artifact.root,
                "refusing incremental deletion with mismatched ownership root: {}",
                artifact.path.display()
            );
        }
    }
    Ok(())
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

        let report = collect_surplus_sessions(&key, None);

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

        let report = collect_surplus_sessions(&root, Some("foo"));

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

        let report = collect_surplus_sessions(&root, None);

        assert_eq!(report.deleted_sessions, 0);
        assert!(root.join("foo-abc/s-a-1").exists());
    }

    /// `foo-` must not cross-match a key belonging to a crate whose name
    /// merely starts with `foo` (e.g. `foo_bar`). Underscore vs dash is what
    /// makes the prefix safe; this pins it.
    #[test]
    fn surplus_collection_prefix_does_not_cross_match_longer_crate_names() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("incremental");
        key_with_sessions(&root.join("foo_bar-abc"), &["s-a-1", "s-b-2"]);

        let report = collect_surplus_sessions(&root, Some("foo"));

        assert_eq!(report.deleted_sessions, 0);
        assert!(root.join("foo_bar-abc/s-a-1").exists());
    }
}
