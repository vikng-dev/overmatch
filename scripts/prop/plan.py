"""Pure source geometry and foliage plans for the common construction bake.

The radius law is r(z) = (dbh / 2) ((H-z)/(H-breast_height))^k, 0 < k <= 1.
DERIVED: r'' <= 0, so the triangle inequality and concavity make
{(x,z): |x| <= r(z)} convex. Chords approximate this solid from inside. Only the
construction pattern cuts its interior; source rings are exterior geometry.
Blender uses Z up; glTF and construction data use Y up.
"""

from __future__ import annotations

import math
import random
from typing import Dict, List

# Resolution: 12 sides keep the round silhouette's radial error below 3.5%,
# with circumferential face angles well above Parry's merge threshold.
RING_SIDES = 12

# Resolution: Parry 0.27 ConvexPolyhedron::from_convex_mesh deletes an edge when
# normal dot > 1 - sqrt(f32::EPSILON). DEFAULT_EPSILON is Real::EPSILON.
PARRY_MERGE_RAD = math.acos(1.0 - math.sqrt(2.0 ** -23))


def radius_at(z_m: float, height_m: float, dbh_m: float, shape: dict) -> float:
    breast, exponent = shape["breast_height_m"][0], shape["exponent"][0]
    if not (0 <= z_m <= height_m and height_m > breast and 0 < exponent <= 1):
        raise ValueError("taper requires 0 <= z <= height, height > breast height and 0 < exponent <= 1")
    return 0.5 * dbh_m * ((height_m - z_m) / (height_m - breast)) ** exponent


def chord_error(low: tuple, high: tuple, height: float, dbh: float, shape: dict) -> tuple:
    """Maximum radial chord error and its height, from r'(z) = chord slope."""
    z0, r0 = low
    z1, r1 = high
    exponent = shape["exponent"][0]
    if exponent == 1:
        return 0.0, (z0 + z1) / 2
    coefficient = radius_at(0, height, dbh, shape) / height ** exponent
    slope = (r0 - r1) / (z1 - z0)
    z = height - (slope / (coefficient * exponent)) ** (1 / (exponent - 1))
    z = min(z1, max(z0, z))
    return radius_at(z, height, dbh, shape) - (r0 - slope * (z-z0)), z


def rings(height: float, dbh: float, shape: dict) -> List[tuple]:
    """Exterior rings chosen by radial error, never by a fracture height.

    Refuse a requested resolution whose side normals would merge; silently merging
    those faces changes the contact solid. The apex is a single vertex.
    """
    tolerance = shape["radial_error_m"][0]
    if not math.isfinite(tolerance) or tolerance <= 0:
        raise ValueError("shape.radial_error_m must be finite and positive")
    def between(low, high):
        error, z = chord_error(low, high, height, dbh, shape)
        if error <= tolerance:
            return [low, high]
        middle = (z, radius_at(z, height, dbh, shape))
        return between(low, middle)[:-1] + between(middle, high)
    out = between((0.0, radius_at(0, height, dbh, shape)), (height, 0.0))
    slopes = [math.atan((b[1]-a[1])/(b[0]-a[0]) * math.cos(math.pi/RING_SIDES))
              for a, b in zip(out, out[1:])]
    if any(abs(a-b) <= PARRY_MERGE_RAD for a, b in zip(slopes, slopes[1:])):
        raise ValueError("shape.radial_error_m requires faces below Parry's merge angle; "
                         "author a coarser radial tolerance or a different convex shape")
    return out


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
    and `z_m` the whorl's height above the source origin: a flat kite in the
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


def tree_ron(species: str, seed: int, height: float, dbh: float, header: str) -> str:
    """Generator source, independent of the simulation's construction schema."""
    return "\n".join(["// " + header, "TreeSource(",
                       '    species: "{}",'.format(species),
                       "    seed: {},".format(seed), "    height_m: {},".format(height),
                       "    dbh_m: {},".format(dbh), ")", ""])


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


def solid(ring_rows: List[tuple]) -> tuple:
    """One closed convex solid: exterior side faces, a base and a point apex."""
    sides = RING_SIDES
    corners = [p for z, r in ring_rows[:-1] for p in ring_corners(z, r)]
    apex = len(corners)
    corners.append((0.0, ring_rows[-1][0], 0.0))
    faces = [list(range(sides - 1, -1, -1))]
    for ring in range(len(ring_rows)-2):
        low, high = ring*sides, (ring+1)*sides
        for side in range(sides):
            nxt = (side+1) % sides
            faces.append([low+side, low+nxt, high+nxt, high+side])
    low = (len(ring_rows)-2)*sides
    for side in range(sides):
        faces.append([low+side, low+(side+1)%sides, apex])
    return corners, faces


def _ron_string(text: str) -> str:
    return '"' + text.replace("\\", "\\\\").replace('"', '\\"') + '"'


def _choice(pair) -> str:
    """A `(value, Job("why"))` pair as RON, the reason carried verbatim."""
    value, job = pair
    return "({}, {}({}))".format(_num(float(value)), job.name, _ron_string(job.args[0]))


def construction_ron(stem: str, trunk: dict, ring_rows: List[tuple], header: str) -> str:
    """One convex source element, cut only by its authored pattern."""
    cells = trunk["cells"]
    lines = ["// " + line for line in header.splitlines()]
    lines += [
        "Construction(",
        "    id: {},".format(_ron_string(stem)),
        "    foundation_y_m: 0.0,",
        "    foundation_attachment: {},".format(_ron_string(trunk["anchor"])),
        "    pieces: [",
    ]
    corners, faces = solid(ring_rows)
    lines += [
        "        (",
        '            name: "trunk",',
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
