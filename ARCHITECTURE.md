# ARCHITECTURE — cargo-orphan-gc

## 1. Problem statement

Cargo/rustc can retain many build artifacts across rapid rebuilds and build shapes. The desired behavior for this POC is not `cargo clean` and not "disable incremental compilation." It is:

```text
incremental compilation: ON
useful current state:     RETAIN
superseded known state:   DELETE AUTOMATICALLY
unknown ownership:        RETAIN
```

The key design constraint is **orphan-only deletion**. A platform/target/profile family that has not been observed being replaced is allowed to remain forever. This sacrifices a hard global disk bound in exchange for a much stronger safety invariant.

## 2. Why `rustc-wrapper` (the outer slot)

Cargo exposes two wrapper slots and, when both are set, nests `rustc-wrapper`
*outside* `rustc-workspace-wrapper`. This tool takes the outer slot:

```toml
[build]
rustc-wrapper = "cargo-orphan-gc"
```

Cargo invokes the wrapper with the real rustc executable as argument 1
followed by the normal rustc arguments. This is a supported Cargo integration
point:

<https://doc.rust-lang.org/cargo/reference/config.html#buildrustc-wrapper>

The tool originally sat in `rustc-workspace-wrapper` — a natural fit, since
only workspace members are of interest — and that choice was measured to be
wrong (2026-08-11): under the nesting, a caching wrapper such as sccache in
the outer slot is handed the *inner wrapper binary*, not rustc, as its first
argument. It cannot recognise it as a compiler, falls off its rust-aware path,
and silently stops caching every workspace crate (`multiple input files` in
its stats; nothing errors). See §12 for the measurement and the chain design
that replaced it.

The cost of the outer slot is coverage: `rustc-wrapper` sees *every* unit,
registry and git dependencies included. The wrapper therefore identifies
non-workspace units cheaply — a unit is tracked only when its crate root is a
`.rs` file inside the workspace — and passes everything else straight through
to the chained inner wrapper with no locks, leases, or state I/O.

This boundary gives the POC the information it needs at exactly the useful moment:

- crate name;
- crate root;
- `--out-dir`;
- `--emit`;
- `--extern` concrete inputs;
- cfg/features;
- target and codegen flags;
- Cargo/rustc's `-C metadata`, `-C extra-filename`, and `-C incremental` values.

It also lets the tool make generation advancement conditional on **rustc success**.

### Why not `build.rs`?

A package build script is a real pre-package-build hook, but it is not a workspace-wide stop-the-world GC callback. Cargo may run unrelated compilation work in parallel. More importantly, Cargo's build-script contract says scripts should not modify files outside their `OUT_DIR`.

This tool therefore does not use build scripts to delete Cargo-owned files.

Reference: <https://doc.rust-lang.org/cargo/reference/build-scripts.html>

## 3. Components

```text
                         normal developer command
                               cargo build
                                    |
                                    v
                        Cargo workspace scheduler
                                    |
                   workspace-member rustc invocation
                                    |
                                    v
                         +--------------------+
                         | cargo-orphan-gc    |
                         | wrapper mode       |
                         +--------------------+
                           |       |       |
              family lock |       |       | active-input lease
                           |       |       |
                           |       v       |
                           |    real rustc  |
                           |       |       |
                           |    success?    |
                           |       |       |
                           |       v       |
                           | generation commit
                           |       |
                           |       v
                           | orphan sweep
                           v
                $CARGO_HOME/orphan-gc/
                    workspaces/<hash>/
                      families/
                      locks/
                      leases/
                      pending/
```

The same binary has CLI mode when invoked by Cargo as a custom subcommand:

```text
cargo orphan-gc bootstrap
cargo orphan-gc status
cargo orphan-gc sweep
```

Cargo's custom-subcommand calling convention is documented here:
<https://doc.rust-lang.org/cargo/reference/external-tools.html#custom-subcommands>

## 4. Safety invariants

These are more important than reclamation rate.

### Invariant A — no successful replacement, no retirement

A previous generation becomes orphaned only after real rustc returns success for a newer generation of the same family.

Compiler error:

```text
G17 current
   |
compile G18
   |
FAIL
   |
G17 remains current
```

### Invariant B — delete only learned ownership

The POC never decides that "everything old-looking in target/" is garbage.

