"""The declared inputs of the LOD ladder — every number the generator is allowed to read.

ADR 0033 (`.agents/docs/adr/0033-geometry-mipmapping-declarative-lod.md`) is the doctrine; this
file is its machine-readable half. Nothing downstream may hardcode a threshold: `generate.py`,
`chain.py` and the gates all read from here, and every value lands in the manifest so a shipped
chain says which constants produced it.

WHY A CONFIG AND NOT CONSTANTS AT THE POINT OF USE. The right wall is the example that forced it.
It is a property of the WORLD (how far a camera can be from a surface), it will move when maps grow
toward 5 km, and the moment it is spelled twice the two copies disagree. One declaration, read by
generation, recorded in the manifest, re-derived by `chain.py` — a map change is a one-line edit
followed by a regeneration, and the manifest proves which wall the shipped levels were cut for.

THE LADDER IS GLOBAL, THE CHAINS ARE SPARSE. `E1_MM` and `OCTAVE` define one grid for the whole
game: e_N = E1_MM * OCTAVE^(N-1). A per-asset chain is a SUBSET of that grid — a rung whose best
candidate cannot shed `SKIP_FRACTION` of the previous kept level's triangles is not worth a file, a
draw-call switch or a manifest row, so it is skipped and the ladder continues at the next octave.
That is the whole of "no per-asset tuning": the targets are global, the triangle counts are
outputs, and the only per-asset freedom is WHICH rungs earned a level.
"""

import math
import os

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

#: The maximum renderable camera-to-surface distance, in metres. Past it a level never renders, so
#: no rung beyond it is generated.
#:
#: PROVENANCE, and it is deliberately recorded in the manifest: this repo has NO far-plane, fog or
#: streaming-distance constant. `grep -rn "far\s*[:=]\|PerspectiveProjection\|DistanceFog" src/`
#: finds only cascade bounds and unrelated locals, and the camera never overrides bevy's default
#: projection. So the bound is the map's own geometry: the world is a square of side
#: `WORLD_SIZE = 1000.0` m (`src/terrain_grid.rs:53`), and the farthest two points in it are the
#: corners of that square — the DIAGONAL, not the radius (ADR 0033 §3).
#:
#: NORTH STAR: maps grow toward 5 km. When `WORLD_SIZE` moves, edit these two lines and regenerate;
#: the deeper rungs appear on their own because the stop rule reads this number. If a far plane or a
#: fog cut-off is ever introduced it becomes the wall instead, and `RIGHT_WALL_SOURCE` says so.
WORLD_SIZE_M = 1000.0
RIGHT_WALL_M = WORLD_SIZE_M * 2.0 ** 0.5
RIGHT_WALL_SOURCE = (
    "map diagonal: WORLD_SIZE = 1000.0 m (src/terrain_grid.rs:53) x sqrt(2). "
    "No far-plane, fog or streaming-distance constant exists in src/ as of this generation."
)


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

