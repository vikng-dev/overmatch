# Drive control: drivetrain truths, driver policy, gearbox algorithms (2026-11-02)

Scope: how a tank is driven, its engine, clutch, gearbox, steering and brakes. Nothing here concerns
the damage model. Labels: **MEASURED** (stated by a source's spec, test or manual), **DERIVED** (my
arithmetic, inputs shown), **RECALLED** (from memory, unverified). **[S]** marks a secondary source.
Source keys are listed at the end.

## TLDR

- The Tiger's OLVAR was **semi-automatic**. The driver picked the gear. Pushing the selector made the
  box disengage the main clutch by hydraulics, sync the shafts (throttle blip or brake), engage the
  dogs and let the clutch back in gently [W, quoting D 656/21+][S]. Upshifts need "neither throttle
  lift nor clutch" [F p.40].
- The Tigerfibel bounds what the box may do. You start in 1st–4th only. You may skip only **1–2
  gears**, and only when **warm**. Reverse is 1st–4th only. The direction lever is **locked while a
  gear above 4th is engaged** [F pp.40–41]. The repo's `shift_addressing: Direct` ("arbitrary gear")
  claims more than this.
- **Every Tiger shift cuts the drive.** So the Fibel says not to shift in swamps, on bridges or on ice
  [F p.39]. Choosing a gear in advance is part of the driver's job.
- Engine: 700 PS @ 3000 rpm and 185 mkp @ 2100 rpm (MEASURED [W][S]). Best cruise 2600 rpm, never over
  3000. The governor's second carburettor stage runs 1900–2800 rpm [F pp.31, 38]. No source gives
  base idle.
- Brake: 33 km/h stops in 12 m, which is 0.36 g (DERIVED) [F p.34]. The repo's 0.343 g is
  consistent with it.
- Later automatics:
  - A torque converter replaces the eased-in clutch (X1100: 2.2:1 at stall, MEASURED).
  - The driver selects a **range**, and output speed plus engine load schedule the shifts.
  - The X1100 **inhibits F↔R above 3–5 mph** [OD].
- Industrial control converges on four mechanisms [US5938711, E, HHC, OD]:
  - pedal × speed shift lines with a **hysteresis gap**;
  - **anti-hunt offsets** that decay after each shift;
  - a fixed **1–3 s hill hold**;
  - **reversing only at or near zero speed**.
- AMT launch control holds **engine speed at a throttle-set target** [US4081065]. An AMT shift runs:
  torque cut, clutch open, sync, engage, torque restore [US6468182].
- Games:
  - PhysX's sample toggles to reverse below 0.1 m/s with the brake held. Its tank variant requires
    the brake to be **released and pressed again** [PX].
  - Keyboard throttle is ramped by rise and fall rates (3/s and 5/s), optionally snapping to zero on
    reversal [PX, U].
- §5 lists the physical facts separately from the driver's decisions: launch gear, clutch
  engagement, shift timing, when not to shift, gear for a turn, hold, reversal, overspeed.

## 1. The Tiger I driveline as its driver operated it

### 1.1 Controls and mechanism

| Item | Fact | Source |
|---|---|---|
| Gearbox | Maybach OLVAR OG 40 12 16 A, 8F/4R, hydraulically operated | [W] citing Jentz & Doyle p.181 [S] |
| Shift sequence | Pressing selector lever 8 raises pump pressure to 17 atü, which disengages the main clutch. Dog rings are held off "until the speeds … are equal". Brake I or a cone clutch syncs the shafts. A *Gasgeber* piston blips the throttle. A membrane retarder slows clutch re-engagement. | [W] citing D 656/21+ Pt 2 pp.4–5 [S] |
| Shift time | "less than one second. Theoretically" (no datum) | [W][S] |
| Driver controls | Selector, clutch pedal (mechanical linkage to the same clutch), throttle, F–0–R direction lever, holding-brake button behind the selector, steering wheel, brake lever | [W][S]; [F pp.21, 40] |
| Holding brake | Button that brakes the gearbox input side; used in the neutral turn | [P][S]; [F p.41] |
| Governor | Centrifugal; butterflies in the inlet manifolds override the driver's throttle | [H] reproducing British Prelim. Report No. 19 [S] |
| Steering | L 600 C, two radii per gear: "the smaller the turn, the lower the gear must be" | [F p.41]; repo `tiger-transmission-data.md` |