An artifact is deletion-eligible only if it was written into a persisted `Generation.artifacts` record created after a successful wrapped rustc invocation.

### Invariant C — family identity is conservative

The family key retains `--out-dir`. Moving to another target/profile/platform layout therefore creates another family rather than allowing one family to delete the other.

This intentionally leaks across build-grid changes.

### Invariant D — current paths dominate orphan paths

If G17 and G18 use the same pathname, that pathname belongs to G18 after successful compilation and is removed from G17's orphan set without deletion.

```text
G17: target/debug/deps/libfoo-abc.rmeta
G18: target/debug/deps/libfoo-abc.rmeta

=> reused path; KEEP
```

### Invariant E — active inputs are leased

Every wrapped rustc publishes the concrete filesystem paths from `--extern` and holds a file lock on that lease until the compiler exits.

An orphan artifact matching an active input path is deferred.

An unreadable active lease causes deletion to fail closed.

### Invariant G — current paths dominate across families, not just within one

Invariant D is stated per family, which is sufficient while orphans only ever arise from a family superseding *itself*. `budget-mode = "lru-current-families"` (§10.1) breaks that assumption: it retires one family's current generation while a different family may still hold some of the same paths current.

Cargo emits unhashed outputs — the final binary, its `.d` — that several families legitimately share:

```text
family "default":        target/debug/deps/app   <- shared
family "--features x":   target/debug/deps/app   <- same path
```

Retiring the cold family must not delete that path. The budget sweep therefore passes every *other* family's current path set as protected, and deletion skips them exactly as it skips the retiring family's own reused paths.

Without this, retiring a cold family silently damages a surviving one, and the damage is invisible in the sweep report because the surviving family's state still lists artifacts that are no longer on disk.

### Invariant F — unsafe path validation fails closed

Out-dir entries must be direct children of their recorded out-dir.
Explicit emit files must remain under the recorded out-dir.
The incremental path must exactly equal its recorded ownership root.

If those checks fail, deletion returns an error and the orphan stays queued.

## 5. Logical build family

A naive family key containing every rustc argument would leak every time Cargo changes a hash-bearing artifact path. A dangerously broad family key could cross-delete unrelated configurations.

The POC hashes a **normalized rustc invocation**.

### Retained identity dimensions

Examples:

```text
crate root
crate name
--out-dir
--target
--crate-type
--edition
--cfg / features
-C opt-level
-C debuginfo
-C panic
-C target-cpu / target-feature
lint/check-cfg settings
logical --extern name/modifiers
-C metadata / -C extra-filename (cargo's unit hash)
RUSTUP_TOOLCHAIN when present
Cargo package name/version when present
real rustc executable string
```

The unit hash is retained **deliberately, since R748-B9**. It was originally
stripped, on the reasoning that a rebuilt version should map back to the same
logical family — which is sound only if at most one hash of a crate is live at a
time, and on a real workspace it is not. Cargo compiles one crate several ways
inside a single invocation, and two such units can have argv that differs *only*
in the unit hash and in `--extern` filenames this tool normalizes away (below).
Under the old key they were one family, so the second compile superseded the
first and deleted `.rmeta`/`.rlib` that units still queued in that same build
link against; cargo then failed with `extern location for <crate> does not
exist`.

The reproducing shape, if you ever need it again — it is
`tests/live_cargo.rs::two_units_of_one_crate_differing_only_in_hash_do_not_reclaim_each_other`:
put the divergence one level *below* the crate under watch (resolver 2 does not
unify features between the build graph and the normal graph, so a dependency
built two ways duplicates its dependents without changing *their* argv), and set
`full-scan-every = 1`. At the default 16 the second compile reuses the first
unit's recorded path list (§11.3) and never supersedes, which is why the camp
symptom was intermittent and named a different crate each run.

Hash-exact families mean supersession fires only when cargo genuinely rebuilds
the same unit. A differently-configured build forks a family and leaks instead
(Invariant C) — the fallback this design takes whenever liveness is ambiguous.
The reclamation that actually pays is unaffected: surplus-session collection
keys on the incremental dir (§6.3), and budget mode retires cold families by
LRU, which is the safe way to reap a stale hash.

### Stripped generation dimensions

```text
-C incremental=<hash-bearing path>
```

