# Composable rig control: servos ⊥ weapons ⊥ views, pure servos, layered gates

With a tank's parts now enumerable ([[0012-spec-driven-rig-binder]]), the control layer is decomposed into three **orthogonal** concerns, each its own RON section, so a variant composes them freely:

- **Traverse** = `servos` — 1-DOF role-keyed motors; the model hierarchy composes nested mounts.
- **Firing** = `weapons` — a muzzle + ballistics + `trigger` routing; recoil is a separate translating node.
- **Viewing** = `views` — a camera/optic anchored to a node, keyed by a closed `ViewKind`.

No part "owns" another: the coax rides the gun chain by *hierarchy* (it is a child of the gun node), not by code wiring; a weapon names its muzzle, never its servos.

## Pure servos / one shared aim point

Every servo of the controlled tank autonomously points its forward at a single shared **aim point** (line-of-sight) each frame, by role — Yaw solves azimuth, Pitch elevation, each from its own pose. There is **no "chain" concept in code** and no per-weapon aim: the turret+gun and the hull MG converge on the same point independently, weapon-agnostic. Both view modes write that one point (third-person screen-ray; gunner optic intent), and a single mode-agnostic system (`drive_aim_servos`) drives the servos. `trigger: Primary|Secondary` routes *fire input* only (LMB / Space), never traverse.

A consequence forced this shape: **superelevation cannot live in the servo.** It is per-weapon (depends on muzzle velocity) and the servo is shared (the coax rides the gun), so a lob baked into the gun servo would mis-elevate the coax. Servos are therefore pure line-of-sight; the ballistic lob is a deferred **firing-side** concern (the shell launches at bore + per-weapon superelevation; the bore/optic stay LOS). Interim state: the main gun fires flat.

## Layered capability gates

The old single global capability map (Drive / Traverse / Fire / Load / GunnerSight / CommanderView) is dissolved into per-concern gates, all evaluated against the same part-quality vocabulary (`design/armor-penetration-and-damage.md` §7b; `part_qualities` resolved once per tank, then `evaluate` over Group / Part / Pool / Backup):

- per-**servo** `requires` — the slew gate (a dead gunner freezes the turret; a future traverse motor would slow it).
- per-**weapon** `fire` / `load` — independent gates (the loader can *fire* the coax but not lay it; the breech + barrel gate the main gun).
- per-**view** `requires` — the view-death gate (a dead gunner blacks the optic; a dead commander closes third-person).
- the global `Capability` map keeps **only tank-wide verbs** — currently just `Drive`.

"Operator" dissolved: the eligible crew are simply the `Part`s named in each gate, with `Pool` / `Backup` expressing multi-crew redundancy.

## Composition over kind

A ballistic volume is `material_factor` + optional facets (`hp`, `crew`, `ammo`, `function`) — no `kind` enum. A steel barrel *module* resists like steel yet still takes damage; a station is a volume that carries a crew role. Behaviour is composed from data, the same spirit as the gate grammar — not switched on a type.

## Considered options

- **Per-weapon aim / weapon selection.** Each weapon aims independently, or the player selects "the active weapon." Rejected: the player already commits one aim point (the existing intent slice); letting every weapon chase it is simpler and matches multi-station tanks firing on one sighted spot. Selection is firing-only (`trigger`).
- **Servo carries its weapon's superelevation.** Keeps the lob on the gun, but breaks with a shared chain (the coax inherits the main gun's lob) and re-couples servo to weapon. Rejected; superelevation deferred firing-side.
- **One global capability map (status quo).** Could not express "the loader can fire but not traverse the coax," and forced a single requirement per verb across every weapon/mount. Replaced by the per-part gates.
- **`kind: Armor | Module | Crew` enum on volumes.** Rejected for facets — a volume can be steel-grade *and* damageable; an enum forces false either/ors.

## Consequences

- Adding a weapon / mount / view to a variant is data, and its gate travels with it.
- The aim/sight/shooting split is fixed by a **one-writer-per-mode** invariant: third-person `commit_aim` and gunner `drive_gunner_aim` each write the shared aim point, and `drive_aim_servos` (after both) drives the servos — so the same servo logic serves both views.
- Superelevation owes its own slice; until then ranged main-gun fire is flat.
- The `Capability` glossary term (which listed Traverse / Fire / Load / GunnerSight / CommanderView) is updated: those are no longer global capabilities.
