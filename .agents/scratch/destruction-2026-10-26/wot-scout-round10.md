# WoT scout, round 10: the rest-pose query and a fallen object's collision (received 2026-10-28)

Source: the WoT study session in `../bounty` (now named `wot`), by message, static decompile of its
own client plus its readable scripts (writeup there: `re-study/wot/README.md`). Marked as given:
ENGINE / SCRIPT / DATA, READ / INFERRED. Design input only. Subject: the game's destruction model.
The session is holding until the next topic is sent.

## 1. What the rest-pose query samples [ENGINE, READ]
- Segment casts, not one ray and not a swept shape: an outward ray along the fall direction at the
  object's top height, a downward ray at the far end for the ground or obstacle height there, then
  an angular bisection that re-casts at candidate pitches, halving the step until under an
  epsilon. The constraint is a constant minus `atan(drop / length)`.
- The object is a THIN LINE of its scaled characteristic length along the fall direction: no
  radius or width term. The casts carry a filter so the falling object hits neither itself nor
  other destructibles. (Overmatch's `trees::resting_pose` samples each segment boundary lowered by
  its local radius, scans the tilt range at a fixed step before bisecting, and reads the terrain
  only.)

## 2. Who derives a tree's rest pose [SCRIPT, READ — definitive]
- **Tree:** the pitch constraint comes FROM the wire; the client re-runs the query but discards the
  pitch and keeps only the collision flags (for touchdown effects). A tree's rest pose is
  server-derived and replicated: identical on every client by construction.
- **Column / falling atom:** each client runs the query and keeps both values; its rest pose is
  per-client and cosmetic.
- INFERRED why: the server needs the settled tree itself (collision or sight of the fallen trunk);
  columns are decoration. (Overmatch: every peer derives the pose from the state word and the
  terrain grid as a pure libm-only function, so the bits agree by determinism, not by replication;
  playtest fork F8.)

## 3. A fallen object's collision [SCRIPT/DATA, READ; per-query matrix INFERRED]
- Per-triangle collision flags on the collision model decide which queries it blocks: camera
  no-collide, transparent, blended, terrain, no-collide, double-sided, water, projectile no-collide
  (low byte); a material kind in the high byte. A query respects or skips geometry by these flags;
  the fall code reads the terrain flag to know its far end rests on ground.
- Native filter predicates exclude destructibles from chosen casts (the fall query installs one).
- Not provable from the binary: which gameplay queries (movement, shell traces, sight) skip a
  fallen tree. INFERRED from in-game behaviour: a fallen tree's settled model carries projectile
  no-collide (shells pass through) and likely camera no-collide. (Overmatch, slice contract A2: a
  fallen trunk blocks rounds and sight, not hulls.)
