# Contact-oracle research — deterministic penetration for track contact

2026-07-16. Four parallel research agents (engine manifolds / engineering track sims / shipped
games / field-based contact), dispatched from the step-18 zoom-out: *"can we make both
longitudinal and lateral sampling non-noisy by nature — is there a cheaper way to get
deterministic penetration?"* Yan's read was correct; this memo is the synthesis. Full agent
reports are summarized per section; every claim carries its source.

## The reframe (what the research confirms)

The noise never came from measuring penetration — it comes from asking a narrow-phase solver for
an **extremum point** (a shape-cast witness). Depth *at a fixed, app-chosen sample point* is
deterministic and pose-continuous by construction. Every mature discipline that touched this
problem converged on the same move: **stop consuming witness points; evaluate a field at fixed
samples (collocation), or integrate it over the patch, or force the engine to return a full
clipped manifold instead of one point.**

Convergent evidence, one line each:

- **Robotics (Drake hydroelastic)**: pressure-field contact was *invented* because point contact
  is discontinuous; force = integral of a field over the patch; even TRI's production version
  coarsened to ~1 quadrature point per contact polygon. Continuity comes from the integral;
  determinism from fixed evaluation. (drake.mit.edu hydroelastic user guide; arxiv 2110.04157.)
- **Terramechanics (NTVPM/Wong, RecurDyn, ADAMS, DLR/Chrono SCM)**: no engineering-grade track
  simulator lets a collision solver define track–soil contact. Two shipped patterns: fixed
  collocation stations on the shoe sole (per-link force elements — our closed-form profile is
  exactly an NTVPM station), or terrain-grid nodes ray-testing the vehicle (SCM, ~10–40
  nodes/shoe). (projectchrono SCMTerrain.cpp; Wong et al. 1984; Rubinstein & Hitron 2004.)
- **Shipped games**: gameplay track contact is *N stations with fixed-origin rays/fans against a
  heightfield or dedicated tracer*, everywhere — Dagor `rayCar` + `custom_tracer` (War Thunder),
  WoT's spring-chain treads colliding with a pre-raycast heightfield patch, Jolt's
  TrackedVehicleController (wheels + ray/sphere/cylinder cast, no links), Rocket League (Bullet
  raycast vehicle + ruthless simplification, 120 Hz rollback), Arma Reforger (deterministic
  wheel model; chassis-vs-world collision *skipped during replay*). Per-link physics tracks ship
  only as visual layers or single-player assets (PTM needed a "track repair function").
  Middleware answer to curb/flat noise: **fixed wheel-local ray fan, weighted-average blend**
  (NWH, VPP) — never a shape-cast. (GaijinEntertainment/DagorEngine; GDC 2020 WoT deck; GDC 2018
  Rocket League; reforger.armaplatform.com server-authoritative vehicles.)
- **Engines themselves**: even the witness camp doesn't trust single witnesses — Bullet persists
  4-point manifolds, AGX merges/reduces track contacts, Box2D v3 clips reference faces and adds
  hysteresis to feature selection. Determinism there is "stable per build/ordering," never
  pose-continuity. (Catto GDC 2007/2013; box2d.org determinism post.)

## The parry finding (engine-native fix, verified + benchmarked locally)

parry3d 0.27's `PersistentQueryDispatcher::contact_manifold_convex_convex` (re-exported at
`avian3d::parry`) returns, for our exact pill-on-flat geometry, a **deterministic 2-point
manifold**: both capsule endpoints, per-point penetration (`-dist`), one face normal, stable
`PackedFeatureId`s — **bit-identical positions under sub-mm pose noise** (measured; probe at
scratchpad `manifold_bench`). The arbitrary GJK witness we've been fighting appears as a third
point with `fid == UNKNOWN` — *labeled and filterable*. Cost measured on this machine: 297 ns
fresh vs 342 ns for our current cast (~50 µs/tick for all 166 links; persistent-manifold fast
path 12 ns but manifolds become rollback state — use fresh per tick for resim purity).

**Guardrail found**: the path stays on GJK only while the capsule's core segment is outside the
terrain — i.e. penetration < capsule radius. Our per-link rest penetration (~25–50 mm) EXCEEDS
the honest t/2 = 20 mm radius → EPA fallback (the one residually unstable step). Fix that is
also a feature: **inflate the query radius** (e.g. r_q = 60–80 mm about the pin line) and
subtract (r_q − t/2) from returned depth — a pure Minkowski offset: exact on flat faces, rounds
terrain edges *more* (free smoothing knob, decoupled from the physical shoe), and keeps GJK
valid to 3–4× deeper penetration. This mirrors the field-side lesson below.

## The field option (deterministic by construction)

