# Bondsolve, session 5

Subject: the video game's environment destruction and damage model, game state only; crew are
HP pools at stations.

## Authored dimensions

Every construction now keeps its authored dimensions. The map instance record carries position,
rotation and identity only. The file decoder refuses any `scale` field, including unity, null and
non-array values, before the map reaches scatter. Its message names the level file, instance,
field and repair: remove the field and author a separate construction size variant.

Scatter placements and the construction spawn/footprint interfaces use Bevy's `Isometry3d`, so
they cannot carry a size. Proxy boxes still encode their authored bounds as unit-cube transforms;
that geometric encoding does not resize a construction instance. The map content digest changes
with the edited level and prototype bytes; replicated types and their protocol shape are unchanged.

The scripted map edit removed **709 scale members** (MEASURED), from 74 house and 635 fir
placements. JSON comparison against the parent version verified that every other field is
unchanged. No construction geometry, strength, material or attachment fact was changed.

DERIVED diff for the scale step: **408 Rust lines added, 1,186 removed, net -778**;
the level edit removes 3,545 JSON lines. These are text counts, not a complexity or speed metric.

Per-scale admission, structure/equilibrium maps, nearest-size guesses, root-hold rescaling,
contact-footprint scaling, element and chunk hull scaling, blast-backing scaling, deposit/visual
hole conversions, loose-group scaling and view scaling are deleted. Each asset retains one checked
structure and one initial gravity equilibrium; instances share them until their own loads change.

The solver iteration, projection, convergence and progress rules are unchanged. Production changes
in the law module remove `Patch::scaled` and the scale-specific initial-gravity route. Lane `settle` owns
iteration work. For the merge, `at_rest` now contains the former gravity-preparation body,
with only the optional size-guess block removed; retain `settle`'s iteration changes in that body.
The redundant placed-scale census and its exclusive residual/traction helpers are
deleted; the all-asset gravity census remains. Analytical load, contact, root-footprint refinement,
release accounting, shared equilibrium, collider and replica contracts remain.

The inline root-hold conversion preserves its anchor, attachment and rebake context on every
refusal. The boundary witness supplies finite anchor areas whose accumulation overflows in f32;
it requires the same named refusal and repair as an unrepresentable root hold. The prototypes
header now states that proxy colliders stay outside the immutable terrain field, matching scatter.

## Verification

MEASURED final verification: **316 game tests (three ignored), 50 asset-tool tests, and
2,384 full-library tests (39 ignored) passed, with zero failures**. The required source/bake
guards are included in the targeted and full runs. Formatting and workspace/all-target CI clippy
passed on the final Rust source. Logs live under `target/bondsolve-s5-*`; the completed sequence
is recorded in `target/bondsolve-s5-final-gates.log` and `target/bondsolve-s5-status.json`.

The new rigid-pose calls were checked against `bevy_math` and `bevy_transform` 0.19.1 source;
collider and contact-point interfaces were checked against the vendored Avian 0.7 source.

The dedicated server build succeeded. MEASURED in the completed ten-second observation:
**11 stats lines, 662 fixed ticks, zero cracked joints, zero separated joints and zero departed
chunks**. Every stats window contains fixed ticks. The unattended, headless, muted, loopback,
zero-bot server kept ticking and was terminated after observation. Counts are in
`target/bondsolve-s5-boot/counts.json`. This proves the authored-size placed-house spawn bar
and continued operation under gravity, not later response to changed loads or a performance price.

The first observer attempt stopped before launch because its freshness check compared binary
time with the build log's final write. The corrected observer checks the binary against all
current Rust sources; only the completed observation above is a runtime result.
No timing run, capture, external issue comment, push or visible binary was performed.

MEASURED by the retained all-asset census: fir_1 has 37 chunks and 132 joints; house_1 has
1,122 chunks and 3,258 joints. Both have zero initial overloads, cracks or departures, and are
Equilibrated with zero sparse rounds at their first authoritative law stage. These are observations,
not asset-specific pass conditions.

## Remaining ownership

The owner's three separately authored house and fir variants belong to other asset rows. This
session scatters the current house and fir at authored size. Roof framing, board-edge nailing,
directional timber and Unknown connection facts remain as recorded in `roof-s3.md`.
Pricing and play review belong to the orchestrator; this session proves counts and continued ticks.

Session start: 2026-11-05 17:42:47 local. All work and runs stayed in the leased bay.
Final gates completed at 18:35 local, within the 90-minute limit; no lane process remains running.
