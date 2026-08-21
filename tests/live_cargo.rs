//! End-to-end through a real cargo: the wrapper is installed as
//! `build.rustc-wrapper` in a scratch package, real builds run through it,
//! a fabricated surplus incremental session is collected by `sweep`, and —
//! the point of collecting conservatively — the tree still builds
//! incrementally afterwards.
#![cfg(unix)]

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

/// A cargo whose view of the world is the scratch workspace and nothing else.
///
/// The env_removes are load-bearing, not defensive tidiness: this test asserts
/// on paths under `ws/target`, and an outer `CARGO_TARGET_DIR` (a shared target
/// dir is a common setup, and `cargo test` passes it straight through to child
/// processes) silently redirects the inner build elsewhere, so the assertions
/// fail on a missing directory rather than on anything this crate did. Same for
/// an ambient wrapper, which would displace the one the scratch config installs.
///
/// `CARGO_ORPHAN_GC_INNER_WRAPPER` is the subtle one. It is this tool's own
/// `[env]` transport, so a checkout that has run `bootstrap` on itself — or on
/// any ancestor directory — hands its real compiler cache to this test's
/// fixture builds. With `CARGO_INCREMENTAL=1` set two lines up, sccache then
/// hard-fails ("incremental compilation is prohibited") and the failure reads
/// as a broken wrapper rather than a leaked variable. Dogfooding the tool on
/// the repo that develops it is exactly how this surfaced.
fn cargo(ws: &Path, cargo_home: &Path) -> Command {
    let mut c = Command::new("cargo");
    c.current_dir(ws)
        .env("CARGO_HOME", cargo_home)
        .env("CARGO_INCREMENTAL", "1")
        .env_remove("CARGO_TARGET_DIR")
        .env_remove("CARGO_BUILD_TARGET_DIR")
        .env_remove("CARGO_ORPHAN_GC_INNER_WRAPPER")
        // The outer `cargo test` sets CARGO_MANIFEST_DIR to THIS crate, and
        // `config::discover_for_wrapper` prefers it over the cwd (deliberately
        // — that is how registry-dependency units find the policy). Inherited
        // into the scratch build, it makes the wrapper discover the *outer*
        // workspace's policy instead of the fixture's, inner-wrapper included.
        .env_remove("CARGO_MANIFEST_DIR")
        .env_remove("RUSTC_WRAPPER")
        .env_remove("CARGO_BUILD_RUSTC_WRAPPER")
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

/// R748-B9's end-to-end proof: a real cargo build in which one crate is
/// compiled twice with argv identical *except* for its own unit hash and the
/// `--extern` hash of a dependency — the one shape the pre-fix family key
/// could not tell apart, because it stripped `-C metadata` / `-C extra-filename`
/// and normalizes extern hashes to `<artifact>` on purpose.
///
/// This one has teeth. Measured 2026-08-12 against a pre-fix binary (the same
/// tree with `parse_codegen`'s two hash arms reverted to dropping the value):
/// run 2 exits 101 with
/// `error: extern location for mid does not exist: .../libmid-<hash>.rmeta`
/// and that unit's `.rlib`/`.rmeta` are gone from `target/debug/deps` —
/// byte-for-byte the failure reported off the camp. With the fix both runs exit
/// 0 and both units survive.
///
/// Two details are load-bearing, and both are why earlier attempts at this test
/// came back green against the pre-fix binary:
///
/// * **The divergence must be one level BELOW the crate under watch.** Resolver
///   2 deliberately does not unify features between the build graph and the
///   normal graph, so `dep` is built two ways; `mid` depends on `dep` and is
///   therefore built twice, while `mid`'s own argv never mentions `dep`'s
///   features. Make `mid` itself differ (a dev-dep feature, `--all-targets`,
///   a `--test` harness) and cargo also changes `--crate-type` / `--emit`,
///   which forks the family even under the old identity — the test then proves
///   nothing. `[profile.dev.build-override]` is pinned to the dev profile for
///   the same reason: an unpinned `codegen-units` shows up in argv.
/// * **`full-scan-every = 1`.** At the default 16 the second compile of a
///   family takes the artifact-reuse fast path, re-records the FIRST unit's
///   path list, and so never supersedes — the collision is real but invisible.
///   That is also the explanation for the camp symptom nobody could pin down:
///   the failure hit a different crate on each run because it needed a family
///   whose `compiles_since_scan` happened to come due on the second unit.
#[test]
fn two_units_of_one_crate_differing_only_in_hash_do_not_reclaim_each_other() {
    let tmp = tempfile::tempdir().unwrap();
    let ws = tmp.path().join("ws");
    let cargo_home = tmp.path().join("cargo-home");
    fs::create_dir_all(ws.join("dep/src")).unwrap();
    fs::create_dir_all(ws.join("mid/src")).unwrap();
    fs::create_dir_all(ws.join("app/src")).unwrap();
    fs::create_dir_all(ws.join(".cargo")).unwrap();
    fs::create_dir_all(&cargo_home).unwrap();

    fs::write(
        ws.join("Cargo.toml"),
        "[workspace]\nmembers = [\"dep\", \"mid\", \"app\"]\nresolver = \"2\"\n\n\
         [profile.dev.build-override]\nopt-level = 0\ndebug = true\nincremental = true\n\n\
         [workspace.metadata.orphan-gc]\nenabled = true\ndry-run = false\nverbose = true\n\
         full-scan-every = 1\n",
    )
    .unwrap();
    fs::write(
        ws.join("dep/Cargo.toml"),
        "[package]\nname = \"dep\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n\
         [features]\nextra = []\n",
    )
    .unwrap();
    fs::write(
        ws.join("dep/src/lib.rs"),
        "pub fn base() -> u64 { 1 }\n#[cfg(feature = \"extra\")]\npub fn extra() -> u64 { 2 }\n",
    )
    .unwrap();
    // `mid` is the crate under watch: compiled once for the normal graph and
    // once for the build graph, with argv identical except its own unit hash
    // and the `--extern dep=` hash — which the wrapper normalizes away.
    fs::write(
        ws.join("mid/Cargo.toml"),
        "[package]\nname = \"mid\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n\
         [dependencies]\ndep = { path = \"../dep\" }\n",
    )
    .unwrap();
    fs::write(ws.join("mid/src/lib.rs"), "pub fn m() -> u64 { dep::base() }\n").unwrap();
    fs::write(
        ws.join("app/Cargo.toml"),
        "[package]\nname = \"app\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n\
         [dependencies]\nmid = { path = \"../mid\" }\n\n\
         [build-dependencies]\nmid = { path = \"../mid\" }\n\
         dep = { path = \"../dep\", features = [\"extra\"] }\n",
    )
    .unwrap();
    fs::write(ws.join("app/src/lib.rs"), "pub fn f() -> u64 { mid::m() }\n").unwrap();
    fs::write(
        ws.join("app/build.rs"),
        "fn main() { println!(\"cargo:rustc-env=X={}\", mid::m() + dep::extra()); }\n",
    )
    .unwrap();
    fs::write(
        ws.join(".cargo/config.toml"),
        format!(
            "[build]\nrustc-wrapper = \"{}\"\n",
            env!("CARGO_BIN_EXE_cargo-orphan-gc")
        ),
    )
    .unwrap();

    // Twice: run 1 learns both units, and run 2 is where a wrongly-reclaimed
    // artifact from run 1 surfaces as a missing extern. The pre-fix binary
    // fails on run 2, not run 1.
    for run in 1..=2 {
        let out = cargo(&ws, &cargo_home)
            .arg("build")
            .arg("--workspace")
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "run {run}: wrapped build must not break the build it is watching:\n{}",
            String::from_utf8_lossy(&out.stderr)
        );
    }

    // The fixture really did produce two units of `mid` — otherwise the
    // assertion above proves nothing about family identity.
    let families = fs::read_dir(
        fs::read_dir(cargo_home.join("orphan-gc/workspaces"))
            .unwrap()
            .next()
            .expect("the wrapper recorded a workspace")
            .unwrap()
            .path()
            .join("families"),
    )
    .unwrap()
    .filter_map(|e| e.ok())
    .filter_map(|e| fs::read_to_string(e.path()).ok())
    .filter(|text| text.contains("\"label\": \"mid ("))
    .count();
    assert!(
        families >= 2,
        "expected >= 2 unit hashes of `mid` to be tracked separately, got {families}"
    );

    let rmetas = fs::read_dir(ws.join("target/debug/deps"))
        .unwrap()
        .filter_map(|e| e.ok())
        .filter(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            name.starts_with("libmid-") && name.ends_with(".rmeta")
        })
        .count();
    assert!(
        rmetas >= 2,
        "both unit hashes' rmeta must survive the invocation that built them, got {rmetas}"
    );
}