### 1.2 Procedures in the Tigerfibel (D 656/27, 1 Aug 1943)

| Situation | Instruction | Page |
|---|---|---|
| Warm-up | 5 min in summer, 15 min in winter, until coolant reaches 50 °C and oil pressure 3 atm (MEASURED). Fast idle at **1000–1500 rpm** (MEASURED); "do not remain at base idle". Engage the clutch slowly to warm the box, "otherwise no gear change". | pp.21, 27 |
| Moving off | "One gear is always engaged. If that one is suitable … you will not need to change the selector." Otherwise: clutch down, direction lever forward, selector to **1st–4th** ("5th–8th impossible"), engage, accelerate, "slowly engage the clutch". | p.40 |
| Upshift | Selector to the detent, engage it. "Neither throttle lift nor clutch necessary. 1–2 gears can be skipped once the transmission is warmed up. Watch the tachometer!" | p.40 |
| Downshift | Selector to the detent, "brake lever by feel", engage. "Neither double-declutch nor clutch necessary." Same skip rule. | p.40 |
| Turns | "Shift down before the turn." Each gear gives one wide and one tight arc. "If it doesn't work out: brake lever, change gears." | p.41 |
| Neutral turn | Gear 1st–3rd, clutch down, pull left or right, press the large button on the gearbox | p.41 |
| Stopping | Down to 1st–4th, brake lever, clutch down, direction lever to "0", clutch up | p.41 |
| Reversing | Clutch down, direction lever back, gear 1st–4th, accelerate, release the clutch slowly. The lever "cannot be moved to 0 or reverse as long as a gear above #4 is engaged." | p.41 |
| Firing halt | Clutch down and brake lever applied; "In position": engage 2nd | p.41 |
| Swamp / bridge / ice | "Go through swiftly, do not steer or change gears." On bridges: "low gear, do not change gears, do not stop". On ice: "use inertia, do not steer". | p.39 |
| Cold | "A cold transmission cannot be shifted through the gear range"; pre-select the starting gear before a long halt | p.43 |
| Never | "Do not race her over **3000 rpm** ever." "**2600 rpm** … is what the Tiger loves" (best output for the fuel). | p.38 |

The Fibel has **no** instruction on hill starts, holding on a slope or engine braking downhill.

### 1.3 Numbers

| Quantity | Value | Label |
|---|---|---|
| Rated power | 700 PS @ 3000 rpm (HL 230 P45) | MEASURED [W] citing Spielberger [S]; [F p.2] |
| Peak torque | 185 mkp @ 2100 rpm = **1814 N·m** (185 × 9.807) | MEASURED [W][S]; conversion DERIVED. The repo's 1850 is 2 % higher. |
| Torque @ 3000 | 700 × 735.5 W / (3000·2π/60) = 1639 N·m | DERIVED; matches the repo |
| Governor | 1st stage to 1900 rpm, 2nd stage 1900–2800 rpm, closes above 2800 | MEASURED [F p.31] |
| Governed to 2500 rpm from Nov 1943 | fleet change | MEASURED [P][S] |
| Speeds | 45 km/h on roads, 20 km/h in rough terrain | MEASURED [F p.2] |
| Brake | 33 km/h stops in 12 m → 9.17²/(2·12) = **3.50 m/s² = 0.36 g** | MEASURED statement [F p.34]; DERIVED |
| Repo brake | 2·96 kN / 57 t = 3.37 m/s² = 0.343 g, a 12.5 m stop from 33 km/h | DERIVED from `tiger_1.tank.ron` |
| Base idle, motoring drag, inertia, clutch capacity, shift time | not found | repo values (600 rpm, 0.25, 4.0 kg·m², 2400 N·m, 0.31 s) are its own INFERRED |

## 2. Later-era tank automatics

