"""The declared inputs of the LOD ladder — every number the generator is allowed to read.

ADR 0033 (`.agents/docs/adr/0033-geometry-mipmapping-declarative-lod.md`) is the doctrine, as
amended by ADR 0036 (`0036-the-generator-proves-less.md`); this file is their machine-readable
half. Nothing downstream may hardcode a threshold: `generate.py`, `chain.py` and the gates all read
from here, and every value lands in the manifest so a shipped chain says which constants produced
it.

WHY A CONFIG AND NOT CONSTANTS AT THE POINT OF USE. The right wall is the example that forced it.
It is a property of the WORLD (how far a camera can be from a surface), it will move when maps grow
toward 5 km, and the moment it is spelled twice the two copies disagree. One declaration, read by
generation, recorded in the manifest, re-derived by `chain.py` — and the manifest proves which wall
the shipped levels were cut for. The wall itself is not even declared here any more: it is PARSED
from the map manifest the game builds its grid from, so a map change is a regeneration and nothing
else, and `src/track/link_view.rs` fails the build if the two ever come apart.

NOTHING IN THIS FILE IS A SECOND COPY OF SOMETHING. The pins come from `scripts/toolchain.py`, the
world's size from `assets/maps/<id>/level.json`, and the map's id from `src/map.rs` — each read
where it lives rather than retyped here, because a config that mirrors other files is a config that
silently describes a tree that no longer exists.

THE LADDER IS GLOBAL, THE CHAINS ARE SPARSE. `E1_MM` and `OCTAVE` define one grid for the whole
game: e_N = E1_MM * OCTAVE^(N-1). A per-asset chain is a SUBSET of that grid — a rung whose best
candidate cannot shed `SKIP_FRACTION` of the previous kept level's triangles is not worth a file, a
draw-call switch or a manifest row, so it is skipped and the ladder continues at the next octave.
That is the whole of "no per-asset tuning": the targets are global, the triangle counts are
outputs, and the only per-asset freedom is WHICH rungs earned a level.
"""

import json
import math
import os
import re
import sys

sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))

import toolchain  # noqa: E402  — the path above is what makes this importable

# ── the tree ─────────────────────────────────────────────────────────────────────────────────────


def repo_root(start=None):
    """The git work-tree root, walked up from this file (or `start`)."""
    directory = os.path.dirname(os.path.abspath(start or __file__))
    while directory != os.path.dirname(directory):
        if os.path.exists(os.path.join(directory, ".git")):
            return directory
        directory = os.path.dirname(directory)
    raise RuntimeError("scripts/lod: not inside a git work tree")


# ── the ladder ───────────────────────────────────────────────────────────────────────────────────

#: The first lie, in millimetres — the one declared constant of the system (ADR 0033 §2).
#:
#: RATIFIED by Yan 2026-08-01 from the left-wall measurement of the reference asset: on the Tiger
#: track shoe a 3.89 mm target buys 831 triangles against a 1 661-triangle source, i.e. the smallest
#: deviation that still sheds about half the triangles. Below it the decimator emits near-copies and
#: the level is pure storage; above it the exact-world radius shrinks for no triangles back.
E1_MM = 3.89

#: The grid's ratio. 2 = octaves, which is the whole point of the mip analogy: each rung doubles the
#: allowed lie, doubles its switch distance and roughly halves its triangles.
OCTAVE = 2.0

#: A rung earns a level only if its best candidate sheds at least this fraction of the PREVIOUS KEPT
#: level's triangles. 0.30 is the declared floor: below it the new level costs a file, an LFS object,
#: a manifest row and a runtime switch to save less than a third of the geometry it replaces, and the
#: switch itself (a pop, however small) is not free. Measured on the reference asset this is what
#: drops rung 3 — 533 triangles against rung 2's 585 is an 8.9 % shed.
SKIP_FRACTION = 0.30

#: Hard stop on how deep a chain may go, so a pathological asset fails loudly instead of looping.
MAX_RUNGS = 12


# ── the right wall ───────────────────────────────────────────────────────────────────────────────

#: `DEFAULT_MAP_ID` as `src/map.rs` declares it — READ, not copied. The world this ladder is cut for
#: has to be the world the game loads, and a second spelling of the id here would pick a different
#: one in silence the day a map is renamed. A pattern that stops matching is a named refusal.
_DEFAULT_MAP_ID = re.compile(r'DEFAULT_MAP_ID:\s*&str\s*=\s*"([^"]+)"')


