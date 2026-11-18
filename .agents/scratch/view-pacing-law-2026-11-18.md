**Decision: option (b), made exact.** Keep attempt 1's critically damped loop, replace its fixed `1.0` with a rate estimate `q` read from the stamp map at the target point (not from the cursor, not from the window), and make the stored rate absolute. In the linear regime this equals the type-2 loop of option (a), so R1 and R2 both hold with one derived constant. R3's "no slower than main" must yield by the loop's memory (simulated 0–0.21 s); every other requirement is met.

All SIMULATED numbers come from an in-memory Python model of `view_clock` (120 Hz frames, 40 ms link, v = 0.2, T = 15.625 ms, W = 10 s). It reproduces the lane report's MEASURED rows for main and attempt 1 (430/215; 640/480, 480/384, 240/240, 96/96; 5.332 ms lag at 1.1×; 1440 and 480 alternating). No Rust was run.

## Why the others lose
- **(a) integrator on cursor error.** Any loop whose rate estimate integrates lead `z` repays every lag as lead once `q` returns to its value (∫z dt = 0). Its own manoeuvres then end in lead. SIMULATED with a manifold anti-windup: 15 holds after 0.5→1.0 (main 0), 21 after a forced delay step, re-contacts until 1.8 s after a 200 ms stall, holds until 11.5 s after 1.0→0.75.
- **(c) filtering S alone** leaves the delay slew and the catch-up landing as rate steps, so a cursor loop is still needed.
- **Attempt 2's window rate** lags by W and carries stall bias.

## The law
Notation: target point τ = min(now − Δ, newest stamp); θ = S⁻¹(τ)·T; map slope σ = dθ/dτ; lead z = c·T − θ; cursor rate u.

```
dq/dτ = ω(σ − q)                 q: stream rate, learned from map facts only
du/dt = −2ω(u − q) − ω² z        attempt 1's loop about q; u is absolute
```
With Δ still, U/Θ = s(3ω²s + ω³)/(s+ω)³: a triple pole, type 2, so zero steady lead at any speed. `q` never sees the cursor, so holds, caps and cuts cannot wind it up. Δ slews are not in σ, so the cursor's own manoeuvres stay type 1 and do not overshoot.

**Per-frame update** (h = frame length, a = e^(−ωh)):
```
Δτ = τ − τ_prev;  if Δτ > 0:  q ← clamp(q + (1 − e^(−ωΔτ))·((θ − θ_prev)/Δτ − q), 0, 1+v)
if previous frame was held:  u ← q
ℓ = (c_prev·T − θ) + q·h;   r = u − q;   b = r + ωℓ
ℓ⁺ = (ℓ + b·h)·a;   u ← clamp(q + (r − ω·b·h)·a, 0, 1+v)
advance = clamp(ℓ⁺ − ℓ + q·h, 0, (1+v)·h);   next = min(c_prev + advance/T, newest)
held = c_prev + advance/T > newest + quantum
```
- **Stability:** the estimator gain is in [0,1); the loop's eigenvalue is e^(−ωh), double; h = 0 is the identity. SIMULATED at 30 Hz, random 2–60 ms, 250 ms and zero-length frames: monotone, never past newest, never above (1+v)h.
- **Cut or first frame:** c = target, u = q. The first q is the slope across the stamps retained at that moment, so no `1.0` is authored.
- **Starved target:** Δτ = 0, so q freezes; the release frame's update spans the gap with its true mean slope, which keeps q unbiased.
- **Anti-windup:** only the two clamps above. Leaving the 1+v cap gives z = (z₀ + (z₀ + r/ω)x)e^(−x) with both coefficients negative, so no overshoot (SIMULATED 0.0000 ms lead after a 200 ms stall and after a 100 ms delay fall).

**State:** `rate` (now u), `stream_rate` (q), and the previous target point (τ, θ). `window_rate` is deleted. `held` is read before it is reset.

## The one constant
The rate kernel is h(x) = ω²(3 − 5x + x²)e^(−x), x = ωt, with zero integral and ‖h‖₁ = 2κω.

κ = (√13 − 2)e^(−(5−√13)/2) + (√13 + 2)e^(−(5+√13)/2) = 0.875358 (DERIVED, form constant).

