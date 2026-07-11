//! Per-variant spec sheets as RON data assets (ADR-0010). The Blender model owns geometry and
//! spatial anchors; this owns the tuning numbers — mass + inertia, drivetrain, suspension, servo
//! configs — that differ per tank variant. A `.tank.ron` file deserializes (via serde) straight
//! into the same components the sim reads (`Mass`, `Drivetrain`, `SuspensionParams`, `ServoSpec`), so
//! values stay plain-text, git-diffable, and hot-reloadable, with no recompile and no Blender
//! round-trip. There are **no code defaults** (ADR-0011): a competitive sim never runs on guessed
//! stats, so a failed load is fatal. The spec is a *load dependency* — the tank is spawned only
//! once it's loaded — so `tank::spawn_tank_sim` builds the sim body from its values in one pass.

use bevy::asset::io::Reader;
use bevy::asset::{AssetLoader, LoadContext, LoadState};
use bevy::prelude::*;
use serde::Deserialize;
use std::collections::HashMap;

use crate::damage::{Capability, CrewStation, FunctionRole, Requirement};
use crate::driving::{Drivetrain, SuspensionParams};
use crate::tank::{ServoSpec, Tank};

/// One tank variant's spec sheet — the typed contents of a `.tank.ron` file. Its fields *are* the
/// components the sim consumes; `tank::apply_tank_spec` copies them onto the rig once ready.
/// One ballistic volume's data, keyed by model node name in [`TankSpec::volumes`]. **Composition
/// over a `kind` enum** (design `armor-penetration-and-damage.md` §2/§12): `material_factor` is the
/// base every volume has (shell-resistance per metre), and optional facets layer roles on top:
/// `hp` makes it damageable, `crew` makes it a crewman, `ammo` makes depletion cook off, and
/// `function` marks a repairable capability. Never add a central `kind` enum; "is it crew?" means
/// "does it have the crew facet?"
#[derive(Deserialize, Clone)]
#[serde(deny_unknown_fields)]
pub struct VolumeSpec {
    /// Reference-mm of armour per metre of material — the shell-resistance cost, decoupled from role
    /// (a steel barrel module carries the same factor as a steel plate).
    pub material_factor: f32,
    /// HP pool if damageable (module/crew/ammo); absent → pure armour, nothing to lose. The RON
    /// enables `implicit_some`, so this is authored bare (`hp: 8.0`, not `hp: Some(8.0)`); omitting
    /// it yields `None`. Future facets follow the same optional-field-per-facet shape.
    #[serde(default)]
    pub hp: Option<f32>,
    /// Crew station served by this volume. Requires `hp`.
    #[serde(default)]
    pub crew: Option<CrewStation>,
    /// Ammunition volume: HP depletion cooks off and kills all crew. Requires `hp`.
    #[serde(default)]
    pub ammo: bool,
    /// Repairable capability served by this module. Function loss is derived from HP.
    #[serde(default)]
    pub function: Option<FunctionRole>,
}

/// Which fire input a weapon answers to (design: LMB = the main gun, Spacebar = the MGs). Pure fire
/// routing — it has *no* bearing on aiming or traverse (servos are weapon-agnostic). The `Primary`
/// weapon also supplies the rig's main-bore handles (its chain → `Rig.turret`/`gun`/`muzzle`).
#[derive(Deserialize, Clone, Copy, PartialEq, Eq)]
pub enum Trigger {
    Primary,
    Secondary,
}

/// One weapon's data, keyed by logical name in [`TankSpec::weapons`]. `muzzle` (the bore the shot
/// leaves from) and the optional recoiling `barrel` are model node names; the weapon's aiming chain
/// is *not* declared here — it's the muzzle's servo ancestors, derived from the model hierarchy.
#[derive(Deserialize, Clone)]
#[serde(deny_unknown_fields)]
pub struct WeaponSpec {
    /// Fire input this weapon answers to. The single `Primary` also marks the rig's main bore.
    pub trigger: Trigger,
    /// Bore node — shot origin + direction.
    pub muzzle: String,
    /// Recoiling barrel node, if the weapon reciprocates; omitted → no recoil (e.g. a coax).
    #[serde(default)]
    pub barrel: Option<String>,
    /// Muzzle velocity (m/s).
    pub speed: f32,
    /// Shell calibre (m) — drives overmatch in the penetration march.
    pub caliber: f32,
    /// Projectile mass (kg) — primary driver of penetration capability.
    pub mass: f32,
    /// Reload cooldown before the weapon can fire again (s).
    pub reload: f32,
    /// **Belt composition** — the tracer cadence, authored as ammunition data (real belts are loaded
    /// e.g. 4-ball-1-tracer), NOT a VFX knob. Every `tracer_every`-th round down the belt is a tracer:
    /// `1` = every round is a tracer (the 88's single-round "belt"), `5` = one-in-five (the 7.9 mm MG
    /// belts). `0` = a tracerless belt (a future stealth belt) — no round ever traces. This is the
    /// seed of the belt-customization feature; a future load-out UI edits the same field. Required (no
    /// code default, ADR-0011): a belt with unstated tracer content is an authoring omission, not a
    /// guessable default. The counter that walks the belt is sim state ([`crate::tank::WeaponState`]).
    pub tracer_every: u32,
    /// Recoil spring, present iff `barrel` is. Authored alongside it.
    #[serde(default)]
    pub recoil: Option<RecoilSpec>,
    /// Fire gate (design §7b): what must be crewed/intact to fire — operator + ordnance (e.g. the
    /// main gun's `[Gunner, Breech, GunBarrel]`, a coax's `Backup(Gunner|Loader)`). The per-weapon
    /// successor to the old global `Fire` capability. Empty = always firable.
    #[serde(default)]
    pub fire: Requirement,
    /// Load gate: what must hold for the reload timer to tick (e.g. `[Loader, Breech]`). The
    /// per-weapon successor to the old global `Load`. Empty = always loading.
    #[serde(default)]
    pub load: Requirement,
}

