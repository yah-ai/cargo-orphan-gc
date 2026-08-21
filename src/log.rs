//! Where operational output goes, and why it is emphatically not the
//! compiler's stderr.
//!
//! Cargo captures each rustc invocation's stderr into
//! `target/<profile>/.fingerprint/<unit>/output-<...>` and replays it verbatim
//! on every later build where that unit is fresh. A line this wrapper writes to
//! stderr therefore describes whichever build last *compiled* that unit — not
//! the build whose output the reader is watching — and on screen the two are
//! indistinguishable. Replay is also per-unit, so a stale line appears
//! alongside genuinely fresh output from its neighbours, which is what makes
//! the "surely a cached run would print nothing" reflex wrong.
//!
//! That cost three wrong conclusions in one day (R748-B10), including a
//! "verified" end-to-end run that compiled nothing at all and a kill-switch bug
//! report against a kill switch that worked.
//!
//! So the operational log is a file under the state dir, appended with a
//! timestamp and a pid so a reader can tell one invocation from another, and it
//! is echoed to stderr only when stderr is a terminal — a terminal is by
//! definition not cargo's capture pipe.

use std::fs::OpenOptions;
use std::io::{IsTerminal, Write};
use std::path::{Path, PathBuf};

use crate::state::{now_ms, Store};

/// Bytes the live log may reach before it is rotated. A GC tool that grows an
/// unbounded file in `$CARGO_HOME` would be arguing against itself; one
/// rotation keeps the whole channel under 2 MiB forever.
const MAX_LOG_BYTES: u64 = 1 << 20;

pub struct Log {
    path: PathBuf,
}

impl Log {
    pub fn for_store(store: &Store) -> Self {
        Self { path: log_path(&store.root) }
    }

    /// Record one operational line. Best-effort by construction: this runs
    /// inside a rustc invocation, and failing a compile because a log file
    /// could not be opened would be a far worse bug than losing the line.
    pub fn write(&self, message: &str) {
        if std::io::stderr().is_terminal() {
            eprintln!("cargo-orphan-gc: {message}");
        }
        let line = format!("{} pid={} {message}\n", timestamp(now_ms()), std::process::id());
        let _ = self.append(&line);
    }

    fn append(&self, line: &str) -> std::io::Result<()> {
        self.rotate_if_full();
        let mut file = OpenOptions::new().create(true).append(true).open(&self.path)?;
        // One `write_all` of a short line through an O_APPEND handle is what
        // keeps concurrent rustc processes from interleaving mid-line; there is
        // deliberately no lock here, because the family lock is held across
        // real work and this must not be able to serialize compiles.
        file.write_all(line.as_bytes())
    }

    fn rotate_if_full(&self) {
        let Ok(meta) = std::fs::metadata(&self.path) else {
            return;
        };
        if meta.len() <= MAX_LOG_BYTES {
            return;
        }
        let _ = std::fs::rename(&self.path, self.path.with_extension("1"));
    }
}

pub fn log_path(state_root: &Path) -> PathBuf {
    state_root.join("log")
}

/// The last `lines` lines of the log, oldest first. Reads the rotated file too,
/// so a tail that spans a rotation is still contiguous.
pub fn tail(state_root: &Path, lines: usize) -> Vec<String> {
    let live = log_path(state_root);
    let rotated = live.with_extension("1");
    let mut all = Vec::new();
    for path in [&rotated, &live] {
        if let Ok(text) = std::fs::read_to_string(path) {
            all.extend(text.lines().map(str::to_string));
        }
    }
    let start = all.len().saturating_sub(lines);
    all.split_off(start)
}

/// `YYYY-MM-DDTHH:MM:SSZ` from unix milliseconds.
///
/// Hand-rolled rather than pulling in a date crate: this is the only place the
/// tool formats a time, and the point of the timestamp is only to let a reader
/// tell two invocations apart.
fn timestamp(unix_ms: u128) -> String {
    let secs = (unix_ms / 1000) as i64;
    let (year, month, day) = civil_from_days(secs.div_euclid(86_400));
    let tod = secs.rem_euclid(86_400);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z",
        tod / 3600,
        (tod % 3600) / 60,
        tod % 60
    )
}

/// Days since the unix epoch to a civil date — Howard Hinnant's
/// `civil_from_days`, which is exact for the whole proleptic Gregorian range.
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let month = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let year = yoe + era * 400 + i64::from(month <= 2);
    (year, month, day)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store_in(dir: &Path) -> Store {
        let store = Store { root: dir.join("state") };
        store.ensure_layout().unwrap();
        store
    }

    #[test]
    fn a_line_carries_a_timestamp_and_a_pid_so_a_replay_could_not_masquerade_as_fresh() {
        let tmp = tempfile::tempdir().unwrap();
        let store = store_in(tmp.path());
        Log::for_store(&store).write("deleted 3 artifacts");

        let text = std::fs::read_to_string(log_path(&store.root)).unwrap();
        assert!(text.contains("deleted 3 artifacts"), "{text}");
        assert!(text.contains(&format!("pid={}", std::process::id())), "{text}");
        assert!(text.starts_with("20"), "line must lead with the timestamp: {text}");
    }

    #[test]
    fn the_log_is_bounded_by_one_rotation() {
        let tmp = tempfile::tempdir().unwrap();
        let store = store_in(tmp.path());
        let log = Log::for_store(&store);

        let live_path = log_path(&store.root);
        std::fs::write(&live_path, vec![b'x'; (MAX_LOG_BYTES + 1) as usize]).unwrap();
        log.write("after the cap");

        let live = std::fs::read_to_string(&live_path).unwrap();
        assert!(live.contains("after the cap"));
        assert!(
            live.len() < MAX_LOG_BYTES as usize,
            "the live file restarts after rotation, got {} bytes",
            live.len()
        );
        assert!(
            live_path.with_extension("1").exists(),
            "the previous log is kept, exactly once"
        );
    }

    #[test]
    fn tail_spans_a_rotation_oldest_first() {
        let tmp = tempfile::tempdir().unwrap();
        let store = store_in(tmp.path());
        let live = log_path(&store.root);
        std::fs::write(live.with_extension("1"), "older-1\nolder-2\n").unwrap();
        std::fs::write(&live, "newer-1\nnewer-2\n").unwrap();

        assert_eq!(
            tail(&store.root, 3),
            vec!["older-2", "newer-1", "newer-2"],
            "the tail crosses the rotation boundary in order"
        );
    }

    #[test]
    fn timestamps_are_utc_iso8601() {
        assert_eq!(timestamp(0), "1970-01-01T00:00:00Z");
        // 2026-08-12T03:35:14Z — the hour R748-B10 was filed.
        assert_eq!(timestamp(1_786_505_714_000), "2026-08-12T03:35:14Z");
        // A leap day, which is where a hand-rolled calendar goes wrong.
        assert_eq!(timestamp(1_709_164_800_000), "2024-02-29T00:00:00Z");
    }
}