def default_map_id(root=None):
    """The map the game resolves when `OVERMATCH_MAP` names none — `crate::map::DEFAULT_MAP_ID`."""
    path = os.path.join(root or repo_root(), "src", "map.rs")
    with open(path, encoding="utf-8") as handle:
        found = _DEFAULT_MAP_ID.search(handle.read())
    if found is None:
        raise RuntimeError(
            f"scripts/lod: {path} declares no `DEFAULT_MAP_ID: &str = \"...\"` — the right wall is "
            f"cut from the map the game loads, and this is where that id lives"
        )
    return found.group(1)


def map_world_size_m(map_id=None, root=None):
    """The side of the square world `assets/maps/<id>/level.json` declares, in metres.

    The manifest is the SINGLE truth about a map's scale — `crate::map::parse` reads this same
    `terrain.heightmap.world_extent_xz` and hands its side to `TerrainExtent::world_size_m`, so a
    number derived here is the number the grid is built at rather than a claim about it. Square and
    origin-centred are the manifest's own requirements (`map::ExtentXz::side_m`), re-stated because
    a diagonal means nothing without them.
    """
    root = root or repo_root()
    path = os.path.join(root, "assets", "maps", map_id or default_map_id(root), "level.json")
    with open(path, encoding="utf-8") as handle:
        extent = json.load(handle)["terrain"]["heightmap"]["world_extent_xz"]
    (min_x, min_z), (max_x, max_z) = extent["minimum"], extent["maximum"]
    side = max_x - min_x
    if max_z - min_z != side or min_x != -max_x or min_z != -max_z or not side > 0.0:
        raise RuntimeError(
            f"scripts/lod: {path} declares world_extent_xz {min_x}..{max_x} by {min_z}..{max_z} — "
            f"the world is a positive square centred on the origin (map::ExtentXz::side_m)"
        )
    return float(side)


#: The id and the side the wall is cut from, resolved once at import so everything downstream reads
#: one answer and the manifest records which map produced it.
MAP_ID = default_map_id()
WORLD_SIZE_M = map_world_size_m(MAP_ID)

#: The maximum renderable camera-to-surface distance, in metres. Past it a level never renders, so
#: no rung beyond it is generated.
#:
#: PROVENANCE, and it is deliberately recorded in the manifest: this repo has NO far-plane, fog or
#: streaming-distance constant. `grep -rn "far\s*[:=]\|PerspectiveProjection\|DistanceFog" src/`
#: finds only cascade bounds and unrelated locals, and the camera never overrides bevy's default
#: projection. So the bound is the map's own geometry: the world is a square of side
#: `WORLD_SIZE_M`, and the farthest two points in it are the corners of that square — the DIAGONAL,
#: not the radius (ADR 0033 §3).
#:
#: NORTH STAR: maps grow toward 5 km, and this file no longer has to be edited when one does. The
#: side is PARSED from the shipped map manifest, which is what `crate::map` parses to build the
#: grid, so a map that grows moves this wall by itself and the deeper rungs appear on the next
#: regeneration because the stop rule reads this number. What still has to happen deliberately is
#: the regeneration: `chain.py` compares the manifest's `right_wall_m` against this value, so a
#: grown map fails verification until the corpus is re-cut, and `link_view`'s
#: `the_shipped_corpus_reaches_the_worlds_far_corner` fails the build against the live grid. If a
#: far plane or a fog cut-off is ever introduced it becomes the wall instead, and
#: `RIGHT_WALL_SOURCE` says so.
RIGHT_WALL_M = WORLD_SIZE_M * 2.0 ** 0.5
RIGHT_WALL_SOURCE = (
    "map diagonal: assets/maps/{}/level.json declares terrain.heightmap.world_extent_xz spanning "
    "{:g} m (the map is crate::map::DEFAULT_MAP_ID, src/map.rs) x sqrt(2). "
    "No far-plane, fog or streaming-distance constant exists in src/ as of this generation."
).format(MAP_ID, WORLD_SIZE_M)


# ── the reference view ───────────────────────────────────────────────────────────────────────────