/// A weapon's procedural barrel-recoil spring (a 1-DOF damped spring along the bore).
#[derive(Deserialize, Clone)]
#[serde(deny_unknown_fields)]
pub struct RecoilSpec {
    /// Backward impulse on firing (m/s along −bore). Higher = harder, longer kick.
    pub kick: f32,
    /// Spring stiffness pulling the barrel back to battery. Lower = longer stroke + slower return.
    pub stiffness: f32,
    /// Damping; slightly underdamped lets the barrel lumber home with a small settle.
    pub damping: f32,
}

/// A crew viewpoint — the camera/optic anchor. A closed set of kinds (each its own bespoke camera
/// behaviour in code), keyed in [`TankSpec::views`]; the *parameters* (which node, later FOV/zoom)
/// are data. The gunner's view node is also how the binder finds the gunner's chain for the rig.
#[derive(Deserialize, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ViewKind {
    Gunner,
    Commander,
}

impl ViewKind {
    pub fn label(self) -> &'static str {
        match self {
            ViewKind::Gunner => "Gunner sight",
            ViewKind::Commander => "Commander view",
        }
    }
}

/// One view's parameters: the model node the camera bolts to (which rides its servo's lay), the
/// camera vertical FOV (radians — narrow = magnified optic, wide = third-person), and the `requires`
/// gate that decides whether the view is usable. `requires` is the per-view successor to the old
/// global `GunnerSight`/`CommanderView` capabilities (same slew/fire-gate grammar, evaluated against
/// the controlled tank); empty = always available.
#[derive(Deserialize, Clone)]
#[serde(deny_unknown_fields)]
pub struct ViewSpec {
    pub node: String,
    pub fov: f32,
    #[serde(default)]
    pub requires: Requirement,
}

#[derive(Asset, TypePath, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TankSpec {
    /// Total mass (kg) — authored balance data; the collision proxy contributes none (ADR-0011).
    pub mass: f32,
    /// Hull box full dimensions (x, y, z metres) approximating the angular-inertia distribution.
    pub inertia_extents: (f32, f32, f32),
    pub drivetrain: Drivetrain,
    pub suspension: SuspensionParams,
    /// Servos (actuator mounts) keyed by model node name — the **source of truth** for which nodes
    /// rotate and how. Each carries its aim `role` (which also derives the rotation axis: Yaw→Y,
    /// Pitch→X) and slew tuning; `spawn_tank_sim` resolves each name to its node and binds the servo.
    /// Replaces the old fixed `turret`/`gun` fields, so a variant can declare any number of mounts.
    pub servos: HashMap<String, ServoSpec>,
    /// Ballistic volumes keyed by model node name — the **source of truth** for which nodes are
    /// volumes and what they are (design §12). The march reads `material_factor`; `spawn_tank_sim`
    /// layers components from the facets. The `Armor_/Module_/...` name prefix is documentation only.
    pub volumes: HashMap<String, VolumeSpec>,
    /// Weapons keyed by logical name — the **source of truth** for the tank's armament. Each names
    /// its bore (+ optional recoiling barrel) node and carries its ballistics/reload/recoil; the
    /// binder attaches a `Weapon` the shooting systems read. Replaces the hardcoded `shooting.rs`
    /// consts. (Multi-weapon control — selecting/aiming the coax + hull MG — is a later increment.)
    #[serde(default)]
    pub weapons: HashMap<String, WeaponSpec>,
    /// Crew viewpoints (camera/optic anchors) keyed by [`ViewKind`]. The gunner's also identifies
    /// the gunner's chain for the rig's main-bore handles.
    #[serde(default)]
    pub views: HashMap<ViewKind, ViewSpec>,
    /// Per-tank capability requirements (design §7b). Each capability maps to a list of requirement
    /// groups (AND'd): a bare `Part` is mandatory; `Pool(..)`/`Backup(..)` express graded redundancy.
    /// Drives [`crate::damage::capability_effectiveness`] — the single gate consuming systems query.
    #[serde(default)]
    pub capabilities: HashMap<Capability, Requirement>,
}

