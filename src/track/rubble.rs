//! Buildings and rubble under the running gear, in the game's tank driving physics (game state
//! only). The terrain field ([`super::oracle::BlockField`]) is the heightmap and the authored
//! blocks; a building's elements and standing chunks and every loose piece are Avian colliders the
//! field never holds. [`RubbleOracle`] folds those colliders onto a terrain oracle with the field's
//! own probe semantics and names the body each probe hit, so the belt model's support and grip
//! press on it, and [`apply_reactions`] puts the equal and opposite force on it in the same tick.
//!
//! - **What counts as rubble**: a collider carrying [`BuildingElement`] or [`BuildingChunk`] (on a
//!   static building root), or any collider of a [`DebrisPiece`] body. Terrain colliders, trees and
//!   vehicle colliders are not: the field already holds the ground, and a hull is no ground.
//! - **Probe semantics**: one solid ray over the segment [`TerrainOracle::depth_along`] walks
//!   (origin `reach` behind the station, length `2 × reach`). A buried origin hits at distance 0,
//!   depth `reach`: the field's own buried saturation. The deeper answer wins; a tie goes to the
//!   field.
//! - **Cost gate**: one AABB query per tank per tick over the whole probe reach
//!   ([`RubbleQuery::near`]). With no rubble collider in it every probe is the field's alone and
//!   no ray is cast, so open ground is bit-identical to the field-only oracle.
//! - **Tick truth**: avian refits dynamic bodies' AABBs at the end of its physics step
//!   (`update_solver_body_aabbs`) and each candidate's narrow test reads its current
//!   `Position`/`Rotation`, so a cast in `FixedUpdate` sees the poses the last step wrote.

use avian3d::prelude::{
    ColliderAabb, ColliderOf, Forces, RigidBody, SpatialQuery, SpatialQueryFilter,
    WriteRigidBodyForces,
};
use bevy::ecs::query::QueryFilter;
use bevy::ecs::system::SystemParam;
use bevy::prelude::*;

use crate::buildings::BuildingElement;
use crate::buildings::breakage::BuildingChunk;
use crate::debris::DebrisPiece;

use super::forces::ForceApp;
use super::oracle::{Probe, TerrainOracle};

/// The spatial query and the markers that say which colliders are rubble.
#[derive(SystemParam)]
pub(crate) struct RubbleQuery<'w, 's> {
    spatial: SpatialQuery<'w, 's>,
    built: Query<'w, 's, (), Or<(With<BuildingElement>, With<BuildingChunk>)>>,
    collider_of: Query<'w, 's, &'static ColliderOf>,
    pieces: Query<'w, 's, (), With<DebrisPiece>>,
}

impl RubbleQuery<'_, '_> {
    /// Whether `collider` is rubble (module doc).
    fn is_rubble(&self, collider: Entity) -> bool {
        self.built.contains(collider)
            || self
                .collider_of
                .get(collider)
                .is_ok_and(|of| self.pieces.contains(of.body))
    }

    /// The rubble colliders whose AABBs meet the sphere of `radius` about `center`, in the order
    /// avian's trees visit them.
    pub(crate) fn near(&self, center: Vec3, radius: f32) -> Vec<Entity> {
        let mut near = Vec::new();
        self.spatial.aabb_intersections_with_aabb_callback(
            ColliderAabb::new(center, Vec3::splat(radius)),
            |collider| {
                if self.is_rubble(collider) {
                    near.push(collider);
                }
                true
            },
        );
        near
    }

    /// The rigid body `collider` belongs to (itself when it has none).
    fn body_of(&self, collider: Entity) -> Entity {
        self.collider_of
            .get(collider)
            .map_or(collider, |of| of.body)
    }
}

/// A terrain oracle with the rubble near one tank folded on (module doc).
pub(crate) struct RubbleOracle<'a, 'w, 's, O> {
    pub(crate) ground: &'a O,
    pub(crate) rubble: &'a RubbleQuery<'w, 's>,
    /// [`RubbleQuery::near`] of this tank this tick.
    pub(crate) near: &'a [Entity],
}