#: The view every switch distance is quoted in: the Tiger's gunner optic, at a 1-pixel budget.
#:
#: `vfov_rad` is the authored optic FOV (`assets/tiger_1/tiger_1.tank.ron:326`, mirrored by
#: `GUNNER_FOV_FALLBACK` in `src/camera.rs:232`). `height_px` is the reference display height.
#: `budget_px` = 1 is the honest top rung of the pixel-budget setting: sub-pixel means positionally
#: indistinguishable from the source.
#:
#: THE PROJECTION IS EXACT, NOT SMALL-ANGLE (ADR 0033 §9):
#:
#:     D = dev_m * height_px / (2 * tan(vfov/2) * budget_px)
#:
#: The `D = dev_m * height_px / (vfov * budget_px)` shortcut is 5.5 % wrong at the commander FOV.
#: At the optic the two agree to 0.06 %, which is exactly why the shortcut survived unnoticed in a
#: hand-written ledger until a wider view was quoted through it.
REFERENCE_VIEW = {
    "name": "gunner optic",
    "vfov_rad": 0.12,
    "height_px": 2160.0,
    "budget_px": 1.0,
    "provenance": "assets/tiger_1/tiger_1.tank.ron:326 (Gunner fov: 0.12); src/camera.rs:232",
}


def switch_distance_m(deviation_mm, radius_m=0.0, view=None):
    """Metres beyond which `deviation_mm` is under budget in `view`, plus a bounding-radius slack.

    `radius_m` is the asset's bounding radius, added because bevy's `VisibilityRange` measures to the
    entity ORIGIN while the guarantee is about the SURFACE: the near face of the asset is up to one
    radius closer than the origin the runtime tested. Conservative by construction, and 0.38 m on a
    track shoe — it matters for a hull, not for this.
    """
    view = view or REFERENCE_VIEW
    denominator = 2.0 * math.tan(float(view["vfov_rad"]) / 2.0) * float(view["budget_px"])
    return (deviation_mm / 1000.0) * float(view["height_px"]) / denominator + radius_m


# ── the gates ────────────────────────────────────────────────────────────────────────────────────

#: Every gate is numeric and every one runs on the DECODED SHIPPED GLB (ADR 0033 §6, ADR 0036 §4).
#:
#: THE LIST IS WHAT THE MEASUREMENT NEEDS AND NOTHING ELSE. ADR 0036 §4 re-scoped it: finite
#: attributes, non-degenerate, non-empty, plus the component-survival gate that deviation cannot
#: see. The UV-area and tangent counters served the rendered-difference gate, which is deleted, and
#: manifoldness is the armour pipeline's law rather than this lane's — the deviation bound polices
#: decimator misbehaviour by construction, because a mangled region deviates and fails while an
#: undeviating one is invisible by definition. Every counter below is still MEASURED and recorded
#: per level (and re-derived from the shipped bytes by `chain.py --verify`); what changed is which
#: of them refuse a level.
GATES = {
    # Branch-and-bound bracket on the certified worst-case deviation. Acceptance is always on the
    # UPPER bound, so a loose bracket costs triangles and never honesty. `tol_m` is the absolute floor
    # of the bracket; the RELATIVE tolerance is what makes it affordable, because the cost of
    # closing an absolute bracket scales inversely with the answer (proving 0.05 mm +/- 0.02 mm
    # needs a quarter-million patches; proving 3.9 mm +/- 0.04 mm needs a few thousand).
    #
    # THERE IS NO SEARCH TOLERANCE ANY MORE. The search asks a BOOLEAN (`measure.fits_target`) with
    # a node budget instead of bracketing a number nobody reads; only certification runs a bracket,
    # because THAT number is what the manifest records and what the switch distance is derived from.
    "deviation_tol_m": 2.0e-5,
    "deviation_rel_tol_certify": 0.01,
    "deviation_max_nodes_certify": 1_500_000,
    # What the cleanup pass dissolves, as a fraction of the mesh's bounding diagonal. 7.7 um on the
    # reference shoe: enough to take out the 4.7 um NEEDLE the decimator inherits from the source,
    # and well clear of the next-thinnest features at 21 um. Three orders of magnitude below e1, so
    # it cannot move a level's deviation anywhere the ladder can express — and the deviation is
    # re-certified afterwards regardless.
    #
    # IT IS ALSO THE DEGENERACY FLOOR, squared. `measure.validity_gate_failures` refuses a face
    # whose area is at or under `(this * diagonal)**2` — both extents at the scale the lane already
    # treats as one point, so no consistent normal. One declaration, two consequences, rather than a
    # second number that would drift from this one. On the reference shoe that floor is 5.9e-5 mm^2
    # against a shipped minimum of 1.425 mm^2, which is the clearance a gate on EXACT degeneracy
    # should have — slivers are cleanup's business, not this gate's.
    "cleanup_dissolve_frac_of_diag": 1.0e-5,
    # Finite, and non-degenerate in topology: a face listed twice, a NaN, and an edge its two faces
    # traverse the same way (an inconsistent winding, which has no outside).
    "max_duplicate_faces": 0,
    "max_nonfinite": 0,
    "max_orientation_flips": 0,
    # Non-empty. A collapse that reaches zero triangles is not a level, and every counter below it
    # would be a statement about nothing.
    "max_empty_surfaces": 0,
    # A vanished small part has a near-zero Hausdorff distance, so deviation cannot see it. Counted
    # by connected component after welding coincident positions.
    "components_must_match": True,
}

