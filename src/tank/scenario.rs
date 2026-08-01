use avian3d::prelude::RigidBody;
use bevy::asset::LoadState;
use bevy::prelude::*;

use super::model::{Controlled, Tank};
use super::spawn::{
    PendingTankAssets, TankContent, TankPresentation, TankSimSource, load_tank_assets,
    spawn_complete_tank,
};
use crate::sight::SightMode;
use crate::state::{AppState, GameplaySet};

pub fn sp_spawn_plugin(app: &mut App) {
    app.add_systems(Startup, load_tank_assets).add_systems(
        Update,
        spawn_tank_when_loaded.run_if(in_state(AppState::Loading)),
    );
}

/// Install the local duel's possession switch.
pub fn client_plugin(app: &mut App) {
    // Control systems in GameplaySet must see the new owner in the same frame.
    app.add_systems(
        Update,
        swap_controlled_tank
            .run_if(in_state(AppState::Playing))
            .before(GameplaySet),
    );
}

/// The local duel's two spawn POINTS — horizontal only (XZ), like every spawn definition in the
/// game. Their Y is sampled from the live surface at spawn time (`terrain_grid::spawn_pos`).
///
/// These used to be full poses with a hardcoded `y = 2.0`, which was correct for exactly one world:
/// the flat slab. Once the heightmap world landed, `--offline` spawned both Tigers ~116 m UNDER the
/// terrain (79 m after the 2560 → 1000 m re-scale) — they were never "falling through" anything,
/// they simply started inside the hill. The net server had already been fixed for this class; the
/// offline/single-player path had not, which is precisely why spawn Y is no longer authorable
/// anywhere.
pub(crate) const DUEL_SPAWN_XZ: [Vec2; 2] = [Vec2::new(10.0, 5.0), Vec2::new(10.0, -12.0)];

/// Where the duel stands under the FAR probe placement (`OVERMATCH_PROBE_FAR` — see
/// [`spawn_probe_tanks`]). Tiger A's point; Tiger B keeps its 17 m offset behind it, so the duel's
/// own geometry (and the camera's relationship to it) is the one it always has.
///
/// MEASURED off the shipped heightmap, not chosen by eye: the footprint relief under both duel
/// points is 0.22 m / 0.29 m (`terrain_grid::SPAWN_FOOTPRINT_HALF_M` square), the flattest pad on
/// the map that has a matching flat block 600 m down +Z, and the ground between the two rises
/// nowhere above the sight line — so the probe block is actually IN FRAME rather than behind a
/// crest. Flat matters for the same reason it did for the near band: parked tanks on a grade slide,
/// and a sliding scene measures contact resolution instead of the thing under test.
const PROBE_FAR_DUEL_XZ: Vec2 = Vec2::new(-410.0, -440.0);

/// Where the FAR grid's near edge sits, world z. Same units and same meaning as
/// [`PROBE_GRID_NEAR_Z`]: an absolute world coordinate, not an offset from the duel.
///
/// Paired with [`PROBE_FAR_DUEL_XZ`] so the whole grid lands 588..633 m from the controlled tank's
/// spawn — squarely inside the shoe chain's LOD1 band, past `track::link_view::SHOE_LOD1_DISTANCE_M`
/// by ~238 m at the near end, and every probe inside the camera's 1 000 m far plane with room to
/// spare. Its footprint relief is 0.24..2.45 m per tank, comparable to the near band's valley floor.
///
/// The band claim is asserted against the distance BEVY measures — camera to shoe, orbit radius and
/// hull extent taken off the near end — by `terrain_grid`'s
/// `the_far_probe_placement_puts_every_probe_in_the_shoe_lod1_band`, which is where the margin
/// arithmetic lives. Nothing here should be re-derived by hand.
const PROBE_FAR_NEAR_Z: f32 = 148.0;

/// Is this process running the FAR probe placement?
///
/// A dev/profiling lever on the offline path only, like [`spawn_probe_tanks`]'s own count: unset is
/// every normal run, and nothing outside the probe grid consults it.
pub(crate) fn probe_far() -> bool {
    crate::env_flag("OVERMATCH_PROBE_FAR", false)
}

