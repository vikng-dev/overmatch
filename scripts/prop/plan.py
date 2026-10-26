"""The tree's plan: every number the generator builds geometry from, as pure functions.

No `bpy` here, so the sizing laws are tested under the system interpreter and the generator only
turns the plan into meshes. A tree is fixed by (species row, seed): `sample` draws its height and
breast-height diameter from the species' ranges, `segments` stacks the trunk, `whorls` places the
canopy's branches. Every draw comes from one `random.Random(seed)`, consumed in a fixed order, so
the plan is a function of its inputs.

Units: metres and radians throughout. Blender authors Z up; the glTF export turns that into +Y up.
"""

from __future__ import annotations

import math
import random
from typing import Dict, List

#: Trunk segment length band, metres (slice contract: "a stack of ~1.5–2 m segments").
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
    their sum is `round_m(height_m)` exactly in decimal.
    """
    count = segment_count(height_m)
    length = height_m / count
    out = []
    z = 0.0
    total = round_m(height_m)
    for index in range(count):
        step = round_m(length) if index < count - 1 else round_m(total - round_m(z))
        out.append({
            "height_m": step,
            "r_bottom_m": round_m(radius_at(z, height_m, dbh_m)),
            "r_top_m": round_m(radius_at(z + step, height_m, dbh_m)),
        })
        z = round_m(z + step)
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


def tree_ron(species: str, dbh_m: float, segment_rows: List[dict], header: str) -> str:
    """`<id>.tree.ron` in the slice contract's shape, the header as a leading comment."""
    lines = ["// " + line for line in header.splitlines()]
    lines += [
        "TreeAsset(",
        '    species: "{}",'.format(species),
        "    dbh_m: {},".format(dbh_m),
        "    segments: [",
    ]
    for row in segment_rows:
        lines.append("        Segment(height_m: {}, r_bottom_m: {}, r_top_m: {}),".format(
            row["height_m"], row["r_bottom_m"], row["r_top_m"]))
    lines += ["    ],", ")", ""]
    return "\n".join(lines)
