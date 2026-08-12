#!/usr/bin/env bash
# Wrapper-chain A/B (R748-F1).
#
# Cargo nests `rustc-wrapper` OUTSIDE `rustc-workspace-wrapper`. When sccache
# holds the outer slot it is handed argv[1] = the inner wrapper binary rather
# than rustc, cannot recognise it as a compiler, and falls off its rust-aware
# path — reporting `multiple input files` and caching nothing. Nothing errors;
# the cache simply stops working for exactly the crates being wrapped, which is
# invisible unless you read sccache's stats.
#
#   A — sccache outer, tool inner   => `multiple input files` appears
#   B — tool outer, calling sccache => it does not; only sccache's legitimate
#                                      refusals remain (crate-type, incremental)
#   C — the shipped fix: cargo-orphan-gc in the outer slot with
#       inner-wrapper = "sccache" => same clean stats as B, plus family state
#       actually recorded (the tool is doing its job, not just passing through)
#
# A deliberately uses a PURE PASSTHROUGH as the inner wrapper, so a failure here
# indicts cargo's nesting rather than anything cargo-orphan-gc does.
#
# C needs the built binary; pass ORPHAN_GC_BIN or have target/debug/cargo-orphan-gc
# present next to this script's repo (cargo build first). C is skipped otherwise.
#
# Re-run this after any change to how the wrapper is installed.
set -euo pipefail

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ORPHAN_GC_BIN="${ORPHAN_GC_BIN:-$script_dir/../target/debug/cargo-orphan-gc}"

command -v sccache >/dev/null || { echo "sccache not on PATH; this A/B needs it"; exit 1; }

d="$(mktemp -d)"

# ---------------------------------------------------------------------------
# This script runs against a PRIVATE sccache server, never the machine's.
#
# Case A deliberately induces the broken nesting, and that turns out to do more
# than disable caching: cargo probes the toolchain by running `rustc -vV`
# through the wrapper chain, so sccache is handed a shell script where it
# expects a compiler — and it HANGS rather than failing. The hung client never
# exits, and enough of them wedge the server for every user on the machine:
# `sccache --show-stats` keeps answering instantly while every real compile
# request blocks forever. It did exactly that to three other sessions' builds
# once, which is how this isolation got written.
#
# A private port + cache dir means the blast radius is this script. The trap
# stops that server and reaps any client still stuck against it.
# ---------------------------------------------------------------------------
export SCCACHE_SERVER_PORT="$(( 4300 + RANDOM % 400 ))"
export SCCACHE_DIR="$d/sccache"
cleanup() {
  pkill -9 -f "sccache $d/" 2>/dev/null || true
  SCCACHE_SERVER_PORT="$SCCACHE_SERVER_PORT" sccache --stop-server >/dev/null 2>&1 || true
  rm -rf "$d"
}
trap cleanup EXIT
sccache --start-server >/dev/null 2>&1 || true
mkdir -p "$d/ws/lib/src" "$d/ws/app/src" "$d/ws/.cargo"

cat > "$d/ws/Cargo.toml" <<'TOML'
[workspace]
members = ["lib", "app"]
resolver = "2"
TOML
cat > "$d/ws/lib/Cargo.toml" <<'TOML'
[package]
name = "wlib"
version = "0.1.0"
edition = "2021"
TOML
for i in $(seq 1 200); do echo "pub fn f$i(x: u64) -> u64 { x.wrapping_mul($i) }"; done > "$d/ws/lib/src/lib.rs"
cat > "$d/ws/app/Cargo.toml" <<'TOML'
[package]
name = "wapp"
version = "0.1.0"
edition = "2021"

[dependencies]
wlib = { path = "../lib" }
TOML
echo 'fn main() { println!("{}", wlib::f1(2)); }' > "$d/ws/app/src/main.rs"

printf '#!/usr/bin/env bash\nexec "$@"\n'          > "$d/passthrough.sh"
printf '#!/usr/bin/env bash\nexec sccache "$@"\n'  > "$d/chained.sh"
chmod +x "$d/passthrough.sh" "$d/chained.sh"

stats() {
  sccache --show-stats \
    | grep -E "Cache hits  |Cache misses  |Non-cacheable calls|^multiple|^incremental|^crate-type" \
    || true
}

# Leg A's broken nesting can hang sccache outright (see the isolation note
# above), so bound it when coreutils timeout is available.
maybe_timeout() {
  if command -v timeout >/dev/null 2>&1; then timeout 120 "$@"; else "$@"; fi
}

cd "$d/ws"

echo "===== A: nested — sccache outer, tool inner (expect: multiple input files) ====="
cat > .cargo/config.toml <<TOML
[build]
rustc-wrapper = "sccache"
rustc-workspace-wrapper = "$d/passthrough.sh"
TOML
# `rm -rf target`, not `cargo clean`: clean takes cargo's global
# ~/.cargo/.package-cache lock and will block for as long as any other build on
# the machine holds it — which, on a shared machine, looks exactly like this
# script hanging. The target dir here is inside our own mktemp, so removing it
# directly is equivalent and takes no lock.
rm -rf target; sccache --zero-stats >/dev/null
maybe_timeout cargo build -q 2>&1 | head -3 || echo "(leg A build failed or timed out — the nesting hazard itself)"
stats

echo
echo "===== B: inverted — tool outer, invoking sccache (expect: NO multiple input files) ====="
cat > .cargo/config.toml <<TOML
[build]
rustc-wrapper = "$d/chained.sh"
TOML
# `rm -rf target`, not `cargo clean`: clean takes cargo's global
# ~/.cargo/.package-cache lock and will block for as long as any other build on
# the machine holds it — which, on a shared machine, looks exactly like this
# script hanging. The target dir here is inside our own mktemp, so removing it
# directly is equivalent and takes no lock.
rm -rf target; sccache --zero-stats >/dev/null
cargo build -q 2>&1 | head -3
stats

echo
if [ ! -x "$ORPHAN_GC_BIN" ]; then
  echo "===== C: skipped — no binary at $ORPHAN_GC_BIN (cargo build it, or set ORPHAN_GC_BIN) ====="
  exit 0
fi
echo "===== C: shipped fix — cargo-orphan-gc outer, inner-wrapper = sccache (expect: NO multiple input files, family state recorded) ====="
cat > .cargo/config.toml <<TOML
[build]
rustc-wrapper = "$ORPHAN_GC_BIN"

[env]
CARGO_ORPHAN_GC_INNER_WRAPPER = "sccache"
TOML
cat >> Cargo.toml <<'TOML'

[workspace.metadata.orphan-gc]
enabled = true
inner-wrapper = "sccache"
TOML
# Private CARGO_HOME so the state this leg records lands inside the mktemp,
# not in the operator's real ~/.cargo/orphan-gc.
export CARGO_HOME="$d/cargo-home"
rm -rf target; sccache --zero-stats >/dev/null
maybe_timeout cargo build -q 2>&1 | head -3
stats
"$ORPHAN_GC_BIN" orphan-gc status | grep -E "families|current artifacts|orphan artifacts"