For:

```text
--extern foo=/target/debug/deps/libfoo-AAA.rlib
```

the family identity stores approximately:

```text
--extern foo=<artifact>
```

while the concrete `/target/.../libfoo-AAA.rlib` path is separately recorded in the active-input lease.

That means rebuilding a dependency does not automatically create a forever-new downstream family solely because its `--extern` filename changed.

## 6. Artifact ownership discovery

After successful rustc execution, `artifacts::collect` records three categories.

### 6.1 Out-dir entries

Given:

```text
--crate-name foo
--out-dir /w/target/debug/deps
-C extra-filename=-abc123
```

it records direct entries matching shapes such as:

```text
foo-abc123
foo-abc123.d
foo-abc123.pdb
libfoo-abc123.rmeta
libfoo-abc123.rlib
libfoo-abc123.so
```

and matching directories such as a platform-specific debug-info bundle if its top-level name follows the same stem.

It does **not** recursively glob the whole out-dir.

### 6.2 Explicit emit outputs

For an argument such as:

```text
--emit=metadata=/w/target/debug/deps/foo.rmeta
```

the exact path can be learned, but only if it is under `--out-dir`.

### 6.3 Incremental directory

The exact path from `-C incremental=` is recorded as one directory-owned
artifact. Measured 2026-08-11: cargo passes the **profile-wide shared root**,
not a per-crate directory —

```text
-C incremental=/w/target/debug/incremental
```

— and rustc itself creates a per-configuration key dir inside it
(`foo-06nsz2nsvkzno/`) holding session dirs (`s-<ts>-<rand>/`), their
`.lock` files, and at most one `s-…-working` dir per running compile. Every
crate in the profile shares that root, so several families legitimately
record the same incremental path; Invariants D and G are what keep one
family's retirement from deleting it while another still holds it current.

The artifact record also stores the crate name as a `session_prefix`, which
scopes §6.4's session collection to key dirs attributable to that crate.

### 6.4 Surplus sessions inside the incremental directory

Generation tracking alone reclaims nothing from the dominant daily case: a
source edit leaves the family's artifact *path set* unchanged, so §7
refreshes the generation rather than orphaning anything — while the tree
grows *inside* the recorded directory. rustc keeps one live session per key
and deletes the prior one when finalizing a new one, but that GC needs the
key's directory lock and silently skips under concurrent builds. On real
shared trees the leftovers are 0–41% of the incremental tree by bytes.

The sweep therefore collects **surplus finalized sessions**: inside each key
dir attributable to the family's crate (`<session_prefix>-*` under the
recorded root, since rustc crate names cannot contain `-`), every finalized
session except the newest is deleted. The deletion authority is rustc's own
session lifecycle rather than learned generation history — a surplus
finalized session is one rustc already intended to delete — so it costs
nothing to reclaim and survives state loss (§16).

Hard rules, in order of what they protect:

- never touch `s-<ts>-<rand>-working` — a rustc is mid-compile in it;
- never touch the `.lock` files beside sessions;
- never delete individual files inside a session — a session is a dep-graph
  plus a query cache that reference each other, and partial deletion corrupts
  rather than shrinks;
- always keep the newest finalized session — it seeds the next compile.

Concurrency: a key dir is written only by rustc invocations of one
configuration, i.e. one family, and wrapped compiles of one family are
serialized by the family lock (§8.1) — collection runs under that same lock.
`cargo orphan-gc sweep` takes the family lock too. Keep-newest is therefore
safe against every cooperating process; non-cooperating processes are outside
the protocol exactly as in §14.

## 7. Generation commit protocol

State per family resembles:

```json
{
  "family_key": "...",
  "current": {
    "id": "hash-of-owned-path-set",
    "artifacts": ["..."]
  },
  "orphans": []
}
```

On successful compile:

```text
1. collect newly owned paths
2. lock family state
3. previous current -> orphan queue
4. new generation -> current
5. persist state
6. mark family pending
7. sweep orphan queue
```

If path sets are unchanged, the generation is refreshed rather than creating a fake orphan generation.

State publication uses a temporary file followed by rename. The metadata database is intentionally disposable. A crash can cause lost bookkeeping and therefore a leak; it must not manufacture deletion authority.