#: Every gate is numeric and every one runs on the DECODED SHIPPED GLB (ADR 0033 §6/§7).
GATES = {
    # Branch-and-bound bracket on the certified worst-case deviation. Acceptance is always on the
    # UPPER bound, so a loose bracket costs triangles and never honesty. `tol_m` is the absolute floor
    # of the bracket; the RELATIVE tolerances are what make it affordable, because the cost of
    # closing an absolute bracket scales inversely with the answer (proving 0.05 mm +/- 0.02 mm
    # needs a quarter-million patches; proving 3.9 mm +/- 0.04 mm needs a few thousand).
    #
    # The search additionally stops the moment the rung's question is answered — upper bound under
    # the target accepts, a sampled point over it rejects — so its brackets are decisive rather
    # than tight. Certification re-runs at the tighter tolerance, because THAT number is what the
    # manifest records and what the switch distance is derived from.
    "deviation_tol_m": 2.0e-5,
    "deviation_rel_tol_search": 0.05,
    "deviation_rel_tol_certify": 0.01,
    "deviation_max_nodes_search": 400_000,
    "deviation_max_nodes_certify": 1_500_000,
    # SLIVER FLOOR, and it is anchored to the source rather than declared out of thin air.
    #
    # A collapsed vertex pair leaves a needle: ~zero area, an interpolated normal that is numeric
    # noise, and a UV triangle that may collapse with it and default the tangent. But "how thin is
    # too thin" is a property of the asset — the reference shoe's own thinnest triangle is 4.7 um
    # on a 768 mm diagonal, and a floor above that would condemn the artist's mesh. A floor the
    # SOURCE fails is not a floor, it is a bug (this exact number was caught by that check on the
    # first run, which is why the rule is written down here).
    #
    # So the effective floor is `max(frac_of_diag * diagonal, source_min_altitude / margin)`:
    #   * the fraction is the absolute scale-aware bound — a triangle under a micron on a
    #     three-quarter-metre part has no interior at any resolution anyone renders at,
    #   * the margin says a generated level may be a little thinner than the source and no more,
    #     which is the actual defect this gate is for.
    "min_altitude_frac_of_diag": 1.0e-6,
    "sliver_margin_vs_source": 4.0,
    # A face whose UV triangle has (near) zero area cannot produce a tangent: mikktspace divides by
    # that area and bevy hands the shader a defaulted one. ZERO tolerated, on the shipped bytes.
    "uv_area_eps": 1.0e-12,
    "max_tangent_default_faces": 0,
    "max_tangent_default_verts": 0,
    # Structural: duplicate faces, non-finite attributes, orientation flips across a shared edge,
    # and edges with more than two faces on them. The last one was measured and REPORTED for a
    # while before it was enforced, which is its own lesson: a counter nothing compares against is
    # decoration. A non-manifold edge has no consistent normal or tangent frame, and the ballistics
    # bake treats a non-watertight volume as zero armour, silently.
    "max_duplicate_faces": 0,
    "max_nonfinite": 0,
    "max_orientation_flips": 0,
    "max_nonmanifold_edges": 0,
    # A vanished small part has a near-zero Hausdorff distance, so deviation cannot see it. Counted
    # by connected component after welding coincident positions.
    "components_must_match": True,
}