Phase noise of spread p gives |rate − s| ≤ κωp. Requiring attempt 1's bound v·p/T gives **ω = v/(κT) = 14.62 s⁻¹, 1/ω = 68.4 ms** (Resolution, DERIVED from v and T). The estimator using the same ω is the equal-pole form (ASSUMED as form, like critical damping; not optimised). There is no other number.

## Bounds
| Req | Bound (DERIVED) | SIMULATED |
|---|---|---|
| R1 | \|rate − s\| ≤ v·p/T: 0.05 at p = T/4, 0.2 at p = T; cursor within 0.75p of the mean line | worst-case bang-bang phase 0.0480–0.0499 at 60–1000 Hz; jittered fixture 0.9716–1.0265, 0 holds (main 0.80–1.20) |
| R2 | 0 < s < 1+v, zero steady lead | 0.5–1.19: rate = s to 4 decimals, 0 holds, lead 0.000 ms |
| R3 speed step δ | lead peaks at 0.840·\|δ\|/ω, 111 ms after: 28.7 ms for δ = 0.5; under 0.1 ms by 0.73 s; rate change ≤ 0.80·\|δ\|·ω = 5.85 s⁻² | 27.2 ms at 108 ms; 0.048 per frame |
| R3 delay slew | rate change ≤ v·ω/e = 1.08 s⁻²; lag 2v/ω = 27.4 ms during the slew, none after | 0.009 per frame; 26.4 ms |
| R3 stall in/out of W | none: no window in the rate path | rate 1.00000 through a 1 s stall's exit |

Speed changes against main (last hold after the step; held frames over 20 s):

| Step | Main | This law |
|---|---|---|
| 1.0→0.5 | 6.758 s; 430 | 6.883 s; 116 |
| 1.0→0.75 | 5.767 s; 138 | 5.808 s; 140 |
| 1.0→0.9 | 5.300 s; 76 | 5.508 s; 29 |
| 0.5→1.0 | none | none |

**Why R3 yields.** At a zero-slack link, main's holds end the instant Δ finishes slewing, because main stops dead when the target decelerates. A law meeting R1 has a rate response with L1 norm ≤ 2v/T, so it cannot follow a rate step in under about T/v = 78 ms; while decelerating it leads, and at zero slack any lead is a contact. DERIVED worst case: lead after the slew ≤ (2vs/ω)(1+x)e^(−x), under one cursor quantum by x = 14.4, i.e. 0.98 s. With slack above the lead there is no excess.

**Cost against attempt 1.** ω falls from 34.8 to 14.6 because the R1 budget now covers the rate path. Slew lag rises from 11.5 to 27.4 ms. A 200 ms stall closes to within 1 ms in 1.09 s (main 0.82 s).

## R4 and R6
- **Hold flag:** one condition, the rate-limited request exceeds the newest tick by more than a quantum. A starved cursor always requests on (u = q > 0), so every starved frame is held. A request that lands exactly on a clamped target is not held, which is review 2's counterexample.
- **Release: u = q (r = 0).** A hold is a gap in facts, not a change in the world's speed. From r = 0 the loop gives z = z₀(1+ωt)e^(−ωt), single-signed, so a release cannot overshoot into the next contact. It is also the least rate deviation among the non-overshooting states.
- **Rejected, r = −ωz:** also monotone, but it is the fastest admissible approach. SIMULATED 830 against 480 held frames (alternating, constant link) and 169 against 116 (1.0→0.5).
- **Rejected, rest:** loses up to q/(eω) = 25 ms after every hold and dips to 0.11×. It holds less only by silently adding delay.

## What the estimator gap still costs
At a constant link (zero slack) with quarter-tick production phase, SIMULATED over 60 s:

| Production | Main | This law |
|---|---|---|
| Alternating | 1440 held, rate 0.34–1.20 | 480 held, rate 0.79–1.03 |
| Jittered | 311 held | 187 held |

A slowdown still holds until the median flips at W/(1+s). Pacing cannot remove these; the fix remains the owner's (lateness measured from an ideal production schedule).