| | Allison cross-drive (CD-850 family; M46–M60) | Allison X1100-3B (M1/M1A1) | Renk HSWL 354 (Leopard 2) |
|---|---|---|---|
| Converter | 4-element, two freewheel stators. Lock-up engages automatically at a set transmission speed. [TM 24-4] | 3-element; **2.2:1** stall ratio; turbine stall torque 6400 lb-ft = 8677 N·m (DERIVED). Lock-up automatic in all ranges. [OD pp.6, 10, 14] | Converter with lock-up clutch [AG][S] |
| What the driver selects | Low / High / Reverse / Neutral; low "when extra driving torque is required … heavy mud or sand" [TM 24-5] | P/N, Pivot, D (2–4), L (1–4, "shifts at a higher speed"), R (1–2) [OD p.8, 11] | Gear and direction by electric selector; "fully and semi-automatic" [AG][S] |
| Shift logic | No automatic shifting; an accumulator cushions engagement [TM 24-7] | Output-speed flyweight governor, with shift points modulated by engine load (compressor discharge pressure). Solenoid range control. On power loss it **holds the current range**. [OD pp.8–9, 11, 32] | Power-shift under load; mechanical emergency 2nd forward/reverse [AG][S] |
| Steering | Steer clutches, pressure proportional to input; pivot in neutral [TM 24-6] | Hydrostatic into combining planetaries; pivot in neutral [OD p.6, 24] | Infinitely variable hydrostatic–hydrodynamic [AG][S] |
| Brakes | Foot service brake [TM 24-1] | Wet multi-plate service **and parking** [OD p.9] | Hydrodynamic (retarder) plus mechanical service brake; no parking brake in the box [AG][S] |
| Reversal | not stated | F↔R inhibited above **3–5 mph** (1.3–2.2 m/s, DERIVED) [OD p.8] | not stated |

Soviet example: no primary source reached. RECALLED: the T-64/T-72 had two planetary side gearboxes
(7F/1R) with hydraulic servo, lever-selected by the driver, steering by downshifting or braking one
side.

**What a converter changes** [OD pp.12–13]:
- At idle it "acts as a disconnect clutch (low flow – low torque)".
- It multiplies torque most at stall and becomes a fluid coupling at the coupling point.
- A launch is slip in oil (heat bounded by the "converter out 315 °F" limit, MEASURED), not a
  clutch the driver eases in.
- On soft ground the turbine slows as resistance rises, so multiplication rises while the engine
  holds speed. The box gains torque **with no shift and no drive cut** (DERIVED from the converter
  function). The Tiger driver's equivalent was "do not change gears" [F p.39].

## 3. Control algorithms of automatic and automated transmissions

**Shift schedules.**
- Up and down lines are drawn over throttle and output speed. The gap between them is the
  hysteresis zone against hunting [US7890237, US4501171, US3726159; abstracts].
- Kickdown moves the lines. Some designs widen the band after a kickdown to stop "shift-busy"
  behaviour [US9458930, CA2874520; abstracts].
- Eaton anti-hunt (MEASURED, patent example values) [US5938711]: after an upshift, the 1250 rpm
  downshift point drops to 1050. After 4 s it rises to 1200. It returns to 1250 once output speed
  passes 1350 rpm. Near-full throttle cancels the offset.
- Grade and load adaptation: estimate mass and grade from tractive effort, and switch to a
  "performance" schedule that holds lower gears above a grade threshold [US7499784; US9014931
  abstract].

**AMT launch.**
- Smyth engages the clutch while N − A + K·dN/dt > 0 (N engine speed, A a throttle-set speed), so
  engine speed tracks a throttle-chosen target [US4081065].
- Eaton picks the **start gear from load, grade and axle ratio**, overrides bad driver choices, and
  opens or closes a hot clutch [E].

**AMT shift.** Torque is cut "on a time gradient comfortable for the driver" and the clutch opens.
During synchronisation engine torque goes "as rapidly as possible" to a low level or into drag mode.
The gear goes in when speeds match, and torque is restored [US6468182, LuK; phases are generic].
The OLVAR did the same work in hydraulics [W][S].

