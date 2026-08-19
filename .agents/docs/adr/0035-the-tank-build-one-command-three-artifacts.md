# 0035 — The tank build: one command, three artifacts

Status: ACCEPTED 2026-08-12. Amends ADR-0033 (packaging, seam granularity, transcription);
0033's doctrine — the global octave grid, the gates, certification order, projection math,
human-rate recompute — stands unchanged.

## Decision

One command per tank — the **build** (`scripts/tank/build.py`; the asset door becomes its
certification step). A `.blend` goes in; three shipped artifacts come out:

1. **`<id>.glb`** — the view artifact: scene, textures, and EVERY LOD level embedded.
   Rung 0 is not stored twice: it IS the meshes the scene nodes already reference; higher
   rungs are additional mesh records in the same file.
2. **`<id>.sim.glb`** — the sim artifact, a byte-strip of the certified view glb: LOD0
   geometry and material names (material = membership), no textures, no UVs, no rungs.
   The server loads only this; the client loads it for the ballistic walk — both sides
   walk identical accessor bytes by construction, not by convention.
3. **`<id>.lod.json`** — the certificate, five fields, nothing derivable:
   `blend_digest` (staleness vs current source+config), `view_glb_sha` + `sim_glb_sha`
   (trio coherence), `mesh_count` (coverage tripwire), and `chains`: per unique source
   primitive, a bounding `radius_m` and ordered `rungs[{mesh, deviation_mm}]`, deviations
   strictly ascending. NO metre distances ship — the runtime derives switch distances from
   certified deviation × the active view profile (vfov, viewport height px, pixel budget)
   and recomputes them only on human-rate view events (ADR-0033 §9, §11).

## Seam granularity: the glTF primitive

The render atom is the PRIMITIVE — Bevy spawns one entity per primitive — so chains key on
(mesh, primitive), and ADR-0033 §10's multi-primitive refusal is retired deliberately, not
lifted silently. Multi-material objects stay legal for artists. Shared meshes share chains:
dedup is by source-geometry digest (the Tiger's 8 road-wheel nodes consume ONE wheel chain).
Abutting primitives decimate independently; their seam can open by at most the sum of the
two certified deviations, which the budget already bounds at switch distance.

## Runtime

One `geometry_lod` module loads the certificate as data and spawns coincident
`VisibilityRange` siblings (the mechanism the track view proved). Nothing swaps at runtime;
Bevy selects per view. The track view becomes a consumer of the same module (identity and
MirrorX variants). `SHOE_LOD_CHAIN` and every hand-transcribed measurement in Rust are
DELETED — the certificate is the single seam ADR-0033 §8 demanded. Failure law mirrors map
loading (ADR-0011): missing, malformed, or hash-mismatched trio → panic in every build; a
chain absent from the certificate renders at source detail (benign; the build's own tests
own coverage). Authored scale is a lint error (float-dust in composed matrices stays a
warning; the exporter ships translation-only nodes), so the projection carries no scale
term. Shadows inherit observer-based range selection plus the caster-proxy policy;
no shadow-specific derivation exists.

AMENDED 2026-08-19 — the track's POOLED SHOES are the one consumer that swaps. 194 moving shoes
per tank made the coincident siblings the dominant per-frame cost (propagation, the visibility
sweep, the extract scan all charge for a hidden sibling of a moving parent), so a shoe is one
entity whose `Mesh3d` handle its BELT writes, selected per belt from the same certified
switch distances. What this gives up, deliberately: PER-VIEW rung selection. A `VisibilityRange`
is evaluated per view; a mesh handle cannot be, so every view — a second camera, a distinct
`ShadowLodOrigin` — draws the rung the one camera selected. Not exercised today (one `Camera3d`,
and the shoes stop casting under the shadow proxy's `PROXIED_CASTER`); a mirror or spotter camera
would inherit the near view's rung rather than its own. Scene primitives keep the siblings.

## Locality and the source/product boundary

The certificate is PER TANK — building tank #2 touches zero tiger files. Retired with this
ADR: the global `assets/lod_manifest.json`, the four `tiger_1_link.rungN.glb` sidecars, and
`--emit-rust`. Publication is staged: binaries first, certificate last, so an interrupted
publish fails hash-loud rather than pairing stale measurements with new bytes. Source
(`.blend`, `.tank.ron`) is tracked in git; products (`.glb`, `.sim.glb`, `.lod.json`) are
LFS today and a Steam depot when distribution demands it — the boundary is drawn now so
nothing restructures then.

## YAGNI ledger (deliberate absences)

No worker pools until a measured build hurts; no hot reload (regen = restart); no
per-camera band sets (one profile: the most demanding active view); no imposters/HLOD
(0033's parked list stands); no certificate schema version until schema #2 exists; no
source-only rows (`mesh_count` is the whole coverage check); no audit distances (tests
derive metres the same way the runtime does — recording both invites drift).