#[test]
fn surplus_sessions_are_collected_and_the_tree_still_builds_incrementally() {
    let tmp = tempfile::tempdir().unwrap();
    let ws = tmp.path().join("ws");
    let cargo_home = tmp.path().join("cargo-home");
    fs::create_dir_all(ws.join("src")).unwrap();
    fs::create_dir_all(&cargo_home).unwrap();

    // `dry-run = false` is load-bearing, not boilerplate: a fresh install is in
    // shadow mode (R748-F6), so a manifest that only says `enabled = true`
    // reclaims nothing on purpose. This test is about what deletion does, so it
    // authorizes deletion — the shadow half is asserted below, first.
    fs::write(
        ws.join("Cargo.toml"),
        "[package]\nname = \"scratch\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n\
         [package.metadata.orphan-gc]\nenabled = true\ndry-run = false\n",
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

    let sweep_cmd = |extra: &[&str]| {
        let out = Command::new(env!("CARGO_BIN_EXE_cargo-orphan-gc"))
            .arg("orphan-gc")
            .arg("sweep")
            .args(extra)
            .current_dir(&ws)
            .env("CARGO_HOME", &cargo_home)
            .env_remove("CARGO_TARGET_DIR")
            .env_remove("CARGO_BUILD_TARGET_DIR")
            .output()
            .unwrap();
        assert!(out.status.success(), "{out:?}");
        String::from_utf8_lossy(&out.stdout).into_owned()
    };

    // Shadow first, through real cargo: the report a `dry-run = true` install
    // would show an operator, against a tree with a real surplus session.
    let shadow = sweep_cmd(&["--dry-run"]);
    assert!(
        shadow.contains("would collect 1 surplus incremental sessions"),
        "a shadow sweep must report the surplus it would take: {shadow}"
    );
    assert!(fake.exists(), "a shadow sweep must leave the surplus session on disk");

    let stdout = sweep_cmd(&[]);
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
