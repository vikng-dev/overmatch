#!/usr/bin/env python3
"""Transmission-gate report for track-sandbox harness captures (phase 2.5).

Stdlib-only. Consumes the JSONL a scripted `SANDBOX_HARNESS` run writes (harness.rs),
including the `tr` rows the regenerative modes emit, and reports the phase-2.5 physics
gates as numbers:

  straight FILE --expect-top M/S   top speed vs the gearing-implied value (5% gate) +
                                   the gear sequence and up-down hunting pairs.
  turn FILE [REF_FILE]             steady turn radius over the on-pad window (v̄/|ω̄|);
                                   with REF_FILE (the governor capture at the same
                                   command) asserts the radius is strictly smaller.
  pivot FILE [--max-yaw R]         steady pivot yaw rate (finite; statelier than the
                                   governor snap when --max-yaw is given).
  fixed FILE --radii t1,w1;t2,w2…  fixed-radius run: dominant gear + detent, the BELT
                                   ratio d/m vs the commanded κ (the constraint's own
                                   contract), and the hull radius vs the commanded one
                                   (reported with the slip gap — the element grip law's
                                   emergent μ(R) falloff runs the hull wide of belt
                                   kinematics; that gap is contact physics, not a
                                   transmission defect).
  hold FILE                        parked-slope drift (mm; JSONL quantization is 0.1 mm)
                                   and the frozen-phase check (belt speeds bit-zero).

Windows: `straight`/`pivot` use the last 256/384 recorded ticks; `turn`/`fixed` start
384 ticks after command onset (slew + spin-up) and end at course exit (support < 50% of
weight), like scripts/track/analyze_steer.py's course-exit rule.
"""

import json
import statistics as st
import sys


def load(path):
    meta, ks, trs = None, [], {}
    for line in open(path):
        r = json.loads(line)
        t = r.get("t")
        if t == "meta":
            meta = r
        elif t == "k":
            ks.append(r)
        elif t == "tr":
            trs[r["k"]] = r
    return meta, ks, trs


def planar_speed(k):
    return (k["vel"][0] ** 2 + k["vel"][2] ** 2) ** 0.5


def on_course_window(meta, ks, settle):
    start = meta["warmup"] + settle
    end = len(ks)
    for i, k in enumerate(ks):
        if k["k"] > start and k["sup"] < 0.5 * meta["weight"]:
            end = i
            break
    return [k for k in ks[:end] if k["k"] >= start]


def gear_sequence(trs):
    seq = []
    for k in sorted(trs):
        g = trs[k]["gear"]
        if not seq or seq[-1] != g:
            seq.append(g)
    return seq


def hunting_pairs(seq):
    """Up-down pairs: an upshift immediately undone (…g, g+1, g…)."""
    return sum(
        1 for a, b, c in zip(seq, seq[1:], seq[2:]) if b == a + 1 and c == a
    )


def cmd_straight(path, expect_top):
    meta, ks, trs = load(path)
    tail = [k for k in ks if k["k"] >= ks[-1]["k"] - 256]
    top = st.mean(planar_speed(k) for k in tail)
    err = 100.0 * (top - expect_top) / expect_top
    seq = gear_sequence(trs)
    pairs = hunting_pairs(seq)
    ok = abs(err) <= 5.0 and pairs <= len(set(seq))
    print(f"top speed {top:.2f} m/s vs gearing-implied {expect_top:.2f} ({err:+.1f}%)")
    print(f"gear sequence {seq} | up-down hunting pairs {pairs}")
    print("GATE straight:", "PASS" if ok else "FAIL")
    return ok


def steady_radius(path, settle=384):
    meta, ks, _ = load(path)
    win = on_course_window(meta, ks, settle)
    v = st.mean(planar_speed(k) for k in win)
    w = st.mean(abs(k["yawrate"]) for k in win)
    return v, w, v / w, len(win)


def cmd_turn(path, ref=None):
    v, w, r, n = steady_radius(path)
    print(f"turn: v={v:.2f} m/s yaw={w:.4f} rad/s R={r:.2f} m (n={n})")
    if ref:
        rv, rw, rr, rn = steady_radius(ref)
        print(f"ref:  v={rv:.2f} m/s yaw={rw:.4f} rad/s R={rr:.2f} m (n={rn})")
        ok = r < rr
        print("GATE turn (R strictly < ref):", "PASS" if ok else "FAIL")
        return ok
    return True