#: Samples for the shading-normal-angle DIAGNOSTIC, which is recorded per level and gates nothing
#: (ADR 0033 §7, ADR 0036 §3). It used to live inside the render gate's config block; the gate is
#: gone and the diagnostic outlived it.
NORMAL_DIAGNOSTIC_SAMPLES = 20_000

# ── the assets ───────────────────────────────────────────────────────────────────────────────────

#: One row per source mesh that gets a chain. `blend` is READ-ONLY input and is never saved.
#:
#: `l0_glb` / `l0_node` name where the SOURCE ships — L0 is not generated, it is the artist's mesh,
#: and for the track shoe that is the `Link` node inside the tank glb. The generator certifies those
#: bytes against the evaluated source too, which is how "the surface that ships is the surface that
#: anchors deviation" stops being a claim.
#:
#: `stem` names the generated files: `<stem>.rung<N>.glb`, N being the OCTAVE RUNG, not a chain
#: index — so a level's filename says what lie it tells, and a chain that gains a rung does not
#: renumber the ones beside it.
ASSETS = (
    {
        "name": "tiger_1_link",
        "blend": "assets/tiger_1/tiger_1.blend",
        "object": "Link",
        "l0_glb": "assets/tiger_1/tiger_1.glb",
        "l0_node": "Link",
        "stem": "assets/tiger_1/tiger_1_link",
    },
)

#: Bumped whenever the generation ALGORITHM changes in a way that can move a shipped level. The
#: manifest records it; a mismatch between a committed manifest and this constant means the chain
#: was cut by a different pipeline than the one in the tree.
GENERATOR_VERSION = "3.0.0"

#: The toolchain the corpus was cut with, ASSERTED before generation rather than merely recorded.
#:
#: A decimator is a program, not a specification: Blender's collapse can change its tie-breaking in
#: a point release and quietly move every level in the game. Recording the version after the fact
#: tells you which build produced a chain; asserting it first is what stops two machines producing
#: two different corpora and nobody noticing. Set `OVERMATCH_LOD_ALLOW_BLENDER` to override for a
#: deliberate upgrade — which is a corpus regeneration review, per ADR 0033's "Simplifier choice".
#: ALL THREE ARE ASSERTED, because a version string is the weakest of the three. Two Blenders can
#: report 5.1.2 and be different builds; the glTF exporter is an add-on that moves on its own
#: schedule and decides the bytes; and double generation inside ONE process cannot see either kind
#: of difference, which is precisely the gap it looks like it closes.
#:
#: THE VALUES ARE `scripts/toolchain.py`'S — this lane names them, it does not declare them. Two
#: copies of a pin is two pins, and the equality between them was held by a test whose only job was
#: to notice them drifting; a reference cannot drift, so that test is gone.
EXPECTED_BLENDER = toolchain.BLENDER_VERSION
EXPECTED_BLENDER_BUILD = toolchain.BLENDER_BUILD
EXPECTED_GLTF_EXPORTER = toolchain.GLTF_EXPORTER_VERSION
BLENDER_OVERRIDE_ENV = "OVERMATCH_LOD_ALLOW_BLENDER"

