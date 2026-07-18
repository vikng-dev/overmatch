# Transmission declaration — design brief (codex, 2026-07-18)


Put a single two-output transmission module between contact-force calculation and belt integration:

\[
I\dot v_i = Q_i-R_i,\qquad i\in\{L,R\}
\]

`R_i` remains the contact-law reaction. The transmission jointly computes `Q_L,Q_R`; it must not remain two independent calls to the symmetric governor in [forces.rs](/Users/Yan/Desktop/github/vikng-dev/personal/overmatch/src/track/forces.rs:762).

Use:

\[
m=(v_L+v_R)/2,\qquad d=(v_L-v_R)/2
\]

where `m` is propulsion speed and `d` is steering difference.

## 1. Architecture menu

### A. Clutch-and-brake — T-34

\[
Q_i=C_i+B_i+E_i
\]

- `C_i`: clutch force. While slipping, \(|C_i|\le C_{\max}\) and it opposes clutch slip; when locked it is the constraint force enforcing `v_i = gearbox output speed`.
- `B_i`: track brake, opposing motion with \(|B_i|\le B_{\max}\).
- `E_i`: engine drag, only while that clutch is engaged.
- Both clutches share one engine torque/power budget. Opening one clutch lets the remaining side receive the available common-engine power.

This matches the real staged control: pull first disengages the steering clutch, then applies the brake ([T-34 service manual](https://www.allworldwars.com/T-34%20Tank%20Service%20Manual.html?level=1)).

The symmetric governor is replaced by three asymmetric paths:

\[
F_{\rm drive}\le \min(F_{\rm gear},P_{\rm engine}/|v|),\quad
F_{\rm brake}\le B_{\max},\quad
F_{\rm engdrag}=T_{\rm drag}(\omega)g/r_s
\]

Per-vehicle data: engine torque curve, ratios/final drive, clutch capacity, sprocket brake torque, efficiencies. No neutral counter-rotation.

### B. Fixed-radius regenerative/geared steering — Tiger L600 style

For steering sign \(s\) and selected gear/detent:

\[
d-s\kappa_{g,j}m=0,\qquad
\kappa=b/R_{g,j}
\]

Equivalently:

\[
r=v_{\rm inner}/v_{\rm outer}=(R-b)/(R+b)
\]

The lossless constraint contribution is:

\[
Q_L=F_p/2+\lambda(1-s\kappa)/2
\]
\[
Q_R=F_p/2-\lambda(1+s\kappa)/2
\]

It does zero ideal constraint work; inner-track negative power returns to the outer track. Efficiency adds a loss \((1-\eta)|P_{\rm transfer}|\).

A continuous steer axis must select discrete states: straight → wide radius → tight radius, with hysteresis. Axis magnitude may control engagement during transition, but interpolating radii continuously is a hybrid, not an L600 model.

Per-vehicle data: gear ratios, two radii/ratios per gear, output and steering-member torque capacities, efficiency, emergency/service brakes.

### C. Continuous regenerative — Merritt-Brown / HSWL family

\[
2I\dot m=F_p-(R_L+R_R)
\]
\[
2I\dot d=F_s-(R_L-R_R)
\]

with output forces:

\[
Q_L=(F_p+F_s)/2,\qquad Q_R=(F_p-F_s)/2
\]

Power is:

\[
P_{\rm outputs}=F_pm+F_sd
\]

Negative power from one output offsets positive power at the other, less losses. At `m=0`, the steering shaft alone produces `vL=+d`, `vR=-d`: genuine neutral steer. Modern RENK units explicitly use infinitely variable hydrostatic superimposed steering and support variable-speed pivot turns ([official HSWL 106 data](https://staging.renk.com/_Resources/Persistent/3/b/3/b/3b3bb5892d9d3a81fbb8fbbeabd1b0231abde5a5/HSWL%20106.pdf)).

Per-vehicle data: propulsion gears, steering-shaft ratio/displacement, steering torque and power capacities, efficiencies, engine/retarder curves, service-brake torque.

### D. Arcade-honest hybrid

Use C’s `m/d` power-flow math with real gearbox ratios, but omit explicit planetary/hydraulic shaft simulation:

- Real engine torque/power envelope.
- Real gear and final-drive ratios.
- Declared steering-output torque/power capacity.
- Regenerative power accounting.
- Declared brakes.
- Continuous `d` command.

This preserves physical emergence while avoiding clutch-pressure, hydraulic-pressure and planetary-carrier state.

## 2. Coupling and netcode

Keep authoritative `vL,vR`.

Reparameterizing to `m,d` saves no state—two floats remain—and both belt phases must still remain independent. It would also force conversions around every per-side contact calculation. Use `m,d` internally inside the transmission module only.

Minimal scheduling change:

1. Evaluate both contact patches at their pre-tick belt speeds; obtain `R_L,R_R`.
2. Solve the joint transmission once.
3. Integrate both speeds simultaneously; advect both existing phases.

Constraint forces are algebraic and need not replicate. REV 14 should add only genuine path-dependent transmission state:

- selected gear;
- shift countdown/phase;
- steering detent for fixed-radius boxes;
- clutch lock/slip state if modeled;
- engine speed only if it cannot be derived while shifting.

That state must be constructed from tank data at spawn, alongside the existing rollback state.

## 3. Braking law

Author brake torque, then convert at the sprocket:

\[
B_{\max}=T_{\rm brake,sprocket}/r_s
\]

The Tiger’s declared \(r_s=0.3931\) m means 250 kN would equal **98.3 kN·m DERIVED** brake torque. That is only a comparison value, not valid provenance; ship values must come from an actual vehicle brake/output-torque source.

Do not attenuate ground reaction with `(1-h)R`. Always apply `-R`, and turn ADR-0026 hold into a capacity-limited static brake:

\[
B_i=\operatorname{clamp}(R_i-Q_{other,i},-hB_{\max},hB_{\max})
\]

while stationary; when sliding, it saturates opposing motion. Thus hold remains smooth, but a slope exceeding brake capacity back-drives the belt honestly. The 20° Tiger load is about **95.6 kN/side DERIVED**, so brake capacity becomes testable rather than infinite.

Retarding paths:

- Clutch/brake: steering/service brake; engine drag only with clutch engaged; no recovery.
- Fixed-radius differential: regenerative steering constraint; common-mode engine drag; separate service/emergency brakes.
- Continuous regenerative: steering shaft, common-mode engine drag/retarder, service/parking brakes.
- Engine braking should be a reflected drag-torque curve \(T_{\rm drag}(\omega)\), not the negative half of rated engine power. It only becomes “power-like” after multiplying torque by engine speed.

## 4. Gearing

The existing envelope has a knee at:

\[
260\,{\rm kW}/250\,{\rm kN}=1.04\ {\rm m/s}
\]

Above that, it behaves like an ideal continuously variable transmission. It captures broad acceleration and hill-climb limits, but not:

- per-gear force bands;
- engine RPM and torque-curve shape;
- torque interruption during shifts;
- gear-dependent engine braking;
- reverse ratios;
- the Tiger’s gear-dependent steering radii.

Therefore a gear set is warranted for the Tiger: without it, an “L600” model is only cosmetic. The minimum rollback cost is gear plus shift phase/ticks; engine RPM may remain derived outside clutch-slip/shift periods.

## 5. Feel and yaw budget

For comparison, these are **DERIVED instantaneous/onset ceilings**, with \(b=1.49\) m, 250 kN/output, 520 kW total engine, and the repo-recorded ~478 kN·m scrub. The clutch row temporarily assumes a 250 kN brake solely for comparison.

| Architecture | 4 m/s | 8 m/s | Against 478 kN·m |
|---|---:|---:|---|
| Current symmetric governor | 194 kN·m | 97 kN·m | Far short |
| Clutch/brake | 566 kN·m | 469 kN·m | Strong at 4; marginal at 8 |
| Fixed-ratio regenerative | \(372.5(1+r)\) kN·m | same capacity law | 373–745; e.g. `r=.4` → 521 |
| Continuous/hybrid regenerative, turn onset | 745 kN·m | 745 kN·m | Comfortable margin |

The current neutral-steer launch is **745−478=267 kN·m excess DERIVED**, explaining the snap. Under the constant-power curve, scrub balance occurs around belt difference `d=1.62 m/s`, or **1.09 rad/s yaw DERIVED**. A real pivot ratio, engine curve and reflected gear inertia should make this statelier—not another slew dial.

For regenerative steering, sustainable rather than onset torque depends on turn radius:

\[
P_{\rm yaw}=M_{\rm scrub}V/R
\]

With 520 kW, sustaining 478 kN·m requires approximately `R ≥ 3.68 m` at 4 m/s and `R ≥ 7.35 m` at 8 m/s. Tighter commands must slow the tank. That is the desirable emerging high-speed behavior: strong turn-in, followed by physically required speed loss.

## Ranked recommendation

1. **Tiger: fixed-radius, geared regenerative model behind the joint transmission seam.** Historically characteristic, fixes high-speed sluggishness, and derives stately pivot behavior from ratios and power.
2. **Arcade default: geared regenerative `m/d` hybrid.** Best controls and least mechanism state while retaining honest energy flow.
3. **Full continuous Merritt-Brown/HSWL adapter** for vehicles that actually use it.
4. **Clutch-and-brake adapter** only for T-34-class vehicles; its lack of neutral steer and heat-wasting turns should remain genuine traits.