def cmd_pivot(path, max_yaw=None):
    meta, ks, _ = load(path)
    tail = [k for k in ks if k["k"] >= ks[-1]["k"] - 384]
    w = [k["yawrate"] for k in tail]
    mean = st.mean(w)
    print(f"pivot yaw: mean {mean:.4f} rad/s (min {min(w):.4f}, max {max(w):.4f})")
    ok = abs(mean) > 1e-3 and all(abs(x) < 100 for x in w)
    if max_yaw is not None:
        ok = ok and abs(mean) < max_yaw
        print(f"GATE pivot (finite, |yaw| < {max_yaw}):", "PASS" if ok else "FAIL")
    return ok


def cmd_fixed(path, radii):
    meta, ks, trs = load(path)
    win = on_course_window(meta, ks, settle=384)
    win = [k for k in win if k["k"] in trs]
    from collections import Counter

    gear = Counter(trs[k["k"]]["gear"] for k in win).most_common(1)[0][0]
    step = Counter(trs[k["k"]]["step"] for k in win).most_common(1)[0][0]
    win = [k for k in win if trs[k["k"]]["gear"] == gear]
    half_tread = meta["half_tread"]
    r_cmd = radii[gear - 1][0] if step == 2 else radii[gear - 1][1]
    kappa_cmd = half_tread / r_cmd
    m = st.mean((k["belt"][0] + k["belt"][1]) / 2 for k in win)
    d = st.mean((k["belt"][0] - k["belt"][1]) / 2 for k in win)
    v = st.mean(planar_speed(k) for k in win)
    w = st.mean(abs(k["yawrate"]) for k in win)
    r_hull = v / w
    print(f"dominant gear {gear} step {step} (2=tight,1=wide) n={len(win)}")
    print(
        f"belt ratio d/m = {d / m:.4f} vs commanded kappa = {kappa_cmd:.4f} "
        f"({100 * (d / m - kappa_cmd) / kappa_cmd:+.1f}%)"
    )
    print(
        f"hull radius {r_hull:.2f} m vs commanded {r_cmd:.1f} m "
        f"({100 * (r_hull - r_cmd) / r_cmd:+.1f}% — the gap past the belt ratio is "
        f"track slip, the grip law's emergent mu(R))"
    )
    ok = abs(d / m - kappa_cmd) <= 0.02 * max(kappa_cmd, 0.05)
    print("GATE fixed (belt ratio holds commanded kappa within 2%):", "PASS" if ok else "FAIL")
    return ok


def cmd_hold(path):
    meta, ks, _ = load(path)
    win = [k for k in ks if k["k"] >= meta["warmup"]]
    p0 = win[0]["hull"]
    drift = max(
        sum((k["hull"][i] - p0[i]) ** 2 for i in range(3)) ** 0.5 for k in win
    )
    ph0, phn = win[0]["phase"], win[-1]["phase"]
    dphase = max(abs(phn[0] - ph0[0]), abs(phn[1] - ph0[1]))
    belt = max(max(abs(k["belt"][0]), abs(k["belt"][1])) for k in win)
    frozen = dphase == 0.0 and belt == 0.0
    print(
        f"hold: max drift {drift * 1000:.2f} mm (0.1 mm JSONL LSB) | "
        f"phase delta {dphase:.4f} m | max |belt| {belt:.6f} m/s"
    )
    ok = frozen and drift <= 0.00025  # <= 2 print LSBs + rounding: the settle wobble
    print("GATE hold (frozen phase, drift at print resolution):", "PASS" if ok else "FAIL")
    return ok


def main():
    args = sys.argv[1:]
    if not args:
        print(__doc__)
        return 2
    cmd, path, rest = args[0], args[1], args[2:]
    if cmd == "straight":
        expect = float(rest[rest.index("--expect-top") + 1])
        ok = cmd_straight(path, expect)
    elif cmd == "turn":
        ok = cmd_turn(path, rest[0] if rest else None)
    elif cmd == "pivot":
        my = float(rest[rest.index("--max-yaw") + 1]) if "--max-yaw" in rest else None
        ok = cmd_pivot(path, my)
    elif cmd == "fixed":
        raw = rest[rest.index("--radii") + 1]
        radii = [tuple(float(x) for x in pair.split(",")) for pair in raw.split(";")]
        ok = cmd_fixed(path, radii)
    elif cmd == "hold":
        ok = cmd_hold(path)
    else:
        print(__doc__)
        return 2
    return 0 if ok else 1


if __name__ == "__main__":
    sys.exit(main())
