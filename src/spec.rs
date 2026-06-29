//! Per-variant spec sheets as RON data assets (ADR-0010). The Blender model owns geometry and
//! spatial anchors; this owns the tuning numbers — mass + inertia, drivetrain, suspension, servo
//! configs — that differ per tank variant. A `.tank.ron` file deserializes (via serde) straight
//! into the same components the sim reads (`Mass`, `Drivetrain`, `SuspensionParams`, `ServoSpec`), so
//! values stay plain-text, git-diffable, and hot-reloadable, with no recompile and no Blender
//! round-trip. There are **no code defaults** (ADR-0011): a competitive sim never runs on guessed
//! stats, so a failed load is fatal. The spec is a *load dependency* — the tank is spawned only
//! once it's loaded — so `tank::on_tank_ready` binds its values onto the rig in a single pass.

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

#[derive(Asset, TypePath, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TankSpec {
    /// Total mass (kg) — authored balance data; the collision proxy contributes none (ADR-0011).
    pub mass: f32,
    /// Hull box full dimensions (x, y, z metres) approximating the angular-inertia distribution.
    pub inertia_extents: (f32, f32, f32),
    pub drivetrain: Drivetrain,
    pub suspension: SuspensionParams,
    pub turret: ServoSpec,
    pub gun: ServoSpec,
    /// Ballistic volumes keyed by model node name — the **source of truth** for which nodes are
    /// volumes and what they are (design §12). The march reads `material_factor`; `on_tank_ready`
    /// layers components from the facets. The `Armor_/Module_/...` name prefix is documentation only.
    pub volumes: HashMap<String, VolumeSpec>,
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
        // Volumes: a steel-grade *module* (barrel) and a pure-armour plate (no hp) exercise the
        // composition facet — material decoupled from role.
        assert_eq!(spec.volumes["Ballistic_Gun_Barrel"].material_factor, 1000.0);
        assert_eq!(spec.volumes["Ballistic_Gun_Barrel"].hp, Some(8.0));
        assert_eq!(
            spec.volumes["Ballistic_Gun_Barrel"].function,
            Some(FunctionRole::GunBarrel)
        );
        assert_eq!(
            spec.volumes["Ballistic_Commander"].crew,
            Some(CrewStation::Commander)
        );
        assert!(spec.volumes["Ballistic_Ammo_L_0"].ammo);
        assert_eq!(spec.volumes["Ballistic_Hull_UFP"].hp, None);
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
        assert_eq!(
            spec.capabilities[&Capability::Fire],
            vec![
                Group::Single(Part::Gunner),
                Group::Single(Part::Breech),
                Group::Single(Part::GunBarrel),
            ]
        );
        assert_eq!(
            spec.capabilities[&Capability::Load],
            vec![Group::Single(Part::Loader), Group::Single(Part::Breech)]
        );
        assert_eq!(
            spec.capabilities[&Capability::Traverse],
            vec![Group::Single(Part::Gunner)]
        );
    }
}
