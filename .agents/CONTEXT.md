# Overmatch

A realistic 3D multiplayer tank game (Bevy/Rust), single-player vertical slice in progress.
This file is the project glossary — terms only. Decisions live in `.agents/docs/adr/`.

## Aiming

**Sight** (reticle):
The camera's view direction, marked by the fixed dot at screen center. Where the player is *looking*.
_Avoid_: crosshair, cursor

**Aim point**:
The ground point the gun is *commanded* to hit, resolved from the camera's screen-center ray and stored in the hull's local frame. Intent — where we've told the gun to go, not where it actually points.
_Avoid_: target, aim target

**Bore axis**:
The line straight down the barrel (the muzzle's forward direction); shells depart along it.
_Avoid_: gun line, muzzle direction

**Bore point**:
Where the bore axis currently meets the ground; what the green bore indicator marks. The gun's *reality*, as opposed to the aim point's intent.
_Avoid_: bore aim point

**Target**:
A designated thing to engage (a locked-on or selected enemy). Reserved for future designation; not yet implemented. Never use it for the commanded ground point — that is the aim point.
_Avoid_: using "target" for the aim point

## Tank rig

**Hull**:
The tank body — the chassis the turret sits on, and the frame all aim math is computed relative to.

**Turret**:
The rotating top; yaws to bear on the aim point.

**Gun**:
The gun mount — the elevation pivot and the (stationary) mantlet. Elevates in pitch.
_Avoid_: barrel (that is a separate, recoiling node)

**Gun barrel**:
The recoiling barrel — child of the Gun, parent of the Muzzle. Slides under recoil while the Gun mount stays put.

**Muzzle**:
The barrel's tip. Its forward is the bore axis; shells spawn here.

## Gunnery

**Servo**:
A 1-DOF *kinematic* rotational motor with a trapezoidal motion profile, slewing turret yaw / gun pitch toward a commanded angle. Not a physics joint — we drive it ourselves.

**Recoil**:
The barrel's rearward kick on firing and its damped spring back to battery — a 1-DOF translational motor, the bore-axis cousin of the Servo.

**Battery**:
The barrel's rest (fully forward) position, to which recoil returns. "Return to battery."

**Stabilization**:
Keeping the gun's lay steady against hull motion. Three regimes, by what is held fixed:
- *Unstabilized* — the gun holds a hull-relative bearing and sweeps as the hull moves (WW2). Aim stored hull-local.
- *Directional stabilization* — the gun holds a fixed world *direction* (a ray: bearing + elevation), counter-rotating against hull motion but not tracking a point while driving (the modern two-plane stabilizer; fire-on-the-move). Aim stored as a world ray.
- *Point stabilization* — the gun holds a fixed world *point* (a position), re-laying as the hull rotates *and* translates so it tracks the spot through parallax (lock-on / FCS auto-tracker). Aim stored as a world point.
Today's default is unstabilized; the other two are deliberate later mechanics.
_Avoid_: "stab" (write it out)

## Driving

**Running gear**:
The whole ground-contact mechanism of one side — roadwheels, track, sprocket, idler.

**Roadwheel**:
A load-bearing wheel of the running gear; the wheels whose share of the tank's weight presses the track onto the ground.
_Avoid_: wheel (ambiguous with the sprocket / idler / return rollers, which carry no ground load)

**Sprocket / Idler**:
The drive sprocket (where engine torque enters the track) and the idler (track tensioner) at the ends of each side. They shape the track loop but bear no ground load.
_Avoid_: drive wheel

**Track**:
The continuous belt around the running gear. In the sim it is **cosmetic** — it carries no physics; locomotion is modelled at the roadwheels.
_Avoid_: tread, caterpillar

**Contact station**:
A longitudinal point where a roadwheel transfers load to the ground; the unit at which both suspension and track-against-ground friction are sampled.
_Avoid_: contact patch

**Effective radius**:
The hub-centre-to-ground distance — wheel radius plus track thickness — shared by the suspension and the visual track so they agree on where the ground is.
_Avoid_: wheel radius (that is only part of it)

**Ride height**:
The hull's resting height, set by where the loaded suspension settles each roadwheel above the ground.

**Suspension travel**:
A roadwheel's vertical range between full compression (bump) and full extension (droop).

**Differential thrust**:
Independent longitudinal force per track; steering arises from the left–right difference, not a separate turn input.

**Skid steer**:
Turning by differential thrust, resisted by the tracks shearing sideways against the ground.

**Neutral steer**:
Pivoting in place with the tracks counter-rotating — equal and opposite thrust giving a pure yaw couple and zero net travel.
_Avoid_: pivot turn, neutral turn

**Friction circle**:
The shared grip budget at a contact station — longitudinal and lateral force together capped at μ × normal load.
_Avoid_: friction ellipse
