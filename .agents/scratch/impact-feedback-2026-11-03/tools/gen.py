#!/usr/bin/env python3
"""Throwaway: write the impact stills' design presentation copies, capture plans and run list.

The game's impact presentation, the view only. Model numbers are impact_model.py's output (MODEL);
every other value here is capture configuration (ASSUMED) or a taste this lane picked, named in
INDEX.md.
Usage: gen.py <bay> <out dir for ron, plans> <raw dir>
"""
import json
import math
import os
import re
import sys

BAY, OUT, RAW = sys.argv[1:4]
SHIPPED = open(f"{BAY}/assets/vfx/presentation.ron").read()
G = 9.81
T0 = 6.0
TIMES = [T0 - 0.5, T0 + 0.02, T0 + 0.15, T0 + 0.5, T0 + 1.0, T0 + 2.5, T0 + 5.0]
STEP = 0.02

# The map's height, sampled nearest (assets/maps/kalinovo/level.json's decode) for pose heights only.
N = 1024
HALF = 750.0
hbytes = open(f"{os.path.dirname(os.path.abspath(__file__))}/height.gray16", "rb").read()


def height(x, z):
    c = min(max(int((x + HALF) / (2 * HALF / N)), 0), N - 1)
    r = min(max(int((HALF - z) / (2 * HALF / N)), 0), N - 1)
    o = (r * N + c) * 2
    return (hbytes[o] << 8 | hbytes[o + 1]) / 65535.0 * 50.0


# The powder cells' visible footprint: the part of a quad's height a saturated sprite covers (MEASURED
# by signal.py: 0.55 to 0.64 on the two large cells), so a stated height is a quad height over this.
FOOT = 0.6

# Strike sites from survey4.py and survey6.py, chosen for ground behind the read: x, z, and the bearing from the strike to each firing
# position, degrees from +Z toward +X.
SITES = {
    "mud": dict(x=665.0, z=-675.0, b500=315.0, b1000=345.0),
    "gravel": dict(x=-15.0, z=375.0, b500=270.0, b1000=225.0),
    "dirt": dict(x=55.0, z=390.0, b500=230.0, b1000=230.0),
}


def framing(site, name):
    s = SITES[site]
    dist, bearing = {"g500": (500.0, s["b500"]), "g1000": (1000.0, s["b1000"]), "t500": (500.0, s["b500"])}[name]
    dx, dz = math.sin(math.radians(bearing)), math.cos(math.radians(bearing))
    ex, ez = s["x"] + dx * dist, s["z"] + dz * dist
    return dict(ex=ex, ez=ez, heading=(bearing + 180.0) % 360.0, dx=dx, dz=dz, bearing=bearing)


BASE_DESIGN = {
    "impact_size_exponent": "0.000001",
    "splash_ring_alpha": "0.0",
    "powder_erosion_end": "0.4",
    "streak_erosion_end": "0.8",
    "ground_mark_lifetime": "60.0",
    "scar_end_scale": "1.0",
    "ejecta_end_scale": "1.0",
    "ejecta_width_ratio": "6.0",
    "splash_plume_count": "(3, 3)",
    # Under erosion the column keeps about 0.45 of its quad's height near its peak (signal.py, by eye).
    "splash_plume_aspect": f"({0.71 / 0.45:.4f}, {1 / 0.45:.4f}, 1.0)",
    "splash_plume_sharpness": "14.0",
    "splash_cloud_sharpness": "8.0",
    "dust_sharpness": "14.0",
    "spall_sharpness": "8.0",
    "hot_green": "(0.35, 0.6)",
    "hot_blue": "(0.05, 0.65)",
    "hot_heat": "(0.35, 0.95)",
    "armor_spark_width_ratio": "1.2",
    "spark_width_ratio": "1.2",
    "armor_spark_stretch": "(0.14, 0.2)",
    "spark_stretch": "(0.14, 0.2)",
    "spark_glow": "14.0",
    "spark_end_scale": "0.8",
    "ping_glow": "13.0",
    "ping_lifetime": "0.045",
    "spark_speed": "(10.0, 13.5)",
    "spark_lifetime": "(0.4, 0.56)",
    "armor_spark_speed": "(10.0, 13.5)",
    "armor_spark_lifetime": "(0.4, 0.56)",
}