#: The rendered-difference gate — the authoritative attribute check (ADR 0033 §7).
#:
#: Each kept level is rendered against its PARENT at the parent->child switch distance, under the
#: asset's material, and the two images are differenced. (The material is taken from the shipped
#: glb when it can be read back, and from the .blend when it cannot — see `RENDER_GATE_BLOCKING`;
#: which one was used is recorded per level as `material_source` and is checked by verification.)
#: Pixels are counted over the
#: FOOTPRINT (the union of the two silhouettes plus a dilation), never over the whole frame: at
#: 300 m a track shoe is 40 pixels across and a frame-wide mean would divide every difference by
#: four thousand empty pixels and pass anything.
#:
#: The camera preserves the reference view's ANGULAR resolution (pixels per radian) rather than its
#: pixel count, so a 512-pixel tile carries exactly the same detail per steradian as the 2160-pixel
#: reference display — the difference measured is the difference a player's pixel sees.
RENDER_GATE = {
    "tile_px": 512,
    # Render at `tile_px * supersample` and box-average down to `tile_px` before differencing.
    #
    # A PLAYER'S PIXEL INTEGRATES; SO MUST THE GATE. One render sample-pixel per player pixel makes
    # the comparison an aliasing contest — a coarse level's edges land on a different side of the
    # sample grid and the difference reports geometry that both meshes agree about. Averaging 4x4
    # sub-pixels reproduces what the display actually shows, and it does two other useful things: it
    # divides the Monte-Carlo noise floor by 4, and it gives a small far-distance asset (13 pixels
    # across at the last switch) an interior to measure shading in at all.
    "supersample": 4,
    "samples": 64,  # x16 sub-pixels = 1024 effective samples per player pixel
    "seed": 20260801,
    # Views, as (name, elevation_deg, azimuth_deg). Three-quarter and grazing at minimum: grazing is
    # where a collapsed silhouette shows and where specular skims a wrongly-oriented facet.
    "views": (
        ("three_quarter", 28.0, 52.0),
        ("grazing", 9.0, 118.0),
        ("edge_on", 2.0, 200.0),
    ),
    # THE VERDICT IS A POSITION BETWEEN TWO MEASURED REFERENCES, not an absolute pixel number.
    #
    # Three runs of this gate were needed to learn why. An absolute budget ("at most 2 % of pixels
    # may change by 0.1") failed every level of a chain whose positional deviation was PROVEN
    # sub-pixel — because merging flat-shaded facets changes their shading, which is the mechanism
    # working, not failing, and because nobody can say what the right absolute number is anyway.
    #
    # So each pair is bracketed. The NOISE FLOOR is the renderer disagreeing with itself on
    # identical geometry (two seeds). The DEFECT FLOOR is the same level with every shading normal
    # rotated by `defect_normal_deg` — positionally perfect, lit wrong, which is precisely the
    # red-test class this gate exists to catch (a defaulted tangent draws exactly that). The score
    #
    #     (signal - noise) / (defect - noise)
    #
    # is 0 when a switch is as invisible as re-rendering the same frame and 1 when it looks as wrong
    # as broken normals. `defect_fraction` is how far along that line a switch may land. Being
    # dimensionless, it does not drift when the sample count, tile size or machine changes.
    #
    # 20 deg is chosen as the defect because it is the scale at which a wrong normal is unarguably
    # a bug rather than a shading nuance, and it is well inside what a dropped custom-normal layer
    # produces. 0.5 says a shipped switch must be closer to correct than to broken.
    #
    # `defect_fraction` IS NOT RATIFIED, and until it is this gate reports instead of blocking —
    # see `RENDER_GATE_BLOCKING` below. Everything else in this file is either measured (e1, the
    # right wall) or structural (a duplicate face is a defect at any threshold). "How much may a
    # switch be allowed to look different" is neither: it is a taste call about the game's own
    # assets, of exactly the kind e1 was, and e1 was ratified by Yan rather than picked here.
    #
    # THE MEASUREMENT THAT WOULD INFORM THAT RULING is `RATIFICATION_EVIDENCE` below, which is a
    # CONSTANT rather than a comment for a reason: transcribed into prose it went stale twice, and
    # both times it sat next to the decision it was supposed to inform, reading as current. A test
    # now compares it against the shipped manifest, so it cannot rot a third time.
    "defect_normal_deg": 20.0,
    "defect_fraction": 0.50,
    # Absolute floors, kept only as an escape hatch: a pair whose difference is under these passes
    # regardless of the bracket, so a degenerate defect reference cannot fail an invisible switch.
    "max_mean_abs_diff": 0.020,        # mean |dI| over the interior, 0..1
    "max_footprint_frac_over": 0.020,  # fraction of interior pixels differing by more than...
    "over_threshold": 0.100,           # ...this, 0..1
    # Diagnostic only, never a gate (ADR 0033 §7): worst shading-normal angle.
    "normal_samples": 20_000,
}


#: Does a failing rendered-difference gate BLOCK publication, or only report?
#:
#: False until `RENDER_GATE["defect_fraction"]` is ratified. The gate always runs, always measures,
#: always records every number per level in the manifest, and always prints its verdict; what this
#: switch changes is whether a verdict of FAIL aborts generation. It is a deliberate, named,
#: one-line piece of state rather than a threshold quietly loosened until the corpus went green —
#: which is the exact failure mode this whole file exists to prevent. Every other gate here blocks
#: unconditionally, because every other gate is measured or structural.
#:
#: TWO THINGS MUST BE TRUE BEFORE IT CAN BE FLIPPED, and both are enforced rather than remembered:
#:
#:   1. Yan rules on `defect_fraction`. It is a taste call of exactly the kind e1 was.
#:   2. The gate must render the SHIPPED material. It does not today: it re-imports the L0 glb, and
#:      on this asset every texture comes back empty because the mip bake writes KTX2 via
#:      `KHR_texture_basisu` and Blender's importer cannot read it, so the gate falls back to the
#:      .blend material and records that per level. Blocking on a number measured under the wrong
#:      textures would be a gate certifying the wrong thing — the failure this file exists to
#:      prevent. `chain.py` REFUSES a blocking manifest whose levels record a fallback, so flipping
#:      this flag without fixing the material fails verification instead of shipping a false pass.
#:
#: The route for (2), when it matters: decode the KTX2 to PNG with `basisu` and rebuild the material
#: from the decoded images, or render through the runtime instead of Blender.
RENDER_GATE_BLOCKING = False

