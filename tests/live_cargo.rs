//! End-to-end through a real cargo: the wrapper is installed as
//! `build.rustc-wrapper` in a scratch package, real builds run through it,
//! a fabricated surplus incremental session is collected by `sweep`, and —
//! the point of collecting conservatively — the tree still builds
//! incrementally afterwards.
#![cfg(unix)]

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

fn cargo(ws: &Path, cargo_home: &Path) -> Command {
    let mut c = Command::new("cargo");
    c.current_dir(ws)
        .env("CARGO_HOME", cargo_home)
        .env("CARGO_INCREMENTAL", "1")
        .env_remove("RUSTC_WRAPPER")
        .env_remove("RUSTC_WORKSPACE_WRAPPER");
    c
}

fn finalized_sessions(key: &Path) -> Vec<PathBuf> {
    fs::read_dir(key)
        .unwrap()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().map(|t| t.is_dir()).unwrap_or(false))
        .map(|e| e.path())
        .filter(|p| {
            let name = p.file_name().unwrap().to_string_lossy().into_owned();
            name.starts_with("s-") && !name.ends_with("-working")
        })
        .collect()
}

#[test]
fn surplus_sessions_are_collected_and_the_tree_still_builds_incrementally() {
    let tmp = tempfile::tempdir().unwrap();
    let ws = tmp.path().join("ws");
    let cargo_home = tmp.path().join("cargo-home");
    fs::create_dir_all(ws.join("src")).unwrap();
    fs::create_dir_all(&cargo_home).unwrap();

    fs::write(
        ws.join("Cargo.toml"),
        "[package]\nname = \"scratch\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n\
         [package.metadata.orphan-gc]\nenabled = true\n",
    )
    .unwrap();
    fs::write(ws.join("src/lib.rs"), "pub fn f() -> u64 { 1 }\n").unwrap();
    fs::create_dir_all(ws.join(".cargo")).unwrap();
    fs::write(
        ws.join(".cargo/config.toml"),
        format!(
            "[build]\nrustc-wrapper = \"{}\"\n",
            env!("CARGO_BIN_EXE_cargo-orphan-gc")
        ),
    )
    .unwrap();

    let status = cargo(&ws, &cargo_home).arg("build").status().unwrap();
    assert!(status.success(), "wrapped build must succeed");

    // Exactly the shape cargo produces: a shared incremental root holding a
    // per-crate key dir with one finalized session inside.
    let incr = ws.join("target/debug/incremental");
    let key = fs::read_dir(&incr)
        .unwrap()
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .find(|p| {
            p.is_dir() && p.file_name().unwrap().to_string_lossy().starts_with("scratch-")
        })
        .expect("wrapped build should have produced a scratch-* incremental key");
    let sessions = finalized_sessions(&key);
    assert_eq!(sessions.len(), 1, "one build, one finalized session");
    let real = sessions[0].clone();

    // Fabricate the surplus rustc leaves behind when its own session GC skips
    // under lock contention: a second finalized session in the same key. The
    // copy is made first and the real session touched after, so the real one
    // is strictly newest and must be the survivor.
    let fake = key.join("s-0000000000-fake00");
    let cp = Command::new("cp")
        .arg("-R")
        .arg(&real)
        .arg(&fake)
        .status()
        .unwrap();
    assert!(cp.success());
    std::thread::sleep(std::time::Duration::from_millis(20));
    assert!(Command::new("touch").arg(&real).status().unwrap().success());

    let sweep = Command::new(env!("CARGO_BIN_EXE_cargo-orphan-gc"))
        .arg("orphan-gc")
        .arg("sweep")
        .current_dir(&ws)
        .env("CARGO_HOME", &cargo_home)
        .output()
        .unwrap();
    assert!(sweep.status.success(), "{sweep:?}");
    let stdout = String::from_utf8_lossy(&sweep.stdout);
    assert!(
        stdout.contains("collected 1 surplus incremental sessions"),
        "sweep must report the surplus session: {stdout}"
    );
    assert!(!fake.exists(), "the older finalized session is collected");
    assert!(real.exists(), "the newest finalized session survives");

    // A smaller VALID cache, not an empty one: an edit still builds, and the
    // surviving session still seeds rustc's incremental state.
    fs::write(
        ws.join("src/lib.rs"),
        "pub fn f() -> u64 { 2 }\npub fn g() -> u64 { 3 }\n",
    )
    .unwrap();
    let status = cargo(&ws, &cargo_home).arg("build").status().unwrap();
    assert!(status.success(), "the tree must still build after collection");
    assert!(
        !finalized_sessions(&key).is_empty(),
        "incremental state is still in use after the edit"
    );
}
