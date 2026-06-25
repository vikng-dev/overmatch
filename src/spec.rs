//! Per-variant spec sheets as RON data assets (ADR-0010). The Blender model owns geometry and
//! spatial anchors; this owns the tuning numbers (thrust, servo speeds, …) that differ per tank
//! variant. A `.tank.ron` file deserializes — via serde — straight into the same components the
//! sim reads (`Drivetrain`, `ServoSpec`), so values stay plain-text, git-diffable, and
//! hot-reloadable, with no recompile and no Blender round-trip.

use bevy::asset::io::Reader;
use bevy::asset::{AssetLoader, LoadContext};
use bevy::prelude::*;
use serde::Deserialize;

use crate::driving::Drivetrain;
use crate::tank::{Gun, ServoSpec, Tank, Turret};

/// One tank variant's spec sheet — the typed contents of a `.tank.ron` file. Its fields *are* the
/// components the sim consumes; `apply_tank_spec` copies them onto the rig once both are ready.
#[derive(Asset, TypePath, Deserialize)]
pub struct TankSpec {
    pub drivetrain: Drivetrain,
    pub turret: ServoSpec,
    pub gun: ServoSpec,
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
        .add_systems(Update, apply_tank_spec);
}

/// Copy the spec sheet's values onto the rig once everything is present: the tank's handle has
/// resolved to a loaded asset *and* the servos have been bound by name. Runs each frame until the
/// inserts land (the `Without<…>` filters then stop matching), so it's robust to the asset and the
/// scene becoming ready in either order. Until then the sim runs on `Drivetrain`'s code default.
fn apply_tank_spec(
    mut commands: Commands,
    specs: Res<Assets<TankSpec>>,
    tank: Query<(Entity, &TankSpecHandle), (With<Tank>, Without<Drivetrain>)>,
    turret: Query<Entity, (With<Turret>, Without<ServoSpec>)>,
    gun: Query<Entity, (With<Gun>, Without<ServoSpec>)>,
) {
    let Ok((tank_entity, handle)) = tank.single() else {
        return;
    };
    let Some(spec) = specs.get(&handle.0) else {
        return; // asset not loaded yet
    };
    let (Ok(turret_entity), Ok(gun_entity)) = (turret.single(), gun.single()) else {
        return; // servos not bound yet
    };

    commands.entity(tank_entity).insert(spec.drivetrain.clone());
    commands.entity(turret_entity).insert(spec.turret.clone());
    commands.entity(gun_entity).insert(spec.gun.clone());
}