def pair(a, b):
    return f"({a:.4f}, {b:.4f})"


def num(a):
    return f"{a:.4f}"


def column(h, fly, alpha, lifted, r):
    a = 1 - math.sqrt(max(1 - alpha, 0.0))
    start = 0.3 * h
    return {
        "splash_plume_size": pair(start, h),
        "splash_plume_rise": num((h - start) / (2 * fly)),
        "splash_plume_lifetime": num(fly),
        "plume_alpha": num(min(a / lifted, 1.0)),
        "ground_mark_size": num(2 * r),
    }


def haze(diameter, alpha, life, lifted):
    drawn = alpha >= 0.05
    return {
        "splash_cloud_size": pair(0.5 * diameter / FOOT, diameter / FOOT),
        "splash_cloud_alpha": num(alpha / lifted if drawn else 0.0),
        "splash_cloud_lifetime": num(life),
    }


def pieces(n, clump, v0, fly, lifted):
    count = round(n / lifted)
    stretch = clump / 0.6 / (0.8 * v0)
    return {
        "ejecta_count": f"({count}, {count})",
        "ejecta_speed": pair(0.6 * v0, v0),
        "ejecta_lifetime": pair(0.8 * fly, fly),
        "ejecta_stretch": pair(0.8 * stretch, 1.2 * stretch),
    }


def spall(h, alpha, fly, v0):
    return {
        "spall_puff_size": pair(0.5 * h / FOOT, h / FOOT),
        "spall_puff_alpha": num(alpha),
        "spall_puff_lifetime": pair(0.95 * fly, 1.05 * fly),
        "spall_puff_rise": num(v0),
    }


def rifle_column(alpha, lifted):
    return {
        "dust_size": pair(0.55 / FOOT, 0.8 / FOOT),
        "dust_alpha": num(alpha / lifted),
        "dust_lifetime": pair(0.76, 0.85),
        "dust_normal_push": "0.0",
        "dust_rise": "0.5",
    }


LIFTED = {"mud": 0.391, "gravel": 0.836, "dirt": 0.906}
R88 = 7.71 / 6.8
RHE = 8.86 / 6.8

DESIGN = {
    "r79_mud": rifle_column(0.0754, LIFTED["mud"]),
    "r79_mud_loft3": rifle_column(0.21, LIFTED["mud"]),
    "r79_gravel": {**rifle_column(0.23, LIFTED["gravel"]), "ping_size": "0.159", "spark_count": "(1, 1)"},
    "r79_steel": {"dust_alpha": "0.0", "ping_size": "0.30", "spark_count": "(3, 3)"},
    "ap88_mud": {
        **column(7.71, 2.51, 0.532, LIFTED["mud"], R88),
        **haze(9.25, 0.04, 7.22, LIFTED["mud"]),
        **pieces(24, 0.402, 12.3, 2.51, LIFTED["mud"]),
    },
    "ap88_mud_h217": {
        **column(2.17, 1.33, 0.532, LIFTED["mud"], R88),
        **haze(2.6, 0.04, 2.0, LIFTED["mud"]),
        **pieces(24, 0.402, 6.52, 1.33, LIFTED["mud"]),
    },
    "ap88_gravel": {
        **column(7.71, 2.51, 0.92, LIFTED["gravel"], R88),
        **haze(9.25, 0.127, 7.22, LIFTED["gravel"]),
        **pieces(18, 0.201, 12.3, 2.51, LIFTED["gravel"]),
        "ping_size": "1.54",
        "spark_count": "(4, 4)",
    },
    "ap88_steel": {
        "armor_flash_size": "2.9",
        "armor_spark_count": "(22, 22)",
        **spall(0.771, 0.316, 0.793, 3.89),
    },
    "skid_steel": {
        "armor_flash_size": "1.89",
        "armor_spark_count": "(15, 15)",
        "armor_spark_deflect_bias": "0.76",
        **spall(0.503, 0.219, 0.64, 3.14),
    },
}
BURST = {
    **column(8.86, 2.69, 1.0, LIFTED["dirt"], RHE),
    **haze(10.6, 0.375, 8.3, LIFTED["dirt"]),
    **pieces(20, 0.289, 13.2, 2.69, LIFTED["dirt"]),
    "dust_size": pair(2.29 / FOOT, 4.58 / FOOT),
    "dust_lifetime": pair(4.5, 4.66),
    "dust_alpha": "0.7",
    "dust_rise": "0.5",
    "dust_normal_push": "0.0",
    "dust_frame_rate": "0.5",
}
DESIGN["he88_dirt"] = BURST
DESIGN["he88_dirt_thin"] = {**BURST, "plume_alpha": num((1 - math.sqrt(0.4)) / LIFTED["dirt"])}