#: The measurements that would inform that ruling, from the reference asset — worst of three
#: viewpoints, so each is the least favourable angle.
#:
#: A CONSTANT, NOT A COMMENT, because as prose it went stale twice: it kept quoting scores from a
#: superseded defect reference and triangle counts from before a regeneration, sitting next to the
#: decision it exists to inform and reading as current the whole time. Stale evidence beside a
#: pending decision is worse than no evidence. `test_chain.py` compares every number here against
#: the shipped manifest, so a regeneration that moves any of them fails until this is refreshed.
#:
#: L3 is the one to look at: at 501 m the switch changes the image MORE than 20-degree broken
#: normals do on the same geometry. Beyond a few hundred metres shading stops being what changes and
#: silhouette — already proven sub-pixel — is the whole story, so a ratified rule probably wants a
#: minimum resolvable footprint below which this gate abstains and says so.
RATIFICATION_EVIDENCE = {
    "levels": (
        # (level, tris, switch_m, defect_score, verdict)
        (1, 855, 55.9, 0.486600, "PASS"),
        (2, 581, 126.6, 0.619408, "FAIL"),
        (3, 315, 501.0, 1.673924, "FAIL"),
        (4, 194, 1049.9, 0.729534, "FAIL"),
    ),
    "enumerated_outputs": 736,
    "skipped_rung": (3, 533, 581, 0.0826),  # rung, best tris, incumbent tris, shed fraction
}


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
GENERATOR_VERSION = "2.2.0"

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
EXPECTED_BLENDER = "5.1.2"
EXPECTED_BLENDER_BUILD = "ec6e62d40fa9"
EXPECTED_GLTF_EXPORTER = "5.1.20"
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
GENERATOR_SOURCES = ("config.py", "measure.py", "generate.py", "render_gate.py", "manifest.py")


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
SCHEMA_VERSION = 1

#: What the exhaustive search is allowed to spend before it refuses.
#:
#: EXHAUSTIVE IS A PROMISE ABOUT COMPLETENESS, NOT ABOUT COST. Every enumerated output stays
#: resident for the whole chain so that a plateau costs one certification rather than many, which
#: means geometry storage grows with the number of distinct outputs — and that grows with the
#: asset. A 1 661-triangle shoe enumerates ~700 outputs; something twenty times larger would not
#: merely be slower, it would be quadratically hungrier, and the first anyone would know is a
#: machine swapping. So the limits are declared, and passing one is a NAMED REFUSAL rather than a
#: slow death: raising them is a decision someone makes on purpose, having read this.
SEARCH_LIMITS = {
    "max_enumerated_outputs": 4_000,
    "max_retained_tris": 4_000_000,
    "max_enumeration_seconds": 900.0,
    # Random budgets drawn from the intervals the walk jumped over, each of which must return THAT
    # INTERVAL'S incumbent. Together with the idempotence check over every enumerated output (see
    # `measure._verify_oracle`) this is what holds the decimator to its contract on real geometry.
    # Accepting mere membership in the output set — the first version of this guard — is unsound:
    # an oracle that answers B-1 to everything passes any number of such probes while enumerating
    # only half the staircase.
    "max_enumeration_spot_checks": 48,
    "spot_check_seed": 20260802,
}


def repo_root(start=None):
    """The git work-tree root, walked up from this file (or `start`)."""
    directory = os.path.dirname(os.path.abspath(start or __file__))
    while directory != os.path.dirname(directory):
        if os.path.exists(os.path.join(directory, ".git")):
            return directory
        directory = os.path.dirname(directory)
    raise RuntimeError("scripts/lod: not inside a git work tree")


def resolve_source(root, relpath):
    """Where the (UNTRACKED) source .blend actually is, given a work tree that may not hold it.

    The .blend is deliberately not in git — it is 145 MB of authoring state with its own backups —
    so a `git worktree` checkout has every tracked asset and no source at all. Generation run from
    one would otherwise fail with "the .blend does not exist", which is true and useless.

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