#: The sources that PRODUCE a corpus, hashed into the manifest. `GENERATOR_VERSION` is a promise a
#: human remembers to keep; this is the thing that actually changed. A manifest whose source digest
#: does not match the tree was cut by code that is no longer here, whatever the version string says.
#:
#: THE LINE IS "CAN THIS CHANGE WHAT A MANIFEST SAYS", not "is this the generator". `manifest.py`
#: is in the list because it derives every switch distance and merges targeted regenerations — the
#: generator calls into it. `chain.py` is out because it can only change how a manifest is CHECKED,
#: and forcing a twelve-minute Blender run to reword a failure message is the kind of friction that
#: gets a check switched off rather than kept.
#:
#: That line was drawn in the wrong place once: chain.py was excluded while the generator imported
#: it and called its merge, so verifier logic really was participating in production and the
#: exclusion was unsound. The fix was to split the module, not to keep arguing for the exclusion.
#:
#: `../toolchain.py` is in the list for exactly that rule, and it joined it the moment the pins
#: stopped being copied into this file: `EXPECTED_BLENDER`, `EXPECTED_BLENDER_BUILD` and
#: `EXPECTED_GLTF_EXPORTER` are recorded in every manifest's `generator` block, so the file that
#: declares them can change what a manifest says. Names are relative to THIS directory and are
#: hashed alongside the bytes, so the path is part of the digest.
GENERATOR_SOURCES = (
    "config.py", "measure.py", "generate.py", "manifest.py", "../toolchain.py",
)


def generator_digest():
    """sha256 over the generator's sources, in a fixed order."""
    import hashlib

    here = os.path.dirname(os.path.abspath(__file__))
    digest = hashlib.sha256()
    for name in GENERATOR_SOURCES:
        with open(os.path.join(here, name), "rb") as handle:
            digest.update(name.encode())
            digest.update(handle.read())
    return digest.hexdigest()

#: Where the manifest lands. Committed; the single seam between generation and runtime.
MANIFEST_RELPATH = "assets/lod_manifest.json"

#: The manifest format this tree reads and writes. Bumped when the SHAPE changes, not the contents.
SCHEMA_VERSION = 2


# ── the search's budget ──────────────────────────────────────────────────────────────────────────

#: What ONE direction of one rung verdict may spend, as a node count. NEVER a clock: the corpus is
#: a function of the geometry, and a wall-clock budget would make it a function of the machine.
#:
#: SCALE-FREE, and the shape is measured rather than chosen (ADR 0036 §6 and its recorded fork).
#: `.agents/scratch/lod-directed-prototype-2026-08-12.md` §2 timed what a PROVEN_PASS costs on the
#: shipped levels, whose verdicts are known-good by construction, and the cost falls as the SQUARE
#: of the target: 308 111 nodes at e = 3.89 mm, 19 683 at 15.56 mm, 4 772 at 31.12 mm, 83 at
#: 124.48 mm. That is the covering-radius bound's own geometry — proving a maximum is under `e`
#: forces every patch on the WHOLE SURFACE below `e`'s length scale — so the price of an acceptance
#: is a property of the mesh and the rung, not of the candidate:
#:
#:     nodes_to_accept ~= 7.9 * (bbox_diagonal_mm / e_target_mm) ** 2
#:
#: A FLAT budget therefore makes fine-rung availability a function of mesh SIZE: measured, the
#: 769 mm link kept all its rungs at 400 000 while the 3 817 mm `Turret_Decor` lost its two finest,
#: because it needs ~7.6 M for the same proof. The constraint this coefficient has to satisfy is
#: that a rung an unbounded search would accept is not lost for want of budget; 10.3 is the fitted
#: 7.9 with the 30 % headroom the prototype's own budget carried, and it reproduces 400 000 for the
#: link at rung 1 (769.0 / 3.890 = 197.7; 197.7^2 * 10.3 = 402 600).
VERDICT_NODES_PER_SQUARE = 10.3

