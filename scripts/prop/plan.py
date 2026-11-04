"""The tree's plan: every number the generator builds geometry from, as pure functions.

No `bpy` here, so the sizing laws are tested under the system interpreter and the generator only
turns the plan into meshes. A tree is fixed by (species row, seed): `sample` draws its height and
breast-height diameter from the species' ranges, `segments` stacks the trunk, `whorls` places the
canopy's branches. Every draw comes from one `random.Random(seed)`, consumed in a fixed order, so
the plan is a function of its inputs.

The trunk's construction (`envelope`, `construction_ron`) is the common cutter's input: one convex
piece, the hull of the view's trunk rings to `PROFILE_M`, cut into grain-stretched cells the
species row authors (`tools/src/buildings/fracture.rs`). Its rings come from the same rows the
mesh is built from, so the construction and the view are one source.

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


#: Resolution: how far, m, a view ring may stand radially off the construction's profile. The law
#: block (`src/buildings/fracture.rs`, `Block::face_on`) reads a face as the corners within a
#: millimetre of its plane, so side faces that turn by a trunk's taper (about 1e-4 rad a ring,
#: DERIVED from fir_1's rings) read as one face: with every hull ring kept, fir_1's neighbouring
#: bands' side planes lay 0.004° to 0.02° apart, a chunk's corners 0.1 to 1 mm off the other's
#: plane (MEASURED). A ring within a centimetre of its kept neighbours' chord is dropped, and its
#: sliver of wood is not in the construction's mass.
PROFILE_M = 0.01


def envelope(rings: List[tuple]) -> List[int]:
    """The indices of the rings that are corners of the trunk's construction, base to tip.

    Every ring is the same polygon scaled, so the hull of the rings is the hull of the radius
    profile: the base, the tip and each ring on the profile's upper chain in `(z, r)`, which turns
    strictly inward at every ring. Of that chain the base and the tip are kept, then, between two
    kept rings, the one standing farthest above their chord while it stands more than `PROFILE_M`
    above it. A subsequence of a strictly convex chain is strictly convex, so no two side faces
    are coplanar, and every ring lies inside the profile or within `PROFILE_M` of it."""
    chain: List[int] = []
    for index, (z, r) in enumerate(rings):
        while len(chain) >= 2:
            (z0, r0), (z1, r1) = rings[chain[-2]], rings[chain[-1]]
            if (z1 - z0) * (r - r0) - (r1 - r0) * (z - z0) < 0.0:
                break
            chain.pop()
        chain.append(index)

    def standoff(index: int, low: int, high: int) -> float:
        (z, r), (z0, r0), (z1, r1) = rings[index], rings[low], rings[high]
        return r - (r0 + (r1 - r0) * (z - z0) / (z1 - z0))

    def between(low: int, high: int) -> List[int]:
        inner = [i for i in chain if low < i < high]
        if not inner:
            return []
        far = max(inner, key=lambda i: standoff(i, low, high))
        if standoff(far, low, high) <= PROFILE_M:
            return []
        return between(low, far) + [far] + between(far, high)

    return [chain[0]] + between(chain[0], chain[-1]) + [chain[-1]]


def _num(value: float) -> str:
    """`value` to `ENVELOPE_DECIMALS` places as RON reads it: a decimal point, no exponent, no -0."""
    text = "{:.{}f}".format(value, ENVELOPE_DECIMALS).rstrip("0")
    text = text + "0" if text.endswith(".") else text
    return "0.0" if text in ("-0.0", "0.0") else text


#: Decimal places a construction corner is written to (Resolution: far below the cutter's
#: micrometre plane tolerance, so a side face's four corners stay coplanar in the written file).
ENVELOPE_DECIMALS = 12


def ring_corners(z_m: float, radius_m: float, sides: int) -> List[tuple]:
    """One ring's corners in the glTF frame (+Y up), counter-clockwise seen from +Y: Blender's
    `(r cos a, r sin a, z)` exported as `(x, z, -y)`."""
    out = []
    for side in range(sides):
        angle = 2.0 * math.pi * side / sides
        out.append((round(radius_m * math.cos(angle), ENVELOPE_DECIMALS), z_m,
                    round(-radius_m * math.sin(angle), ENVELOPE_DECIMALS)))
    return out


def envelope_solid(segment_rows: List[dict], sides: int):
    """`(corners, faces)` of the trunk's convex envelope: the kept rings' corners, ring by ring,
    and its faces, each counter-clockwise seen from outside: the base, the tip, then each band's
    quads."""
    rings = boundaries(segment_rows)
    kept = envelope(rings)
    corners = []
    for index in kept:
        corners += ring_corners(rings[index][0], rings[index][1], sides)
    top = (len(kept) - 1) * sides
    faces = [list(range(sides - 1, -1, -1)), [top + side for side in range(sides)]]
    for band in range(len(kept) - 1):
        low, high = band * sides, (band + 1) * sides
        for side in range(sides):
            nxt = (side + 1) % sides
            faces.append([low + side, low + nxt, high + nxt, high + side])
    return corners, faces


def _ron_string(text: str) -> str:
    return '"' + text.replace("\\", "\\\\").replace('"', '\\"') + '"'


def _choice(pair) -> str:
    """A `(value, Job("why"))` pair as RON, the reason carried verbatim."""
    value, job = pair
    return "({}, {}({}))".format(_num(float(value)), job.name, _ron_string(job.args[0]))


def construction_ron(stem: str, trunk: dict, segment_rows: List[dict], sides: int,
                     header: str) -> str:
    """`tools/buildings/<stem>.construction.ron`: the trunk's envelope as one piece of the species
    row's `trunk` material, its grain up the trunk, cut by the row's cells, anchored at its base
    by the row's root plate. `header` leads as a comment."""
    corners, faces = envelope_solid(segment_rows, sides)
    cells = trunk["cells"]
    lines = ["// " + line for line in header.splitlines()]
    lines += [
        "Construction(",
        "    id: {},".format(_ron_string(stem)),
        "    foundation_y_m: 0.0,",
        "    foundation_attachment: {},".format(_ron_string(trunk["anchor"])),
        "    pieces: [",
        "        (",
        '            name: "trunk",',
        "            kind: Trunk,",
        '            component: "trunk",',
        "            material: {},".format(_ron_string(trunk["material"])),
        "            surface_frame: Grain((0.0, 1.0, 0.0)),",
        "            corners: [",
    ]
    lines += ["                ({}, {}, {}),".format(*(_num(c) for c in corner))
              for corner in corners]
    lines += ["            ],", "            faces: ["]
    lines += ["                [{}],".format(", ".join(str(i) for i in face)) for face in faces]
    lines += [
        "            ],",
        "            pattern: Cells((size_m: {}, stretch: {})),".format(
            _choice(cells["size_m"]), _choice(cells["stretch"])),
        "        ),",
        "    ],",
        "    joins: [],",
        ")",
        "",
    ]
    return "\n".join(lines)