/// The duel's two spawn points under the placement in force. `far` is [`probe_far`] — passed rather
/// than read so the terrain-clearance test can assert BOTH placements without touching the process
/// environment.
pub(crate) fn duel_spawn_xz(far: bool) -> [Vec2; 2] {
    if !far {
        return DUEL_SPAWN_XZ;
    }
    // A pure TRANSLATION of the authored duel, so the far scene differs from the near one in where
    // it stands and in nothing else.
    let shift = PROBE_FAR_DUEL_XZ - DUEL_SPAWN_XZ[0];
    DUEL_SPAWN_XZ.map(|xz| xz + shift)
}

/// Probe `index`'s spawn point under the placement in force — the grid's one geometry, shared by
/// the spawn and by the test that checks every one of those points against the shipped terrain.
pub(crate) fn probe_spawn_xz(far: bool, index: usize) -> Vec2 {
    let (column, row) = (index % PROBE_GRID_COLUMNS, index / PROBE_GRID_COLUMNS);
    let near_z = if far {
        PROBE_FAR_NEAR_Z
    } else {
        PROBE_GRID_NEAR_Z
    };
    Vec2::new(
        // Centre the block on the duel's x so it fills the view symmetrically.
        duel_spawn_xz(far)[0].x
            + (column as f32 - (PROBE_GRID_COLUMNS as f32 - 1.0) / 2.0) * PROBE_GRID_SPACING_M,
        near_z + row as f32 * PROBE_GRID_SPACING_M,
    )
}

/// Admit the local duel after presentation preloading, then construct both roots completely. This
/// is an admission policy; assets do not initialize simulation state. Failed loads remain fatal.
fn spawn_tank_when_loaded(
    mut commands: Commands,
    asset_server: Res<AssetServer>,
    pending: Option<Res<PendingTankAssets>>,
    source: TankSimSource,
    height: Option<Res<crate::terrain_grid::HeightGrid>>,
    mut next: ResMut<NextState<AppState>>,
) {
    let Some(pending) = pending else {
        return;
    };
    for handle in [pending.spec.id().untyped(), pending.scene.id().untyped()] {
        if let LoadState::Failed(err) = asset_server.load_state(handle) {
            error!("required tank asset failed to load: {err}");
            panic!("required tank asset failed to load: {err}");
        }
    }
    if !pending.loaded(&asset_server) {
        return;
    }
    let Some(content) = source.get() else {
        return;
    };
    // Both bodies simulate; only the Controlled marker selects input ownership. Each spawn point is
    // horizontal — the ground under it is sampled NOW, through the one rule every spawn path shares
    // (`terrain_grid::spawn_pos`: footprint max + clearance), so the duel lands on whatever world
    // this build actually has: heightmap, flat slab, or a re-authored map later.
    let grid = height.as_deref();
    let far = probe_far();
    let duel = duel_spawn_xz(far);
    spawn_tank(
        &mut commands,
        content,
        pending.presentation(),
        Transform::from_translation(crate::terrain_grid::spawn_pos(grid, duel[0]))
            .with_rotation(Quat::from_rotation_z(0.7)),
        "Tiger I (A)",
        true,
    );
    spawn_tank(
        &mut commands,
        content,
        pending.presentation(),
        Transform::from_translation(crate::terrain_grid::spawn_pos(grid, duel[1])),
        "Tiger I (B)",
        false,
    );
    spawn_probe_tanks(&mut commands, content, pending.presentation(), grid, far);
    commands.remove_resource::<PendingTankAssets>();
    next.set(AppState::Playing);
}

/// Centre-to-centre spacing of the [`spawn_probe_tanks`] grid, metres. Wider than a Tiger's hull is
/// long (8.45 m over the tracks) so the extra bodies never spawn interpenetrating — a stack of
/// overlapping tanks would spend the whole capture resolving contacts and measure nothing useful.
const PROBE_GRID_SPACING_M: f32 = 11.0;

