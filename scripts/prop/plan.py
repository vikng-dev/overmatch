"""The tree's plan: every number the generator builds geometry from, as pure functions.

No `bpy` here, so the sizing laws are tested under the system interpreter and the generator only
turns the plan into meshes. A tree is fixed by (species row, seed): `sample` draws its height and
breast-height diameter from the species' ranges, `segments` stacks the trunk, `whorls` places the
canopy's branches. Every draw comes from one `random.Random(seed)`, consumed in a fixed order, so
the plan is a function of its inputs.

The trunk's construction (`bands`, `construction_ron`) is the common cutter's input: the trunk's
rings stacked as bands, each a frustum and its own convex piece, one component of one wood, cut
into grain-stretched cells the species row authors (`tools/src/construction/fracture.rs`). Its
rings and their `RING_SIDES` corners are the rows and the section the mesh is built from, so the
construction and the view are one source.

Units: metres and radians throughout. Blender authors Z up; the glTF export turns that into +Y up.
"""

from __future__ import annotations

import math
import random
from typing import Dict, List

#: Trunk segment length band, metres: the view's ring stack and the segment tree runtime's
#: (`src/trees.rs`) segments.
SEGMENT_MIN_M = 1.5
SEGMENT_MAX_M = 2.0
SEGMENT_TARGET_M = 1.75

#: Breast height, metres: where `dbh_m` is measured (forestry convention).
BREAST_HEIGHT_M = 1.3

#: Taper exponent of the trunk radius law `r(z) = r_bh * ((H - z) / (H - 1.3))^k`: 1 is a cone
#: and 0 a cylinder. Placeholder era, chosen by eye (DERIVED: between a cone and a paraboloid,
#: which is where stem-form tables put a conifer); no sourced form factor yet.
TAPER_EXPONENT = 0.8

#: The thinnest the trunk is allowed to get at the tip, metres, so the top cap is a disc and not a
#: point.
TIP_RADIUS_MIN_M = 0.02

#: Corners of every trunk ring, the round section's polygon: the view's tubes and caps and the
#: construction's bands are built with it (Resolution: a placeholder section, its sides about
#: 30° apart, far above the collision library's 1.5° face merge).
RING_SIDES = 12

#: Decimal places the plan is written to. Four, so a `.tree.ron` reads as metres to the tenth of
#: a millimetre and the segment heights sum to the tree's height in decimal.
DECIMALS = 4


def round_m(value: float) -> float:
    return round(float(value), DECIMALS)


def segment_count(height_m: float) -> int:
    """How many segments a trunk of this height stacks: the count whose segment length is nearest
    `SEGMENT_TARGET_M`, pushed into the band where the height allows it."""
    count = max(1, round(height_m / SEGMENT_TARGET_M))
    while height_m / count > SEGMENT_MAX_M:
        count += 1
    while count > 1 and height_m / count < SEGMENT_MIN_M:
        count -= 1
    return count


def radius_at(z_m: float, height_m: float, dbh_m: float) -> float:
    """Trunk radius at height `z_m` above the base, from the breast-height diameter."""
    if height_m <= 2.0 * BREAST_HEIGHT_M:
        raise ValueError("a {:.2f} m tree has no breast height to taper from".format(height_m))
    fraction = max(0.0, (height_m - z_m) / (height_m - BREAST_HEIGHT_M))
    return max(TIP_RADIUS_MIN_M, 0.5 * dbh_m * fraction ** TAPER_EXPONENT)


def segments(height_m: float, dbh_m: float) -> List[Dict[str, float]]:
    """The trunk, bottom to top: `[{height_m, r_bottom_m, r_top_m}]`, segment 0 at the base.

    The heights are written to `DECIMALS` places and the last one takes the rounding remainder, so
    their sum is `round_m(height_m)` exactly in decimal. Each boundary's radius is computed once,
    at the boundary's written height: a row's `r_top_m` is the next row's `r_bottom_m`, the last
    row's is the tip's, so the mesh's top ring is the radius `tree_ron` writes for that boundary.
    """
    count = segment_count(height_m)
    length = height_m / count
    total = round_m(height_m)
    steps, boundaries = [], [0.0]
    for index in range(count):
        z = boundaries[-1]
        step = round_m(length) if index < count - 1 else round_m(total - z)
        steps.append(step)
        boundaries.append(round_m(z + step))
    radii = [round_m(radius_at(z, height_m, dbh_m)) for z in boundaries]
    return [{"height_m": step, "r_bottom_m": radii[index], "r_top_m": radii[index + 1]}
            for index, step in enumerate(steps)]


