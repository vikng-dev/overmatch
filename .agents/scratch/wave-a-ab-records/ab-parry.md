# Parry game-level A/B record (wave-A) — 2026-07-11

Bay branch wave-a/parry-alone = d7d103e + parry3d fork override (ec81280) + SPIKE_RAW_TOI lever
(gate-off of the witness reconstruction in sphere_cast_ground_contact). All greps zero.

## (a) Tripwire — FIRED as designed

`cargo test --test spherecast_scale` vs patched fork: `spherecast_reconstruction_beats_raw_toi_at_scale`
FAILS — raw TOI error at 5 m half-extent now 0.1132 mm (was 0.246 mm; 500 m was 139 mm), so the
reconstruction no longer beats raw TOI ("witness-geometry reconstruction degraded at half-extent 5:
max error 0.000113248825 m" = exactly the patch's +0.113 mm overshoot from the crate review).
The 3 fallback-behavior tests still pass. Retirement condition formally met.

## (b) At-rest idle (flat spawn, SPIKE_SIM_IDLE, 80/10) — patched+raw-TOI ≡ workaround-on

| condition | p.y spread (settle 400) | per-tick |dy| p50/p99/max (mm) | limit cycle? |
|---|---|---|---|
| SPIKE_RAW_TOI=1 (patched parry, workaround OFF) | 0.195 mm | 0.0053 / 0.0265 / 0.0399 | none |
| workaround ON (same build) | 0.165 mm | 0.0030 / 0.0289 / 0.0436 | none |

Same class; per-tick p99 ~0.027 mm both ways (the handoff's ≲0.02 mm bar read as per-tick motion
— both conditions sit at it; as window max-min both read ~0.2 mm). No trace of the historic
~12 mm / 0.8 Hz limit cycle. Client and server traces identical metrics.

## (c) Full-course divergence (long, 80/10, SPIKE_RAW_TOI=1, patched) — at baseline

94.85% match (1198/1263), physics bit-exact every shared tick, mismatches hsim-only (65).
Baseline (workaround on): 94.45% unpatched / 94.69% avian-patched. At-or-above. PASS.

## Verdict

RETIRE `sphere_cast_ground_contact`'s witness reconstruction when the parry fix ships in a
release we pin (keep until then — unpatched parry still has the 139 mm defect). The tripwire
test will flag the moment the upgrade lands. Note for the retirement PR: the post-fix TOI can
overshoot ≤ ~0.12 mm (two-sided error) — irrelevant to suspension at these magnitudes, but the
old "TOI is provably never long" comment must go.