## Tests
Each mounts `view_clock::install`; bounds are computed in the test from v, T, W and κ's closed form.
1. **Irregular production stays in band.** |rate − 1| ≤ v(p + Δ-range)/T + quantisation, no holds. Kills main's per-segment rule (0.80–1.20).
2. **Regular speeds 0.5 to 1 + v/2.** No hold; lead within quantisation; rate = s. Kills fixed feedforward 1.0 (640 holds at 0.5×).
3. **Speed step locks in.** No hold after W/(1+s) + link + frame + (1/s − 1)T/v + 14.4/ω (7.8 s for s = 0.5); lead ≤ 0.840·|δ|/ω. Kills window-rate feedforward (holds to 9.9 s).
4. **Stall crossing the window.** A 1 s stall, then |rate − 1| ≤ quantisation across its entry to and exit from W. Kills window-rate feedforward.
5. **Rate continuity.** Away from held frames and cuts, the frame-to-frame change is ≤ (0.80·|δ|·ω + v·ω/e)·h on speed and delay steps. Kills per-segment and a base-relative rate state.
6. **Hold flag, forced states.** (a) Cut lands half a tick below newest, next frame's target clamps: not held. (b) Silence: every frame held. (c) Leading cursor reaches newest while the target is behind it: held. Kills a flag that fires on clamped targets, and a time-domain-only flag.
7. **Release.** After a 200 ms stall: one hold episode, lead never positive, within 1 ms by B/v + 6/ω. Kills the rest and r = −ωz releases by their derived lag and catch-up rate.
8. **Hard limits** under frame lengths 0, 2–60 ms and 250 ms: monotone, never past newest, never above (1+v)h.

## Not verified
- **Cut margin:** q dips during a stall, so the catch-up ramps and peak backlog runs about 20–25 ms above main's. With debt exactly at the 250 ms cap, the model cut once where main did not. Run the UDP 1 s capped stall and expect 0 cuts.
- **After a cut,** the burst leaves q near the cap. DERIVED lead ≤ 0.54·v/ω = 7.4 ms (SIMULATED 4.3 ms), which re-contacts at zero slack for about 0.4 s.
- **All SIMULATED rows** need re-measuring in `pacing.rs`.

## Recommendation (paste into the builder brief)
In `src/net/view_clock.rs` on `lane/view-rate`, delete `window_rate` and replace the base with a fact-only estimate: each frame, take the target point τ = min(now − Δ, newest stamp) and θ = S⁻¹(τ)·T, and when τ advanced by Δτ set `stream_rate ← clamp(q + (1 − e^(−ωΔτ))·((θ − θ_prev)/Δτ − q), 0, 1+v)`. Initialise it from the slope across the stamps retained at the first cursor frame. Store the previous target point. Keep `pace`'s critically damped step about `q`, but store `rate` as the absolute cursor rate (r = rate − q inside the step, clamp the result to [0, 1+v]), so a change of q never steps playback. Set ω = v/(κT) with κ = (√13−2)e^(−(5−√13)/2) + (√13+2)e^(−(5+√13)/2), labelled Resolution with the L1 derivation. Set `held` only when the rate-limited request exceeds the newest tick by more than a quantum, and start the frame after a held frame, and a cut, from `rate = q`. Rewrite `pacing.rs` to the eight tests above with independently derived bounds, re-measure every SIMULATED row, and run the UDP 1 s capped stall to confirm 0 cuts. In the report, state that release is a rate step from 0 to q by decision, and that a slowdown's last hold may trail main's by up to 14.4/ω ≈ 1 s (simulated ≤ 0.21 s) because no law meeting R1 can stop as abruptly as main.

### Critical Files for Implementation
- /Users/Yan/github/vikng-dev/overmatch/src/net/view_clock.rs
- /Users/Yan/github/vikng-dev/overmatch/src/net/view_clock/pacing.rs
- /Users/Yan/github/vikng-dev/overmatch/.agents/scratch/view-rate-396.md
- /Users/Yan/github/vikng-dev/overmatch/.agents/docs/design/net-motion-contract.md
- /Users/Yan/github/vikng-dev/overmatch/src/net/frontier.rs

## As built (2026-11-18)

Two points differ from the text above; `.agents/docs/design/net-motion-contract.md` owns the current statement.

- **Target point.** τ is also clamped to the oldest retained stamp, so the inverse is always defined and the stream rate never reads time the map does not hold.
- **1 s capped stall.** The expected 0 cuts did not hold on real UDP: it cuts once (MEASURED peak backlog 263.7 ms against the 250 ms floor). The cut floor and the server's debt cap both read `admission_s`, and the slow-start ramp adds 25–30 ms. Accepted; the law is unchanged.