**Hold, creep, hill start** (Eaton, all MEASURED) [E]:
- Hill Start Aid holds "for up to **3 seconds**" on a grade over 1 % after brake release.
- Creep mode holds a slow speed at idle.
- Auto-neutral engages when the parking brake is set.
- LOW mode never upshifts and "will downshift at the earliest opportunity to provide maximum engine
  braking".
- The box forces an upshift against engine overspeed.

Car hill-hold (part of ESP) keeps brake pressure about 2 s, or until drive torque suffices [HHC][S].

**Pedal to torque.** Bosch ME7 maps pedal angle (0–100 %) × engine speed to a relative torque, which
becomes the desired engine torque [SAE 980795; map detail via quoted Bosch excerpt][S].

**Direction reversal.**
- The X1100 inhibits F↔R above 3–5 mph [OD]. Eaton shuttles reverse→forward only "at or near zero"
  [E].
- On a Linde forklift, the opposite direction brakes the truck electrically to rest, then
  accelerates it [Linde][S].
- Doosan calls direction changes above 6 km/h (1.67 m/s, DERIVED) "abusive" [S].

## 4. Games (mechanisms only; references, never targets)

- **PhysX 3.4 SampleVehicle** [PX]:
  - Auto-reverse when |v_fwd| < 0.1 m/s and |v_side| < 0.2 m/s in a forward gear, with brake pressed
    and no throttle. It also toggles if the vehicle rolls against its gear by more than 0.1 m/s with
    no input.
  - In reverse, brake and throttle **swap roles**.
  - The commented-out tank version needs rest under braking, then brake release and re-press.
  - The autobox shifts one gear at a time on engine-speed ratios, with a latency. It "disconnects the
    accelerator for the entire duration" of the shift, and the car is "in neutral" for `mSwitchTime`.
  - Key smoothing (MEASURED example values): throttle rises at 3.0/s and falls at 5.0/s; steering
    2.5/s and 5.0/s. That is 0.33 s to full and 0.2 s to release (DERIVED).
- **Unity Input Manager** [U]: *Sensitivity* is the rise rate, *Gravity* the fall rate to neutral.
  *Snap* resets the axis "to zero when pressing a button that corresponds to the opposite direction".
- **Unreal** [UE]:
  - `bReverseAsBrake`: holding reverse "also functions as brake", arcade style.
  - `StopThreshold`: auto-brakes below a speed.
  - `WrongDirectionThreshold`: auto-brakes when speed opposes the input.
- **BeamNG** [BG]:
  - Arcade and realistic modes; `smoothShiftLogicAV` decides the "arcade reverse switch and auto
    braking".
  - Throttle-dependent shift pairs, defaults 2500/5000 rpm up and 2000/3500 down (MEASURED defaults).
  - Aggression smoothing, a 0.5 s minimum between shifts, wheel-slip shift blocking.
  - Players say real automatics "stay still" on the brake instead of reversing [S].
- **Arma 3** (PhysX TankX) [AR]: `changeGearType` "effective" (shift if a better gear exists and
  effectivity < `changeGearMinEffectivity`, default 0.95) or "rpmratio". `latency` (default 2.0 s)
  must exceed `switchTime`. Read via search excerpts; direct fetch was blocked.
- **War Thunder** [WT]:
  - Automatic or manual. In manual, W is the accelerator, S the brake, Q/E shift. In automatic,
    "press backward to go backward".
  - Cruise control steps: CC R / off / CC1 / CC2 / CC MAX.
  - Devblog of 30 Oct 2026: in neutral, torque flows only through the steering mechanism.
    Dual-transmission tanks downshift the inner track. A downshift "triggers whenever the brake (or
    reverse key) is pressed while turning". Taps give gentle turns, holds hard ones.
- **WoT, Gunner HEAT PC!, Sprocket**: no developer statement found; Sprocket's community reports
  automatic-only shifting [SP][S].

## 5. The truths

### 5.1 Physical facts the drivetrain obeys