def sample(row: dict, seed: int) -> dict:
    """A tree's height and dbh from the species' ranges. One draw drives both, with a little
    independent spread on the diameter, so a tall tree is a thick one (DERIVED: a plausible
    allometry, not a sourced one)."""
    rng = random.Random(seed)
    tall = rng.random()
    thick = min(1.0, max(0.0, 0.7 * tall + 0.3 * rng.random()))
    h_lo, h_hi = row["height_m"]
    d_lo, d_hi = row["dbh_m"]
    return {
        "height_m": round(h_lo + tall * (h_hi - h_lo), 2),
        "dbh_m": round(d_lo + thick * (d_hi - d_lo), 2),
        "rng": rng,
    }


def whorls(height_m: float, row: dict, rng: random.Random) -> List[dict]:
    """The canopy's branch whorls, bottom to top, each `{z_m, branches: [..]}`.

    A whorl sits every `whorl_spacing_m` at the crown base and closer toward the tip (0.4 of the
    spacing there), which is what a spruce reads as: dense near the top. Branch length follows the
    crown's silhouette, `crown_width_frac * height` at the crown base shrinking to a hand's width
    at the tip. Each branch is `{azimuth, droop, length_m, width_m}`; `droop` is the angle below
    horizontal.
    """
    crown_base = row["crown_base_frac"] * height_m
    spacing = row["whorl_spacing_m"]
    width = row["crown_width_frac"] * height_m
    out = []
    z = crown_base
    phase = rng.random() * 2.0 * math.pi
    while z < height_m - 0.3:
        fraction = (height_m - z) / (height_m - crown_base)
        count = rng.randint(4, 7)
        reach = max(0.3, width * fraction ** 0.8)
        branches = []
        for index in range(count):
            branches.append({
                "azimuth": phase + 2.0 * math.pi * index / count + rng.uniform(-0.25, 0.25),
                "droop": math.radians(12.0) + rng.uniform(-0.1, 0.1),
                "length_m": reach * rng.uniform(0.85, 1.15),
                "width_m": 0.45 * reach * rng.uniform(0.85, 1.15),
            })
        out.append({"z_m": z, "branches": branches})
        phase += 0.6
        z += spacing * (0.4 + 0.6 * fraction)
    return out


#: A branch card's kite in its own (along, across) units: its six points as `kite` places them,
#: the base on the trunk axis first. The generator's faces and the dressing's outline index them.
KITE = ((0.0, 0.0), (0.4, -0.5), (1.0, 0.0), (0.4, 0.5), (0.0, -0.08), (0.0, 0.08))

#: The kite's outline, in order around its edge, as indices into `KITE`.
KITE_OUTLINE = (4, 1, 2, 3, 5)


def kite(branch: dict, z_m: float) -> List[tuple]:
    """A branch's card as `KITE`'s six points in Blender's frame (Z up), the trunk axis at x = y = 0
    and `z_m` the whorl's height in whatever frame the caller places it: a flat kite in the
    branch's own vertical plane, a stem-width base at the axis, its full width at 40 % of its
    length and a point at its tip."""
    along = (math.cos(branch["azimuth"]) * math.cos(branch["droop"]),
             math.sin(branch["azimuth"]) * math.cos(branch["droop"]),
             -math.sin(branch["droop"]))
    side = (-math.sin(branch["azimuth"]), math.cos(branch["azimuth"]), 0.0)
    length, width = branch["length_m"], branch["width_m"]

    def at(t, s):
        return (along[0] * t * length + side[0] * s * width,
                along[1] * t * length + side[1] * s * width,
                z_m + along[2] * t * length + side[2] * s * width)

    return [at(t, s) for t, s in KITE]


