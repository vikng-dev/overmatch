//! The client's scatter-crossing read: a round passing through a house wall or a fir trunk lights
//! the surface where it crossed.
//!
//! DIRECTOR'S RULING — a deliberate interim, recorded here so an ADR-0023 (VFX honesty) reader sees
//! a decision rather than an oversight. The deterministic shell walk resolves ground through the
//! [`crate::terrain_grid::HeightGrid`] alone, and the map's 709 scatter proxies are not in it: a
//! round flies through a building untouched, and this slice does not change that (no march change,
//! no `PROTOCOL_REV` bump — nothing here is sim state). What ships is the VIEW half of the
//! interaction: the crossing point draws the same terrain impact read a ground hit draws, and the
//! shell continues unharmed. The white lie is the shell's survival, NOT the effect — the effect
//! marks a real crossing of real collider geometry, at the real crossing point, with that surface's
//! real normal, sized off the round's real caliber. The ballistic cost of a wall (obstruction,
//! energy loss, debris) belongs to the destruction saga, which owns the sim side; when that lands,
//! the crossing becomes an `Impact` and this module's cast retires.
//!
//! Invariant (ADR-0014): reads shell transforms and static collider geometry, writes only its own
//! view state ([`ScatterProbe`]) and transient render entities. The projectile is never modified.
//! Parry is the caster here, which is a VIEW-layer allowance (the same one `aim`/`sight` take); the
//! determinism ban binds the sim path, and no sim path reads this module.

use avian3d::prelude::{SpatialQuery, SpatialQueryFilter};
use bevy::prelude::*;

use crate::Layer;
use crate::ballistics::Projectile;
use crate::scatter::ScatterProxy;
use crate::view::PlayerView;

use super::ViewRng;
use super::billboard::{BillboardRing, VfxBillboardMaterial};
use super::impact::{GroundMarkRing, ImpactAssets, spawn_terrain_read};

/// Minimum distance (m) into a frame's segment for a hit to count as an ENTRY — the march's own
/// 1 mm boundary nudge. A hit nearer than this began at or inside the surface, which is the
/// previous frame's crossing seen again, never a new one.
const ENTRY_MIN_M: f32 = 1.0e-3;

pub(super) fn plugin(app: &mut App) {
    // `Update`, so the segment spans exactly one frame of marched motion: the march runs in
    // `FixedUpdate` (or, in `Demo` mode, this same schedule) and advances a shell's transform once
    // per tick, so a per-frame probe of that transform sees each crossing exactly once.
    app.add_systems(Update, read_scatter_crossings);
}

/// Per-shell crossing state, inserted the first frame a shell is seen and dropped when it despawns.
/// View-only: nothing outside this module reads or writes it.
#[derive(Component)]
struct ScatterProbe {
    /// The shell's position at the previous read — this frame's segment origin.
    previous: Vec3,
    /// The proxy the segment currently begins inside, when it begins inside one. An entry against
    /// this same proxy is a continuation, so one crossing reads once however many frames the shell
    /// spends inside it.
    inside: Option<Entity>,
}