| Fact | Tiger status |
|---|---|
| Full-load torque vs rpm; a governor that overrides the throttle above a band | Two points MEASURED; governor bands MEASURED [F p.31]; curve shape between them unknown |
| Zero-throttle drag rises with rpm; engine braking needs a gear and a closed clutch | Typical only; no HL 230 motoring curve |
| The governor cuts fuel but cannot stop a hill from back-driving the crank; only gear choice limits overspeed | "Never over 3000" [F p.38]; mechanism DERIVED from [H] |
| A clutch carries torque up to its capacity and heats in proportion to slip speed × torque; a converter multiplies torque up to its stall ratio and heats oil | Tiger capacity unknown; X1100 2.2:1 MEASURED |
| A shift is a window with **no drive** (Tiger: clutch out by hydraulics), lasting until the shafts sync | Mechanism MEASURED [W][S]; duration unknown |
| Allowed transitions: start 1–4; skip at most 2, only when warm; reverse 1–4; direction lever locked above 4th | MEASURED [F pp.40–41] |
| Steering radius scales with gear; neutral turn only in 1–3 with clutch out and holding brake on | MEASURED [F p.41]; repo radii anchors [S] |
| Brake deceleration is bounded by grip and brake heat | 0.36 g from 33 km/h, DERIVED [F p.34] |
| Ground resistance caps speed; a slope hold must beat W·sin θ | 45/20 km/h MEASURED [F p.2]; slope-hold capacity unknown |
| Cold oil blocks shifting | MEASURED, qualitative [F pp.21, 43] |

### 5.2 The driver's policy layer (decisions and their inputs)

| Decision | Inputs | Tiger driver | Later automatic or controller |
|---|---|---|---|
| Launch gear | ground, grade, load | 1st–4th; keeps the engaged gear if it suits [F p.40] | Start gear from load and grade [E]; L range [OD] |
| Clutch engagement | rpm, throttle | "Accelerate, slowly engage the clutch" | rpm held at a throttle target [US4081065]; converter [OD] |
| When to shift | rpm, load | ~2600 cruise, ≤3000, 2-gear skips at most [F pp.38, 40] | Pedal × speed lines, hysteresis, anti-hunt, load modulation [OD, US5938711] |
| When **not** to shift | terrain ahead | Swamp, bridge, ice: hold the gear [F p.39] | Converter absorbs it; MANUAL mode on steep grades [E] |
| Gear for a turn | radius, speed | Downshift before the turn [F p.41] | Continuous steer; none needed [OD] |
| Stop and hold | speed, slope | ≤4th, brake, clutch, lever to 0; holding button | HSA 3 s, parking brake, auto-neutral [E, OD] |
| Reversal | speed ≈ 0 | Clutch, lever back, gear 1–4 [F p.41] | Speed inhibit [OD, E]; brake-to-rest-then-reverse [Linde][S] |
| Overspeed | rpm | Driver's judgement | Forced upshift; LOW for engine braking [E] |
| Input conditioning (games) | key state | — | Rise and fall rates, snap, release-then-repress to reverse [PX, U] |

**WWII vs later, in short.**
- The Tiger driver chose each of 8 gears, launched on an eased friction clutch, and paid a drive cut
  for every shift, so he pre-selected for the ground ahead.
- The later driver chooses a range, and a schedule chooses the gear. The converter launches, and the
  power-shift keeps drive.
- Both eras enforce reversal with interlocks: the Tiger's lever lock above 4th, the X1100's speed
  inhibit.
- Only modern trucks hold on a hill for a fixed time.

## What would settle the open facts

- **D 656/21+ Pt 2 pp.4–5 and D 656/23 p.18, Bilder 20–24.** The OLVAR sequence first-hand, with
  pressures, timing and perhaps slope instructions. [W] cites pdf pages, so a scan exists.
- **British Prelim. Report No. 19, full text** (Tank Museum). Base idle, clutch type, governor
  setting.
- **Jentz & Doyle, Panzer Tracts 6-1/6-2.** Ratio table, clutch, date of the governor change.
- **Timed audio or video of Tiger 131 shifting** (Bovington). It would measure the no-drive window
  from the blip and the roll-out, the only route to a MEASURED shift time.
- **An HL 230 test-bed curve.** Full-load curve, motoring drag, idle.
- **Operator chapters of TM 9-2350-264-10 (M1A1) and TM 9-2350-257 (M60/CD-850-6), and a RENK HSWL
  354 data sheet.** Shift speeds and interlocks.
