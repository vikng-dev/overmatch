//! The driver's two-stage crossing: the hand-off from the entrance corridor to the transit
//! corridor counts every entry exactly once.

use avian3d::prelude::{CollisionLayers, LayerMask, Position, RigidBody, Rotation};
use bevy::ecs::system::RunSystemOnce;

use super::*;
use crate::ballistics::march_tests::{box_trimesh, world_with_trimesh_plates};
use crate::ballistics::walk::FaceHit;
use crate::ballistics::{BallisticVolume, RoundBody};

/// Fixture round and plate: an 88-class disc against 50 mm of steel. Fixture data; nothing below
/// depends on whether it perforates.
const ROUND: RoundBody = RoundBody {
    mass: 10.2,
    caliber: 0.088,
    nose_factor: 7.0 / 24.0,
};
const SPEED: f32 = 800.0;
const PLATE: Vec3 = Vec3::new(3.0, 3.0, 0.05);
/// Steel: reference-mm per metre.
const STEEL: f32 = 1000.0;
/// Where the march segment starts, short of first contact — the anchor's distance from the
/// hand-off, which sizes the seed's "on the hand-off plane" band (DERIVED: 5.2e-6 m at 10.6 m).
const STANDOFF: f32 = 10.585;
/// Plate orientations per map position, and shots per plate.
const PLATES: usize = 12;
const SHOTS: usize = 12;

/// The first boundary each sample's transit corridor closes is owned exactly once: by the seed
/// (declared presence) or by one entry crossing in the corridor before that exit.
fn assert_single_ownership(transit: &DiscCorridor, pose: &str) {
    for (index, sample) in transit.samples.iter().enumerate() {
        let mut hits: Vec<&FaceHit> = sample.hits.iter().collect();
        hits.sort_by(|a, b| a.t.total_cmp(&b.t));
        let Some(at) = hits
            .iter()
            .position(|hit| hit.true_normal.dot(transit.axis) > 0.0)
        else {
            continue;
        };
        let exit = hits[at];
        let key = ShellKey {
            volume: exit.volume,
            primitive: exit.primitive,
            shell: exit.shell,
        };
        let entries = hits[..at]
            .iter()
            .filter(|hit| {
                hit.volume == key.volume
                    && hit.primitive == key.primitive
                    && hit.shell == key.shell
                    && hit.true_normal.dot(transit.axis) < 0.0
            })
            .count();
        let seeded = usize::from(sample.initial_presence.contains(&key));
        assert_eq!(
            entries + seeded,
            1,
            "{pose}, sample {index}: the entry of {key:?} is owned by the seed {seeded} time(s) \
             and by the corridor {entries} time(s)",
        );
    }
}

