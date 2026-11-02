# Drive control: owner decisions (2026-11-02)

The game's driving model. Decisions taken with the owner during the redesign; the design note (`drive-control-design-2026-11-02.md`) and the lanes build on these.

1. **Intent is a signed pedal:** the share of engine power asked for. A modelled Driver works throttle, gear, clutch and brakes by one generic law each; players never choose gears. Accepted ("sounds very good").
2. **No brake key, to try in play:**
   - W held then released: coast, throttle ramps to 0, no brake.
   - S held while moving forward: full service brake to a complete stop. At the stop a **0.5 s window** (tunable, possibly a player setting) holds the brakes; then the direction change (the gearbox's own shift time, Unknown, ~0.3 s) and the reverse throttle begin. A player who releases S inside the window gets a full stop with brakes held. Symmetric for W while reversing. Confirmed.
3. **Taps:** a quick tap on W/A/S/D steps that axis by **25 %**, and the level persists (a notch). A tap is a press shorter than ~0.15 s (Taste). The notch shows in-game (drive gauge, engine pitch).
   - **Taps form a signed lever** (confirmed): an S tap at +50 % steps to +25 %, then 0, then −25 % (which runs the stop-then-reverse window). A/D likewise for steer.
4. **A hold overrides taps:** while held, the axis follows the hold; on release the notch is cleared (throttle ramps to 0, steer to straight); the next tap starts from 0. Accepted.
5. **Held for later:** an emergency brake that stops the tank without the Driver (crew skill, gameplay rule or tank fact), or brakes applying when the Driver station is knocked out.
6. **The design note is accepted** (`drive-control-design-2026-11-02.md`): the drivetrain laws stay; four Driver laws (throttle, gear, clutch, brakes) replace the scheduler, the hill-hold latch, the hold blend and the direction gate; the gear law carries its three amendments (C1: a lower gear only on a deficit beyond M or a lugging engine; C2: throttle off holds the gear; C3: launch in the highest start gear 1–4 whose spare clears M, chosen while parked); the decision tree's recommended answers stand; migration in six lanes (§6).