## 8. Locking and concurrency

### 8.1 Family lock

A lock file exists per normalized family key.

The wrapper holds that lock across:

```text
real rustc execution
+ generation commit
+ immediate family sweep
```

Therefore two cooperating Cargo processes cannot simultaneously replace the exact same logical family.

Different family keys remain fully parallel, so Cargo still compiles unrelated crates concurrently.

### 8.2 Input leases

A wrapper creates:

```text
$CARGO_HOME/orphan-gc/workspaces/<workspace>/leases/<pid>-<time>.json
```

containing its concrete `--extern` inputs and holds an exclusive advisory lock on the lease file while rustc executes.

GC checks every lease:

```text
can acquire lease lock?
    yes -> stale lease; remove it
    no  -> active lease; protect its input paths
```

If an active lease cannot be decoded, all candidate deletion is deferred for that sweep.

This protects cooperating simultaneous Cargo processes better than "just rm the old hash immediately."

## 8.3 Cargo build-cache locking assumption

Current Cargo build/artifact layouts include `.cargo-lock` files whose documented purpose is to prevent multiple Cargo processes from using the same profile build cache at the same time. The POC benefits from that normal Cargo invariant in addition to its own family locks and rustc input leases.

Cargo also has an experimental `-Zfine-grain-locking` mode that changes the locking model. This POC does **not** claim deletion correctness under that experimental mode yet; production support would need to model Cargo build sessions explicitly or integrate with the finer-grained Cargo lock units.

References:

- <https://doc.rust-lang.org/beta/nightly-rustc/cargo/core/compiler/layout/index.html>
- <https://doc.rust-lang.org/cargo/reference/unstable.html#fine-grain-locking>

## 9. Pending retry queue

An artifact may be a valid orphan but temporarily in use by another compiler process.

Deleting it immediately is therefore not required for correctness.

When a family still has orphans, an empty marker is left in:

```text
pending/<family-key>
```

After each successful compilation the wrapper retries a small configured number of pending families:

```toml
pending-sweeps-per-compile = 4
```

This turns GC into an automatic amortized activity without scanning tens of thousands of family metadata files on every rustc invocation.

`cargo orphan-gc sweep` retries all families manually.

## 10. Why there is no hard disk cap in the default POC

The requested safety rule is:

> only throw out stuff that has been orphaned

Suppose current families are:

```text
Linux debug       80 GiB
Linux release     70 GiB
Windows debug     75 GiB
macOS debug       65 GiB
```

and none has been superseded within its own family.

A `max-bytes = 200 GiB` hard cap cannot be enforced without deleting at least one **current** family. That contradicts orphan-only deletion.

Therefore `max-bytes` is a warning watermark **by default**, under the default `budget-mode = "orphan-only"`.

### 10.1 `budget-mode = "lru-current-families"` (implemented)

The second policy is opt-in and makes `max-bytes` a real ceiling:

```toml
[workspace.metadata.orphan-gc]
enabled = true
max-bytes = 4294967296
budget-mode = "lru-current-families"
```

When the total size of all current generations exceeds `max-bytes`, the least recently used families are retired oldest-first until it fits. `FamilyState.last_used_unix_ms` is the LRU key.

**This trades Invariant A for a hard bound, and only under that explicit key.** A retired family was never superseded, so rebuilding it costs a cold compile. LRU is what keeps that cost off the hot path: a family in active use is touched by every build that needs it, so it is never the coldest. Note also what LRU sidesteps — it never has to decide whether a family is still *reachable*, a question the one-way family hash makes unanswerable from outside. "When was this last used?" is answerable; "will this be used again?" is not.

Two properties keep the blast radius contained:

- **Retirement is not a second deletion path.** Retiring moves the current generation into the same orphan queue a supersession would have, and deletion still goes through `gc::sweep_locked` — active-input leases, current-path domination, and the Invariant F path validation all apply unchanged. Budget mode adds an *authority to orphan*, nothing else. An active lease defers a budget retirement exactly as it defers a supersession.
- **Measurement is re-validated under the family lock.** A family touched by a build between the sizing pass and the lock is skipped and counted as `raced`, so the tool never retires the family that is about to be needed.

