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

AMENDED 2026-08-19 — the track's POOLED SHOES are the one consumer that swaps. 970 extra entities
per tank, all of them moving, charged propagation, the visibility sweep and the extract scan for
four hidden siblings per drawn shoe: MEASURED 2026-08-19 on an M-series Mac at 2560×1440, the
siblings cost 0.61 ms/frame of aggregate cross-thread `propagate_descendants` going from 2 to 30
tanks (~0.1–0.3 ms wall-clock). So a shoe is one entity whose `Mesh3d` handle its BELT writes,
selected per belt from the same certified switch distances, at the distance to the belt's NEAREST
SHOE — every other shoe is further off and earns a coarser-or-equal rung, so one rung for the side
is conservative by construction. It costs MEASURED +5.0 % belt triangles against a per-shoe
selection, integrated over 5–150 m and azimuth at 45°/1440p.

What this gives up, deliberately: PER-VIEW rung selection. A `VisibilityRange` is evaluated per
view; a mesh handle cannot be, so a belt makes ONE selection, and it is made FOR THE DECLARED
PLAYER VIEW (`view::PlayerView`) — the same one fact both LOD ladders take their projection from,
read here for its eye position. Which view a rung is for is a domain question, so it is answered by
a declaration and never by counting the cameras that happen to exist: a mirror, spotter or
render-to-texture camera is not a player view and re-tunes nothing. (The nearest-active-camera rule
this replaces, shipped 2026-08-19 and retired the same day, derived that domain fact from world
shape — so an overlay camera parked on a belt silently re-tuned it for a view no player has.) A
declaration that fails to resolve refuses loudly rather than holding its last rung; "no view yet" is
scheduling, not a value the selector encodes. Scene primitives keep the siblings.

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
per-camera band sets (one profile: the declared player view's); no imposters/HLOD
(0033's parked list stands); no certificate schema version until schema #2 exists; no
source-only rows (`mesh_count` is the whole coverage check); no audit distances (tests
derive metres the same way the runtime does — recording both invites drift).
