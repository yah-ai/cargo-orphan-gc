use std::collections::HashSet;
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use fs2::FileExt;
use serde::{Deserialize, Serialize};

use crate::family::Invocation;
use crate::state::{now_ms, Store};

#[derive(Clone, Debug, Serialize, Deserialize)]
struct LeaseRecord {
    pid: u32,
    created_unix_ms: u128,
    inputs: Vec<PathBuf>,
}

pub struct Lease {
    file: File,
    path: PathBuf,
}

impl Lease {
    pub fn create(store: &Store, inv: &Invocation) -> Result<Self> {
        Self::create_with_inputs(store, &inv.extern_paths)
    }

    /// The lease body, taking the concrete input paths directly. Split out of
    /// [`Lease::create`] so the protection it provides can be exercised
    /// without standing up a whole rustc [`Invocation`].
    pub(crate) fn create_with_inputs(store: &Store, inputs: &[PathBuf]) -> Result<Self> {
        let nonce = format!("{}-{}", std::process::id(), now_ms());
        let path = store.root.join("leases").join(format!("{nonce}.json"));
        let mut file = OpenOptions::new()
            .create_new(true)
            .read(true)
            .write(true)
            .open(&path)
            .with_context(|| format!("create lease {}", path.display()))?;
        file.lock_exclusive()?;

        let record = LeaseRecord {
            pid: std::process::id(),
            created_unix_ms: now_ms(),
            inputs: inputs.to_vec(),
        };
        serde_json::to_writer(&mut file, &record)?;
        file.flush()?;
        Ok(Self { file, path })
    }
}

impl Drop for Lease {
    fn drop(&mut self) {
        let _ = self.file.unlock();
        let _ = fs::remove_file(&self.path);
    }
}

#[derive(Default)]
pub struct ActiveInputs {
    pub paths: HashSet<PathBuf>,
    pub unknown_active_lease: bool,
}

pub fn active_inputs(store: &Store) -> Result<ActiveInputs> {
    let mut active = ActiveInputs::default();
    for entry in fs::read_dir(store.root.join("leases"))? {
        let entry = entry?;
        let path = entry.path();
        if !path.is_file() {
            continue;
        }

        let mut file = match OpenOptions::new().read(true).write(true).open(&path) {
            Ok(file) => file,
            Err(_) => {
                active.unknown_active_lease = true;
                continue;
            }
        };

        match file.try_lock_exclusive() {
            Ok(()) => {
                // Nobody owns the lease anymore. It is stale (usually a process
                // that crashed or exited without cleanup).
                let _ = file.unlock();
                drop(file);
                let _ = fs::remove_file(&path);
            }
            Err(_) => {
                if let Ok(record) = read_record(&mut file) {
                    active.paths.extend(record.inputs);
                } else {
                    // Fail closed. An unreadable active lease blocks deletion.
                    active.unknown_active_lease = true;
                }
            }
        }
    }
    Ok(active)
}

fn read_record(file: &mut File) -> Result<LeaseRecord> {
    file.seek(SeekFrom::Start(0))?;
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes)?;
    Ok(serde_json::from_slice(&bytes)?)
}

pub fn is_active_input(active: &ActiveInputs, path: &Path) -> bool {
    active.paths.contains(path)
}