`budget_sweep` is deliberately **not** called from the wrapper. Costing the budget means sizing every artifact of every family, which is far too expensive per rustc invocation — §9's amortized pending sweeps exist precisely to keep the hot path cheap. Budget enforcement belongs on `cargo orphan-gc sweep` or a timer.

A ceiling below the hot working set will thrash. `cargo orphan-gc status` reports the overage so that is visible rather than inferred.

One consequence of §6.3's measurement deserves stating: the incremental artifact every family records is the profile-wide shared root, so under budget mode that directory is deleted only when the *last* family holding it current is retired — i.e. when the entire profile has gone cold. Retiring a whole cold profile's incremental tree is the intended reading of the bound; a profile with any recent build keeps its root through Invariant G's protection.

## 11. Bootstrap and opt-in model

The policy lives in Cargo.toml:

```toml
[workspace.metadata.orphan-gc]
enabled = true
dry-run = true              # shadow mode — the install default (§11.1)
pending-sweeps-per-compile = 4
# written automatically when bootstrap adopts an existing rustc-wrapper:
inner-wrapper = "sccache"
```

The execution hook lives where Cargo defines compiler wrappers, `.cargo/config.toml`:

```toml
[build]
rustc-wrapper = "cargo-orphan-gc"

[env]
CARGO_ORPHAN_GC_INNER_WRAPPER = "sccache"   # written when inner-wrapper is set
```

The `[env]` entry is the inner wrapper's *transport*, and it exists because of
a measured cargo behaviour: registry/git dependency units run with both
`CARGO_MANIFEST_DIR` and the working directory inside the registry checkout,
so no ancestor walk from either can reach the workspace metadata — the
wrapper cannot *discover* `inner-wrapper` for exactly the units the chained
cache serves best. Cargo's `[env]` table applies to every rustc it spawns, so
the wrapper falls back to that variable when discovery fails. (Workspace
metadata remains the source of truth; it wins when both are visible.)

`cargo orphan-gc bootstrap` writes all of it, and negotiates the wrapper slots
rather than assuming them:

- an existing `build.rustc-wrapper` (e.g. sccache) is **adopted**: bootstrap
  moves it to `inner-wrapper` and takes the outer slot, so the cache keeps
  seeing rustc as its argv[1] (§12);
- an existing `build.rustc-workspace-wrapper` set by an earlier version of
  this tool is migrated to the outer slot;
- an existing `build.rustc-workspace-wrapper` set by **anything else** is a
  hard refusal — cargo would nest this tool outside it and hand it that
  wrapper as the compiler, corrupting family identity;
- a set `RUSTC_WRAPPER` environment variable draws a warning: it takes
  precedence over `.cargo/config.toml`, so the installed hook would silently
  never run.

The split is unavoidable without changing Cargo itself: arbitrary workspace metadata does not execute a program, while `rustc-wrapper` is the supported execution hook.

Turning `enabled = false` makes the already-installed wrapper transparent
(units still flow through `inner-wrapper` when one is configured, so
disabling the GC never disables the cache).

## 11.1 Shadow mode — the install default

`dry-run = true` is the default, and bootstrap writes it explicitly. A fresh
install therefore **learns and reports, and deletes nothing**: ownership is
discovered, generations are recorded, supersessions are queued as orphans, and
`cargo orphan-gc status` reports what a real sweep would reclaim. Authorizing
deletion is a second, deliberate edit: `dry-run = false`.

There are now three rungs, and the middle one is the point:

| setting | wrapper runs | learns | deletes |
|---|---|---|---|
| `enabled = false` | passthrough only | no | no |
| `enabled = true`, `dry-run = true` (default) | yes | yes | **no** |
| `enabled = true`, `dry-run = false` | yes | yes | yes |

Without the middle rung, adopting this tool on a shared tree means authorizing
deletion before any evidence exists that it is safe *there*. R748-B9 is the
concrete argument: a family-identity bug deleted a live `.rmeta` mid-invocation
and broke every agent's build on a ten-agent camp. Shadow mode surfaces exactly
that class with zero blast radius, because the report is generated by the real
sweep's own decision path.

Three properties make the report worth trusting:

- **Same gate.** A shadow sweep calls `artifacts::check_deletable` — the
  function `remove` itself calls — so it can never promise reclamation the real
  sweep would refuse. The surplus-session term likewise reuses
  `finalized_sessions`, so the two modes cannot disagree about which sessions
  are surplus.
