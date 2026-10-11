//! The driver's two-stage crossing against the real bound tank at a captured, non-spawn pose.

use avian3d::prelude::{Position, Rotation};
use bevy::ecs::system::RunSystemOnce;

use super::*;
use crate::ballistics::RoundBody;
use crate::ballistics::fuzz::{ProbeTank, probe_world};

fn v(bits: [u32; 3]) -> Vec3 {
    Vec3::new(
        f32::from_bits(bits[0]),
        f32::from_bits(bits[1]),
        f32::from_bits(bits[2]),
    )
}

fn q(bits: [u32; 4]) -> Quat {
    Quat::from_xyzw(
        f32::from_bits(bits[0]),
        f32::from_bits(bits[1]),
        f32::from_bits(bits[2]),
        f32::from_bits(bits[3]),
    )
}

/// FIXTURE DATA, captured bit-exact from the headless sim (`mg_rounds_stream_tracers_and_spawn_no_shell_scene`):
/// the target tank after a machine-gun burst had shoved it ~2 cm off its spawn pose, and the
/// main-gun round that then met a turret side plate. The shot origin is where that march segment
/// started, 10.58 m short of first contact. The root pose is the one that reproduces the struck
/// primitive's live pose to the bit, which the test checks. Nothing here is a law; it is one pose at
/// which a ring sample's entry face sits inside the seed's "on the hand-off plane" band (1.72e-6 m
/// behind a plane 10.585 m from the anchor) and beyond `topology_abs`.
const TANK_POSITION: [u32; 3] = [1092616860, 3181426928, 3242197090];
const TANK_ROTATION: [u32; 4] = [956264294, 3055613835, 3092825637, 1065353216];
const STRUCK_PRIMITIVE_POSITION: [u32; 3] = [1092617047, 1070797509, 3242196649];
const STRUCK_PRIMITIVE_ROTATION: [u32; 4] = [956264294, 3055613835, 3092825637, 1065353216];
const SHOT_ORIGIN: [u32; 3] = [1092339405, 1074225219, 3194200864];
const SHOT_DIRECTION: [u32; 3] = [3156905043, 3149174561, 3212835701];
const SHOT_SPEED: u32 = 1145096455;
const SHOT_CONTACT: u32 = 1093220202;
const SHOT_FRAME_U: [u32; 3] = [3093821269, 1065352979, 3148748123];
const SHOT_FRAME_V: [u32; 3] = [1065352308, 0, 3156905213];
const SHOT_BODY: RoundBody = RoundBody {
    mass: 10.2,
    caliber: 0.088,
    nose_factor: 7.0 / 24.0,
};

/// A ring sample whose entry face lies inside the seed's "on the hand-off plane" band is left
/// unseeded, so the transit corridor must count that entry itself. Dropping it as already traversed
/// meets the plate's exit with nothing open: `UnexpectedExit`, and the round fails closed on a plate
/// it bit into.
#[test]
fn a_ring_sample_entering_on_the_hand_off_plane_is_counted_once() {
    let mut app = probe_world().expect("the bound tank builds");
    // Physics has to step for the colliders to follow the moved root.
    app.insert_resource(bevy::time::TimeUpdateStrategy::ManualDuration(
        std::time::Duration::from_millis(16),
    ));
    let tank = app.world().resource::<ProbeTank>().0;
    app.world_mut().entity_mut(tank).insert((
        Position(v(TANK_POSITION)),
        Rotation(q(TANK_ROTATION)),
        Transform::from_translation(v(TANK_POSITION)).with_rotation(q(TANK_ROTATION)),
    ));
    for _ in 0..4 {
        app.update();
    }

    let result = app
        .world_mut()
        .run_system_once(
            |world: ProjectileMarchWorld,
             colliders: Query<(
                &'static Position,
                &'static Rotation,
                &'static Collider,
                Option<&'static BallisticSurfaces>,
            )>| {
                // The fixture must place the struck plate exactly where the live run had it, or it
                // reproduces nothing.
                assert!(
                    colliders.iter().any(|(position, rotation, _, _)| {
                        position.0 == v(STRUCK_PRIMITIVE_POSITION)
                            && rotation.0 == q(STRUCK_PRIMITIVE_ROTATION)
                    }),
                    "the captured root pose no longer reproduces the struck primitive's live pose"
                );
                let armor = SpatialQueryFilter::from_mask(crate::Layer::Armor);
                let context = ResolveContext {
                    world: &world,
                    colliders: &colliders,
                    armor: &armor,
                    deposit: false,
                    laws: WalkLaws::default(),
                };
                let direction = v(SHOT_DIRECTION);
                let speed = f32::from_bits(SHOT_SPEED);
                let contact = f32::from_bits(SHOT_CONTACT);
                let frame = DiscFrame {
                    u: v(SHOT_FRAME_U),
                    v: v(SHOT_FRAME_V),
                };
                let radius = SHOT_BODY.caliber * 0.5;
                let entrance = closing_walk(
                    &context,
                    &|_| true,
                    v(SHOT_ORIGIN),
                    Vec3::ZERO,
                    direction,
                    frame,
                    radius,
                    &[],
                    contact + ENTRANCE_SPAN,
                )
                .expect("the entrance corridor walks");
                let shot = Shot {
                    caliber: SHOT_BODY.caliber,
                    capability: capability(SHOT_BODY, speed),
                };
                let Begin::Transit(request) = walk::begin(&entrance, &shot, &context.laws) else {
                    panic!("the round bites into the plate at this pose");
                };
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
                walk::finish(&transit, &request, &shot, &context.laws)
            },
        )
        .expect("the probe system runs");

    let plan = result.expect("the transit corridor pairs every sample's crossings");
    assert!(
        matches!(plan.outcome, Outcome::Perforated { .. }),
        "the 88 perforates the plate it bit into: {:?}",
        plan.outcome
    );
}
