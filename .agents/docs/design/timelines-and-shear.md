# Timelines and shear: the client's four tick indices

2026-07-09. A networked client does not render "the simulation at an instant." It renders a
**composite**: each entity is drawn from the sim at *some* tick, and different entities sit at
different ticks in the same frame. The gap between two interacting entities' ticks is **shear**, and
four separate combat problems — ramming, aim lead, the opponent's tracer, and the unfelt hit — are
one phenomenon expressed in it. The model was written down nowhere; three agents plus the coordinator
re-derived it in one day and two got the `C`-vs-`I` ordering backwards (the error that commit
`8783520` reverses). This doc fixes the model, cites it to vendored source, and **measures** the
offsets. It builds on [ADR-0015](../adr/0015-divergence-doctrine.md) (why the two sides drift) and
[ADR-0016](../adr/0016-replicate-causes-derive-consequences.md) (replicate causes, derive
consequences); it does not repeat them.

The tick rate is **64 Hz** — `ClientPlugins { tick_duration: 1.0/64.0 }` (`src/net/client.rs:109`),
so **1 tick = 15.625 ms** throughout. Every number below is labelled **MEASURED** or **DERIVED**.

## 1. The four tick indices

At one instant the client holds four distinct tick indices. All are DERIVED here from vendored
source; §2 measures them.

**`P` — the predicted present.** `LocalTimeline::tick()`
(`lightyear_core/src/timeline.rs:115`), incremented once per `FixedUpdate` in `FixedFirst`
(`:126`,`:183`). This is where the client's **own** tank lives: it predicts its own input forward
and reconciles by rollback (the committed model, ADR-0015). `P` is ahead of the server. The offset
is set by `InputTimeline::sync_objective` (`lightyear_sync/src/timeline/input.rs:285`):

```
obj = remote.current_estimate()            // = S, see below
    + network_delay(RTT/2)
    + jitter_margin
    + 1                                     // server reads inputs one tick after receipt
    + sync_error_margin
    − input_delay                           // input.rs:313–315
```

so `P − S = RTT/2 + jitter_margin + 1 + error_margin − input_delay`. With
`InputDelayConfig::balanced()` (`:181–187`: `max_input_delay_before_prediction = 3`,
`max_predicted_ticks = 7`) the input-delay term absorbs latency up to 3 ticks before any prediction
runs (`input_delay_ticks`, `:223–259`). This is the term ADR-0015 names the **prediction margin**:
at LAN/loopback RTT it drives the margin toward zero and `P` collapses onto `S` — visible in §2.

**`S` — the server's now.** The client's estimate of where the server's simulation *actually is
this instant*: `RemoteTimeline::current_estimate() = now + offset`
(`lightyear_sync/src/timeline/remote.rs:273`), where the offset is smoothed each receive toward
`last_received_tick + RTT/2` (`:138–139`; `last_received_tick` at `:90`). So `S ≈ last_received_tick
+ RTT/2`: the newest server tick on the wire, pushed forward half a round-trip to account for its
travel time.

