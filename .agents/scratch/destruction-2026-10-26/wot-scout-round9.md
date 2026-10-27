# WoT scout, round 9: the toppled object, hull marks, break-off (received 2026-10-28)

Source: the `master` session in `../bounty`, by message, static decompile of its own WoT client plus
its readable scripts (writeup there: `re-study/wot/README.md`). Marked as the session gave it:
ENGINE / SCRIPT / DATA, READ / INFERRED. Design input only; WoT is arcade, structure not numbers.
Subject: the game's destruction model and hull feedback.

## 1. A toppled object, end to end (resolved)
- **Data, not mesh** [SCRIPT, READ]: the lean solve's terms come from authored per-destructible
  falling parameters (mass, height, stiffness, spring angle, spring and air resistance, bury
  depth), read from the destructibles' XML; the instance's scale is the only runtime input
  (mass ∝ scale³, stiffness ∝ scale², height ∝ scale).
- **The rest pose reads the world** [ENGINE, READ]: a pitch constraint comes from a geometry query in
  the fall direction (a contact position and normal) and returns `(pitchConstr, collisionFlags)`.
  The spring solve finds the gravity-against-stiffness equilibrium; the constraint caps it to what
  the ground under the fall path allows; the flags report obstacles (the script picks touchdown
  effects from them).
- **Fall direction on the wire** [SCRIPT, READ; server choice INFERRED]: a 6-bit quantised yaw plus a
  2-bit fall speed, picked by the server, never computed by the client.
- **State, not transform** [SCRIPT, READ]: replicated arrays of fallen columns, fallen trees,
  destroyed fragiles and destroyed modules on one area entity; each record is 1–2 bytes (a tree's
  also packs its pitch constraint); the client diffs the array and derives the whole fall and rest
  pose. (Overmatch's `DestructionLedger` has the same shape: one word per tree, clients derive.)
- **Animation** [SCRIPT, READ]: a pendulum ODE at 60 Hz (gravity torque, torsional spring, air
  resistance) from an initial pitch speed set by the fall speed, until the pitch reaches the
  constraint (touchdown). Far or unanimated objects skip the ODE and snap to the fixed-point
  solve's rest angle: the solve is the no-animation shortcut to the same equilibrium.
- **Collision changes at touchdown, not at the fall's start** [SCRIPT + ENGINE, READ]: during the
  fall the object is kinematic; landing finalises it into the physics world. Bushes are only
  deactivated. (Overmatch's sim collider goes down at the transition tick; playtest fork A14.)
- **Blocking after the fall** [ENGINE anchors READ, behaviour INFERRED]: destructibles sit in a
  collision category gameplay queries may ignore; consistent with WoT's fallen trees not blocking
  shots or sight. Not proven.

## 2. Hull marks
- **Placement** [ENGINE + SCRIPT, READ]: a decal is a ray projection along the shot's entry segment in
  the hit component's local frame onto its collision component; a grazing or short segment's start
  is pushed back so the ray still lands.
- **Texture and size** [DATA; calibre → size INFERRED]: a sticker id carried in the server's hit record
  picks the decal; size is the decal asset's own.
- **Cap and eviction** [SCRIPT, READ; bound INFERRED]: the marks mirror a server-replicated array per
  vehicle; the client adds and removes as the set changes, so the cap is the server's.
- **LOD** [READ]: decals drop out beyond the vehicle type's emblem LOD distance.

## 3. Break-off
Havok's own break-off modifier over authored breakable-constraint data [ENGINE anchor READ,
threshold DATA]; no WG-side threshold found; shell hits and vehicle contact INFERRED to arrive as
ordinary contact impulses into the same modifier.