/// Where the grid's near edge sits in the DEFAULT placement: down +Z, in FRONT of the third-person
/// camera. The camera spawns at `(10, 7, −7)` looking at `(10, 1, 5)` (`camera.rs`), i.e. behind
/// Tiger A looking along +Z — so +Z is the only direction that puts probe tanks in frame as well as
/// in the cascade volume. (Tiger B, at z = −12, is behind the camera and always was.) The far
/// placement keeps every one of those properties and only moves the scene: [`PROBE_FAR_NEAR_Z`].
///
/// 48 m rather than "just past the duel" because the ground between z ≈ 5 and z ≈ 30 falls at ~40°:
/// tanks parked there SLIDE (measured — the first layout drifted 6..13 m downhill during a capture,
/// which is both a moving scene and a pile of contact work). z ≈ 46..100 is the valley floor under
/// that slope, flat to a couple of metres across the whole block, and still ~65..115 m from the
/// camera — inside the 150 m cascade envelope with room to spare.
const PROBE_GRID_NEAR_Z: f32 = 48.0;

/// Columns in the block. Fixed rather than square-rooted so the grid keeps the same shape at every
/// count: six columns at 11 m is 55 m across, inside the camera's horizontal FOV at the block's
/// distance, and extra rows recede along the view axis where everything stays in frame.
const PROBE_GRID_COLUMNS: usize = 6;

/// Spawn `OVERMATCH_PROBE_TANKS` EXTRA idle Tigers in a grid in front of the duel, for render/sim
/// cost profiling at caster counts the two-tank duel cannot reach (a 15v15 frame is 30 tanks).
///
/// Dev instrument, not content: unset (or `0`) spawns nothing, which is every normal run. The count
/// is the TOTAL wanted, so `OVERMATCH_PROBE_TANKS=30` yields 30 tanks — the two duel Tigers plus 28
/// here. They are ordinary complete tanks (same body, same tracks, same `RigidBody::Dynamic`), so a
/// count sweep measures the real per-tank cost of both halves; nothing drives them.
///
/// Grid geometry is deliberately dumb — a square-ish block laid out on XZ and dropped onto the
/// surface by the one shared rule (`terrain_grid::spawn_pos`), like every other spawn point.
///
/// # `OVERMATCH_PROBE_FAR=1`: the same block, on the far side of the shoe LOD
///
/// The near block is the RIGHT default and the wrong half of one question. `track::link_view` swaps
/// every shoe for a 477-triangle reduction beyond `SHOE_LOD1_DISTANCE_M`, and that costs ONE extra
/// entity per shoe — at 194 shoes per tank and 30 tanks, 5 820 more walked by
/// `check_visibility_ranges` every frame, whether or not any of them is far enough to matter. So the
/// LOD has two frames to answer for: the FAR one, where the triangle win is real and the walk is
/// paid for, and the NEAR one, where the rendered geometry is byte-identical to before and the walk
/// is pure overhead. The near block at 55..99 m can only show the second. This flag is the other
/// placement, and nothing else.
///
/// It moves the WHOLE offline scene, not just the grid, because the world is 1 000 m on a side
/// (`terrain_grid::WORLD_SIZE`) and the camera sits at the duel: with the duel where it is, the
/// furthest in-bounds probe is ~490 m away and every shoe is still the base mesh. So the duel
/// translates to
/// [`PROBE_FAR_DUEL_XZ`] and the grid to [`PROBE_FAR_NEAR_Z`] — same spacing, same columns, same
/// shape, same count, 588..633 m of separation instead of 55..99 m.
///
/// One thing the far placement inherently is NOT: a shadow-caster measurement. The cascade envelope
/// is ~150 m, so the far probes cast nothing into it. That is a property of distance, not of this
/// lever — the shadow ladder is what the near placement is for.
fn spawn_probe_tanks(
    commands: &mut Commands,
    content: TankContent,
    presentation: TankPresentation,
    grid: Option<&crate::terrain_grid::HeightGrid>,
    far: bool,
) {
    let Some(total) = crate::env_parse::<usize>("OVERMATCH_PROBE_TANKS") else {
        return;
    };
    let extra = total.saturating_sub(DUEL_SPAWN_XZ.len());
    if extra == 0 {
        return;
    }
    let anchor = duel_spawn_xz(far)[0];
    let (mut nearest, mut furthest) = (f32::INFINITY, 0.0f32);
    for index in 0..extra {
        let xz = probe_spawn_xz(far, index);
        let distance = xz.distance(anchor);
        nearest = nearest.min(distance);
        furthest = furthest.max(distance);
        spawn_tank(
            commands,
            content,
            presentation.clone(),
            Transform::from_translation(crate::terrain_grid::spawn_pos(grid, xz)),
            &format!("Probe Tiger {index}"),
            false,
        );
    }
    // Self-documenting capture: a frame stream is evidence for a placement, and the placement is
    // this line. Distances are to the CONTROLLED tank's spawn — the camera orbits up to 18 m behind
    // that point (`camera::orbit_camera`'s `ORBIT_FAR`), so the rendered distances are these or
    // greater, which is the direction that keeps a "beyond the LOD swap" claim conservative.
    info!(
        "offline: spawned {extra} probe tanks (OVERMATCH_PROBE_TANKS={total}) — {} placement, grid \
         x {:.0}..{:.0} z {:.0}..{:.0}, {nearest:.0}..{furthest:.0} m from the controlled tank at \
         ({:.0}, {:.0})",
        if far {
            "FAR (OVERMATCH_PROBE_FAR=1)"
        } else {
            "near"
        },
        probe_spawn_xz(far, 0).x,
        probe_spawn_xz(far, extra.min(PROBE_GRID_COLUMNS) - 1).x,
        probe_spawn_xz(far, 0).y,
        probe_spawn_xz(far, extra - 1).y,
        anchor.x,
        anchor.y,
    );
}