- **Pure read.** A shadow sweep writes no state: it does not clear `pending`,
  does not drop anchors, does not advance the orphan queue. Two consecutive
  shadow sweeps report the same thing, and the queue an operator read about is
  the queue that gets reclaimed when they flip the flag. It also takes no family
  lock, so `status` never blocks behind a live compile.
- **All three deletion paths.** Orphan supersession (§7), surplus-session
  collection (§6.4) and budget retirement (§10.1) are separate paths; a flag
  that reached only one would lie. Budget retirement in shadow reports *which*
  families the ceiling would cost — exactly — and treats their bytes as fully
  reclaimable, which is an optimistic bound: a real run may defer some behind an
  active lease or a path another family still holds current.

## 11.2 Operational log — never the compiler's stderr

The wrapper's operational output goes to `<state-dir>/log`, appended with an
ISO-8601 UTC timestamp and the pid, and is echoed to stderr only when stderr is
a terminal. `cargo orphan-gc log` tails it.

This is not a style choice. Cargo captures each rustc invocation's stderr into
`target/<profile>/.fingerprint/<unit>/output-*` and **replays it verbatim** on
every later build where that unit is fresh, so a line written to stderr from
inside the wrapper describes whichever build last *compiled* that unit rather
than the build being watched. Replay is per-unit, so stale lines interleave with
genuinely fresh output from neighbouring units — which defeats the natural
defence ("other things compiled, so this must be real"). On the camp where this
was found, 269 fingerprint files carried tool log lines, and the resulting
confusion produced three wrong conclusions in a single day, including a
"verified" end-to-end run that had compiled nothing (R748-B10).

Two consequences worth stating plainly:

- The log is a bounded file (1 MiB, one rotation to `log.1`). A GC tool that
  grows an unbounded file in `$CARGO_HOME` would be arguing against itself.
- Lines already captured in a tree's fingerprints do not disappear when this
  ships. They age out only as each unit next recompiles, so they will keep
  surfacing — and keep being misread — for as long as those units stay fresh.

## 11.3 Per-compile cost, and the walk that dominates it

Measured 2026-08-12 on a 10-agent camp (83 GB target dir, 210,770 entries in
`target/debug/deps`), per rustc invocation, against a no-op compiler so the
wrapper's own cost is the whole signal:

| out-dir entries | before | after |
|---|---|---|
| 1,000 | 7.1 ms | 5.8 ms |
| 50,000 | 33.5 ms | 5.6 ms |
| 200,000 | **145 ms** | 6.7 ms (14.1 ms amortized) |

The cost was `artifacts::collect` walking the out-dir — a directory shared by
the whole workspace, so the walk is linear in the size of everyone's
accumulated output, and a tree old enough to need a GC is exactly a tree where
that directory is enormous. Two findings from attacking it:

- **The loop's allocations were not the problem.** Removing them (a `String`
  per entry plus up to five `format!`s in the predicate, ~1M allocations per
  compile at this scale) bought only 145 → 117 ms.
- **The walk is at the filesystem's floor.** Rust's `fs::read_dir` costs
  551 ns/entry here — 3.5x *faster* than a C-level `scandir` (1934 ns/entry) on
  the same directory. There is no implementation left to improve; the walk can
  only be skipped.

So it is skipped: `collect` takes the previous generation's artifact list, and
if every one of those files is still present it reuses it (one `stat` each) and
does not walk. `full-scan-every` (default 16) forces a real walk periodically,
and a missing recorded path forces one immediately.

The safety argument is what makes this a performance knob rather than a risk:
reusing a stale set can only *under*-record ownership, and an unrecorded file is
one this tool will never delete (Invariant B). The failure mode is a leak — the
fallback the whole design already takes whenever ownership is uncertain — and it
is bounded by the next full scan. Set `full-scan-every = 0` to walk every time.

One consequence worth knowing when reasoning about *when* supersession fires:
while a family is on the reuse path it re-records the previous generation's
paths verbatim, so its generation never changes and nothing is ever orphaned.
Supersession is therefore due only on a full scan. That is harmless — it delays
reclamation, never deletion — but it made R748-B9 intermittent, and it is why
its regression test pins `full-scan-every = 1`.