def gltf(point: tuple) -> tuple:
    """A Blender point (Z up) in the glTF frame (+Y up), as the export turns it."""
    x, y, z = point
    return (x, z, -y)


def dressing_ron(whorl_rows: List[dict], header: str) -> str:
    """`assets/constructions/<id>/<id>.dressing.ron`, the trunk's view dressing
    (`construction::view::Dressing`): the `trunk` part skinned with bark, and every branch card of
    `whorl_rows` as its base on the trunk axis and its outline (`KITE_OUTLINE`), glTF metres. The
    game gives each card to the law chunk holding its base. `header` leads as a comment."""
    lines = ["// " + line for line in header.splitlines()]
    lines += ["(", '    skin: ["trunk"],', "    cards: ["]
    for whorl in whorl_rows:
        for branch in whorl["branches"]:
            points = [gltf(p) for p in kite(branch, whorl["z_m"])]
            corners = ", ".join("({}, {}, {})".format(*(_num(c) for c in points[i]))
                                for i in KITE_OUTLINE)
            lines.append("        (base: ({}, {}, {}), corners: [{}]),".format(
                *(_num(c) for c in points[0]), corners))
    lines += ["    ],", ")", ""]
    return "\n".join(lines)


def tree_ron(species: str, segment_rows: List[dict], header: str) -> str:
    """`<id>.tree.ron` in the slice contract's shape (A6: no `dbh_m`; the loader derives DBH
    from the taper at 1.3 m), the header as a leading comment.

    Each boundary radius is written once: every segment's `r_bottom_m`, then the last row's
    `r_top_m` as the tree's `r_tip_m`. The other rows' `r_top_m` are not written; `segments`
    makes each the next row's `r_bottom_m`, which is what the loader reads as that top."""
    lines = ["// " + line for line in header.splitlines()]
    lines += [
        "TreeAsset(",
        '    species: "{}",'.format(species),
        "    segments: [",
    ]
    for row in segment_rows:
        lines.append("        Segment(height_m: {}, r_bottom_m: {}),".format(
            row["height_m"], row["r_bottom_m"]))
    lines += [
        "    ],",
        "    r_tip_m: {},".format(segment_rows[-1]["r_top_m"]),
        ")",
        "",
    ]
    return "\n".join(lines)


def boundaries(segment_rows: List[dict]) -> List[tuple]:
    """Every ring of the trunk, base to tip, as `(z_m, radius_m)`: each segment's base, then the
    last segment's top. Heights accumulate as `segments` writes them, to `DECIMALS` places."""
    out, z = [], 0.0
    for row in segment_rows:
        out.append((z, row["r_bottom_m"]))
        z = round_m(z + row["height_m"])
    out.append((z, segment_rows[-1]["r_top_m"]))
    return out


#: Resolution: how far, m, a ring may stand radially off the chord of the rings bounding its band
#: before it bounds a band of its own: one written unit of the plan (`DECIMALS`), the rounding
#: every radius carries, so a ring nearer the chord than that is on it as far as the plan states.
BAND_M = 10.0 ** -DECIMALS


def bands(rings: List[tuple]) -> List[int]:
    """The indices of the rings that bound the trunk's bands, base to tip: the base, the tip, then,
    between two kept rings, the ring standing farthest off their chord while it stands more than
    `BAND_M` off it. Every ring lies within `BAND_M` of its band's chord; each band is a frustum,
    one convex piece, so a ring may turn either way."""
    def standoff(index: int, low: int, high: int) -> float:
        (z, r), (z0, r0), (z1, r1) = rings[index], rings[low], rings[high]
        return abs(r - (r0 + (r1 - r0) * (z - z0) / (z1 - z0)))

    def between(low: int, high: int) -> List[int]:
        if high - low < 2:
            return []
        far = max(range(low + 1, high), key=lambda i: standoff(i, low, high))
        if standoff(far, low, high) <= BAND_M:
            return []
        return between(low, far) + [far] + between(far, high)

    return [0] + between(0, len(rings) - 1) + [len(rings) - 1]