impl<O: TerrainOracle> TerrainOracle for RubbleOracle<'_, '_, '_, O> {
    fn depth_along(&self, station: Vec3, out: Vec3, reach: f32) -> f32 {
        self.probe(station, out, reach).depth
    }

    fn probe(&self, station: Vec3, out: Vec3, reach: f32) -> Probe {
        let ground = self.ground.probe(station, out, reach);
        if self.near.is_empty() {
            return ground;
        }
        let Ok(dir) = Dir3::new(out) else {
            return ground;
        };
        let Some(hit) = self.rubble.spatial.cast_ray_predicate(
            station - out * reach,
            dir,
            2.0 * reach,
            true,
            &SpatialQueryFilter::default(),
            &|collider| self.near.contains(&collider),
        ) else {
            return ground;
        };
        let depth = reach - hit.distance;
        if depth > ground.depth {
            Probe {
                depth,
                body: Some(self.rubble.body_of(hit.entity)),
            }
        } else {
            ground
        }
    }
}

/// The probe sphere's radius about the hull origin (m), DERIVED from the probe geometry: a ray
/// ends at most `|pin| + face + 2 × travel + reach` from the origin (station = pin + out ×
/// (face + travel), ray end = station + out × (reach + travel)), with `pin_radius` the farthest
/// pin-line point from the hull origin and `column` the widest lateral column offset.
pub(crate) fn probe_radius(
    pin_radius: f32,
    column: f32,
    face: f32,
    max_travel: f32,
    reach: f32,
) -> f32 {
    pin_radius + column + face + 2.0 * max_travel + reach
}

/// One reaction a body took: `force` (N, world) at `point`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Reaction {
    pub(crate) body: Entity,
    pub(crate) force: Vec3,
    pub(crate) point: Vec3,
}