What remains is ~6 ms of constant per-compile cost (state I/O, the family lock,
the lease, and the shadow sweep). Against a real rustc invocation rather than
this benchmark's no-op that is a low single-digit percentage, but it is a real
number and W306's contention objection is about exactly this, at scale, under
concurrent agent load.

## 12. Interaction with sccache and other wrappers

Cargo has two wrapper slots:

```text
rustc-wrapper
rustc-workspace-wrapper
```

If both are configured, Cargo nests them — `rustc-wrapper` outermost — so a workspace could in principle keep a global wrapper such as sccache in `rustc-wrapper` while this tool occupies `rustc-workspace-wrapper`. That was this tool's original design.

**Measured 2026-08-11: nesting under sccache does not work. It silently destroys sccache's hit rate.**

Wrapped under sccache, every workspace-member compile becomes non-cacheable with sccache's `multiple input files` reason — five wrapped compiles produced **8 non-cacheable calls, 0 hits, 0 misses**. The nesting changes the argv shape sccache is handed, and sccache declines argv it cannot reduce to a single translation unit. Nothing errors; the cache simply stops working for exactly the crates the tool is wrapping.

**It is worse than lost cache hits: the broken nesting can wedge the sccache server for every user of the machine.**

Cargo probes the toolchain by running `rustc -vV` through the wrapper chain, so sccache is handed a shell script where it expects a compiler. It does not error — it **hangs**, and the hung client never exits. Enough of them and the server stops serving: `sccache --show-stats` keeps answering instantly while every real compile request blocks forever, with no error anywhere to explain it. Observed in practice taking out three unrelated sessions' builds simultaneously, each sitting on ~15 sleeping sccache clients and 0.01s of CPU, plus an orphaned client still wedged 16 minutes after its workspace had been deleted.

Recovery is `sccache --stop-server && sccache --start-server` after reaping the hung clients (`pkill -f 'sccache .*<wrapper-path>'`). `scripts/wrapper-chain-ab.sh` demonstrates this failure and therefore runs against a **private** server on its own port and cache dir — inducing this pathology against a shared server is not safe.

This is not a tuning problem: the nesting order is cargo's, and no configuration of the two slots avoids it while sccache sits outside.

**The resolution (2026-08-11): invert the chain.** This tool now takes the outer `rustc-wrapper` slot itself and invokes the cache as a configured `inner-wrapper` (§11), so sccache receives rustc as its argv[1] with its argv shape byte-identical to an unwrapped build. A/B/C-verified in `scripts/wrapper-chain-ab.sh` against a private sccache server:

```text
A  sccache outer, tool inner      multiple input files 2   (the broken nesting)
B  shell chain outer              crate-type 1, incremental 1   (clean baseline)
C  cargo-orphan-gc outer,
   inner-wrapper = "sccache"      crate-type 1, incremental 1, families: 2
```

Leg C is the shipped arrangement: sccache's only remaining refusals are its own structural ones (`--crate-type bin`, `-C incremental`) — which are exactly the artifact classes this tool exists to govern, and why the two tools are complementary once the chain is the right way out.

Bootstrap enforces the chain rather than documenting it (§11): it adopts an existing `rustc-wrapper` as `inner-wrapper`, and refuses outright when a foreign `rustc-workspace-wrapper` would recreate a nesting, since the resulting cache collapse is invisible without reading sccache stats.

## 13. Build-script output is intentionally excluded

Cargo documents build-script `OUT_DIR` as persistent and under the build script's ownership. Generic deletion of files inside it cannot be inferred safely from a rustc invocation.

A future extension could require build scripts to publish an ownership manifest, for example:

```text
OUT_DIR/.cargo-orphan-gc-owned.json
```

Without such a protocol, this POC leaks build-script-generated state.

That is consistent with the global safety rule: unknown ownership leaks.

## 14. Failure modes

### Tool not installed / not on PATH

Cargo cannot start the configured wrapper, so the build fails immediately and visibly. Bootstrap documentation requires installing the binary first.

### Wrapper crashes before rustc

Build fails. No current generation is retired.

### rustc fails

Exit status propagates. No generation commit.

### Wrapper crashes after rustc succeeds but before commit

The new compiler outputs remain. Previous state remains current. Result: possible leak, no unsafe deletion.

