# lightyear 0.28: an inverted native-input tick range allocates until client OOM

**Target:** github.com/cBournhonesque/lightyear · crate: lightyear_inputs_native 0.28
**Severity for us:** CRITICAL before containment — one client process allocates until the host or cgroup kills it · **Status:** unfiled
**Lineage:** distinct from OPEN issue [#1559](https://github.com/cBournhonesque/lightyear/issues/1559) and server-side input lookahead fix [#1525](https://github.com/cBournhonesque/lightyear/pull/1525).
**Our commits:** [`40536fd`](https://github.com/vikng-dev/overmatch/commit/40536fd089d26e4f3e16fa495420e83ea11f74d7) contains the Docker/cgroup boundary; [`86cbdc6`](https://github.com/vikng-dev/overmatch/commit/86cbdc61af0788a4b15e6073e4d8a86afda9f111) gives the fan-out harness shipping input-delay parity and installs the shared pre-encoder guard.

## Suggested title

Native input encoder casts an inverted tick range to `usize` and allocates until OOM

## Verdict

**Yes: the unbounded client allocation is an upstream Lightyear defect.** It is
in `lightyear_inputs_native`'s native `ActionStateSequence` encoder, not in
Overmatch. A downstream timeline rebase is one way to make its invalid input
state reachable, but the encoder owns the missing range validation and the
unbounded allocation. **MEASURED (source inspection, 2026-07-14):** the pinned
`0.28.0` source has the defect, and upstream `main` at `e0bcaf7` has
byte-for-byte equivalent logic. [Pinned tag source](https://github.com/cBournhonesque/lightyear/blob/0.28.0/crates/inputs/inputs_native/src/input_message.rs#L72-L98), [current-main source at `e0bcaf7`](https://github.com/cBournhonesque/lightyear/blob/e0bcaf7901f8b194d5cfadcd155ab59d11c4e08a/crates/inputs/inputs_native/src/input_message.rs#L72-L98), [tag-to-main comparison](https://github.com/cBournhonesque/lightyear/compare/28e823d9df394c193dfc09f8eb891b77424e81c5...e0bcaf7901f8b194d5cfadcd155ab59d11c4e08a).

The appropriate downstream workaround remains the existing pre-encoder guard:
clear or otherwise reject a local `NativeBuffer` whose `start_tick` is later
than the delayed send tick, scheduled before `InputSystems::PrepareInputMessage`.
That prevents the invalid range at the one place this application can cheaply
and safely recover. It is a containment measure, not a substitute for an
upstream fix.

## What the upstream code does

`NativeStateSequence::build_from_input_buffer` first chooses
`start_tick = max(end_tick - num_ticks + 1, buffer_start_tick)`, allocates one
initial state, then computes both indices by subtracting `buffer_start_tick` and
casting the signed result to `usize`. It pushes one element per index in
`buffer_start..buffer_end`; it has no check that `buffer_start <= buffer_end`.
[Pinned tag source](https://github.com/cBournhonesque/lightyear/blob/0.28.0/crates/inputs/inputs_native/src/input_message.rs#L72-L98).

**Encoder-range invariant:** `buffer_start_tick <= end_tick + 1` must hold
before either signed difference is converted to `usize`. The encoder neither
states nor enforces this invariant.

When `buffer_start_tick > end_tick + 1`, `max` selects `buffer_start_tick`, so
`buffer_start` is `1`, while `buffer_end = (end_tick + 1 -
buffer_start_tick) as usize` converts a negative `i32` to a huge unsigned
index. The loop then repeatedly pushes `Compressed::Absent` after the real
`VecDeque` is exhausted, until allocation failure or the process is killed.

For the observed tripwire values (`start = 313`, `end = 20`), the signed end
offset is `-292`; on a 64-bit target its cast is
`18,446,744,073,709,551,324`, and the loop has
`18,446,744,073,709,551,323` iterations. **DERIVED:** those values follow
directly from the source expressions and Rust's integer cast semantics; this
note did not execute the allocating call. On a 32-bit target the corresponding
unsigned range is still enormous. **DERIVED:** it is the same modulo-width
cast, with a 32-bit `usize`.

The arithmetic is genuinely signed: Lightyear's `wrapping_id!` implementation
makes `Tick - Tick` return `i32`, calculated from the two `u32` tick values.
[Tick declaration](https://github.com/cBournhonesque/lightyear/blob/0.28.0/crates/core/core/src/tick.rs#L1-L11), [subtraction implementation](https://github.com/cBournhonesque/lightyear/blob/0.28.0/crates/core/utils/src/wrapping_id.rs#L15-L102).

## Ownership boundary

| Concern | Owner | Evidence |
| --- | --- | --- |
| Producing the invalid `InputBuffer`/timeline relationship | A timing or resync path can expose it; Lightyear itself normally shifts buffered tick metadata on an input-timeline `SyncEvent`. | [Lightyear tick-snap observer](https://github.com/cBournhonesque/lightyear/blob/0.28.0/crates/inputs/inputs/src/client.rs#L1156-L1191) |
| Calling the encoder | Lightyear's client `prepare_input_message` derives the delayed `tick` and calls `S::build_from_input_buffer` for every local input buffer. | [Lightyear sender path](https://github.com/cBournhonesque/lightyear/blob/0.28.0/crates/inputs/inputs/src/client.rs#L603-L730) |
| Unbounded allocation after the relationship is invalid | Upstream `lightyear_inputs_native::NativeStateSequence`; it casts the negative end index and pushes in the unchecked range. | [Native encoder](https://github.com/cBournhonesque/lightyear/blob/0.28.0/crates/inputs/inputs_native/src/input_message.rs#L72-L98) |
| Application-specific recovery | Overmatch: discard the stranded local buffer before Lightyear's sender invokes the encoder, then refill it naturally on subsequent input writes. | Local implementation: [`src/net/client.rs`](../../../src/net/client.rs) (`install_input_buffer_guard`, `clear_stranded_input_buffer`, and `drop_stranded_input_buffer`) |

This distinction matters: a downstream change that avoids a particular
backward rebase is worthwhile, but it cannot make the upstream encoder safe
against every future invalid state, extension, or timing path. The public
buffer has a separate upstream behaviour that refuses a write below its
`start_tick`, which makes a stranded future floor persistent rather than
self-correcting. [InputBuffer write path](https://github.com/cBournhonesque/lightyear/blob/0.28.0/crates/inputs/inputs/src/input_buffer.rs#L121-L173).

## Current upstream status

The local lockfile resolves `lightyear_inputs_native` to `0.28.0`; the cached
crate metadata identifies upstream commit `28e823d9df394c193dfc09f8eb891b77424e81c5`,
which is also the upstream `0.28.0` tag. **MEASURED:** this was read from this
workspace's `Cargo.lock` and the registry crate's `.cargo_vcs_info.json` on
2026-07-14. The upstream current `main` SHA examined was
`e0bcaf7901f8b194d5cfadcd155ab59d11c4e08a`, committed on 2026-07-14.
[Current commit](https://github.com/cBournhonesque/lightyear/commit/e0bcaf7901f8b194d5cfadcd155ab59d11c4e08a).

The tag-to-main comparison has 37 commits ahead of `0.28.0`, but no change to
this native encoder file; its file history lists its most recent relevant
change before the tag. **MEASURED:** GitHub's comparison and file-history APIs
were queried on 2026-07-14. Therefore the defect remains on current upstream
`main`. [Comparison](https://github.com/cBournhonesque/lightyear/compare/28e823d9df394c193dfc09f8eb891b77424e81c5...e0bcaf7901f8b194d5cfadcd155ab59d11c4e08a), [current source](https://github.com/cBournhonesque/lightyear/blob/e0bcaf7901f8b194d5cfadcd155ab59d11c4e08a/crates/inputs/inputs_native/src/input_message.rs#L72-L98).

No issue or PR for this exact `NativeStateSequence::build_from_input_buffer`
negative-range allocation was found in the upstream repository's exact-term
GitHub issue/PR search on 2026-07-14. **MEASURED:** the search returned no
matching item for `NativeStateSequence`, and the only `build_from_input_buffer`
hit was a different open input-buffer-anchor problem. This is search evidence,
not proof that no differently worded report exists. [Exact-term search](https://github.com/cBournhonesque/lightyear/issues?q=%22NativeStateSequence%22&type=issues&state=all), [function-term search](https://github.com/cBournhonesque/lightyear/issues?q=%22build_from_input_buffer%22&type=issues&state=all), [related but distinct issue #1559](https://github.com/cBournhonesque/lightyear/issues/1559).

There is an upstream precedent for fixing a different unbounded input-buffer
allocation: PR [#1525](https://github.com/cBournhonesque/lightyear/pull/1525)
merged a server-side bound on a maliciously far-future wire `end_tick` before
`InputBuffer::set_raw` can grow the server buffer. That protection is in the
server receive path and cannot protect this client-side native encoder, which
runs while constructing an outbound message. [Server bound](https://github.com/cBournhonesque/lightyear/blob/0.28.0/crates/inputs/inputs/src/server.rs#L380-L404), [client outbound path](https://github.com/cBournhonesque/lightyear/blob/0.28.0/crates/inputs/inputs/src/client.rs#L603-L730).

## Recommended upstream fix and downstream posture

Upstream should make the encoder total over any `InputBuffer` state:

1. Before converting to `usize`, return an empty/`None` sequence (or otherwise
   fail closed) when `end_tick < buffer_start_tick`.
2. Calculate a bounded, non-negative count only after that check, and add a
   regression test that exercises the inverted relation without allocating.
3. Retain the existing tick-snap metadata adjustment, but do not rely on it as
   the memory-safety boundary.

Until a released version contains that check and a bounded regression test,
keep Overmatch's pre-`PrepareInputMessage` guard and its safe semantic
tripwires in `tests/net_input_buffer_wrap.rs`. On upgrade, first inspect the
upstream encoder and run a bounded reproduction; do not remove the guard merely
because the tick-snap path or unrelated input-buffer code changed.

## Evidence in Overmatch

The DERIVED 31-App fan-out test on Linux exposed the allocation while connecting
real Lightyear clients over loopback UDP. **MEASURED:** GitHub Actions run
[`29322319914`](https://github.com/vikng-dev/overmatch/actions/runs/29322319914)
reached its 4,294,967,296-byte cgroup limit after 15.49 seconds; `memory.peak`
equalled the limit, `oom=1`, `oom_kill=1`, Docker reported `OOMKilled=true`, and
the container exited 137. **MEASURED (approximate user observation):** the same
test on an uncapped macOS host grew to roughly 30 GB before it was manually
killed; no sampler recorded that host-side peak.

After the test composition adopted the shipping fixed-delay configuration and
mounted the same pre-encoder guard as production, **MEASURED:** run
[`29324124477`](https://github.com/vikng-dev/overmatch/actions/runs/29324124477)
completed in 92.40 seconds with cgroup peak 305,979,392 bytes, zero memory-boundary
or OOM events, Docker exit 0, and `input_buffer_guard_clears=0` throughout. The
zero clear count means this successful run avoided creating the inverted state;
it did not recover by repeatedly discarding inputs. **INFERENCE:** matching the
shipping fixed delay removed this run's trigger, while the guard remains the
memory-safety fallback for other timing paths.

The downstream regression suite never invokes the hazardous allocation. It pins
the two safe semantic enablers (signed negative tick subtraction and refusal to
write below `InputBuffer::start_tick`) plus the scheduled guard's clear/preserve
behaviour and metrics. **MEASURED:** the focused tests passed on 2026-07-14.

## What fixing this unlocks for us

Once a released Lightyear version rejects the inverted range and carries a
bounded regression, Overmatch can delete its client-side input-buffer guard,
guard metrics, malformed-composition fallback, and their focused tests. The
Docker-contained fan-out test stays: it protects the broader shooting transport
contract and resource budget, not this one defect.

This fix alone does **not** make `InputDelayConfig::balanced()` safe for
Overmatch. The independent rollback-check starvation and fabricated-input
reports still require the shipping delay to remain fixed.
