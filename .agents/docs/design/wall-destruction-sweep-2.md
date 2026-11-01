# Environment destruction sweep, part 2

Subject: the game's environment destruction and damage model, game state only.

The lane started at 12:47:08 +07 and reached its measured 90-minute cutoff at 14:17:08.
Base: `4d4dce84`; reviewed part 1: `edf7ef9b^..4d4dce84`. No subagents, captures,
pricing runs, pushes, deployments or external-service calls were used.

## Review of part 1

Standards and correctness: no P1 finding in the reviewed diff; three P2s fixed separately:

- R1: local turns admitted self-intersecting profiles. Every vertex must now lie in every
  edge's interior half-plane. Malformed-geometry refusals include the repair (`b4f09474`).
- R2: tree bindings were validated only on falling. Missing, duplicate and outside bindings
  are now refused at template construction with glb, extras field and repair (`33e1d0dd`).
- R3: invalid scales were refused in scatter without their source filename. The manifest
  parser now names the file, instance, scale axis and repair (`57da2770`).

The peer wall entry is outside process input at `net::walls::receive_ledger`, the first
map-aware receive gate. `world::plugin` spawns the map synchronously in Startup before the
client's asset-gated connection and Update receives. Admission checks happen there once;
downstream shape checks assert the internal record invariant. The structural/weight loop
assertions correctly require monotone crack progress. Removing the segmented-tree merge
fallback is correct: required shipped primitives must merge or refuse.

D1 disagreement: retain the owner-approved hole-cap crumbling path. No alternative was added.

## Physical findings

**Item 1: onset fixed, continued loose-body fracture remains open** (`9154c522`). The old
weight check used the worst joint's tensile ratio to open an entire section, including
compressed neighbours. Only individually overloaded joints now open; disconnected groups
are not static compression props. Material strengths were not tuned.

**MEASURED**, identical headless shipped-hull/house push: before, 1,003 chunks in 48 groups,
largest 664; after, 316 chunks in 59 groups, largest 24. The fixture uses the shipped hull
and house, normal driving/contact systems, ASSUMED initial speed 1.2 m/s and placement.
The roof gate now checks local fixing failure instead of requiring every coplanar fixing
to open. Its residual 74 of 150 roof chunks stand; the pass reports zero touched chunks
unassessed. This does not certify the residual load sharing. A full mixed-contact/fixing
static solve remains a model limit. Loose bodies still have no continued seam assessment:
uniform free fall alone supplies no relative gravity demand (DERIVED), but contact and
angular motion can load seams and require a further generic law.

**Item 2: fixed** (`dfb43316`). The game, sandbox and geometry tests now use one ray driver;
the sandbox also uses `run_laws` and shared exact-collider launches. The cased source is
explicit dev data. Filler and casing facts come together from that compiled round, replacing
an unrelated core lookup. Zero kinetic cavity energy does not mean zero pressure removal.

**MEASURED**, aimed solid-face cases through the shared path:

| Fixture | Standoff | Crushed volume | Rear scabs | Impulse |
|---|---:|---:|---:|---:|
| Cased HE, 0.907 kg | 0.10 m | 0.014013 m3 | 4 | 963.1 N s |
| Cased HE, 0.907 kg | 0.30 m | 0.003444 m3 | 1 | 939.0 N s |
| Cased HE, 0.907 kg | 2.00 m | 0 | 0 | 688.8 N s |
| Bare, 0.900 kg | 0.10 m | 0.010725 m3 | 4 | 2042.6 N s |

All four report zero kinetic cavity energy and zero cracks. Initial off-solid measurements
used a doorway point; the final fixture uses the sandbox's actual aimed solid face.

## Inventory dispositions

- D9-D10 fixed (`4ef94edf`): declared planks visual, palette lanes and repeats; required
  construction grain frames; explicit graybox face lanes/top tangent; no Wood-key or
  orientation fallback. Building asset format 5 was rebaked canonically. MEASURED topology
  remains 31 elements, 1,163 chunks, 3,185 joints and 81 anchors. ARM supplies metallic response.
- D12 and the retired Fir alternative fixed (`d04f19b5`): delete variant, constants,
  collider/view arms, obsolete arguments and the ignored comparison bed. Shipped trees remain.
- D11 left as instructed: terrain-pack schema/loader work is not a data-only fix.
- The three original P3s remain uncommitted: waiting presentation cursor before synchronization,
  checked replica segment conversion and typed tree-word errors. Their implementation passed
  CI clippy, but its targeted test build did not finish within the clock window.
- Two additional review P2s remain uncommitted: repeated traversal of a convex profile boundary,
  and a bound tree template with no standing meshes. Guards and regressions were prepared.

## Verification and handoff

All seven implementation commits are green. Formatting and CI clippy passed before each
batch. The largest completed pass: 10 canonical bake tests plus 395 relevant game tests,
including pen_cam, field_casts, headless_test, doc_citations, ui_ascii, net_boundary and bake.
Ignored measurement beds were not run. The final follow-up passed CI clippy but its test
compile was stopped at the clock cutoff, so none of that unverified source was committed.

The complete pending patch is `/tmp/sweep-destr-2/unverified-followup.patch`; logs are beside it.
The bay was restored to its verified source state. Apply and test that patch in a subsequent
leased lane. The owner still needs a follow-up for mixed load sharing and continued fracture
of loose groups; no arbitrary group-size cap or per-object exception was introduced.
No wire representation changed; PROTOCOL_REV was deliberately unchanged.