/// Spawn one complete dynamic tank for the local duel.
fn spawn_tank(
    commands: &mut Commands,
    content: TankContent,
    presentation: TankPresentation,
    transform: Transform,
    name: &str,
    controlled: bool,
) {
    let root = spawn_complete_tank(
        commands,
        content,
        presentation,
        (transform, Name::new(name.to_string()), RigidBody::Dynamic),
    );
    if controlled {
        commands.entity(root).insert(Controlled);
    }
}

/// Move local possession to the next tank and return to third-person view.
fn swap_controlled_tank(
    keys: Res<ButtonInput<KeyCode>>,
    mut commands: Commands,
    tanks: Query<Entity, With<Tank>>,
    controlled: Query<Entity, With<Controlled>>,
    mut mode: ResMut<SightMode>,
) {
    if !keys.just_pressed(KeyCode::Tab) {
        return;
    }
    let Ok(current) = controlled.single() else {
        return;
    };
    let all: Vec<Entity> = tanks.iter().collect();
    if all.len() < 2 {
        return;
    }
    let idx = all.iter().position(|&e| e == current).unwrap_or(0);
    let next = all[(idx + 1) % all.len()];
    if next == current {
        return;
    }
    commands.entity(current).remove::<Controlled>();
    commands.entity(next).insert(Controlled);

    *mode = SightMode::ThirdPerson;
}

#[cfg(test)]
mod spawn_contract_tests {
    use std::{collections::HashMap, sync::Arc};

    use bevy::prelude::{App, ResMut, Resource, Update};

    use super::super::spawn::TankSimSource;
    use crate::bake::{TankBlueprint, TankGeometry};
    use crate::spec::TankSpec;

    #[derive(Resource, Default)]
    struct SourceProbe(bool);

    fn probe_unresolved_handle(source: TankSimSource, mut probe: ResMut<SourceProbe>) {
        probe.0 = source.get().is_some();
    }

    #[test]
    fn sim_source_does_not_require_a_resolved_asset_handle() {
        let spec: TankSpec =
            ron::de::from_str(include_str!("../../assets/tiger_1/tiger_1.tank.ron"))
                .expect("the shipped Tiger spec must parse");
        let geometry = TankGeometry {
            nodes: Vec::new(),
            by_name: HashMap::new(),
            roadwheels: Vec::new(),
            collision_proxies: Vec::new(),
        };

        let mut app = App::new();
        app.insert_resource(TankBlueprint {
            geometry: Arc::new(geometry),
            spec: Arc::new(spec),
        })
        .init_resource::<SourceProbe>()
        .add_systems(Update, probe_unresolved_handle);
        app.update();

        assert!(
            app.world().resource::<SourceProbe>().0,
            "TankSimSource must read the eager blueprint without an asset-handle argument",
        );
    }
}