**`C` — the confirmed frontier.** `ReplicationCheckpointMap::last_confirmed_tick()`
(`lightyear_replication/src/checkpoint.rs:127`): the newest authoritative server tick for which
**Replicon completed all of its mutate messages** (`record_last_confirmed_tick`, `:139–155`; the
module header, `:1–38`, spells out that this is a Replicon delivery/completeness index mapped into
lightyear's tick domain). It is a **single global scalar** — one mutate-completeness high-water mark
for the whole replication stream — **not a per-entity value**.

**`I` — the interpolation render index.** Where every **non-owned** entity is drawn.
`InterpolationTimeline::sync_objective` (`lightyear_interpolation/src/timeline.rs:88`):

```
I = S − (delay + jitter_margin)
    delay = max(send_interval × 1.7, min_delay = 5ms)   // to_duration, :64–68; defaults :44–49
```

`I` deliberately trails `S` by an interpolation delay so a non-owned entity always has a server
snapshot on both sides of it to interpolate between.

### `C` and `I` are not commensurable, and the naive ordering is wrong

`C` is a **global completeness frontier** (one scalar, "the whole stream is delivered through here").
`I` is a **per-entity render index** (a clock each interpolated entity is sampled at). They measure
different things; there is no reason one must sit ahead of the other, and — the trap — the intuitive
ordering `I < C` (interpolation is "in the past", confirmation is "recent") is **wrong at shipping
latency**. The algebra says why:

```
S − C ≈ RTT/2                        (C tracks the newest received tick; S is RTT/2 past it)
S − I ≈ delay + jitter  ≈ 1.7 ticks  (latency-INDEPENDENT: dominated by send_interval × 1.7)
```

`S − C` **scales with latency**; `S − I` is **roughly fixed**. So the two cross over near
`RTT/2 ≈ delay`, i.e. one-way ≈ 27 ms. **Below** the crossover `C` is nearer `S` than `I` (`I < C`);
**above** it — at any shipping latency — `C` sits *further* behind `S` than `I` does, so the true
ordering is

```
C  <  I  <  S  <  P .
```

§2 measures both regimes and the crossover between them. Commit `8783520` states this ordering;
`57f1405` (its parent) assumed `I < C` and anchored the opponent's shell wrong as a result.

## 2. The measurement

Client instrumented to log `P`, `S`, `C`, `I` (and `P − I`) once a second, reverted after (§ end).
`S` computed from its definition `last_received_tick + RTT/2` (`RemoteTimeline::last_received_tick()`
+ `PingManager::rtt()`); `I` read from `InterpolationTimeline::now()`; `P`, `C` from the resources in
§1. Harness: server `SPIKE_PERTURB=0 OVERMATCH_BOT=1`; client headless `SPIKE_SIM_LONG=1
SPIKE_SIMULATE_INPUT=1` (the ~20 s dead-straight course); client-side `RecvLinkConditioner` on the
**inbound** link only (so measured RTT ≈ the one-way latency setting). macOS loopback, 2026-07-09,
binary at working tree `8783520` + this instrumentation. First 4 samples/run dropped as sync
warm-up. `lat0` avoided (the connect hang in `sim-divergence-and-determinism.md` §7). No
`NAN-TRIPWIRE|FIXED-NAN|panicked|B0004` in any client or server log.

All figures **MEASURED**, in ticks (× 15.625 ms), mean ± population sd over the samples:

| offset | 80 ms / 10 ms jitter (RTT ≈ 91 ms) | 40 ms / 5 ms jitter (RTT ≈ 51 ms) |
|---|---|---|
| `P − S` | **+1.0 tk** (run means +1.75, +0.34) | **−0.76 tk** / −11.8 ms (sd 0.61) |
| `S − C` | **+2.96 tk / +46 ms** (2.94 sd .10 ; 2.97 sd .27) | **+1.63 tk / +26 ms** (sd 0.05) |
| `S − I` | **+1.6 tk** (1.32 sd .50 ; 1.88 sd .54) | **+1.80 tk / +28 ms** (sd 0.31) |
| `P − I` | **+2.6 tk / +41 ms** (3.07 ; 2.22) | **+1.04 tk / +16 ms** (sd 0.61) |

Sample size: 80/10 is **two runs, 16 samples each (32 total)**; 40/5 is **one run, 16 samples**.

**Reading the numbers — the measurement confirms the derivation, and sharpens it.**

- **`S − C` tracks `RTT/2` exactly.** 46 ms at RTT 91, 26 ms at RTT 51 — i.e. ≈ RTT/2 in both,
  and the tightest quantity measured (sd ≤ 0.27 tk). In the raw log `C` equals `last_received_tick`
  to within a tick, confirming `C` is "the newest fully-delivered tick" and `S` is that pushed
  forward half a round-trip.
- **`S − I` is latency-independent** at ≈ 1.6–1.8 tk (25–28 ms), consistent with `delay =
  send_interval × 1.7 ≈ 1.7 tk` and *not* growing with RTT — exactly the split §1 predicts.
- **The `C`/`I` ordering flips at the crossover, as derived.** At **80/10** `S − C` (2.96) **>**
  `S − I` (1.6): `C` sits further back, ordering `C < I < S < P` — the shipping-latency case, and
  **the naive `I < C` is false here**, in both runs independently. At **40/5** `S − C` (1.63) and
  `S − I` (1.80) have converged and just crossed (`I` now marginally the further back): this is the
  crossover the algebra puts near one-way ≈ 27 ms. So there is **no fixed `C`-vs-`I` ordering** —
  concrete proof they are not commensurable.
- **`P − S` is the loose one, and that is expected.** It swung 0.34–1.75 tk across the two 80/10
  runs and went *negative* (−0.76 tk) at 40/5 — `P` fell *behind* `S`. This is the zero-margin
  regime ADR-0015 names: `balanced()` spends ~3 ticks on input delay, which at ≤ RTT 51 absorbs the
  whole round trip, so prediction runs ~0 ticks ahead and the sync controller's deadband
  (`error_margin`, `input.rs:301`) lets `P` settle anywhere within ≈ ±1 tk of `S`. `P` leads `S`
  materially only once latency exceeds what input delay can absorb (the 80/10 runs).

**Verdict: no contradiction.** The derivation's structure holds on every row; the one derived
*magnitude* that is soft — `P − S` — is soft because the sync deadband makes it soft, and the
measurement says so. The load-bearing claim (`C < I` at shipping latency) is confirmed twice.

## 3. Shear

**Shear** is the tick gap between two entities that interact. **An interaction is well-posed only
between entities on the same index** — same tick, same physics world, one collision/query with a
defined answer. Off-index, the two objects are snapshots of different moments pretending to touch.

Static world geometry has **no index** — the terrain is identical on every tick, so driving,
suspension, and terrain casts interact with it at zero shear regardless of `P`/`S`/`C`/`I`. **That is
why driving feels right and everything involving a second tank does not.** The four known problems
are one phenomenon — shear between two live entities:

- **Ramming feels wrong.** Own hull at `P`, the opponent's collider at `I`. Maximum shear:
  `P − I` = **MEASURED +2.6 tk (≈ 41 ms) at 80/10** (table above). The two hulls are ~2–3 ticks
  apart; contact resolves against where the opponent *was*, not where it is drawn relative to you.
- **Aim needs lead you cannot learn.** You lay the gun on an opponent drawn at `I`; the server
  evaluates the shot at `S`. With no lag compensation the required lead is `S − I` (**MEASURED
  ≈ +1.6 tk at 80/10**) and it *changes with latency and jitter*, so it is not a fixed sight
  picture a player can internalise.
- **The opponent's tracer was incoherent, not merely late** (commit `8783520`). Before the fix the
  shell had **no index at all**: origin taken from the fire tick, age taken from packet-arrival
  wall-clock, then stepped on the receiver's own clock. It was not a snapshot of any tick — it could
  not be co-indexed with anything, so it visibly passed behind a moving target. See §5.
- **Getting shot is unfelt.** Two independent reasons, both structural, neither a smoothing artifact:
  1. The local hit impulse is gated **off entirely** on the client — `on_hit_impulse` early-returns
     under `Res<ClientReplica>` (`src/ballistics.rs:927–939`), a **whole-client** gate, so the
     client never applies its own shove (correctly: the struck body is server-owned; a local shove
     would fight replication).
  2. The server's shove is **never delivered** either. The impulse is ~0.14 m/s (MEASURED, prior —
     the 2026-07 hit-feel investigation; memory `mp-hit-feel-view-layer`), which over an 80 ms
     window is `0.14 × 0.08 ≈ 1.1 cm` — **below `ROLLBACK_POSITION_M = 0.05 m`** (5 cm,
     `src/net/protocol.rs:429`). The client's confirmed-vs-predicted comparison therefore reads the
     shoved server state as *matching* its un-shoved prediction, never rolls back, and never adopts
     the shove. It is not smoothed away — it never enters the client's sim at all. (This is why
     ADR-0015 files hit *feel* as a view-layer cue, not a physics correction.)