### Wrapper crashes after commit but during deletion

Already deleted orphan files stay deleted. Remaining orphan records are retried later. Current generation remains protected.

### State metadata is deleted/corrupted

The tool loses historical ownership. Safe response is to stop deleting unknown old files and relearn future generations. A production build should quarantine corrupt family state instead of returning an error for the entire build; this POC still reports the corruption loudly. §16 works through why the lost state cannot be reconstructed from the filesystem, and what the loss actually costs.

### Non-cooperating process consumes old artifacts

The lease mechanism only sees compiler processes launched through this wrapper. A completely unrelated process opening files in `target/` is outside the ownership protocol. Unix open-file semantics reduce some races, but this must not be treated as a general filesystem reference counter.

## 15. What would change upstream in Cargo

An upstream implementation could be both simpler and more complete because Cargo already knows build-unit identities and produced artifact filenames.

Instead of inferring:

```text
normalized rustc args -> family
filename stem -> ownership
```

Cargo could directly persist:

```text
BuildUnitId
GenerationId
ProducedArtifact[]
DependencyArtifact[]
LastUsed
```

and perform GC at scheduler-safe boundaries.

The wrapper POC is intended to validate the **lifetime policy**:

```text
successful replacement => previous generation orphaned
orphan + no active references => delete
otherwise => retain
```

before requiring an upstream Cargo patch.

## 16. Reconcile after state loss — investigated, rejected

The metadata under `$CARGO_HOME/orphan-gc/` is disposable by design (§7), and
§14 accepts that losing it leaks: the tool stops recognising anything it
learned and relearns going forward. The state dir sits outside the tree and is
trivially lost — a `CARGO_HOME` change, a machine move, a cleanup script — so
the question was whether ownership can be *re-derived* from the tree instead.

The spike's conclusion: **no, and the leak is much smaller than it looks.**

### Why re-derivation cannot be made safe

What reconcile would need to authorize is deleting entries that exist on disk
but are referenced by nothing current — say, `deps/libfoo-OLD.rlib` where no
`.fingerprint/` unit names `OLD`. But "no current reference ⇒ garbage" is
exactly the *everything old-looking in target/ is garbage* heuristic Invariant
B exists to forbid, and the reasons it is forbidden are structural, not
stylistic:

- the artifact-name derivation is one-way (rustc's StableCrateId); cargo
  stores no inverse, so absence of a matching fingerprint is absence of
  evidence, not evidence of orphanhood;
- a shared target dir (`CARGO_TARGET_DIR`, several checkouts, several
  toolchains, rust-analyzer's own layout) legitimately contains entries no
  fingerprint in *this* workspace's view references;
- cargo's naming conventions move across versions, and a reconciler keyed to
  them fails open — toward deletion — on the version after the one it was
  written against.

The safe half of reconcile — rebuilding *current* ownership, which cargo's
layout does describe well enough — authorizes deleting nothing by itself, and
the wrapper relearns exactly that information on the next compile of each
family anyway. Reconcile would therefore buy one generation of delta per
family at the price of a second, heuristic deletion authority standing next to
the watched-supersession one. Do not build it, and do not weaken Invariant B
to make it buildable.

### What state loss actually costs

- **Families whose configuration still builds: usually nothing.** The first
  post-loss compile relearns the family with the same artifact path set the
  lost record held; from then on supersession works as before. Bytes leak only
  where the path set changed across the loss (the old paths are never
  orphaned, because no record ties them to the family).
- **The dominant byte term needs no history at all.** Surplus-session
  collection (§6.4) derives its authority from rustc's session lifecycle, not
  from learned generations, so the first post-loss build of each family
  reclaims it in full.
- **What leaks permanently: families that are never rebuilt.** A grid
  configuration abandoned before the loss has no record and will never be
  watched again. Budget mode cannot see it either (nothing to retire). That
  residue is the honest cost of orphan-only safety, and the recovery for it is
  the user's own authority, exercised rarely and explicitly:
  `cargo clean` (or deleting the stale profile dir by hand).

Recovery procedure, stated plainly so it is a documented path rather than an
implicit shrug: delete whatever remains of the state dir, rebuild, accept that
configurations you never rebuild keep their bytes until a `cargo clean`.
