# F3 — Suspension ground-probe shape (line ray vs wheel sphere)

**Status:** open · default chosen 2026-07-06 · settles by SP + MP feel drive

**The question.** Each roadwheel finds the ground with a downward probe; its penetration drives
the spring that holds the hull up. Should that probe be the original **line ray**, or a
**wheel-radius sphere** cast? The sphere is geometrically equivalent to rounding every terrain
edge by the wheel radius, making contact distance — and thus spring force — a *continuous*
function of pose. The ray makes contact a *binary* hit/miss, so a terrain-feature edge teleports
the contact between a curb top and the road below. That binary flip is the MP-jitter amplifier
(the final mechanism from the washboard rollback-storm investigation): millimetre client/server
pose differences flip ray hit/miss and step per-wheel load by up to ~120 kN, tripping the
`LinearVelocity` rollback and then chaining (replay-through-contact re-rolls the outcome).

**Default — sphere cast (`SUSPENSION_PROBE=sphere`, the default).** Each wheel casts a ball of
its effective radius, retracted up the cast axis by a small probe margin so an already-touching
wheel still reports (`SPHERE_PROBE_RETRACT`). The ground distance is reconstructed
`hit.distance + r − retract` so the spring compression on flat ground is **byte-identical** to the
ray model (the radius and retract cancel — the equilibrium is preserved for any radius). Only the
contact-distance *source* and the contact *point* (`hit.point1`, the true terrain contact) change;
the spring/damper math and the force direction are untouched.

**Chosen because** it fixes the divergence *at the source* — a pose-continuous contact means the
same pose gives the same load on client and server, so the washboard rollback *chains* collapse
(see numbers). It is also physically honest: a real wheel with radius rolls over a lip
continuously; the line ray teleports its contact off the lip, which is exactly the "snaps around"
feel Yan flagged in SP as well as MP. Flat-ground ride height, pitch, and the 16 wheel loads are
mathematically unchanged, so it is a strict superset of the ray's rest behaviour.

**Alternative kept alive — line ray (`SUSPENSION_PROBE=ray`).** The original single downward ray
per wheel (`SpatialQuery::cast_ray`, unchanged). Fully intact behind the switch: the env var is
read once at startup into the `SuspensionProbe` resource and both arms live in `apply_suspension`.
Preserved because the sphere is a *feel* change under playtest, and because the ray is the
byte-exact reference the flat-ground equilibrium is validated against.

**Why it's a playtest call.** Whether the continuous contact *feels* better under a controller
(smoother ride over the rough course, no edge-snap) or introduces a new mush/float feel can't be
reasoned out — it needs SP + MP drives. Watch for: ride-height drift on slopes (the sphere places
contact at the nearest terrain point, laterally offset on a slope, shifting the torque arm
slightly vs the ray's straight-down point); the spawn-settle bounce (the sphere's first contact is
marginally stiffer — see below, *not* masked); and whether the constant low-amplitude load churn
the sphere introduces at contact boundaries reads as liveliness or noise.

**A/B numbers** (headless harness, `SPIKE_PERTURB=0`; env set on both server + client; ray vs
sphere. lat0 = `SPIKE_LATENCY_MS=0`, near-deterministic — the reliable signal; 80/10 =
`80ms/10ms` conditioner, whose rollback count is jitter-chaotic per the standing MP-jitter note,
so treated as directional only).

| Metric | ray | sphere |
|---|---|---|
| **Flat equilibrium** (reverse cruise, lat0): total load, 16 wheels | 559 154 N (34 947 N/wheel) | 561 919 N (35 120 N/wheel) — **+0.49 %** |
| Flat straight cruise bit-exactness (anatomy noise floor) | 0 across all fields; 0 in-drive onsets | **0 across all fields; 0 in-drive onsets** (identical bit-exact cruise) |
| **Washboard lat0** (3 runs): rollbacks | 49 / 65 / 64 (med 64) | 19 / 17 / 16 (**med 17, −73 %**) |
| Washboard lat0: max rollback-chain (gap≤15) | 20 / 34 / 26 (med 26) | 7 / 5 / 5 (**med 5**) |
| Washboard lat0: in-drive divergence onsets/s | ~0.27 /s | ~0.20 /s |
| Washboard lat0: per-tick load step p50 / p95 | ~1.0 kN / ~104 kN | ~43.7 kN / ~82 kN |
| Washboard lat0: gnd-flip count (server) | ~164 | ~354 (boundary flicker at ~0 load) |
| **Washboard 80/10** (2 runs): rollbacks | 27 / 30 | 35 / 26 (comparable; chaotic regime) |
| Washboard 80/10: correction-active % | 14.2 % / 14.5 % | 15.6 % / 11.6 % |
| **Beached repro** (idle 80/10, ray-captured pose): rollbacks | 1 (clean) | 1 (clean) |
| **Spawn-settle burst** (rollbacks first 3 s): lat0 flat / 80/10 | 4 / (3, 13) | 5 / (3, 15) |

**Reading the numbers.** The win is the *chain* collapse, not the raw onset count: lat0 washboard
has ~the same number of divergence onsets (5 vs 5 in the sampled run) but the ray's onsets cascade
(max-chain 26–34) while the sphere's don't (max-chain 5) — pose-continuous contact lets a replay
*converge* instead of re-rolling. Net rollbacks fall ~73 % at lat0. Two honest non-wins, left
unmasked: (1) the per-tick load *step* does **not** cleanly slash — the sphere trades the ray's
quiet-then-spiky profile (p50 ~1 kN, p95 ~104 kN) for constant moderate churn (p50 ~44 kN) with a
lower p95 (~82 kN) but a comparable/higher single max, because on the *coarse* washboard (gaps
wider than a wheel diameter) both models genuinely go airborne and slam the next bump — the sphere
only smooths features at or below its radius. (2) The **spawn-settle** bounce is not reduced (in
fact marginally higher): the sphere's first ground contact is slightly stiffer (spawn load step
~443 kN vs ~380 kN), so the initial drop-and-catch costs a comparable rollback burst. (3) The
**beached** wheels, airborne under the ray, now *graze* terrain under the sphere (~5 cm more reach:
185 gnd flips, ~195 N mean — negligible), but the repro stays clean (1 rollback). SIM-EVIDENCE
still reports 16/16 grounded at flat rest under the sphere; SHADOW-BAKE ok both ends; zero
NAN/panic/B0004 across all runs.

**Revert cost.** Trivial — flip `SUSPENSION_PROBE=ray` (env, no rebuild), or delete the `Sphere`
match arm + the `SuspensionProbe`/`wheel_radius`/`SPHERE_PROBE_RETRACT` machinery to make the ray
permanent. **SIM-AFFECTING**: client and server must run the *same* value or they diverge every
tick — the startup log line states this and the choice is read once at boot.

**Lives in.** `src/driving.rs` — `apply_suspension` (both probe arms + the offset-algebra note),
`SuspensionProbe` (the A/B resource + `from_env`), `SuspensionParams::wheel_radius` (the sphere's
radius; `#[serde(default)]` to the Tiger's effective ~0.5166 — not authored, since the geometry
extractor carries only each wheel's node + side, no radius), `SPHERE_PROBE_RETRACT`. Prior art for
the retract idiom: `src/track_sandbox/model2.rs` (the belt plate cast).