/// The handle to a tank's spec sheet, carried on its root entity so each tank knows its variant
/// (multi-variant ready). `spawn_tank` loads it alongside the model.
#[derive(Component)]
pub struct TankSpecHandle(pub Handle<TankSpec>);

/// Parses a `.tank.ron` file into a [`TankSpec`]. Tiny by design — the work is serde + RON.
#[derive(TypePath)]
struct TankSpecLoader;

impl AssetLoader for TankSpecLoader {
    type Asset = TankSpec;
    type Settings = ();
    type Error = BevyError;

    async fn load(
        &self,
        reader: &mut dyn Reader,
        _settings: &(),
        _load_context: &mut LoadContext<'_>,
    ) -> Result<TankSpec, BevyError> {
        let mut bytes = Vec::new();
        reader.read_to_end(&mut bytes).await?;
        Ok(ron::de::from_bytes(&bytes)?)
    }

    fn extensions(&self) -> &[&str] {
        &["tank.ron"]
    }
}

pub fn plugin(app: &mut App) {
    app.init_asset::<TankSpec>()
        .register_asset_loader(TankSpecLoader)
        .add_systems(Update, report_failed_spec);
}

/// Surface a failed spec-sheet load instead of swallowing it. The `.tank.ron` is required, in-repo
/// config with **no fallback** (ADR-0011): a competitive sim must never run on guessed stats, so a
/// parse/schema/IO error is fatal — we log the carried `AssetLoadError` and **panic in every
/// build**. (The schema test catches this class pre-ship; this is the runtime backstop for a bad
/// hot-reload or a file that slipped through.)
fn report_failed_spec(asset_server: Res<AssetServer>, tank: Query<&TankSpecHandle, With<Tank>>) {
    for handle in &tank {
        if let LoadState::Failed(err) = asset_server.load_state(&handle.0) {
            error!("required tank spec sheet failed to load: {err}");
            panic!("required tank spec sheet failed to load: {err}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The shipped spec sheet must always deserialize into `TankSpec`. This catches schema drift —
    /// a renamed/removed field, a changed type, a bad enum variant — at `cargo test` time, before
    /// the bad file ever ships (where `report_failed_spec` would catch it at runtime instead, but
    /// only after a player already has it). With `deny_unknown_fields`, a stray/typo'd key fails
    /// here too instead of being silently ignored.
    #[test]
    fn tiger_1_spec_sheet_matches_schema() {
        let ron = include_str!("../assets/tiger_1/tiger_1.tank.ron");
        let spec: TankSpec =
            ron::de::from_str(ron).expect("tiger_1.tank.ron must deserialize into TankSpec");
        // Spot-check values across sections so the test exercises real field wiring, not just "it
        // parsed".
        assert_eq!(spec.mass, 57000.0);
        assert_eq!(spec.inertia_extents, (3.0, 2.0, 6.3));
        assert_eq!(spec.drivetrain.max_thrust, 12500.0);
        assert_eq!(spec.suspension.stiffness, 551_613.0);
        // Servos are a node-keyed map now (not fixed turret/gun fields); the yaw + pitch mounts must
        // be declared for the rig to bind.
        assert!(spec.servos.contains_key("Turret_Yaw"));
        assert!(spec.servos.contains_key("Main_Gun_Pitch"));
        // Weapons: the main gun's ballistics live in data now, with its muzzle/barrel node refs.
        assert_eq!(spec.weapons["MainGun"].muzzle, "Main_Gun_Muzzle");
        assert_eq!(
            spec.weapons["MainGun"].barrel.as_deref(),
            Some("Main_Gun_Recoil")
        );
        assert_eq!(spec.weapons["MainGun"].speed, 773.0);
        // Belt data: the 88 traces every round (single-round belt); the MG belts are one-in-five.
        assert_eq!(spec.weapons["MainGun"].tracer_every, 1);
        assert_eq!(spec.weapons["Coax"].tracer_every, 5);
        assert_eq!(spec.weapons["HullMG"].tracer_every, 5);
        // Volumes: a steel-grade *module* (barrel) and a pure-armour plate (no hp) exercise the
        // composition facet — material decoupled from role.
        assert_eq!(spec.volumes["Gun_Barrel_Ballistic"].material_factor, 1000.0);
        assert_eq!(spec.volumes["Gun_Barrel_Ballistic"].hp, Some(8.0));
        assert_eq!(
            spec.volumes["Gun_Barrel_Ballistic"].function,
            Some(FunctionRole::GunBarrel)
        );
        assert_eq!(
            spec.volumes["Commander_Ballistic"].crew,
            Some(CrewStation::Commander)
        );
        assert!(spec.volumes["Ammo_L_0_Ballistic"].ammo);
        assert_eq!(spec.volumes["Hull_UFP_Ballistic"].hp, None);
        // Capability requirements: the flat RON shape deserializes into requirement groups. Drive =
        // [Driver, Engine, Transmission] (all mandatory `Single`s); Traverse = [Gunner]. Exercises
        // the `#[serde(untagged)]` bare-`Part` parse.
        use crate::damage::{Group, Part};
        assert_eq!(
            spec.capabilities[&Capability::Drive],
            vec![
                Group::Single(Part::Driver),
                Group::Single(Part::Engine),
                Group::Single(Part::Transmission),
            ]
        );
        // Fire/Load are no longer global capabilities — they're each weapon's own gates.
        assert_eq!(
            spec.weapons["MainGun"].fire,
            vec![
                Group::Single(Part::Gunner),
                Group::Single(Part::Breech),
                Group::Single(Part::GunBarrel),
            ]
        );
        assert_eq!(
            spec.weapons["MainGun"].load,
            vec![Group::Single(Part::Loader), Group::Single(Part::Breech)]
        );
        // Traverse is no longer a global capability — it's each servo's `requires` (slew gate).
        assert_eq!(
            spec.servos["Turret_Yaw"].requires,
            vec![Group::Single(Part::Gunner)]
        );
        // Views carry the camera FOV + their own gate — the per-view successors to the old
        // GunnerSight/CommanderView capabilities, which no longer exist on the global map.
        assert_eq!(spec.views[&ViewKind::Gunner].fov, 0.12);
        assert_eq!(
            spec.views[&ViewKind::Gunner].requires,
            vec![Group::Single(Part::Gunner)]
        );
    }

    /// The spec↔model **bind contract** — the CI-time twin of the runtime contract in
    /// `tank::spawn_tank_sim`, but without launching Bevy: it reads the glTF node names directly and
    /// checks both directions. Every node the spec references must exist in the `.glb`; the fixed
    /// structural nodes must be present; and every authored `*_Ballistic` node must be a declared
    /// volume (no orphans). This catches name drift — a rename, a typo, a forgotten declaration —
    /// before it ever reaches a runtime panic. Add a tank variant → add a case here.
    #[test]
    fn tiger_1_spec_binds_to_model() {
        use std::collections::HashSet;

        let gltf = gltf::Gltf::open("assets/tiger_1/tiger_1.glb").expect("tiger_1.glb must open");
        let nodes: HashSet<String> = gltf
            .nodes()
            .filter_map(|n| n.name().map(str::to_string))
            .collect();
        let spec: TankSpec = ron::de::from_str(include_str!("../assets/tiger_1/tiger_1.tank.ron"))
            .expect("tiger_1.tank.ron must parse");

        let has = |name: &str| {
            assert!(
                nodes.contains(name),
                "spec references node `{name}`, which is absent from the model"
            );
        };

        // Forward: every spec-declared node resolves to a model node.
        for servo in spec.servos.keys() {
            has(servo);
        }
        for weapon in spec.weapons.values() {
            has(&weapon.muzzle);
            if let Some(barrel) = &weapon.barrel {
                has(barrel);
            }
        }
        for volume in spec.volumes.keys() {
            has(volume);
        }
        for view in spec.views.values() {
            has(&view.node);
        }

        // Fixed structural contract (mirrors `spawn_tank_sim`'s singletons + the prefix scans).
        has("Hull");
        has("Center_Of_Mass");
        assert!(
            nodes.iter().any(|n| n.ends_with("_Collider")),
            "model has no `*_Collider` proxy"
        );
        let has_roadwheel = |side: &str| {
            nodes.iter().any(|n| {
                n.strip_prefix(side).is_some_and(|rest| {
                    !rest.is_empty() && rest.bytes().all(|b| b.is_ascii_digit())
                })
            })
        };
        assert!(has_roadwheel("Wheel_L_"), "model has no left roadwheel");
        assert!(has_roadwheel("Wheel_R_"), "model has no right roadwheel");

        // Reverse: no orphan volumes — every authored `*_Ballistic` node is a declared volume.
        for node in &nodes {
            if node.ends_with("_Ballistic") {
                assert!(
                    spec.volumes.contains_key(node),
                    "model node `{node}` is named like a ballistic volume but has no spec entry"
                );
            }
        }
    }
}