## 4. Why collision is structurally special

A projectile hit is a **one-way, instantaneous query**: the shooter (or server) asks, at one tick,
"does this ray/point hit that body?" Because it is one-way and momentary, there is an instant to
rewind to — you *can* lag-compensate it in principle (rewind the target to the shooter's view,
resolve, done). Whether we do is a separate choice; the point is it is *possible*.

Collision between two tanks is **mutual and continuous**, and that combination makes lag compensation
impossible, not merely expensive:

- **Mutual.** Each body is simultaneously querier and queried. Un-shearing the pair for tank A (rewind
  B to A's tick) re-shears it for B (B is now off its own tick). There is no frame in which both are
  on-index at once, so no single rewind resolves it symmetrically.
- **Continuous.** The contact persists across ticks and feeds back (positions this tick depend on the
  contact forces last tick). There is no single instant to rewind *to* — a momentary query has one; a
  standing constraint has none.

This is structural, not folklore. There are exactly three ways out, and every shipping title takes
one:

1. **Make shear zero by construction** — determinism + predict both bodies + rollback, so both tanks
   live at `P` and collide on-index. (Costs the full replay-determinism bill; ADR-0015 §
   forward-vs-replay.)
2. **Convert mutual into one-way** — assign, per contact pair, one body as authority and the other as
   follower for the duration of that contact (Glenn Fiedler's networked-physics authority handoff),
   turning the mutual constraint into a one-way one that *can* be compensated.
3. **Resolve where shear is zero — on the server**, which holds both bodies at its own single tick
   `S`. Clients see the *result* replicated.

These are stated neutrally; which we adopt (in particular whether to predict non-owned tanks) is an
open ADR, not decided here. What is settled: **collision is resolved on the server** (exit 3) — the
only place both bodies share an index — and that is what every comparable game ships.

## 5. Which entities may join which index

An entity may be placed on **any** index *exactly* iff its entire future is a function of information
you already hold — a **complete cause** (ADR-0016's classification, applied to time instead of the
wire).

- **A projectile has a complete cause.** Its whole future is `(origin, direction, speed, fire_tick)`
  + physics — nothing about it awaits a decision. So advancing it to *any* tick is **arithmetic, not
  a guess**: elapsed = `target_tick − fire_tick`, integrate. This is why commit `8783520` anchors the
  opponent's shell at **`P`** — co-indexed with the only hull this client predicts and can feel a hit
  on (its own) — and re-derives its flight from the fire event, exactly ADR-0016's "derive the
  consequence from the replicated cause." A shell aged to `P` meets the player's own hull on the same
  tick number the server's shell does: same tick, same result, no rollback needed to agree. (The fix
  also raycasts the muzzle-to-`P` segment and suppresses the tracer if terrain/armor already blocked
  it on the authority — a phantom tracer behind a wall is worse than none.)
- **A tank has an incomplete cause.** Its next state depends on a human's next input, which you do
  not hold. So a remote tank can join **no** index exactly; the honest thing is to place it in the
  past and interpolate between confirmed snapshots — which is precisely why non-owned tanks live at
  `I` and the shear of §3 exists. Giving a client the opponent's *inputs* would complete the cause
  and is what exit 1 / predict-both would require (ADR-0016 notes `ServoAngles`/`FireEvent` are
  deletable under that change); that is the open ADR's territory.

The rule, stated once: **place an entity on an index only up to the completeness of its cause.** A
projectile is complete and may sit anywhere; a tank is not and may sit only in its interpolated past
unless you supply the missing input.

---

*Instrumentation note.* The §2 numbers were taken with a temporary once-a-second `P/S/C/I` logger
added to `src/net/client.rs` (env-gated on `SPIKE_PSCI`), reverted immediately after the runs;
`git status --short -- src` is clean and the gates (`cargo clippy --all-targets --features net -D
warnings`; `cargo test --features net`, 36 lib tests) pass on the reverted tree. To re-measure, add a
system reading `LocalTimeline` (P), `RemoteTimeline::last_received_tick()` + `PingManager::rtt()` (S),
`ReplicationCheckpointMap::last_confirmed_tick()` (C), and `InterpolationTimeline::now()` (I) off the
`Client` entity, and run the harness at ≥ 2 latency conditions (never `lat0` — §7 of
`sim-divergence-and-determinism.md`).