#: The hard cap, per direction. An UNDECIDED verdict costs the FULL budget in both directions and
#: buys nothing — it is the old frozen bracket, bounded — so an uncapped scale-free budget lets one
#: pathological mesh spend the corpus's whole time allowance proving nothing.
#:
#: THE CONSTRAINT: a full-tank cold build stays inside the minutes budget ADR 0036 §6 gates on. At
#: the MEASURED ~50 us a node, 2 000 000 nodes is ~100 s of worst-case burn per direction, and the
#: census (58 unique meshes) cannot afford many of those. A rung whose acceptance needs more than
#: this is LOST, honestly: UNDECIDED counts as FAIL, so the chain is coarser and nothing shipped is
#: unproven. Raising it buys fine rungs on large meshes at a linear price in wall time.
#:
#: IT IS NEVER LOWERED TO MAKE A TIME GATE GREEN. A low cap drops fine rungs while every remaining
#: level stays honestly certified, so the corpus quietly loses fidelity and every check still
#: passes — the one failure mode this file exists to prevent. A projection that misses the ruling
#: is a stop-and-raise, not a smaller number here. Every rung lost this way is recorded in the
#: manifest as `lost_to: verdict_node_budget`, so the trade is legible rather than inferred.
VERDICT_NODES_CAP = 2_000_000


def diagonal_mm_from_bbox(bbox_mm):
    """The bounding diagonal the node budget is computed from — from the RECORDED box.

    ONE SOURCE OF THE NUMBER, and the reason is a split brain that was live: generation computed the
    budget from the evaluated source's full-precision diagonal, verification recomputed it from the
    manifest's box rounded to four decimals, and then demanded exact integer equality. The two agree
    on this asset by luck. `verdict_node_budget` truncates, so a mesh whose budget landed a
    hair under an integer would be granted N by one side and N-1 by the other, and a perfectly valid
    manifest would fail verification on a number nobody had touched.

    So both sides compute from the box AS RECORDED. The rounding is then part of the declared law
    rather than a difference between two readings of it, and equality holds by construction instead
    of by coincidence.
    """
    return math.sqrt(sum(float(value) * float(value) for value in bbox_mm))


def verdict_node_budget(diagonal_mm, e_target_mm):
    """The node budget for one direction of one verdict, from the mesh's size and the rung.

    Integer and deterministic: the same asset gives the same budget on any machine, which is what
    lets two runs be compared field-for-field. Feed it `diagonal_mm_from_bbox` of the geometry the
    MANIFEST records, never a fuller-precision diagonal beside it.
    """
    ratio = float(diagonal_mm) / float(e_target_mm)
    return int(min(VERDICT_NODES_CAP, VERDICT_NODES_PER_SQUARE * ratio * ratio))


def resolve_source(root, relpath):
    """Where the source .blend actually is, given a work tree that may not hold its bytes.

    The .blend is tracked through LFS, so an ordinary checkout has it; a tree whose LFS objects
    were never fetched has a pointer file instead, and generation run from one would otherwise fail
    with "the .blend does not exist", which is true and useless.

    The order is: this work tree, then the MAIN work tree (read out of the `gitdir:` pointer a
    linked worktree's `.git` file carries), then `$OVERMATCH_ASSET_ROOT`. Read-only in every case,
    and the path RECORDED in the manifest is always the repo-relative one, so a manifest cut in a
    worktree is identical to one cut in the main checkout.
    """
    candidates = [os.path.join(root, relpath)]
    marker = os.path.join(root, ".git")
    if os.path.isfile(marker):
        with open(marker, encoding="utf-8") as handle:
            pointer = handle.read().strip()
        if pointer.startswith("gitdir:"):
            gitdir = pointer.split(":", 1)[1].strip()
            # .../<main>/.git/worktrees/<name> -> <main>
            main = os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(gitdir))))
            candidates.append(os.path.join(main, relpath))
    override = os.environ.get("OVERMATCH_ASSET_ROOT")
    if override:
        candidates.append(os.path.join(override, relpath))
    for candidate in candidates:
        if os.path.isfile(candidate):
            return candidate
    raise FileNotFoundError(
        f"scripts/lod: {relpath} is untracked and was not found in any of: "
        + ", ".join(candidates)
        + ". Set OVERMATCH_ASSET_ROOT to the checkout that holds it."
    )


def rungs():
    """The global octave grid, as (rung_index, e_target_mm) pairs, up to `MAX_RUNGS`."""
    return tuple((n, E1_MM * OCTAVE ** (n - 1)) for n in range(1, MAX_RUNGS + 1))
