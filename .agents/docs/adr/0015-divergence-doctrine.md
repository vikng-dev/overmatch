# Divergence doctrine: continuous simulation, removable netcode scaffolding

Client/server divergence is now managed under one doctrine with two layers: a **permanent
simulation-design rule** (contact and force laws must be continuous functions of pose and
velocity) and a set of **deliberately removable netcode workarounds**, each mapped to a named
upstream defect. This ADR is the canonical statement of that doctrine, of the solo-divergence
model that motivates it, and of the binding continuity rule for all future force laws — the track
model explicitly included. It records the 2026-07-06 architecture-review findings; the measured
detail lives in `design/sim-divergence-and-determinism.md` §5, and every number below is measured,
none conjectured.

## Context: what divergence actually is here

We run state replication + prediction ([[0004-avian-physics]] world, lightyear 0.28): the server
is authoritative, the client predicts its own tank and reconciles by rollback. In that
architecture divergence is not an abstract float-determinism worry — it is a concrete, measurable
quantity with a concrete taxonomy, and the 2026-07-06 review settled it:

**The solo-divergence model.** With one player there is nothing to mispredict — the client has
its own inputs and a static world — so client and server *should agree completely*. **Rollbacks
in a solo game are a defect indicator, target ~zero.** They are not "netcode doing its job";
netcode's job only starts when a second player's unpredictable inputs arrive. Measured causes of
solo divergence, ranked:

1. **Contact-adjacent solver noise across two ECS worlds.** Flat-ground cruise is *bit-exact*
   client-vs-server (measured over ~880-tick windows, all replicated fields). Divergence exists
   only at contact transients, where entity-index-keyed constraint ordering differs between the
   two worlds and contact chaos amplifies last-bit float differences. Irreducible by
   configuration; needs upstream canonical constraint ordering.