/// Cast each shell's frame displacement against the scatter proxies alone and draw the terrain read
/// at every surface ENTRY. The mask keeps the traversal on the static terrain layer; the predicate
/// narrows it to [`ScatterProxy`] entities, so the ground surface and every armor volume are
/// invisible to this cast — those already have their own honest impacts.
///
/// Solid casting is what makes entry detection free: a segment beginning inside a proxy reports
/// that proxy at distance zero, which fails [`ENTRY_MIN_M`]. Combined with [`ScatterProbe::inside`],
/// a shell inside a wall re-reads nothing until it enters a DIFFERENT proxy, and an open-air frame
/// (no hit at all) clears the state.
///
/// Every shell entity is covered, whichever peer authored it: shells are not replicated — a remote
/// player's round is reconstructed locally as its own `Projectile` (`net::client`), so predicted and
/// remote fire both pass through this one query.
#[expect(
    clippy::too_many_arguments,
    reason = "one view system, one effect call"
)]
fn read_scatter_crossings(
    mut shells: Query<(Entity, &Transform, &Projectile, Option<&mut ScatterProbe>)>,
    proxies: Query<(), With<ScatterProxy>>,
    spatial: SpatialQuery,
    assets: Res<ImpactAssets>,
    mut materials: ResMut<Assets<VfxBillboardMaterial>>,
    mut ring: ResMut<BillboardRing>,
    mut ground_ring: ResMut<GroundMarkRing>,
    mut rng: ResMut<ViewRng>,
    camera: Query<&GlobalTransform, With<PlayerView>>,
    mut commands: Commands,
) {
    let is_proxy = |entity: Entity| proxies.contains(entity);
    let filter = SpatialQueryFilter::from_mask(Layer::Terrain);
    let camera = camera.single().ok();
    for (shell, transform, projectile, probe) in &mut shells {
        let current = transform.translation;
        let Some(mut probe) = probe else {
            // First sight of this shell: no segment yet, so nothing to cross. A reconstructed
            // shell's catch-up flight is spent before the entity exists here, so its crossings are
            // behind the probe and never read.
            commands.entity(shell).insert(ScatterProbe {
                previous: current,
                inside: None,
            });
            continue;
        };
        let step = current - probe.previous;
        let origin = probe.previous;
        probe.previous = current;
        // A held or spent shell moves nothing; there is no segment to cast.
        let Ok(direction) = Dir3::new(step) else {
            continue;
        };
        let Some(hit) =
            spatial.cast_ray_predicate(origin, direction, step.length(), true, &filter, &is_proxy)
        else {
            probe.inside = None;
            continue;
        };
        let entered = hit.distance > ENTRY_MIN_M && probe.inside != Some(hit.entity);
        probe.inside = Some(hit.entity);
        if !entered {
            continue;
        }
        let position = origin + direction * hit.distance;
        // A degenerate normal (a hit on a seam) falls back to facing the round.
        let normal = hit
            .normal
            .try_normalize()
            .unwrap_or_else(|| -Vec3::from(direction));
        let to_camera = camera
            .map(|cam| cam.translation() - position)
            .unwrap_or(Vec3::Z);
        spawn_terrain_read(
            position,
            normal,
            to_camera,
            projectile.caliber(),
            &assets,
            &mut materials,
            &mut ring,
            &mut ground_ring,
            &mut rng,
            &mut commands,
        );
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use avian3d::prelude::{Collider, CollisionLayers, LayerMask, PhysicsPlugins, RigidBody};
    use bevy::app::PluginsState;
    use bevy::asset::AssetPlugin;
    use bevy::time::TimeUpdateStrategy;

    use super::*;

    use crate::vfx::billboard::Billboard;

    /// An MG-belt round: below the big-caliber threshold, so the compact read.
    const MG_CALIBER: f32 = 0.0079;

    /// A one-proxy world: a 4 x 5 x 6 m house box centred at the origin, tagged and layered exactly
    /// as `scatter::spawn` builds one, with the real read mounted and the physics pipeline settled
    /// so the cast can see the collider.
    fn harness() -> App {
        let mut app = App::new();
        app.add_plugins((
            MinimalPlugins,
            // Avian's collider cache reads `AssetEvent<Mesh>`, so the asset system must be present
            // even though this collider carries no mesh handle.
            AssetPlugin::default(),
            PhysicsPlugins::default(),
        ))
        .init_asset::<Mesh>()
        .insert_resource(TimeUpdateStrategy::ManualDuration(Duration::from_millis(
            16,
        )))
        .init_resource::<BillboardRing>()
        .init_resource::<GroundMarkRing>()
        .init_asset::<VfxBillboardMaterial>()
        .insert_resource(ViewRng::seeded(42))
        .insert_resource(ImpactAssets::test_stub())
        .add_systems(Update, read_scatter_crossings);

        // Drive plugin finish/cleanup by hand: Avian registers its diagnostics resources in
        // `Plugin::finish`, and the spatial-query systems require them.
        while app.plugins_state() == PluginsState::Adding {
            std::thread::sleep(Duration::from_millis(1));
        }
        app.finish();
        app.cleanup();

        app.world_mut().spawn((
            Transform::from_translation(Vec3::ZERO),
            RigidBody::Static,
            Collider::cuboid(4.0, 5.0, 6.0),
            CollisionLayers::new([Layer::Terrain], LayerMask::ALL),
            ScatterProxy,
        ));
        // Settle: let Avian register the static collider and build the spatial-query pipeline.
        for _ in 0..8 {
            app.update();
        }
        app
    }

    /// A straight sweep along +X at `z`, one 1 m station per frame, from 20 m short of the box to
    /// 6 m past it. The stations sit on the half-metre so none of them lands exactly ON a face —
    /// a segment that ENDS on the surface is a measure-zero case the entry test has no business
    /// resting on.
    fn sweep(z: f32) -> (Vec3, Vec<Vec3>) {
        let at = |x: f32| Vec3::new(x, 0.0, z);
        (at(-20.5), (-20..=6).map(|x| at(x as f32 + 0.5)).collect())
    }

    /// Spawn a shell at `from` and walk it through `path`, one frame per station, returning the
    /// billboard count after each step.
    fn walk(app: &mut App, from: Vec3, path: &[Vec3]) -> Vec<usize> {
        let shell = app
            .world_mut()
            .spawn((
                Projectile::view_test_round(MG_CALIBER),
                Transform::from_translation(from),
            ))
            .id();
        // The first update only installs the probe (no segment exists yet).
        app.update();
        path.iter()
            .map(|station| {
                app.world_mut()
                    .get_mut::<Transform>(shell)
                    .expect("the test shell keeps its transform")
                    .translation = *station;
                app.update();
                app.world_mut()
                    .query_filtered::<Entity, With<Billboard>>()
                    .iter(app.world())
                    .count()
            })
            .collect()
    }

    /// The entry law: a straight path crossing the box reads ONCE, at the frame that crossed the
    /// wall — not again on the frames spent inside it, and not again on the way out.
    #[test]
    fn one_crossing_reads_once() {
        let mut app = harness();
        // The box's half-extents are 2 x 2.5 x 3, so the −X face stands at x = −2: the segment
        // from −2.5 to −1.5 is the one that crosses it.
        let (from, path) = sweep(0.0);
        let counts = walk(&mut app, from, &path);
        let entry = counts
            .iter()
            .position(|count| *count > 0)
            .expect("the path crosses the wall");
        assert_eq!(
            path[entry].x, -1.5,
            "the read fires on the frame whose segment crosses the wall"
        );
        let after = counts[entry];
        assert!(after > 0, "the crossing spawns the terrain read");
        assert!(
            counts[entry + 1..].iter().all(|count| *count == after),
            "one entry reads once: {counts:?}"
        );
        // …and it is drawn AT the crossing, not at either endpoint of the frame's segment: every
        // layer is born on the struck face, standing off it along the face normal by its own birth
        // size (sub-decimetre for this read) and never inside the wall.
        let world = app.world_mut();
        let mut billboards = world.query::<&Billboard>();
        for billboard in billboards.iter(world) {
            let offset = billboard.origin - Vec3::new(-2.0, 0.0, 0.0);
            assert!(
                offset.length() < 0.5 && offset.x <= 0.0,
                "the read sits on the struck face, got {}",
                billboard.origin,
            );
        }
    }

    /// …and a path that clears the box entirely reads nothing, however long it is.
    #[test]
    fn a_missing_path_reads_nothing() {
        let mut app = harness();
        // The same sweep, offset well past the box's +Z half-extent of 3 m.
        let (from, path) = sweep(20.0);
        let counts = walk(&mut app, from, &path);
        assert!(
            counts.iter().all(|count| *count == 0),
            "a miss draws nothing: {counts:?}"
        );
    }

    /// A single frame long enough to pass CLEAN THROUGH the box still reads its entry: the segment
    /// is cast, never sampled at its endpoints.
    #[test]
    fn a_single_frame_tunnel_still_reads_the_entry() {
        let mut app = harness();
        let counts = walk(
            &mut app,
            Vec3::new(-40.0, 0.0, 0.0),
            &[Vec3::new(40.0, 0.0, 0.0)],
        );
        assert!(counts[0] > 0, "a tunnelling frame still reads the entry");
    }
}