- **A T-72 technical description**, to replace the RECALLED entry.

## Sources

- [F] *Tigerfibel* D 656/27 (1 Aug 1943), English translation 2013, OCR with "Original page N" markers:
  https://archive.org/details/tiger-i-official-wartime-crew-manual-tigerfibel-2013_202208. German
  scan: https://archive.org/details/DerGeneralinspekteurDerPanzertruppenDieTigerFibel194392S.Scan_201703
- [W][S] Wikipedia user draft "WW2 Maybach gearboxes", quoting D 656/21+ Pt 2 pp.4–5, D 656/23 p.18,
  Spielberger 1998 and Jentz & Doyle: https://en.wikipedia.org/wiki/User:MinorProphet/Draft_subpages/WW2_Maybach_gearboxes
- [H][S] http://www.alanhamby.com/maybach.shtml (reproduces Prelim. Report No. 19)
- [P][S] https://www.panzerbasics.com/panzer/01_basics/01_Tiger_E/driving.htm
- Repo: `assets/tiger_1/tiger_1.tank.ron`; `.agents/docs/design/track-model/tiger-transmission-data.md`, `transmission-design.md`
- [OD] US Army subcourse OD1710, *X1100-3B Transmission, M1A1* (per TM 9-2350-264-10): https://armyordnance.tpub.com/OD1710A/OD1710A0014.htm (pages 0007–0038)
- [TM] TM 9-8000 (1985) ch. 24: https://automotiveenginemechanics.tpub.com/TM-9-8000/TM-9-80000502.htm (to 0512)
- [AG][S] https://army-guide.com/eng/product377.html
- [E] Eaton TRDR0941 (Jan 2014): https://roadtrains.com.au/wp-content/uploads/2019/09/Eaton-Ultrashift-Drivers-Instruction-Manual.pdf
- Patents (Google Patents): US4081065 (Smyth 1978), US6468182 (LuK 2002), US5938711 (Eaton, priority
  1997). Abstracts only: US7890237, US4501171, US3726159, US9458930, CA2874520, US7499784, US9014931.
- [HHC][S] https://www.seat.com/car-terms/h/hill-hold-control; Skoda and MG manuals (manualslib)
- Bosch ME7: SAE 980795 (1998), https://www.audiclub.fi/audifinns/filedata/fetch?id=1594618; [S] https://forum.hptuners.com/showthread.php?p=835506
- [Linde][S] https://www.manualslib.com/manual/2573074/Linde-E12.html?page=45; Doosan https://www.manualslib.com/manual/2766318/Doosan-D20s-5.html?page=84 (search excerpts)
- [PX] https://archive.docs.nvidia.com/gameworks/content/gameworkslibrary/physx/guide/Manual/Vehicles.html; https://github.com/NVIDIAGameWorks/PhysX-3.4/blob/master/PhysX_3.4/Samples/SampleVehicle/SampleVehicle_VehicleController.cpp (ll.160–222, 441–520)
- [U] https://docs.unity.com/en-us/engine/current/manual/input/legacy/class-input-manager
- [UE] https://docs.unrealengine.com/4.27/en-US/API/Plugins/ChaosVehicles/UChaosVehicleMovementComponent/bReverseAsBrake/index.html; https://docs.unrealengine.com/4.26/en-US/API/Plugins/PhysXVehicles/UWheeledVehicleMovementComponent/
- [BG] https://documentation.beamng.com/modding/vehicle/vehicle_system/controller/main/vehiclecontroller; [S] https://www.beamng.com/threads/what-is-the-purpose-of-arcade-gear-box-anyways.75708/
- [AR] https://community.bistudio.com/wiki/Arma_3_Vehicle_Handling_Configuration; https://community.bistudio.com/wiki/Arma_3_Tanks_Config_Guidelines
- [WT] https://warthunder.com/en/news/10298-development-improvements-to-tank-mobility-mechanics-new-steering-mechanisms-and-engine-refinements-en; [S] https://wiki.warthunder.com/controls/8411-transmission-modes-and-cruise-control
- [SP][S] https://steamcommunity.com/app/1674170/discussions/0/3040481180147941186
