use std::collections::BTreeSet;
use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

#[derive(Clone, Debug)]
pub struct Invocation {
    pub real_rustc: OsString,
    pub args: Vec<OsString>,
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
    pub fn parse(real_rustc: OsString, args: Vec<OsString>) -> Result<Self> {
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
            real_rustc,
            args,
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
        // Cargo/rustc generation identity. Strip these so a rebuilt version maps
        // back to the same logical family.
        "metadata" => {}
        "extra-filename" => {
            *extra_filename = Some(value.to_string());
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

    #[test]
    fn volatile_codegen_fields_do_not_change_family() {
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
            Invocation::parse(OsString::from("rustc"), xs.into_iter().map(OsString::from).collect()).unwrap()
        };
        assert_eq!(make(base).family_key, make(next).family_key);
    }
}