2. **The dominant term (RETIRED 2026-07-09): the correction machinery *appeared* to manufacture
   its own divergence.** As recorded 2026-07-06: "hull contact fails to re-form on the first
   replayed tick (hc=0 on 55% of replayed ticks at 80 ms/10 ms jitter, 98.4% at lat0, with Δlv
   exactly −g·dt = 0.1533 m/s vertical at k=1 while pose restore is near-exact, |Δp| p50 1.5 mm)",
   read as a self-feeding engine in hull-contact states, felt as "the hull-stuck tank never
   settles". *(superseded 2026-07-06 by the `SPIKE_CONTACT_PROBE` reclassification (8a08d60),
   retired by the `AuthoredLocalTransform` shield (33cc4e4), re-measured post-shield 2026-07-09.)*
   Two corrections. (a) The mechanism was never a restore defect: it was attachment poisoning —
   child-collider proxies levitating up to 2.8 m above the root, so `hc=0` was avian being honest
   about a collider that had left (§2's contact-restore row in the design doc). (b) More
   fundamentally, the `hc=0`-among-replayed-ticks metric never discriminated the alleged defect:
   it conflates "no hull contact because the tank rides on its wheels or is airborne" (physically
   correct, and the common case) with "contact failed to re-form after restore" — so the
   98.4%/55% are a *poison indicator*, not a contact-restore failure rate, and are evidence for
   neither direction. Post-shield re-measurement (2026-07-09; design doc §6): the raw rate went
   *up* at 80/10 (100%, n=88 pooled replayed ticks) and down at lat0 (~62%, n=8 pooled), while the
   discriminating metric the original lacked — client `hc=0` while the server holds `hc>0` — is
   **0 across all 88 server-joined replayed ticks at 80/10**, and contact re-forms wherever the
   hull is genuinely grounded. **The ranking's #2 is retired, not replaced:** solo rollback
   frequency is now at the noise floor (2–4 per 20 s run vs the pre-shield storm), so no term is
   promoted to dominant. The multi-meter replay errors remain a *separate* machine: in-contact
   friction/load chaos through the replay (per-wheel load deltas to 5.8e6 N), absorbed by the
   Layer-2 thresholds.
3. **Input-timing slips under jitter** — rare. Trigger attribution is ~93% Position; the cause is
   state, not input.

And one settled point of framing: within state replication, **determinism is the
rollback-killer, not a rejected alternative**. Lockstep stays rejected (`design/sim-divergence-and-determinism.md`
§4.4 — the slowest peer gates everyone and one divergence desyncs permanently, with no authority to
re-anchor; *not* an ADR-0004 call, which is silent on netcode), but the Rocket League precedent is
the model — server-authoritative + prediction, with determinism pursued as the optimization that
makes corrections rare. **Determinism is orthogonal to authority**: it is a property of the sim, and
the target quadrant is deterministic *and* server-authoritative, with state kept as the re-anchor and
the divergence detector. Note also what determinism cannot do — it eliminates the *divergence* error
class (same inputs, different results) and cannot touch the *misprediction* class (you do not know a
remote player's next input). The useful
distinction is **forward determinism** (same state + same inputs → same result, on any machine; what
makes corrections rare; what Box2D v3-class engines ship) vs **replay determinism** (restore + resimulate lands
bit-identically on the forward path; what prediction + rollback needs; *no engine sells it
today* — avian issue #734 is the open upstream thread).

## Decision: two layers

### Layer 1 — sim continuity. Permanent. Ours.

*Contact and force laws must be continuous functions of pose and velocity* (divergence
continuity). A discontinuous law lets mm/s-scale divergence pick different force regimes on the
two machines — the sims bifurcate; a continuous law lets the same divergence nudge a blend weight
— the sims converge. Two applications already shipped and measured:

- **Sphere-cast suspension probe** ([[0005-raycast-roadwheel-locomotion]] evolved): washboard
  rollbacks −73%.
- **Friction static↔kinetic smoothstep blend + LuGre anchor relax** ([[0006-static-friction-brush-anchor]]
  evolved): the wedge-storm repro went from 44+ rollbacks with a deterministic runaway to 1 in
  the good regime.

This is **not a netcode workaround**: a continuous sim degrades gracefully under *any* legitimate
divergence — input-timing slips today, the other tank's genuinely unpredictable inputs forever —
while a discontinuous one bifurcates. **Binding rule for all future force laws, the track model
explicitly included:** contact primitives must be divergence-continuous. Sharp oriented box casts
are the known bug class; use rounded shapes or ray/sphere stations.

### Layer 2 — netcode scaffolding. Deliberately removable. Each piece names its upstream defect.

| Scaffold | Upstream defect it works around | Removal condition |
|---|---|---|
| `net/watchdog.rs` forced-rollback backstop | lightyear 0.28: receive-time mismatch check skipped at zero prediction margin and never retried; unchanged-entity scan excludes always-confirmed entities | Delete (or demote to insurance) when lightyear ships a deferred re-check of stored future samples |
| Contact-restore fix: `AuthoredLocalTransform` + `shield_authored_collider_transform` (src/tank.rs, landed 2026-07-06) | lightyear_avian 0.28 `AvianReplicationMode::Position` registers `ApplyPosToTransform` as a required component of `Position`/`Rotation` (plugin.rs:620-623), dragging child colliders into avian's `position_to_transform` write set — each frame rewrites the authored local `Transform` from a render-blended, one-`Propagate`-stale parent `GlobalTransform`, a compounding render→sim leak (ADR-0014 class, introduced upstream; upstream report candidate #3). (superseded 2026-07-06: was "restore path / avian #734 — contact state not restored, first replayed tick free-falls"; the probe showed the abandoned-timeline restore is benign — tree/moved-set/pairs self-heal once poses are honest — and the attachment poisoning is the killer) | Upstream excludes child colliders (non-`RigidBody` entities under a body) from the blanket `ApplyPosToTransform` requirement, or ships a per-entity opt-out from the Position→Transform sync |
| Coarse thresholds + desync-only velocity bars (`net/protocol.rs`) | The divergence the two rows above manufacture | Tighten toward the 1 cm reference values as measured divergence collapses |

**Permanent-but-looks-like-scaffolding: the render-space error layer** (`net/render_error.rs`,
"the sim snaps, the view never does"). It reads like correction cosmetics, but multiplayer
reintroduces legitimate mispredictions forever — you cannot predict the other tank's input — and
this layer is how *any* correction is presented. It stays, whatever upstream ships.

### Strategy

Ship the scaffold now — upstream timelines are not ours (#734 open since May 2025). File the
upstream reports. Keep each workaround small, with its defect and removal condition written into
its module doc. Let the Layer-1 continuity work compound: every force law that ships continuous
shrinks what the scaffolding has to absorb.

## Consequences

- **Solo rollback count is a first-class defect metric.** A solo run's rollbacks measure our bugs
  plus the scaffold's residue, not "expected netcode noise". Caveat inherited from the watchdog
  fix: pre-watchdog lat0 rollback *counts* measured check starvation, not convergence — invalid
  as an A/B metric (lat0 |Δp| tick-row divergence remains valid).
- **Every new Layer-2 piece must carry its upstream mapping** — defect, citation, removal
  condition — in its module doc, or it silently becomes load-bearing architecture.
- **Every new force law is reviewed against the continuity rule** before it lands; "it only
  matters under prediction" is not an exemption, it is the point.
- **Thresholds are a ratchet, not a setting**: as divergence collapses (contact-restore fix,
  upstream ordering), the `net/protocol.rs` bars tighten toward the reference values instead of
  ossifying.
- **Map authoring, defence-in-depth: prefer tiling large static colliders to ≤10 m extents.**
  parry's GJK shape-cast converges on a *relative* tolerance, so cast error scales with the
  target collider's extent (measured: 0.25 mm at 5 m half-extent vs 139–172 mm at 500 m —
  `tests/spherecast_scale.rs`). The sphere probe now reconstructs distance from witness geometry
  and is immune, but any *future* shape-cast consumer inherits the defect; small tiles cap it at
  the source. (Not applied retroactively — the 1000 m slab stands until a map-authoring pass.)

## Related

- `design/sim-divergence-and-determinism.md` — the measured record this ADR distills; §5 carries
  the 2026-07-06 findings in full.
- [[0014-sim-view-split]] — the sim/view split is what allows instant sim correction with all
  visible smoothing in the render-space error layer.
- [[0005-raycast-roadwheel-locomotion]] / [[0006-static-friction-brush-anchor]] — the two force
  laws already brought under the Layer-1 rule.
