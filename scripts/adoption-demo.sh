#!/usr/bin/env bash
# Adoption demo / soak: copy a real workspace, bootstrap cargo-orphan-gc into
# it the way a new user would, manufacture the shared-tree session surplus,
# and reclaim it.
#
#   scripts/adoption-demo.sh <workspace-dir> [file-to-touch]
#
# The workspace is copied (target/ excluded) into a mktemp, so the original is
# never touched. If the workspace patches deps by relative path, copy those
# siblings next to the copy yourself first — this script only copies what it
# is given.
#
# Phases:
#   0. `cargo orphan-gc bootstrap` — through the real CLI. When sccache is on
#      PATH the demo pre-installs it as rustc-wrapper first, so bootstrap's
#      adopt-existing-wrapper path (move it to inner-wrapper + [env]) runs.
#   1. wrapped cold build — every family learned, cache chained
#   2. concurrent UNWRAPPED builds — rustc's session GC skips under lock
#      contention, leaving surplus finalized sessions (the shared-tree leak)
#   3. `cargo orphan-gc sweep` — collects the surplus; zero rebuild cost
#   4. wrapped edit-recheck — the loop this all exists to protect
set -euo pipefail

ws_src="${1:?usage: adoption-demo.sh <workspace-dir> [file-to-touch]}"
script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
bin_dir="$(cd "$script_dir/../target/debug" && pwd)"
[ -x "$bin_dir/cargo-orphan-gc" ] || { echo "build the tool first: cargo build"; exit 1; }
export PATH="$bin_dir:$PATH"

d="$(mktemp -d)"
trap 'rm -rf "$d"' EXIT
rsync -a --exclude target "$ws_src/" "$d/ws/"
cd "$d/ws"

touch_file="${2:-$(find . -path ./target -prune -o \( -name lib.rs -o -name main.rs \) -print | head -1)}"
[ -n "$touch_file" ] || { echo "no lib.rs/main.rs found; pass a file to touch"; exit 1; }

# State is keyed by workspace path; ours is inside the mktemp, but the store
# root is $CARGO_HOME/orphan-gc — keep the demo's records out of the real one.
export CARGO_HOME="$d/cargo-home"
mkdir -p "$CARGO_HOME"
# ...while still using the machine's registry cache, so no re-download.
real_home="${CARGO_HOME_REAL:-$HOME/.cargo}"
for shared in registry git; do
  [ -e "$real_home/$shared" ] && ln -s "$real_home/$shared" "$CARGO_HOME/$shared"
done

echo "===== phase 0: bootstrap (the real CLI, adopting sccache when present) ====="
if command -v sccache >/dev/null 2>&1; then
  mkdir -p .cargo
  printf '[build]\nrustc-wrapper = "sccache"\n' > .cargo/config.toml
fi
cargo orphan-gc bootstrap
grep -A2 '^\[build\]' .cargo/config.toml; grep -A1 '^\[env\]' .cargo/config.toml || true

sessions() { find target/debug/incremental -mindepth 2 -maxdepth 2 -type d -name 's-*' ! -name '*-working' 2>/dev/null | wc -l | tr -d ' '; }
incr_du()  { du -sm target/debug/incremental 2>/dev/null | cut -f1; }

echo
echo "===== phase 1: wrapped cold build ====="
/usr/bin/time -p cargo build -q 2>&1 | tail -3
echo "sessions=$(sessions) incremental_mb=$(incr_du)"

echo
echo "===== phase 2: concurrent unwrapped builds (manufacture the surplus) ====="
mv .cargo/config.toml .cargo/config.toml.off
for round in 1 2 3; do
  touch "$touch_file"
  cargo build -q 2>/dev/null & p1=$!
  sleep 0.4
  cargo build -q 2>/dev/null & p2=$!
  wait $p1 $p2 || true
  echo "round $round: sessions=$(sessions) incremental_mb=$(incr_du)"
done
mv .cargo/config.toml.off .cargo/config.toml

echo
echo "===== phase 3: sweep (reclaim, zero rebuild cost) ====="
before_s=$(sessions); before_mb=$(incr_du)
cargo orphan-gc sweep
echo "sessions=$(sessions) incremental_mb=$(incr_du)  (was ${before_s} sessions / ${before_mb} MB)"

echo
echo "===== phase 4: wrapped edit-recheck ====="
printf '\n// probe\n' >> "$touch_file"
/usr/bin/time -p cargo build -q 2>&1 | tail -3