def substitute(text, overrides):
    for field, value in overrides.items():
        pattern = re.compile(
            r"^(\s*" + field + r": \()(.*?)(, (?:Taste|StandIn|Derived|Resolution|Measured|Unknown|Unit)\(\")",
            re.M,
        )
        text, hits = pattern.subn(lambda m: m.group(1) + value + m.group(3), text, count=1)
        assert hits == 1, field
    return text


# Each case: round, surface kind, site, today's arm's name, and whether the strike lands on a plate.
CASES = {
    "r79_mud": dict(title="7.9 mm AP on mud", round="7.9mm_AP", site="mud", kind="ground", arms=["today", "design"], flash=False, zoom=6),
    "r79_mud_loft3": dict(title="7.9 mm AP on mud, thrown powder x3", round="7.9mm_AP", site="mud", kind="ground", arms=["design"], today="r79_mud", flash=False, zoom=6),
    "r79_gravel": dict(title="7.9 mm AP on gravel", round="7.9mm_AP", site="gravel", kind="ground", arms=["today", "design"], flash=True, zoom=6),
    "r79_steel": dict(title="7.9 mm AP on steel", round="7.9mm_AP", site="gravel", kind="plate", arms=["today", "design"], flash=True, zoom=6),
    "ap88_mud": dict(title="88 mm AP on mud, column 7.7 m", round="88mm_AP", site="mud", kind="ground", arms=["today", "design"], flash=False, zoom=3),
    "ap88_mud_h217": dict(title="88 mm AP on mud, column 2.17 m", round="88mm_AP", site="mud", kind="ground", arms=["design"], today="ap88_mud", flash=False, zoom=3),
    "ap88_gravel": dict(title="88 mm AP on gravel", round="88mm_AP", site="gravel", kind="ground", arms=["today", "design"], flash=True, zoom=3),
    "ap88_steel": dict(title="88 mm AP on steel", round="88mm_AP", site="gravel", kind="plate", arms=["today", "design"], flash=True, zoom=3),
    "skid_steel": dict(title="88 mm AP skid on steel", round="88mm_AP", site="gravel", kind="skid", arms=["today", "design"], flash=True, zoom=3),
    "he88_dirt": dict(title="88 mm HE burst on dirt", round="88mm_HE", site="dirt", kind="ground", arms=["today", "design"], flash=True, zoom=3, smoke=True),
    "he88_dirt_thin": dict(title="88 mm HE burst on dirt, column opacity 0.6", round="88mm_HE", site="dirt", kind="ground", arms=["design"], today="he88_dirt", flash=True, zoom=3, smoke=True),
}
FRAMINGS = ["g500", "t500", "g1000"]

