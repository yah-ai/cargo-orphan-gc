# cargo-orphan-gc

**Cargo has no garbage collector for your target directory — and the compiler
caches can't be one.**

Tools like sccache serve registry dependencies well, but they structurally
refuse the two artifact classes that dominate a working tree: binaries
(`--crate-type bin`) and anything compiled with `-C incremental` — i.e. **the
crates you actually work on**, which are also the largest objects in the tree.
`cargo sweep --maxsize` never walks `incremental/` at all. So the code you
edit all day is written to the one part of the target dir that nothing bounds,
while being denied entry to every cache built to bound things. On a busy
shared tree that gap compounds into hundreds of gigabytes a week.

`cargo-orphan-gc` is a conservative, opt-in GC for exactly that gap. It wraps
rustc, watches build generations *supersede* each other, and deletes only what
it watched being replaced:

- keep Rust incremental compilation enabled;
- when the **same logical build family** compiles again successfully, the
  previously recorded generation becomes garbage — and only then;
- collect the surplus incremental-compilation sessions rustc itself intended
  to delete but couldn't (its session GC silently skips under concurrent
  builds — on real shared trees the leftovers run 0–41% of the incremental
  tree, and deleting them costs zero rebuild time);
- never delete an artifact another cooperating rustc is currently reading;
- if ownership is ambiguous, **leak instead of delete**.

It composes with sccache by design: the tool owns Cargo's outer
`rustc-wrapper` slot and invokes the cache itself (`inner-wrapper`), so the
cache still sees rustc as its argv[1]. (The naive arrangement — nesting under
sccache via `rustc-workspace-wrapper` — silently disables sccache for every
wrapped crate. Measured, with an A/B script in this repo. Bootstrap refuses to
create it.)

This is a working proof of concept extracted from a monorepo where ten agents
build ~60 workspace crates against one target dir all day. Read
[`ARCHITECTURE.md`](ARCHITECTURE.md) before deploying it broadly.

## Setup

### 1. Install

```bash
cargo install --path .
```

`cargo-orphan-gc` must be discoverable on `PATH` (or reference it by absolute
path in `.cargo/config.toml`).

### 2. Bootstrap from the workspace root

```bash
cd /path/to/your/workspace
cargo orphan-gc bootstrap
```

That makes two small, reviewable changes. The policy, in your root
`Cargo.toml`:

```toml
[workspace.metadata.orphan-gc]
enabled = true
pending-sweeps-per-compile = 4
```

and the execution hook, in `.cargo/config.toml`:

```toml
[build]
rustc-wrapper = "cargo-orphan-gc"
```

If `rustc-wrapper` was already set (sccache, say), bootstrap **adopts** it:

```toml
[workspace.metadata.orphan-gc]
inner-wrapper = "sccache"   # invoked around rustc, argv shape preserved
```

plus an `[env] CARGO_ORPHAN_GC_INNER_WRAPPER` entry in `.cargo/config.toml` —
the transport that carries the inner wrapper to registry-dependency compiles,
which run from inside the registry checkout where the workspace metadata is
undiscoverable.

Bootstrap refuses to install alongside a foreign `rustc-workspace-wrapper`
(cargo would nest the two in the order that breaks caching), and warns when a
`RUSTC_WRAPPER` environment variable would shadow the config entry.

Note: installing a compiler wrapper changes cargo's fingerprint hash, so the
first build after bootstrap is a full rebuild — once.

### 3. Build normally

```bash
cargo build   # or check / test / run — no new commands to remember
```

For every workspace unit, cargo runs approximately:

```text
cargo → cargo-orphan-gc [inner-wrapper] <real-rustc> <args…>
          └ on SUCCESS only: commit generation, sweep superseded artifacts,
            collect surplus incremental sessions
```

Registry and git dependencies pass straight through to the inner wrapper with
no bookkeeping. If rustc fails, nothing is retired.

### 4. Inspect and sweep

```bash
cargo orphan-gc status   # families, live/orphan bytes, watermark or ceiling
cargo orphan-gc sweep    # retry deferred deletions; enforce budget mode
```

Sweeping also happens automatically after successful compiles
(`pending-sweeps-per-compile` families per compile), so the manual command is
for diagnostics, timers, and budget enforcement — not normal operation.

## Configuration

```toml
[workspace.metadata.orphan-gc]
enabled = true                      # false = wrapper is transparent (cache still runs)
verbose = false                     # print per-compile GC summaries to stderr
pending-sweeps-per-compile = 4
inner-wrapper = "sccache"           # optional chained compiler cache
# max-bytes = 214748364800
# budget-mode = "lru-current-families"
```

### `max-bytes` is honest about what it can promise

In the default mode (`budget-mode = "orphan-only"`), `max-bytes` is a
**warning watermark, not a cap** — and that is a theorem, not a limitation to
be fixed: if your current, never-superseded families alone exceed the number,
no orphan-only policy can get under it without deleting something *current*.

Opting into `budget-mode = "lru-current-families"` makes it a real ceiling:
when current generations exceed `max-bytes`, the least-recently-used families
are retired coldest-first until the tree fits. **That knowingly trades
Invariant A for a bound** — a retired family was never superseded, so touching
it again costs one cold compile. LRU keeps that off your hot path (a family in
use is never the coldest), retirement routes through the same safety-checked
deletion path as everything else, and the hottest family is never retired even
under an unsatisfiable ceiling.

## Safety model, in one table

| rule | consequence |
|---|---|
| A — no successful replacement, no retirement | a failed compile never costs you artifacts |
| B — delete only learned ownership | pre-existing / foreign files are never touched; unknown ownership leaks |
| C — conservative family identity | profile/target layout changes fork a new family and leak rather than cross-delete |
| D — current paths dominate orphans | a path the new generation reuses is never deleted |
| E — active inputs are leased | an artifact a running rustc is reading is deferred, and unreadable leases fail closed |
| F — path validation fails closed | out-of-root deletion candidates are refused and stay queued |
| G — current paths dominate across families | budget retirement can't delete what a surviving family still owns |

Every invariant has a test named after it (`cargo test invariant_`).

The corresponding costs, stated plainly: build-script `OUT_DIR` contents are
not governed (cargo gives scripts ownership of them); a family you stop
building keeps its bytes until budget mode or `cargo clean` takes them; losing
the state dir (`$CARGO_HOME/orphan-gc/`) means relearning — surplus-session
collection needs no history and resumes at full strength immediately, while
never-rebuilt families leak until a `cargo clean`
([`ARCHITECTURE.md` §16](ARCHITECTURE.md)).

## Verifying it on your machine

```bash
cargo test                      # invariants A–G + an end-to-end through real cargo
./scripts/wrapper-chain-ab.sh   # the sccache composition A/B/C, against a
                                # private sccache server
```

## Uninstall

Set `enabled = false` to make the wrapper transparent, or remove the
`rustc-wrapper` line and the metadata table entirely (another one-time
fingerprint flip). The state under `$CARGO_HOME/orphan-gc/` can be deleted at
any time.

## Known limitations

- Ownership is learned at the rustc boundary from current cargo/rustc naming
  conventions; an upstream Cargo implementation would have direct artifact
  identity instead of inferring it (see `ARCHITECTURE.md` §15).
- Only artifacts observed after installation are governed.
- Windows is untested (advisory `fs2` locking; no CI matrix yet).
- Cargo's experimental `-Zfine-grain-locking` changes the locking model this
  tool's correctness argument leans on; not yet supported.