def _num(value: float) -> str:
    """`value` to `CORNER_DECIMALS` places as RON reads it: a decimal point, no exponent, no -0."""
    text = "{:.{}f}".format(value, CORNER_DECIMALS).rstrip("0")
    text = text + "0" if text.endswith(".") else text
    return "0.0" if text in ("-0.0", "0.0") else text


#: Decimal places a construction corner is written to (Resolution: far below the cutter's
#: micrometre plane tolerance, so a side face's four corners stay coplanar in the written file).
CORNER_DECIMALS = 12


def ring_corners(z_m: float, radius_m: float) -> List[tuple]:
    """One ring's `RING_SIDES` corners in the glTF frame (+Y up), counter-clockwise seen from +Y:
    Blender's `(r cos a, r sin a, z)` exported as `(x, z, -y)`."""
    out = []
    for side in range(RING_SIDES):
        angle = 2.0 * math.pi * side / RING_SIDES
        out.append((round(radius_m * math.cos(angle), CORNER_DECIMALS), z_m,
                    round(-radius_m * math.sin(angle), CORNER_DECIMALS)))
    return out


def band_solids(segment_rows: List[dict]) -> List[tuple]:
    """`(corners, faces)` of each band, base to tip: its bottom ring's corners then its top ring's,
    and its faces, each counter-clockwise seen from outside: the bottom, the top, then the sides."""
    rings = boundaries(segment_rows)
    kept = bands(rings)
    sides = RING_SIDES
    out = []
    for low, high in zip(kept, kept[1:]):
        corners = ring_corners(*rings[low]) + ring_corners(*rings[high])
        faces = [list(range(sides - 1, -1, -1)), [sides + side for side in range(sides)]]
        for side in range(sides):
            nxt = (side + 1) % sides
            faces.append([side, nxt, sides + nxt, sides + side])
        out.append((corners, faces))
    return out


def _ron_string(text: str) -> str:
    return '"' + text.replace("\\", "\\\\").replace('"', '\\"') + '"'


def _choice(pair) -> str:
    """A `(value, Job("why"))` pair as RON, the reason carried verbatim."""
    value, job = pair
    return "({}, {}({}))".format(_num(float(value)), job.name, _ron_string(job.args[0]))


def construction_ron(stem: str, trunk: dict, segment_rows: List[dict], header: str) -> str:
    """`tools/constructions/<stem>.pieces.ron`: the trunk's bands (`band_solids`), each a piece of
    the species row's `trunk` material and one component, so the wood's own seam holds band to
    band; its grain up the trunk, cut by the row's cells, anchored at its base by the row's root
    plate. `header` leads as a comment."""
    cells = trunk["cells"]
    lines = ["// " + line for line in header.splitlines()]
    lines += [
        "Construction(",
        "    id: {},".format(_ron_string(stem)),
        "    foundation_y_m: 0.0,",
        "    foundation_attachment: {},".format(_ron_string(trunk["anchor"])),
        "    pieces: [",
    ]
    for band, (corners, faces) in enumerate(band_solids(segment_rows)):
        lines += [
            "        (",
            '            name: "trunk/band_{}",'.format(band),
            '            part: "trunk",',
            '            component: "trunk",',
            "            material: {},".format(_ron_string(trunk["material"])),
            "            surface_frame: Grain((0.0, 1.0, 0.0)),",
            "            corners: [",
        ]
        lines += ["                ({}, {}, {}),".format(*(_num(c) for c in corner))
                  for corner in corners]
        lines += ["            ],", "            faces: ["]
        lines += ["                [{}],".format(", ".join(str(i) for i in face))
                  for face in faces]
        lines += [
            "            ],",
            "            pattern: Cells((size_m: {}, stretch: {})),".format(
                _choice(cells["size_m"]), _choice(cells["stretch"])),
            "        ),",
        ]
    lines += [
        "    ],",
        "    joins: [],",
        ")",
        "",
    ]
    return "\n".join(lines)