os.makedirs(f"{OUT}/ron", exist_ok=True)
os.makedirs(f"{OUT}/plans", exist_ok=True)
os.makedirs(RAW, exist_ok=True)
runs = []
for case, spec in CASES.items():
    site = SITES[spec["site"]]
    if "design" in spec["arms"]:
        overrides = {**BASE_DESIGN, **DESIGN[case]}
        head = f"// THROWAWAY design arm, case {case}: the impact stills only, never shipped.\n"
        with open(f"{OUT}/ron/{case}.presentation.ron", "w") as f:
            f.write(head + substitute(SHIPPED, overrides))
    for name in FRAMINGS:
        fr = framing(spec["site"], name)
        gy = height(site["x"], site["z"])
        # The gunner's optic as a held pose at gun height: the sight's own writer follows the aim,
        # and the aim follows a third-person camera no one is steering. Its field is the view's data
        # (`tiger_1.tank.ron`, views.Gunner: 25 degrees, magnification 2.5), the other the commander's.
        if name == "t500":
            env = {
                "OVERMATCH_START": f"{fr['ex']:.1f},{fr['ez']:.1f},{fr['heading']:.1f}",
                "OVERMATCH_STILLS_FIELD_DEG": "45",
            }
        else:
            env = {"OVERMATCH_STILLS_FIELD_DEG": "25"}
        # A parked tank beside the strike for scale, broadside to this view; a plate strike lands on it.
        lat = (fr["dz"], -fr["dx"])
        if spec["kind"] == "ground":
            tx, tz = site["x"] + lat[0] * 18.0, site["z"] + lat[1] * 18.0
        else:
            tx, tz = site["x"], site["z"]
        env["OVERMATCH_STILLS_TARGET"] = f"{tx:.1f},{tz:.1f},{(fr['bearing'] + 90.0) % 360.0:.1f}"
        if spec["kind"] != "ground":
            px, py, pz = site["x"] + fr["dx"] * 1.9, gy + 1.4, site["z"] + fr["dz"] * 1.9
            env["OVERMATCH_STILLS_PLATE"] = f"{px:.2f},{py:.2f},{pz:.2f},{fr['dx']:.4f},0.0,{fr['dz']:.4f}"
        if spec["kind"] == "skid":
            # 20 degrees off the plate, along the hull: the draft's assumed graze.
            c, s = math.cos(math.radians(20)), math.sin(math.radians(20))
            dx, dz = lat[0] * c + fr["dx"] * s, lat[1] * c + fr["dz"] * s
            env["OVERMATCH_STILLS_RICOCHET"] = f"{dx:.4f},{s * 0.0:.4f},{dz:.4f}"
        surface = "Terrain" if spec["kind"] == "ground" else 'Substance("RHA")'
        impacts = [f'(x: {site["x"]}, z: {site["z"]}, round: "{spec["round"]}", surface: {surface}, at: {T0}),']
        if name == "t500":
            ax, az = fr["ex"], fr["ez"]
            ey = height(ax, az) + 5.5
            ex, ez = ax + fr["dx"] * 14.0, az + fr["dz"] * 14.0
        else:
            ex, ez = fr["ex"], fr["ez"]
            ey = height(ex, ez) + 2.3
        camera = f"camera: Pose(eye: ({ex:.2f}, {ey:.2f}, {ez:.2f}), look: ({site['x']}, {gy + 3.0:.2f}, {site['z']})),"
        sight = ""
        at = ", ".join(f"{t:.2f}" for t in TIMES)
        for arm in spec["arms"]:
            stem = f"{case}.{arm}.{name}"
            design_impacts = list(impacts)
            if arm == "design" and spec.get("smoke"):
                # The burst's smoke: a second raised strike on a sparkless, near-black substance,
                # whose one powder sprite the dust fields state. AP, so no second flash.
                design_impacts.append(f'(x: {site["x"]}, z: {site["z"]}, round: "88mm_AP", surface: Substance("Rubber"), at: {T0}),')
            plan = "\n".join([
                f"// {spec['title']}, {name}, {arm}. Capture configuration (ASSUMED), not game data.",
                "(",
                "    frames: (",
                f'        out: "{RAW}/{stem}.png",',
                f"        at: [{at}],",
                "        size: (1920, 1080),",
                "        scale_factor: 1.0,",
                "        ui: false,",
                "        gizmos: false,",
                f"        step_s: {STEP},",
                f"        {sight}{camera}",
                "    ),",
                "    impacts: [",
                *("        " + i for i in (design_impacts if arm == "design" else impacts)),
                "    ],",
                ")",
                "",
            ])
            path = f"{OUT}/plans/{stem}.ron"
            with open(path, "w") as f:
                f.write(plan)
            run_env = dict(env)
            if arm == "design":
                run_env["OVERMATCH_STILLS_LOOK"] = f"{OUT}/ron/{case}.presentation.ron"
            runs.append(dict(stem=stem, plan=path, env=run_env))

with open(f"{OUT}/plans/runs.json", "w") as f:
    json.dump(dict(runs=runs, cases=CASES, framings=FRAMINGS, times=TIMES, t0=T0), f, indent=1)
print(len(runs), "runs")