/// THE HAND-OFF PARTITION, END TO END, ON GENERIC GEOMETRY.
///
/// Each ring sample's entry face lies within rounding of the mean entrance plane the transit hands
/// off on, so whether the seed or the transit corridor owns it is decided inside the topology band
/// — the place a dropped entry (`UnexpectedExit`) or a doubled one (`UnexpectedEntry`) lives. A
/// plate in general orientation, struck at up to 60° anywhere on its face, near the world origin
/// and far from it, from the march's own stand-off before contact. Every pose the round bites into must hand off with every sample's
/// entry owned once and the transit corridor pairing; the terminal outcome is not asserted.
#[test]
fn every_entry_is_owned_once_across_the_hand_off() {
    let mut transits = 0;
    // A fixed pseudo-random sequence: general orientations, never axis-aligned, so the plate's
    // faces are rotated in every collider-local coordinate the crossing arithmetic rounds in.
    let mut state = 0x9e37_79b9_u32;
    let mut unit = move || {
        state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        (state >> 8) as f32 / (1u32 << 24) as f32
    };
    for far in [
        Vec3::ZERO,
        Vec3::new(37.3, 0.0, -21.9),
        Vec3::new(1234.5, 0.0, 987.6),
    ] {
        for _ in 0..PLATES {
            let mut app = world_with_trimesh_plates(&[]);
            let centre = far + Vec3::new(unit() - 0.5, 2.0 + unit() - 0.5, unit() - 0.5);
            let rotation = Quat::from_euler(
                EulerRot::YXZ,
                std::f32::consts::TAU * unit(),
                std::f32::consts::PI * (unit() - 0.5),
                std::f32::consts::TAU * unit(),
            );
            app.world_mut().spawn((
                Transform::from_translation(centre).with_rotation(rotation),
                RigidBody::Static,
                box_trimesh(PLATE),
                CollisionLayers::new([crate::Layer::Armor], LayerMask::ALL),
                BallisticVolume {
                    material_factor: STEEL,
                    substance: "RHA".to_string(),
                },
            ));
            for _ in 0..8 {
                app.update();
            }
            // The plate's +Z face, struck at up to 60° from its normal.
            let normal = rotation * Vec3::Z;
            let (side, up) = normal.any_orthonormal_pair();
            for _ in 0..SHOTS {
                let tilt = Quat::from_axis_angle(
                    (side * (unit() - 0.5) + up * (unit() - 0.5)).normalize_or(side),
                    60.0_f32.to_radians() * unit(),
                );
                let direction = tilt * -normal;
                let point = centre
                    + normal * PLATE.z * 0.5
                    + (side * (unit() - 0.5) + up * (unit() - 0.5)) * 1.5;
                let origin = point - direction * STANDOFF;
                let pose =
                    format!("plate {rotation} at {centre}, shot from {origin} along {direction}");
                let result = app
                    .world_mut()
                    .run_system_once(
                        move |world: ProjectileMarchWorld,
                              colliders: Query<(
                            &'static Position,
                            &'static Rotation,
                            &'static Collider,
                            Option<&'static BallisticSurfaces>,
                        )>| {
                            let armor = SpatialQueryFilter::from_mask(crate::Layer::Armor);
                            let context = ResolveContext {
                                world: &world,
                                colliders: &colliders,
                                armor: &armor,
                                deposit: false,
                                laws: WalkLaws::default(),
                            };
                            let frame = DiscFrame::anchored(direction).expect("a real direction");
                            let radius = ROUND.caliber * 0.5;
                            let entrance = closing_walk(
                                &context,
                                &|_| true,
                                origin,
                                Vec3::ZERO,
                                direction,
                                frame,
                                radius,
                                &[],
                                STANDOFF + ENTRANCE_SPAN,
                            )?;
                            let shot = Shot {
                                caliber: ROUND.caliber,
                                capability: capability(ROUND, SPEED),
                            };
                            let Begin::Transit(request) =
                                walk::begin(&entrance, &shot, &context.laws)
                            else {
                                return Ok(None);
                            };
                            let corridor = build_corridor(
                                &context,
                                &|_| true,
                                request.anchor,
                                request.origin,
                                request.axis,
                                ENTRANCE_SPAN,
                                request.frame,
                                request.radius,
                                &request.seeds,
                            )?;
                            let transit = closing_walk(
                                &context,
                                &|_| true,
                                request.anchor,
                                request.origin,
                                request.axis,
                                request.frame,
                                request.radius,
                                &request.seeds,
                                ENTRANCE_SPAN,
                            )?;
                            walk::finish(&transit, &request, &shot, &context.laws)?;
                            Ok::<_, WalkError>(Some(corridor))
                        },
                    )
                    .expect("the probe system runs");
                match result {
                    Ok(Some(corridor)) => {
                        assert_single_ownership(&corridor, &pose);
                        transits += 1;
                    }
                    Ok(None) => {}
                    Err(error) => panic!("{pose}: the hand-off failed to pair: {error:?}"),
                }
            }
        }
    }
    assert!(
        transits >= 3 * PLATES * SHOTS / 2,
        "the sweep exercised the hand-off ({transits} transits)"
    );
}
