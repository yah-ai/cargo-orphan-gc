#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "$0")/.." && pwd)"
tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT

export CARGO_HOME="$tmp/cargo-home"
mkdir -p "$CARGO_HOME"

cargo install --path "$repo_root" --root "$CARGO_HOME"
export PATH="$CARGO_HOME/bin:$PATH"

mkdir -p "$tmp/ws/app/src"
cat > "$tmp/ws/Cargo.toml" <<'TOML'
[workspace]
members = ["app"]
resolver = "2"
TOML
cat > "$tmp/ws/app/Cargo.toml" <<'TOML'
[package]
name = "app"
version = "0.1.0"
edition = "2021"
TOML
cat > "$tmp/ws/app/src/main.rs" <<'RS'
fn main() {
    println!("generation one");
}
RS

cd "$tmp/ws"
cargo orphan-gc bootstrap
cargo build
cargo orphan-gc status

cat > app/src/main.rs <<'RS'
fn helper() -> &'static str {
    "generation two"
}

fn main() {
    println!("{}", helper());
}
RS

cargo build
cargo orphan-gc status
cargo orphan-gc sweep

echo "smoke test completed"