Terrain-as-field: depth = −sdf(p), normal = ∇sdf(p), evaluated at fixed link-local samples.
For our authored box terrain the field is exact and closed-form (~10-flop rounded-box SDF,
`min()` over spatially-bucketed nearby boxes). Fixed samples per link (2 pins + mid, × 2 edge
columns) feed the existing closed-form profile unchanged. No parry call anywhere in track
contact; pure fixed-order arithmetic → bit-deterministic in Rust (no FMA contraction by
default, no transcendentals), and the strongest cross-platform-determinism story we have
(relevant to the lockstep/GGPO door we're keeping open). Thousands of flops per tick — noise.

Two mandatory hardenings from the literature (both are "round the FIELD, not the mesh"):
1. **Rounded edges** — raw `min()` unions and box edges snap the *normal* (C0 depth, C1 breaks);
   round the box SDF (radius offset) and/or smooth-min adjacent boxes so normals turn instead of
   snapping. Same medicine as Jolt active edges / Drake margin / the parry radius-inflation
   above. Likely retires the washboard slap-down and helps the corner-rocking creep.
2. **Speculative margin** — pose-continuity alone does not kill discrete-time resting limit
   cycles (Drake's h²g "wobbling" analysis; they added a negative-pressure skirt). Our
   `CONTACT_ENGAGE` ramp is already this instinct; keep it, size it consciously.

Structural cost: marries track contact to *authored* terrain (primitives/heightfield now, baked
SDF grids for arbitrary meshes later — the UE5 level-set path). That is a map-authoring
commitment and belongs in the promotion ADR.

## Decision matrix

| | Fixed-sample field (SDF/heightfield) | parry manifold (filtered, inflated r) | Status quo (cast + endpoint rays) |
|---|---|---|---|
| Flat-tie noise | none (no extremum anywhere) | none after UNKNOWN filter | guarded by profile invariance only |
| Pose-continuity at edges | C0 depth; C1 with rounding knob | clip-basin continuous; feature flips at face transitions (hysteresis-able) | witness + feature flips |
| Same-build determinism | bit, trivially | bit (fresh manifolds = pure function) | bit but pose-dithered |
| Cross-platform determinism | pure arithmetic — best possible | needs parry `enhanced-determinism` (forwarded by avian 0.7) | same |
| Rollback state | none | none (fresh) / manifold cache (fast path) | none |
| Cost /link/tick | ~6 field lookups ≈ trivial | ~300 ns | ~340 ns + 2 rays |
| Terrain generality | authored primitives/heightfield/baked SDF only | any parry collider today | any parry collider today |
| Deep-pen robustness | unlimited (field is global) | GJK-safe to r_q; EPA beyond | cast is robust (directional GJK) |

## Recommendation (pending Yan)

1. **Sandbox: build the field oracle as MODEL 4 (field-belt), forked from model 3.** Same chain,
   same profile, same edge-column width design — only the oracle changes: rounded-box SDF union
   over the course's block list, fixed samples per link per column. Live `M` A/B against model
   3's cast answers the open feel items (washboard slap-down, corner creep) *and* prototypes the
   MP promotion architecture. Small build: the course is already a static box list we author.
2. **Record the manifold route as the bridge/fallback** for arbitrary-collider contact (props,
   wrecks, un-baked meshes), with the UNKNOWN-filter + radius-inflation recipe. It may also
   replace the cast inside model 3 cheaply if we want the A/B to be three-way.
3. **Promotion ADR gains a terrain-representation section**: SDF-friendly authoring (primitives +
   heightfield + baked grids) as the price of field-grade determinism; games-report evidence
   that per-link gameplay contact is an above-industry ambition we're taking on knowingly, with
   Reforger's "skip chassis collision during replay" and RL's simplification ethos as promotion
   levers if the budget bites.

## Sources (primary, deduplicated)

- Drake hydroelastic: drake.mit.edu/doxygen_cxx/group__hydroelastic__user__guide.html · group__hydro__margin.html · arxiv.org/abs/2110.04157 · ryanelandt.github.io/projects/pressure_field_contact/
- SDF: mmacklin.com/sdfcontact.pdf · iquilezles.org/articles/distfunctions/ · articles/smin/ · mujoco.readthedocs.io (SDF plugin, soft contact) · UE5 Chaos level sets (dev.epicgames.com chaos-destruction-overview)
- Terramechanics: github.com/projectchrono/chrono (SCMTerrain.cpp, M113_TrackShoeSinglePin.cpp, ChTrackShoeBand.cpp) · DLR SCM (Krenn & Hirzinger) · Wong et al. 1984 (10.1243/PIME_PROC_1984_198_155_02) · NTVPM 2019 (10.1177/0954407018765504) · Rubinstein & Hitron 2004 · RecurDyn TrackHM docs
- Games: github.com/GaijinEntertainment/DagorEngine (rayCar) · GDC 2020 WoT (media.gdcvault.com/gdc2020/presentations/6-years-optimizing-world-tanks) · GDC 2018 Rocket League (Cone) · reforger.armaplatform.com/news/server-authoritative-vehicles · gamedeveloper.com MudRunner · nwhvehiclephysics.com (WheelController3D) · vehiclephysics.com (VPP)
- Engines/determinism: parry3d 0.27.0 vendored source (contact_manifolds_pfm_pfm.rs, contact_manifolds_halfspace_pfm.rs, polygonal_feature_map.rs) · box2d.org/posts/2024/08/determinism/ · posts/2026/06/announcing-box3d/ · Catto GDC 2007 Contact Manifolds / GDC 2013 Continuous Collision · rapier.rs/docs/user_guides/rust/determinism/ · Photon Quantum fixed-point docs
- Local benchmark: scratchpad `manifold_bench` (parry3d =0.27.0) — manifold 297 ns vs cast 342 ns, pill-on-flat, 2 endpoint contacts bit-stable under ±0.05 mm noise.