/// Put every force that pressed on a dynamic body back on it, reversed, at the same point, in
/// emission order, and return what was applied. A static or kinematic body (a standing chunk, a
/// building root) takes none.
pub(crate) fn apply_reactions<F: QueryFilter>(
    apps: &[ForceApp],
    bodies: &mut Query<(Forces, &RigidBody), F>,
) -> Vec<Reaction> {
    let mut applied = Vec::new();
    for app in apps {
        let Some(body) = app.body else {
            continue;
        };
        if let Ok((mut forces, RigidBody::Dynamic)) = bodies.get_mut(body) {
            forces.apply_force_at_point(-app.force, app.point);
            applied.push(Reaction {
                body,
                force: -app.force,
                point: app.point,
            });
        }
    }
    applied
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use avian3d::prelude::{
        Collider, ColliderDensity, Gravity, LinearVelocity, PhysicsPlugins, Position, Rotation,
    };
    use bevy::ecs::system::RunSystemOnce;
    use bevy::math::{Affine3A, Quat, Vec2};
    use bevy::time::TimeUpdateStrategy;

    use super::*;
    use crate::track::forces::{
        ForceParams, GripElements, SideInput, SideReport, SideState, TravelField, contact_side,
        grip_stiffness,
    };
    use crate::track::oracle::{BlockField, TerrainBlock};

    /// Avian at 64 Hz, one fixed tick per update, no gravity: a body moves only by what the
    /// test applies.
    fn physics() -> App {
        let mut app = App::new();
        app.add_plugins((
            MinimalPlugins,
            bevy::transform::TransformPlugin,
            AssetPlugin::default(),
            PhysicsPlugins::default(),
        ))
        .init_asset::<Mesh>()
        .insert_resource(Gravity(Vec3::ZERO))
        .insert_resource(TimeUpdateStrategy::ManualDuration(Duration::from_micros(
            15_625,
        )));
        while app.plugins_state() == bevy::app::PluginsState::Adding {
            std::thread::sleep(Duration::from_millis(1));
        }
        app.finish();
        app.cleanup();
        app
    }

    /// A 2 × 0.4 × 2 m loose piece whose top is at y = 0.4, centred at `(x, z)` (fixture data).
    fn piece(app: &mut App, x: f32, z: f32) -> Entity {
        let at = Vec3::new(x, 0.2, z);
        app.world_mut()
            .spawn((
                DebrisPiece,
                Position(at),
                Rotation::default(),
                RigidBody::Dynamic,
                Collider::cuboid(2.0, 0.4, 2.0),
                ColliderDensity(1800.0),
            ))
            .id()
    }

    /// A static building root holding one 2 × 0.6 × 2 m standing chunk whose top is at y = 0.6,
    /// centred at `(x, z)` (fixture data); returns the root.
    fn standing_chunk(app: &mut App, x: f32, z: f32) -> Entity {
        let root = app
            .world_mut()
            .spawn((
                Transform::from_xyz(x, 0.3, z),
                Position(Vec3::new(x, 0.3, z)),
                RigidBody::Static,
            ))
            .id();
        app.world_mut().spawn((
            ChildOf(root),
            Transform::IDENTITY,
            Collider::cuboid(2.0, 0.6, 2.0),
            BuildingChunk(0),
        ));
        root
    }

    /// Flat terrain field, top face at y = 0.
    fn ground() -> BlockField {
        BlockField::new(vec![TerrainBlock::new(
            Vec3::new(0.0, -0.5, 0.0),
            Quat::IDENTITY,
            Vec3::new(200.0, 1.0, 200.0),
        )])
    }

    /// Probe `station` straight down at reach 0.5 m through the rubble near it.
    fn probe_down(app: &mut App, station: Vec3) -> Probe {
        app.world_mut()
            .run_system_once(move |rubble: RubbleQuery| {
                let field = ground();
                let near = rubble.near(station, 1.0);
                let oracle = RubbleOracle {
                    ground: &field,
                    rubble: &rubble,
                    near: &near,
                };
                oracle.probe(station, Vec3::NEG_Y, 0.5)
            })
            .expect("the probe system runs")
    }

    /// A PROBE OVER A PIECE REPORTS THE PIECE'S SURFACE, AND ONLY RUBBLE COUNTS: the track
    /// model's support datum (`forces::contact_side`) reads this depth, so a probe blind to
    /// pieces leaves a tank's belts sinking through rubble its hull then slides on, and a probe
    /// that took any collider would make terrain colliders and a hull's own proxies its ground.
    /// Over a piece (top 0.4 m) a station at 0.3 m is 0.1 m deep and names the piece; over a
    /// standing chunk (top 0.6 m) 0.3 m deep and names its static root; over an unmarked collider
    /// of the piece's size, and over open ground, it reads the field's own bits and no body.
    #[test]
    fn a_probe_over_a_piece_reports_the_piece_surface() {
        let mut app = physics();
        let loose = piece(&mut app, 0.0, 0.0);
        let root = standing_chunk(&mut app, 10.0, 0.0);
        app.world_mut().spawn((
            Position(Vec3::new(20.0, 0.2, 0.0)),
            RigidBody::Static,
            Collider::cuboid(2.0, 0.4, 2.0),
        ));
        app.update();

        let on_piece = probe_down(&mut app, Vec3::new(0.0, 0.3, 0.0));
        assert!((on_piece.depth - 0.1).abs() < 1e-5, "{on_piece:?}");
        assert_eq!(on_piece.body, Some(loose));

        let on_chunk = probe_down(&mut app, Vec3::new(10.0, 0.3, 0.0));
        assert!((on_chunk.depth - 0.3).abs() < 1e-5, "{on_chunk:?}");
        assert_eq!(on_chunk.body, Some(root));

        let field = ground().depth_along(Vec3::new(0.0, 0.3, 0.0), Vec3::NEG_Y, 0.5);
        for open in [Vec3::new(20.0, 0.3, 0.0), Vec3::new(-10.0, 0.3, 0.0)] {
            let probe = probe_down(&mut app, open);
            assert_eq!(probe.depth.to_bits(), field.to_bits(), "{open}: {probe:?}");
            assert_eq!(probe.body, None, "{open}");
        }
    }

    /// THE REACTION ON A PIECE EQUALS AND OPPOSES THE PROBE'S FORCE, AND A STANDING CHUNK TAKES
    /// NONE: what a belt presses into a loose piece, the piece takes reversed at the same point in
    /// the same tick (the momentum a tank's support and grip put into rubble), so a piece pressed
    /// by 9 kN for one 64 Hz tick changes velocity by exactly −F·dt/m. A standing chunk's static
    /// root never enters the applied list. A mutant that pushes the piece WITH the belt force,
    /// skips it, or spends a reaction on the static root fails one of the asserts.
    #[test]
    fn the_reaction_on_a_piece_equals_and_opposes_the_probe_force() {
        let mut app = physics();
        let loose = piece(&mut app, 0.0, 0.0);
        let root = standing_chunk(&mut app, 10.0, 0.0);
        app.update();
        let mass = 2.0 * 0.4 * 2.0 * 1800.0;
        let before = app.world().get::<LinearVelocity>(loose).unwrap().0;

        let press = Vec3::new(0.0, 9_000.0, 0.0);
        let apps = [
            (Vec3::new(0.0, 0.4, 0.0), Some(loose)),
            (Vec3::new(10.0, 0.6, 0.0), Some(root)),
            (Vec3::new(30.0, 0.0, 0.0), None),
        ]
        .map(|(point, body)| ForceApp {
            force: press,
            point,
            body,
        });
        let applied = app
            .world_mut()
            .run_system_once(move |mut bodies: Query<(Forces, &RigidBody)>| {
                apply_reactions(&apps, &mut bodies)
            })
            .expect("the reaction system runs");
        app.update();

        assert_eq!(
            applied,
            vec![Reaction {
                body: loose,
                force: -press,
                point: Vec3::new(0.0, 0.4, 0.0),
            }]
        );
        let dv = app.world().get::<LinearVelocity>(loose).unwrap().0 - before;
        let expected = -press / mass / 64.0;
        assert!(
            (dv - expected).length() < 1e-4,
            "the piece took {dv}, expected {expected}"
        );
        assert_eq!(
            app.world().get::<Position>(root).unwrap().0,
            Vec3::new(10.0, 0.3, 0.0)
        );
    }

    /// One contact pass of a 2 m pad 5 mm into flat ground at y = 0, driving and sliding
    /// (fixture data).
    fn flat_pass<O: TerrainOracle>(oracle: &O) -> SideReport {
        let loop_pts = [
            Vec2::new(-1.0, 0.02),
            Vec2::new(1.0, 0.02),
            Vec2::new(1.0, 0.52),
            Vec2::new(-1.0, 0.52),
            Vec2::new(-1.0, 0.02),
        ];
        let input = SideInput {
            loop_pts: &loop_pts,
            count: 25,
            plane_x: 0.0,
            columns: [(-0.1, 0.25), (0.0, 0.5), (0.1, 0.25)],
            command: 0.3,
            travel: TravelField { knots: &[] },
        };
        let params = ForceParams {
            face_offset: 0.025,
            free_travel: 0.0,
            support_stiffness_per_m: 2.0e6,
            support_damping_per_m: 1.0e5,
            engage_depth: 0.002,
            probe_reach: 0.5,
            mu: 0.9,
            slip_saturation: 0.5,
            max_speed: 10.0,
            engine_power: 1.0e5,
            engine_force: 1.0e5,
            governor_gain: 1.0e4,
            inertia: 500.0,
            grip_stiffness: grip_stiffness(0.9, 30_000.0 * 9.81),
        };
        let mut elements = GripElements::for_links(25);
        let (report, live) = contact_side(
            &input,
            SideState {
                speed: 0.4,
                phase: 0.0,
                grip: Vec2::ZERO,
            },
            Affine3A::IDENTITY,
            1.0 / 64.0,
            &params,
            oracle,
            |_| Vec3::new(0.0, -0.01, 0.3),
            &mut elements,
        );
        assert!(live);
        report
    }

    /// Every bit of a report's force applications, contacts and belt reaction.
    fn bits(report: &SideReport) -> Vec<u32> {
        let mut out = Vec::new();
        for app in &report.apps {
            out.extend(app.force.to_array().map(f32::to_bits));
            out.extend(app.point.to_array().map(f32::to_bits));
            assert_eq!(app.body, None);
        }
        for c in &report.contacts {
            out.extend(
                [
                    c.load,
                    c.load_elastic,
                    c.slip,
                    c.slip_lat,
                    c.f_long,
                    c.f_lat,
                ]
                .map(f32::to_bits),
            );
        }
        out.push(report.belt_reaction.to_bits());
        out
    }

    /// FLAT GROUND WITH NO PIECES GIVES BIT-IDENTICAL TRACK FORCES: the rubble fold is free where
    /// there is no rubble, so a tank on open ground keeps the exact arithmetic of the field-only
    /// oracle (`track::sim` runs `BlockField` alone). A fold that re-derived the ground, or took a
    /// collider hit of equal depth over the field's, would move bits. Checked with nothing near
    /// (no ray cast) and with a piece 50 m away forced into the near list (rays cast, every one
    /// losing to the field).
    #[test]
    fn flat_ground_with_no_pieces_gives_bit_identical_track_forces() {
        let reference = bits(&flat_pass(&ground()));
        assert!(reference.len() > 1, "the fixture pad is in contact");

        let mut app = physics();
        let far = piece(&mut app, 50.0, 0.0);
        app.update();
        let (nothing_near, far_near) = app
            .world_mut()
            .run_system_once(move |rubble: RubbleQuery| {
                let field = ground();
                let empty = RubbleOracle {
                    ground: &field,
                    rubble: &rubble,
                    near: &[],
                };
                let near = [far];
                let forced = RubbleOracle {
                    ground: &field,
                    rubble: &rubble,
                    near: &near,
                };
                (bits(&flat_pass(&empty)), bits(&flat_pass(&forced)))
            })
            .expect("the contact system runs");
        assert_eq!(nothing_near, reference);
        assert_eq!(far_near, reference);
    }
}
