//! REV-16 element-netcode failure battery.
//!
//! These tests stay below the ECS adapter wherever transport is not the behavior under test: the
//! pure force-law interface is the production seam, and fixed-tick fixtures make every injection
//! reproducible without wall-clock scheduling.

use bevy::math::{Affine3A, Quat, Vec2, Vec3};
use lightyear::prelude::Tick;

use super::*;
use crate::tank::TankSim;
use crate::track::forces::{
    ForceParams, GRIP_ELEMENT_LOSS_DWELL_TICKS, SideInput, SideReport, SideState, contact_side,
    grip_stiffness,
};
use crate::track::oracle::TerrainOracle;
use crate::track::sim::{
    TankTransmission, TrackDrive, TrackDriveSide, TrackGrip, coarse_grip_digest,
};

const DT: f32 = 1.0 / 64.0;
const LINKS: usize = 20;
const LOSS_DWELL: u8 = GRIP_ELEMENT_LOSS_DWELL_TICKS;

#[derive(Clone, Copy)]
struct TestGround {
    surface_y: f32,
    /// Linear world-X grade used by the cross-slope replay and JIP cases.
    cross_slope: f32,
    /// A small deterministic load redistribution on the negative-X shoe edge.
    edge_lift: f32,
}

impl TerrainOracle for TestGround {
    fn depth_along(&self, station: Vec3, _out: Vec3, reach: f32) -> f32 {
        let edge = if station.x < -0.6 {
            self.edge_lift
        } else {
            0.0
        };
        (self.surface_y + self.cross_slope * station.x + edge - station.y).min(reach)
    }
}

fn loop_points() -> Vec<Vec2> {
    // Perimeter 5 m / 20 links = an exact 0.25 m pitch. The first eight material links occupy
    // the two-metre ground run at phase zero, front-to-rear in stable order.
    vec![
        Vec2::new(-1.0, 0.02),
        Vec2::new(1.0, 0.02),
        Vec2::new(1.0, 0.52),
        Vec2::new(-1.0, 0.52),
        Vec2::new(-1.0, 0.02),
    ]
}

fn force_params() -> ForceParams {
    ForceParams {
        thickness: 0.05,
        columns: [(-0.2, 1.0 / 6.0), (0.0, 2.0 / 3.0), (0.2, 1.0 / 6.0)],
        support_stiffness_per_m: 2.0e6,
        support_damping_per_m: 1.0e5,
        engage_depth: 0.002,
        probe_reach: 0.5,
        mu: 0.9,
        lateral_ratio: 0.55,
        slip_saturation: 0.4,
        max_speed: 10.0,
        engine_power: 1.0e5,
        engine_force: 1.0e5,
        governor_gain: 1.0e4,
        inertia: 500.0,
        grip_stiffness: grip_stiffness(0.9, 30_000.0 * 9.81),
    }
}

#[derive(Clone, Copy, Debug, Default)]
struct ForceFrame {
    traction_force: Vec3,
    traction_torque: Vec3,
    belt_reaction: [f32; 2],
    aggregate: [[f32; 2]; 2],
}

impl ForceFrame {
    fn effect(self, field: &TrackGripElements) -> TrackGripEffect {
        TrackGripEffect {
            traction_force: self.traction_force,
            traction_torque: self.traction_torque,
            belt_reaction: self.belt_reaction,
            field_digest: coarse_grip_digest(field),
        }
    }
}

fn side_contact(
    side: usize,
    state: TrackDriveSide,
    affine: Affine3A,
    linear_velocity: Vec3,
    angular_velocity: Vec3,
    ground: &impl TerrainOracle,
    elements: &mut crate::track::forces::GripElements,
) -> SideReport {
    let points = loop_points();
    let (report, live) = contact_side(
        &SideInput {
            loop_pts: &points,
            count: LINKS,
            plane_x: if side == 0 { -0.5 } else { 0.5 },
            command: 0.0,
        },
        SideState {
            speed: state.speed,
            phase: state.phase,
            grip: Vec2::ZERO,
        },
        affine,
        DT,
        &force_params(),
        ground,
        |point| {
            linear_velocity + angular_velocity.cross(point - affine.transform_point3(Vec3::ZERO))
        },
        Some(elements),
    );
    assert!(
        live,
        "the deterministic battery rig must have live station geometry"
    );
    report
}

fn force_frame(
    field: &mut TrackGripElements,
    drive: &[TrackDriveSide; 2],
    affine: Affine3A,
    linear_velocity: Vec3,
    angular_velocity: Vec3,
    ground: &impl TerrainOracle,
) -> ForceFrame {
    let center = affine.transform_point3(Vec3::ZERO);
    let mut frame = ForceFrame::default();
    for (side, &side_drive) in drive.iter().enumerate() {
        let report = side_contact(
            side,
            side_drive,
            affine,
            linear_velocity,
            angular_velocity,
            ground,
            &mut field.sides[side],
        );
        frame.belt_reaction[side] = report.belt_reaction;
        frame.aggregate[side] = [report.state.grip.x, report.state.grip.y];
        for contact in report.contacts {
            frame.traction_force += contact.traction;
            frame.traction_torque += (contact.point - center).cross(contact.traction);
        }
    }
    frame
}

fn contact_normals(
    field: &mut TrackGripElements,
    drive: &[TrackDriveSide; 2],
    affine: Affine3A,
    linear_velocity: Vec3,
    angular_velocity: Vec3,
    ground: &impl TerrainOracle,
) -> Vec<Vec3> {
    let mut normals = Vec::new();
    for (side, &side_drive) in drive.iter().enumerate() {
        normals.extend(
            side_contact(
                side,
                side_drive,
                affine,
                linear_velocity,
                angular_velocity,
                ground,
                &mut field.sides[side],
            )
            .contacts
            .into_iter()
            .filter(|contact| contact.load_elastic > 0.0)
            .map(|contact| contact.normal),
        );
    }
    normals
}

fn assert_field_bits_eq(left: &TrackGripElements, right: &TrackGripElements) {
    for side in 0..2 {
        assert_eq!(left.sides[side].dwell, right.sides[side].dwell);
        assert_eq!(
            left.sides[side].strain.len(),
            right.sides[side].strain.len()
        );
        for (element, (left, right)) in left.sides[side]
            .strain
            .iter()
            .zip(&right.sides[side].strain)
            .enumerate()
        {
            assert_eq!(
                left.to_array().map(f32::to_bits),
                right.to_array().map(f32::to_bits),
                "side {side} element {element} differs"
            );
        }
    }
}

fn assert_frame_bits_eq(left: ForceFrame, right: ForceFrame) {
    assert_eq!(
        left.traction_force.to_array().map(f32::to_bits),
        right.traction_force.to_array().map(f32::to_bits)
    );
    assert_eq!(
        left.traction_torque.to_array().map(f32::to_bits),
        right.traction_torque.to_array().map(f32::to_bits)
    );
    assert_eq!(
        left.belt_reaction.map(f32::to_bits),
        right.belt_reaction.map(f32::to_bits)
    );
    assert_eq!(
        left.aggregate.map(|side| side.map(f32::to_bits)),
        right.aggregate.map(|side| side.map(f32::to_bits))
    );
}

fn front_rear_couple(sign: f32) -> TrackGripElements {
    let mut field = TrackGripElements::for_links(LINKS);
    for side in 0..2 {
        for (link, strain) in [(1, sign), (6, -sign)] {
            for column in 0..3 {
                let element = link * 3 + column;
                field.sides[side].strain[element] = Vec3::X * strain;
                field.sides[side].dwell[element] = LOSS_DWELL;
            }
        }
    }
    field
}

fn checkpoint_round_trip(
    tank: Entity,
    epoch: u32,
    tick: Tick,
    authority: &TrackGripElements,
) -> ExactCheckpoint {
    let chunks = make_checkpoint_chunks(crate::CombatantId(tank.to_bits()), epoch, tick, authority)
        .expect("the fixed test field has a valid sparse checkpoint");
    let expected = authority.sides[0].strain.len();
    let mut assembler = CheckpointAssembler::default();
    let mut completed = None;
    for chunk in chunks.iter().rev() {
        completed = assembler
            .push(chunk.clone(), tank, expected)
            .expect("valid checkpoint chunk")
            .or(completed);
    }
    completed.expect("all chunks assemble one atomic checkpoint")
}

#[test]
fn parked_divergence_wrench_anchor_and_checkpoint_close_hidden_couples_without_looping() {
    let ground = TestGround {
        surface_y: 0.0,
        cross_slope: 0.0,
        edge_lift: 0.0,
    };
    let drive = [TrackDriveSide::default(); 2];
    let mut authority = front_rear_couple(0.004);
    let mut client = front_rear_couple(-0.004);
    let authority_frame = force_frame(
        &mut authority,
        &drive,
        Affine3A::IDENTITY,
        Vec3::ZERO,
        Vec3::ZERO,
        &ground,
    );
    let client_frame = force_frame(
        &mut client,
        &drive,
        Affine3A::IDENTITY,
        Vec3::ZERO,
        Vec3::ZERO,
        &ground,
    );

    let authority_legacy = TrackGrip::default();
    let client_legacy = TrackGrip::default();
    assert_eq!(
        authority_legacy, client_legacy,
        "the injected four-float state matches"
    );
    assert!(
        authority_frame
            .aggregate
            .iter()
            .flatten()
            .all(|value| value.abs() < 0.01)
            && client_frame
                .aggregate
                .iter()
                .flatten()
                .all(|value| value.abs() < 0.01),
        "the field resultants must cancel while their spatial moments remain"
    );
    assert_ne!(
        authority_frame.traction_torque.to_array().map(f32::to_bits),
        client_frame.traction_torque.to_array().map(f32::to_bits),
        "the full wrench anchor must expose the hidden yaw couple"
    );

    let tank = Entity::from_raw_u32(701).unwrap();
    let checkpoint = checkpoint_round_trip(tank, 11, Tick(90), &authority);
    let mut pending = PendingGripCorrection::default();
    pending.stage(checkpoint.clone()).unwrap();
    let installed = pending
        .checkpoint
        .take()
        .expect("the assembled exact checkpoint is staged");
    let installed_key = CheckpointKey::from(&installed);
    client = installed.field;
    pending.mark_applied(installed_key);
    assert_field_bits_eq(&client, &authority);

    // Sustained park: both worlds consume the repaired field with identical zero-slip contact.
    // The rest epoch enters once, then remains fixed; replaying the same reliable chunks cannot
    // stage another correction and therefore cannot form a park/correct/park rollback loop.
    let mut rest = GripRestState {
        initialized: true,
        sides: core::array::from_fn(|side| SideRestState {
            field_hash: exact_side_hash(&client.sides[side]),
            occupancy_hash: occupancy_hash(&client.sides[side]),
            phase_bits: drive[side].phase.to_bits(),
            ..default()
        }),
        ..default()
    };
    let chunks =
        make_checkpoint_chunks(crate::CombatantId(tank.to_bits()), 11, Tick(90), &authority)
            .unwrap();
    let mut duplicate_assembler = CheckpointAssembler::default();
    let mut duplicate_completions = 0;
    for _ in 0..3 {
        for chunk in &chunks {
            duplicate_completions += usize::from(
                duplicate_assembler
                    .push(chunk.clone(), tank, client.sides[0].strain.len())
                    .unwrap()
                    .is_some(),
            );
        }
    }
    assert_eq!(duplicate_completions, 1);

    let mut epoch_transitions = 0;
    for _ in 0..(4 * REST_STABLE_TICKS as usize) {
        let mut authority_next = authority.clone();
        let mut client_next = client.clone();
        let authority_next_frame = force_frame(
            &mut authority_next,
            &drive,
            Affine3A::IDENTITY,
            Vec3::ZERO,
            Vec3::ZERO,
            &ground,
        );
        let client_next_frame = force_frame(
            &mut client_next,
            &drive,
            Affine3A::IDENTITY,
            Vec3::ZERO,
            Vec3::ZERO,
            &ground,
        );
        assert_field_bits_eq(&client_next, &authority_next);
        assert_frame_bits_eq(client_next_frame, authority_next_frame);
        authority = authority_next;
        client = client_next;
        let observations = core::array::from_fn(|side| {
            (
                exact_side_hash(&client.sides[side]),
                occupancy_hash(&client.sides[side]),
                drive[side].phase.to_bits(),
                false,
            )
        });
        let (entered, woke) = advance_rest_epoch(&mut rest, observations);
        epoch_transitions += usize::from(entered || woke);
    }
    assert_eq!(
        epoch_transitions, 1,
        "parked state must enter one epoch only"
    );
    assert_eq!(rest.epoch, 1);
    assert!(rest.sides.iter().all(|side| side.resting));
}

#[test]
fn parked_full_wrench_nullspace_is_exposed_by_load_redistribution_and_repaired() {
    let flat = TestGround {
        surface_y: 0.0,
        cross_slope: 0.0,
        edge_lift: 0.0,
    };
    let redistributed = TestGround {
        surface_y: 0.0,
        cross_slope: 0.0,
        edge_lift: 0.000_2,
    };
    let drive = [TrackDriveSide::default(); 2];
    let mut authority = TrackGripElements::for_links(LINKS);
    let mut hidden = TrackGripElements::for_links(LINKS);
    // Same material link, same lateral force direction. The center column carries weight 2/3;
    // the two edge columns carry 1/6 each, so twice the strain on both edges produces the same
    // flat-ground force and wrench while retaining a different inner/outer distribution.
    let link = 3;
    authority.sides[0].strain[link * 3 + 1] = Vec3::X * 0.004;
    authority.sides[0].dwell[link * 3 + 1] = LOSS_DWELL;
    for column in [0, 2] {
        hidden.sides[0].strain[link * 3 + column] = Vec3::X * 0.008;
        hidden.sides[0].dwell[link * 3 + column] = LOSS_DWELL;
    }

    let authority_flat = force_frame(
        &mut authority.clone(),
        &drive,
        Affine3A::IDENTITY,
        Vec3::ZERO,
        Vec3::ZERO,
        &flat,
    );
    let hidden_flat = force_frame(
        &mut hidden.clone(),
        &drive,
        Affine3A::IDENTITY,
        Vec3::ZERO,
        Vec3::ZERO,
        &flat,
    );
    let initial_force_error = (authority_flat.traction_force - hidden_flat.traction_force).length();
    let initial_torque_error =
        (authority_flat.traction_torque - hidden_flat.traction_torque).length();
    assert!(
        initial_force_error < 0.01,
        "flat force error {initial_force_error}"
    );
    assert!(
        initial_torque_error < 0.01,
        "flat torque error {initial_torque_error}"
    );
    assert_eq!(authority_flat.belt_reaction, hidden_flat.belt_reaction);

    let authority_exposed = force_frame(
        &mut authority,
        &drive,
        Affine3A::IDENTITY,
        Vec3::ZERO,
        Vec3::ZERO,
        &redistributed,
    );
    let hidden_exposed = force_frame(
        &mut hidden,
        &drive,
        Affine3A::IDENTITY,
        Vec3::ZERO,
        Vec3::ZERO,
        &redistributed,
    );
    let exposed = (authority_exposed.traction_force - hidden_exposed.traction_force).length()
        + (authority_exposed.traction_torque - hidden_exposed.traction_torque).length();
    assert!(
        exposed > 1.0,
        "the redistributed load did not expose the hidden field: {exposed}"
    );

    let checkpoint =
        checkpoint_round_trip(Entity::from_raw_u32(702).unwrap(), 12, Tick(91), &authority);
    hidden = checkpoint.field;
    assert_field_bits_eq(&hidden, &authority);
    let repaired = force_frame(
        &mut hidden,
        &drive,
        Affine3A::IDENTITY,
        Vec3::ZERO,
        Vec3::ZERO,
        &redistributed,
    );
    assert_frame_bits_eq(repaired, authority_exposed);
}

#[derive(Clone)]
struct MiniTank {
    position: Vec3,
    rotation: Quat,
    linear_velocity: Vec3,
    angular_velocity: Vec3,
    drive: TrackDrive,
    grip: TrackGrip,
    elements: TrackGripElements,
    transmission: TankTransmission,
    weapon_gate: crate::tank::WeaponGate,
    sim: TankSim,
    last_frame: ForceFrame,
}

impl MiniTank {
    fn tick(&mut self, ground: &impl TerrainOracle) {
        let affine = Affine3A::from_rotation_translation(self.rotation, self.position);
        let frame = force_frame(
            &mut self.elements,
            &self.drive.sides,
            affine,
            self.linear_velocity,
            self.angular_velocity,
            ground,
        );
        self.grip.sides = frame.aggregate;

        // DERIVED battery-body constants. The integration order is explicit and identical on the
        // authority and replay paths; it exists only to make a one-tick-late grip install observable
        // in pose, not to approximate Avian's solver.
        const MASS_KG: f32 = 30_000.0;
        const INERTIA: Vec3 = Vec3::new(90_000.0, 120_000.0, 100_000.0);
        self.linear_velocity += frame.traction_force / MASS_KG * DT;
        self.angular_velocity += frame.traction_torque / INERTIA * DT;
        self.position += self.linear_velocity * DT;
        let angular_step = self.angular_velocity * DT;
        let angle = angular_step.length();
        if angle > 0.0 {
            self.rotation =
                (Quat::from_axis_angle(angular_step / angle, angle) * self.rotation).normalize();
        }
        for side in 0..2 {
            self.drive.sides[side].phase += f64::from(self.drive.sides[side].speed * DT);
        }
        self.last_frame = frame;
    }

    fn element_hash(&self) -> u64 {
        crate::trace::canonical_element_hash(
            self.position,
            self.rotation,
            self.linear_velocity,
            self.angular_velocity,
            &self.drive,
            &self.grip,
            &self.elements,
            &self.transmission,
            &self.weapon_gate,
            &self.sim,
        )
    }

    fn digest(&self) -> crate::trace::CanonicalTankStateDigest {
        crate::trace::canonical_tank_state_digest(
            self.position,
            self.rotation,
            self.linear_velocity,
            self.angular_velocity,
            &self.drive,
            &self.grip,
            &self.elements,
            &self.transmission,
            &self.weapon_gate,
            &self.sim,
        )
    }
}

fn mini_tank(elements: TrackGripElements, speeds: [f32; 2], phases: [f64; 2]) -> MiniTank {
    MiniTank {
        position: Vec3::ZERO,
        rotation: Quat::IDENTITY,
        linear_velocity: Vec3::ZERO,
        angular_velocity: Vec3::new(0.0, 0.35, 0.0),
        drive: TrackDrive {
            throttle: 0.0,
            steer: 1.0,
            sides: core::array::from_fn(|side| TrackDriveSide {
                speed: speeds[side],
                phase: phases[side],
            }),
        },
        grip: TrackGrip::default(),
        elements,
        transmission: TankTransmission::for_governor(),
        weapon_gate: crate::tank::WeaponGate::default(),
        sim: TankSim::default(),
        last_frame: ForceFrame::default(),
    }
}

fn assert_pose_bits_eq(left: &MiniTank, right: &MiniTank, scenario: &str) {
    assert_eq!(
        left.position.to_array().map(f32::to_bits),
        right.position.to_array().map(f32::to_bits),
        "{scenario}: replay position"
    );
    assert_eq!(
        left.rotation.to_array().map(f32::to_bits),
        right.rotation.to_array().map(f32::to_bits),
        "{scenario}: replay rotation"
    );
    assert_eq!(
        left.linear_velocity.to_array().map(f32::to_bits),
        right.linear_velocity.to_array().map(f32::to_bits),
        "{scenario}: replay linear velocity"
    );
    assert_eq!(
        left.angular_velocity.to_array().map(f32::to_bits),
        right.angular_velocity.to_array().map(f32::to_bits),
        "{scenario}: replay angular velocity"
    );
}

#[test]
fn rollback_during_pivot_replays_field_wrench_reactions_pose_and_helm_bit_exactly() {
    let pitch = 0.25_f64;
    let cases = [
        (
            "maximum-yaw-resistance",
            [1.0, -1.0],
            [0.0, 0.0],
            TestGround {
                surface_y: 0.0,
                cross_slope: 0.0,
                edge_lift: 0.0,
            },
            GRIP_SHEAR_MODULUS_M,
        ),
        (
            "material-phase-wrap",
            [1.0, -1.0],
            [pitch - 1e-7, -pitch + 1e-7],
            TestGround {
                surface_y: 0.0,
                cross_slope: 0.0,
                edge_lift: 0.0,
            },
            0.03,
        ),
        (
            "opposite-belt-directions",
            [2.0, -1.5],
            [0.07, -0.11],
            TestGround {
                surface_y: 0.0,
                cross_slope: 0.0,
                edge_lift: 0.0,
            },
            0.03,
        ),
        (
            "one-side-nearly-stationary",
            [f32::from_bits(1), 1.5],
            [0.13, 0.19],
            TestGround {
                surface_y: 0.0,
                cross_slope: 0.0,
                edge_lift: 0.0,
            },
            0.03,
        ),
        (
            "cross-slope-contact-frames",
            [0.8, -0.6],
            [0.03, -0.21],
            TestGround {
                surface_y: 0.0,
                cross_slope: 0.003,
                edge_lift: 0.000_5,
            },
            0.03,
        ),
    ];

    for (case_index, (name, speeds, phases, ground, strain)) in cases.into_iter().enumerate() {
        let baseline = mini_tank(front_rear_couple(strain), speeds, phases);
        assert!(
            baseline.elements.is_sized_for(LINKS),
            "{name}: complete fixed field"
        );
        if name == "maximum-yaw-resistance" {
            assert!(
                baseline
                    .elements
                    .sides
                    .iter()
                    .flat_map(|side| &side.strain)
                    .any(|j| { j.length().to_bits() == GRIP_SHEAR_MODULUS_M.to_bits() })
            );
        }
        if name == "material-phase-wrap" {
            assert_eq!(
                crate::track::forces::phase_decompose(phases[0], pitch as f32).0,
                0
            );
            assert_eq!(
                crate::track::forces::phase_decompose(phases[1], pitch as f32).0,
                -1
            );
        }
        if name == "opposite-belt-directions" {
            assert!(speeds[0].is_sign_positive() && speeds[1].is_sign_negative());
        }
        if name == "one-side-nearly-stationary" {
            assert!(speeds[0].abs() < 1e-30 && speeds[1].abs() > 1.0);
        }
        if name == "cross-slope-contact-frames" {
            let mut witness_field = baseline.elements.clone();
            let normals = contact_normals(
                &mut witness_field,
                &baseline.drive.sides,
                Affine3A::from_rotation_translation(baseline.rotation, baseline.position),
                baseline.linear_velocity,
                baseline.angular_velocity,
                &ground,
            );
            assert!(
                normals.iter().enumerate().any(|(index, left)| {
                    normals[index + 1..]
                        .iter()
                        .any(|right| left.dot(*right).abs() < 0.999)
                }),
                "{name}: fixture must load non-parallel belt/link contact frames"
            );
        }

        let checkpoint = checkpoint_round_trip(
            Entity::from_raw_u32(800 + case_index as u32).unwrap(),
            20 + case_index as u32,
            Tick(101),
            &baseline.elements,
        );
        assert_eq!(
            checkpoint_rollback_baseline(checkpoint.state_entering_tick),
            Tick(100)
        );

        let mut authority = baseline.clone();
        for _ in 0..12 {
            authority.tick(&ground);
        }

        let mut divergent = baseline.clone();
        divergent.elements.sides[0].strain[3] = Vec3::ZERO;
        let mut late_install = divergent.clone();
        late_install.tick(&ground);
        late_install.elements = checkpoint.field.clone();
        for _ in 1..12 {
            late_install.tick(&ground);
        }
        assert_ne!(
            late_install.digest(),
            authority.digest(),
            "{name}: installing entering-T state after T must be detectably one tick late"
        );

        // The production correction seam restores baseline B=T-1 and installs the exact entering-T
        // field before the first replayed force tick. The corrected replay must land on every bit.
        let mut replay = baseline.clone();
        replay.elements = checkpoint.field;
        for _ in 0..12 {
            replay.tick(&ground);
        }
        assert_field_bits_eq(&replay.elements, &authority.elements);
        assert_frame_bits_eq(replay.last_frame, authority.last_frame);
        assert_pose_bits_eq(&replay, &authority, name);
        assert_eq!(
            replay.element_hash(),
            authority.element_hash(),
            "{name}: canonical helm"
        );
        assert_eq!(
            replay.digest(),
            authority.digest(),
            "{name}: complete canonical state"
        );

        if name == "maximum-yaw-resistance" {
            assert!(
                authority.last_frame.traction_torque.y.abs() > 1_000.0,
                "{name}: fixture never exercised a material yaw couple"
            );
        }
        if name == "cross-slope-contact-frames" {
            let left_force = authority.last_frame.traction_force;
            assert!(
                left_force.is_finite() && authority.last_frame.traction_torque.is_finite(),
                "{name}: the redistributed contact-frame result must remain finite"
            );
        }
    }
}

fn uniform_strain(direction: Vec3, magnitude: f32) -> TrackGripElements {
    let mut field = TrackGripElements::for_links(LINKS);
    for side in &mut field.sides {
        for link in 0..8 {
            for column in 0..3 {
                let element = link * 3 + column;
                side.strain[element] = direction * magnitude;
                side.dwell[element] = LOSS_DWELL;
            }
        }
    }
    field
}

#[derive(Clone)]
struct JipCase {
    name: &'static str,
    field: TrackGripElements,
    drive: [TrackDriveSide; 2],
    affine: Affine3A,
    linear_velocity: Vec3,
    angular_velocity: Vec3,
    ground: TestGround,
}

fn jip_cases() -> [JipCase; 3] {
    [
        JipCase {
            name: "slope-parked-nonzero-strain",
            field: uniform_strain(Vec3::Z, 0.006),
            drive: [TrackDriveSide::default(); 2],
            affine: Affine3A::from_rotation_translation(Quat::from_rotation_z(0.04), Vec3::ZERO),
            linear_velocity: Vec3::ZERO,
            angular_velocity: Vec3::ZERO,
            ground: TestGround {
                surface_y: 0.0,
                cross_slope: 0.003,
                edge_lift: 0.000_5,
            },
        },
        JipCase {
            name: "zero-aggregate-yaw-couple",
            field: front_rear_couple(0.004),
            drive: [TrackDriveSide::default(); 2],
            affine: Affine3A::IDENTITY,
            linear_velocity: Vec3::ZERO,
            angular_velocity: Vec3::ZERO,
            ground: TestGround {
                surface_y: 0.0,
                cross_slope: 0.0,
                edge_lift: 0.0,
            },
        },
        JipCase {
            name: "moving-near-phase-wrap",
            field: uniform_strain(Vec3::new(0.003, 0.0, -0.004).normalize(), 0.005),
            drive: [
                TrackDriveSide {
                    speed: 1.2,
                    phase: 0.25 - 1e-7,
                },
                TrackDriveSide {
                    speed: -0.9,
                    phase: -0.25 + 1e-7,
                },
            ],
            affine: Affine3A::IDENTITY,
            linear_velocity: Vec3::new(0.0, 0.0, -0.4),
            angular_velocity: Vec3::new(0.0, 0.2, 0.0),
            ground: TestGround {
                surface_y: 0.0,
                cross_slope: 0.0,
                edge_lift: 0.0,
            },
        },
    ]
}

#[test]
fn join_in_progress_gate_seeds_authoritative_field_before_three_first_force_ticks() {
    for case in jip_cases() {
        let wrong_size = TrackGripElements::for_links(LINKS - 1);
        assert!(!crate::net::rig::replica_role_ready_for_test(
            true, false, None, LINKS
        ));
        assert!(!crate::net::rig::replica_role_ready_for_test(
            true,
            false,
            Some(&wrong_size),
            LINKS,
        ));
        assert!(crate::net::rig::replica_role_ready_for_test(
            true,
            false,
            Some(&case.field),
            LINKS,
        ));

        let mut authority = case.field.clone();
        let mut joined = case.field.clone();
        assert_field_bits_eq(&joined, &authority);
        let authority_first = force_frame(
            &mut authority,
            &case.drive,
            case.affine,
            case.linear_velocity,
            case.angular_velocity,
            &case.ground,
        );
        let joined_first = force_frame(
            &mut joined,
            &case.drive,
            case.affine,
            case.linear_velocity,
            case.angular_velocity,
            &case.ground,
        );
        assert_field_bits_eq(&joined, &authority);
        assert_frame_bits_eq(joined_first, authority_first);

        let mut zero = TrackGripElements::for_links(LINKS);
        let zero_first = force_frame(
            &mut zero,
            &case.drive,
            case.affine,
            case.linear_velocity,
            case.angular_velocity,
            &case.ground,
        );
        assert_ne!(
            zero_first.effect(&zero),
            authority_first.effect(&authority),
            "{}: fixture must make a one-tick zero-grip seed observable",
            case.name,
        );
        if case.name == "zero-aggregate-yaw-couple" {
            assert!(
                authority_first
                    .aggregate
                    .iter()
                    .flatten()
                    .all(|value| value.abs() < 0.01)
            );
            assert!(authority_first.traction_torque.y.abs() > 1.0);
        }
    }
}

// The replicate-once ordering probe uses the same real loopback-UDP floor as `net::shot_loss`.
// It is intentionally kept in this module so the sandbox carve-out can name one exact test.
mod jip_udp {
    use std::net::{Ipv4Addr, SocketAddr};

    use avian3d::prelude::{AngularVelocity, LinearVelocity, Position, Rotation};
    use bevy::prelude::*;
    use lightyear::connection::client_of::ClientOf;
    use lightyear::prelude::client::{
        Client, ClientPlugins, Connect, Connected, InputDelayConfig, NetcodeClient, NetcodeConfig,
    };
    use lightyear::prelude::server::{
        NetcodeConfig as ServerNetcodeConfig, NetcodeServer, ServerPlugins, ServerUdpIo, Start,
    };
    use lightyear::prelude::*;

    use super::*;
    use crate::CombatantId;
    use crate::net::protocol::{NetTank, PROTOCOL_FINGERPRINT};
    use crate::net::test_harness::{TICK, base_app, finish, free_port, lock_real_udp_test};

    #[derive(Resource, Default)]
    struct Spawned(bool);

    #[derive(Resource)]
    struct Expected(Vec<JipCase>);

    #[derive(Resource, Default)]
    struct FirstForceTicks {
        observed: Vec<(u64, TrackGripElements, u32, TrackGripEffect)>,
        zero_seed_ticks: u32,
    }

    fn spawn_jip_roots(
        clients: Query<
            (&RemoteId, Entity),
            (With<ClientOf>, With<Connected>, With<ReplicationSender>),
        >,
        expected: Res<Expected>,
        mut spawned: ResMut<Spawned>,
        mut commands: Commands,
    ) {
        if spawned.0 {
            return;
        }
        let Some((remote, _link)) = clients.iter().next() else {
            return;
        };
        for (index, case) in expected.0.iter().enumerate() {
            commands.spawn((
                NetTank,
                CombatantId(10_000 + index as u64),
                case.field.clone(),
                Position(case.affine.translation.into()),
                Rotation(Quat::from_affine3a(&case.affine)),
                LinearVelocity(case.linear_velocity),
                AngularVelocity(case.angular_velocity),
                TrackDrive {
                    sides: case.drive,
                    ..default()
                },
                TankTransmission::for_governor(),
                Replicate::to_clients(NetworkTarget::All),
                DisableReplicateHierarchy,
                PredictionTarget::to_clients(NetworkTarget::Single(remote.0)),
            ));
        }
        spawned.0 = true;
    }

    fn record_first_force_tick(
        expected: Res<Expected>,
        replicas: Query<
            (
                &CombatantId,
                Has<Predicted>,
                Option<&TrackGripElements>,
                &TrackDrive,
                &Position,
                &Rotation,
                &LinearVelocity,
                &AngularVelocity,
            ),
            (
                With<Remote>,
                With<NetTank>,
                // Match the production attachment gate: ordinary predicted macro state must at
                // least have reached the replica before this transport probe admits the seed.
                With<TankTransmission>,
            ),
        >,
        mut result: ResMut<FirstForceTicks>,
    ) {
        for (id, predicted, field, drive, position, rotation, linear, angular) in &replicas {
            if !predicted
                || result.observed.iter().any(|(seen, _, _, _)| *seen == id.0)
                || !crate::net::rig::replica_role_ready_for_test(predicted, false, field, LINKS)
            {
                continue;
            }
            let Some(field) = field else {
                result.zero_seed_ticks += 1;
                continue;
            };
            if field
                .sides
                .iter()
                .flat_map(|side| &side.strain)
                .all(|strain| *strain == Vec3::ZERO)
            {
                result.zero_seed_ticks += 1;
            }
            let index = (id.0 - 10_000) as usize;
            let case = &expected.0[index];
            // Capture the delivered slab before the synthetic force step mutates its clone.
            // `TrackGripEffect::field_digest` is the end-of-tick digest and therefore cannot name
            // the replicate-once seed when ordinary macro components come from another snapshot.
            let seed_digest = coarse_grip_digest(field);
            let mut first_field = field.clone();
            let frame = force_frame(
                &mut first_field,
                &drive.sides,
                Affine3A::from_rotation_translation(rotation.0, position.0),
                linear.0,
                angular.0,
                &case.ground,
            );
            result
                .observed
                .push((id.0, field.clone(), seed_digest, frame.effect(&first_field)));
        }
    }

    fn build_server(port: u16) -> App {
        let mut app = base_app();
        app.add_plugins(ServerPlugins {
            tick_duration: TICK,
        });
        crate::net::protocol::plugin(&mut app);
        app.add_observer(crate::net::server::attach_replication_sender)
            .insert_resource(Expected(jip_cases().into()))
            .init_resource::<Spawned>()
            .add_systems(Update, spawn_jip_roots);
        let server = app
            .world_mut()
            .spawn((
                NetcodeServer::new(ServerNetcodeConfig {
                    protocol_id: PROTOCOL_FINGERPRINT,
                    private_key: [0; 32],
                    ..default()
                }),
                LocalAddr(SocketAddr::new(Ipv4Addr::LOCALHOST.into(), port)),
                ServerUdpIo::default(),
            ))
            .id();
        app.world_mut().commands().trigger(Start { entity: server });
        app
    }

    fn build_client(port: u16) -> App {
        let mut app = base_app();
        app.add_plugins(ClientPlugins {
            tick_duration: TICK,
        });
        crate::net::protocol::plugin(&mut app);
        app.insert_resource(Expected(jip_cases().into()))
            .init_resource::<FirstForceTicks>()
            .add_systems(FixedUpdate, record_first_force_tick);
        let server_addr = SocketAddr::new(Ipv4Addr::LOCALHOST.into(), port);
        let client = app
            .world_mut()
            .spawn((
                Client::default(),
                Link::new(None),
                LocalAddr(SocketAddr::new(Ipv4Addr::LOCALHOST.into(), 0)),
                PeerAddr(server_addr),
                PredictionManager::default(),
                InputTimelineConfig::new(SyncConfig::default(), InputDelayConfig::no_input_delay()),
                NetcodeClient::new(
                    Authentication::Manual {
                        server_addr,
                        client_id: 9_001,
                        private_key: [0; 32],
                        protocol_id: PROTOCOL_FINGERPRINT,
                    },
                    NetcodeConfig::default(),
                )
                .expect("manual loopback token"),
                UdpIo::default(),
            ))
            .id();
        app.world_mut()
            .commands()
            .trigger(Connect { entity: client });
        app
    }

    #[test]
    fn join_in_progress_replicate_once_seed_precedes_first_predicted_force_tick_over_udp() {
        let _udp = lock_real_udp_test();
        let port = free_port();
        let mut server = build_server(port);
        let mut client = build_client(port);
        finish(&mut server);
        finish(&mut client);

        for _ in 0..1_200 {
            server.update();
            client.update();
            if client.world().resource::<FirstForceTicks>().observed.len() == 3 {
                break;
            }
        }

        let result = client.world().resource::<FirstForceTicks>();
        assert_eq!(
            result.zero_seed_ticks, 0,
            "a predicted force tick saw a zero field"
        );
        assert_eq!(
            result.observed.len(),
            3,
            "not every JIP tank reached its first force tick"
        );
        let cases = jip_cases();
        for (id, field, seed_digest, effect) in &result.observed {
            let case = &cases[(*id - 10_000) as usize];
            assert_field_bits_eq(field, &case.field);
            assert_eq!(
                *seed_digest,
                coarse_grip_digest(&case.field),
                "{} replicate-once seed digest",
                case.name
            );
            assert!(
                effect.traction_force.is_finite()
                    && effect.traction_torque.is_finite()
                    && effect
                        .belt_reaction
                        .iter()
                        .all(|reaction| reaction.is_finite()),
                "{} first force must be finite",
                case.name
            );
            assert!(
                effect.traction_force != Vec3::ZERO
                    || effect.traction_torque != Vec3::ZERO
                    || effect.belt_reaction != [0.0; 2],
                "{} first force must retain non-zero grip",
                case.name
            );
        }
    }
}

#[test]
fn packet_loss_anchor_compare_tolerates_isolated_and_burst_gaps() {
    let delivered_ticks = [40, 42, 47, 50];
    let mut effects = PredictionHistory::<TrackGripEffect>::default();
    let mut rotations = PredictionHistory::<Rotation>::default();
    for tick in 40..=50 {
        effects.add_predicted(
            Tick(tick),
            Some(TrackGripEffect {
                traction_force: Vec3::new(tick as f32, -(tick as f32) * 2.0, 3.0),
                traction_torque: Vec3::new(4.0, tick as f32 * 5.0, -6.0),
                belt_reaction: [tick as f32 * 7.0, -(tick as f32) * 8.0],
                field_digest: tick,
            }),
        );
        rotations.add_predicted(
            Tick(tick),
            Some(Rotation(Quat::from_rotation_y(tick as f32))),
        );
    }

    for tick in delivered_ticks {
        let (predicted, rotation) =
            historical_anchor_state(&effects, &rotations, Tick(tick), Tick(50)).expect(
                "a delivered anchor compares at its own producing tick despite sequence gaps",
            );
        assert_eq!(predicted.field_digest, tick);
        assert_eq!(rotation.0, Quat::from_rotation_y(tick as f32));
        let anchor = NetTrackGripAnchor {
            producing_tick: Tick(tick),
            traction_force: predicted.traction_force,
            traction_torque: predicted.traction_torque,
            belt_reaction: predicted.belt_reaction,
            field_digest: predicted.field_digest,
            ..default()
        };
        assert_eq!(
            effect_errors(
                &anchor,
                predicted,
                DT,
                1.0,
                ComputedAngularInertia::INFINITY.inverse(),
                1.0,
            ),
            EffectErrors::default(),
            "tick {tick}: a skipped predecessor must not poison the comparison"
        );
    }

    assert!(
        historical_anchor_state(&effects, &rotations, Tick(39), Tick(50)).is_none(),
        "an anchor older than retained history must wait rather than compare against the present"
    );
}

fn dense_checkpoint_field() -> TrackGripElements {
    let mut field = TrackGripElements::for_links(LINKS);
    for side in 0..2 {
        for element in 0..field.sides[side].strain.len() {
            let x = (element as f32 + 1.0) * 1e-5;
            field.sides[side].strain[element] = Vec3::new(x, -x * 0.5, x * 0.25);
            field.sides[side].dwell[element] = LOSS_DWELL;
        }
    }
    field
}

#[test]
fn packet_loss_checkpoint_chunks_stay_atomic_and_idempotent_under_drop_duplicate_reorder() {
    let tank = Entity::from_raw_u32(950).unwrap();
    let field = dense_checkpoint_field();
    let chunks =
        make_checkpoint_chunks(crate::CombatantId(tank.to_bits()), 31, Tick(400), &field).unwrap();
    assert!(
        chunks.len() >= 3,
        "fixture needs individual, burst, and completion chunks"
    );
    let expected = field.sides[0].strain.len();

    for dropped in 0..chunks.len() {
        let mut assembler = CheckpointAssembler::default();
        // Reverse order and duplicate every admitted chunk. No partial result can escape while one
        // chunk (including the final completion opportunity) is absent.
        for index in (0..chunks.len()).rev().filter(|index| *index != dropped) {
            assert!(
                assembler
                    .push(chunks[index].clone(), tank, expected)
                    .unwrap()
                    .is_none()
            );
            assert!(
                assembler
                    .push(chunks[index].clone(), tank, expected)
                    .unwrap()
                    .is_none()
            );
        }
        let completed = assembler
            .push(chunks[dropped].clone(), tank, expected)
            .unwrap()
            .expect("the reliable resend of the one dropped chunk completes atomically");
        assert_field_bits_eq(&completed.field, &field);
        assert!(
            assembler
                .push(chunks[dropped].clone(), tank, expected)
                .unwrap()
                .is_none(),
            "a duplicated completion delivery must be idempotent"
        );
    }

    // Drop a burst, then deliver it reordered. This is a fresh assembly so no completed-ledger
    // shortcut can make the assertion vacuous.
    let mut assembler = CheckpointAssembler::default();
    for chunk in chunks.iter().skip(2) {
        assert!(
            assembler
                .push(chunk.clone(), tank, expected)
                .unwrap()
                .is_none()
        );
    }
    assert!(
        assembler
            .push(chunks[1].clone(), tank, expected)
            .unwrap()
            .is_none()
    );
    let completed = assembler
        .push(chunks[0].clone(), tank, expected)
        .unwrap()
        .expect("the reordered anchor burst repair completes once");
    assert_field_bits_eq(&completed.field, &field);
}

#[test]
fn packet_loss_of_rest_to_wake_delivery_never_gates_local_grip_physics() {
    let mut field = front_rear_couple(0.004);
    let before = field.clone();
    let rest_side = SideRestState {
        resting: true,
        field_hash: exact_side_hash(&field.sides[0]),
        occupancy_hash: occupancy_hash(&field.sides[0]),
        phase_bits: 0.0_f64.to_bits(),
        stable_ticks: REST_STABLE_TICKS,
    };
    let client_stale_rest = GripRestState {
        initialized: true,
        epoch: 9,
        sides: [rest_side; 2],
        ..default()
    };
    let drive = [
        TrackDriveSide {
            speed: 0.8,
            phase: 0.0,
        },
        TrackDriveSide {
            speed: -0.6,
            phase: 0.0,
        },
    ];
    let frame = force_frame(
        &mut field,
        &drive,
        Affine3A::IDENTITY,
        Vec3::new(0.2, 0.0, -0.3),
        Vec3::new(0.0, 0.4, 0.0),
        &TestGround {
            surface_y: 0.0,
            cross_slope: 0.0,
            edge_lift: 0.0,
        },
    );

    assert!(client_stale_rest.sides.iter().all(|side| side.resting));
    assert!(
        field
            .sides
            .iter()
            .zip(&before.sides)
            .any(|(after, before)| after.strain != before.strain),
        "local material state must evolve even if the wake epoch/anchor was dropped"
    );
    assert!(
        frame.traction_force.length_squared() > 0.0 || frame.traction_torque.length_squared() > 0.0,
        "wake delivery cannot suppress the local force law"
    );
}

#[derive(Clone, Copy, Debug)]
struct HealingSample {
    cumulative_phase_m: [f64; 2],
    field_rms_mm: [f32; 2],
}

fn field_rms_mm(authority: &TrackGripElements, client: &TrackGripElements) -> [f32; 2] {
    core::array::from_fn(|side| {
        let squared: f32 = authority.sides[side]
            .strain
            .iter()
            .zip(&client.sides[side].strain)
            .map(|(authority, client)| (*authority - *client).length_squared())
            .sum();
        (squared / authority.sides[side].strain.len() as f32).sqrt() * 1_000.0
    })
}

fn injected_healing_fields() -> (TrackGripElements, TrackGripElements) {
    let mut authority = TrackGripElements::for_links(LINKS);
    let mut client = TrackGripElements::for_links(LINKS);
    for side in 0..2 {
        for link in 0..8 {
            for column in 0..3 {
                let element = link * 3 + column;
                authority.sides[side].dwell[element] = LOSS_DWELL;
                client.sides[side].dwell[element] = LOSS_DWELL;
                let sign = if link < 4 { 1.0 } else { -1.0 };
                client.sides[side].strain[element] = Vec3::X * (0.03 * sign);
            }
        }
    }
    (authority, client)
}

fn healing_curve(
    speeds: [f32; 2],
    linear_velocity: Vec3,
    angular_velocity: Vec3,
) -> Vec<HealingSample> {
    const SAMPLE_TICKS: [usize; 7] = [0, 16, 32, 64, 128, 192, 256];
    let ground = TestGround {
        surface_y: 0.0,
        cross_slope: 0.0,
        edge_lift: 0.0,
    };
    let (mut authority, mut client) = injected_healing_fields();
    let mut drive = core::array::from_fn(|side| TrackDriveSide {
        speed: speeds[side],
        phase: 0.0,
    });
    let mut cumulative_phase_m = [0.0_f64; 2];
    let mut samples = Vec::with_capacity(SAMPLE_TICKS.len());
    for tick in 0..=256 {
        if SAMPLE_TICKS.contains(&tick) {
            samples.push(HealingSample {
                cumulative_phase_m,
                field_rms_mm: field_rms_mm(&authority, &client),
            });
        }
        if tick == 256 {
            break;
        }
        force_frame(
            &mut authority,
            &drive,
            Affine3A::IDENTITY,
            linear_velocity,
            angular_velocity,
            &ground,
        );
        force_frame(
            &mut client,
            &drive,
            Affine3A::IDENTITY,
            linear_velocity,
            angular_velocity,
            &ground,
        );
        for side in 0..2 {
            let delta = f64::from(drive[side].speed * DT);
            drive[side].phase += delta;
            cumulative_phase_m[side] += delta.abs();
        }
    }
    samples
}

fn print_healing_curve(name: &str, samples: &[HealingSample]) {
    let values: Vec<String> = samples
        .iter()
        .map(|sample| {
            format!(
                "L({:.3}m,{:.3}mm) R({:.3}m,{:.3}mm)",
                sample.cumulative_phase_m[0],
                sample.field_rms_mm[0],
                sample.cumulative_phase_m[1],
                sample.field_rms_mm[1],
            )
        })
        .collect();
    println!(
        "MEASURED grip self-healing vs cumulative belt phase [{name}]: {}",
        values.join(" -> ")
    );
}

#[test]
fn self_healing_error_curves_are_measured_against_cumulative_belt_phase_per_side() {
    let turnover = healing_curve([1.0, 1.25], Vec3::new(0.0, 0.0, -0.4), Vec3::ZERO);
    let locked_slide = healing_curve([0.0, 0.0], Vec3::new(0.0, 0.0, 1.0), Vec3::ZERO);
    let one_side_stationary = healing_curve([0.0, 1.0], Vec3::ZERO, Vec3::new(0.0, 0.55, 0.0));
    print_healing_curve("ordinary-turnover", &turnover);
    print_healing_curve("locked-belt-sliding", &locked_slide);
    print_healing_curve("one-side-stationary-turn", &one_side_stationary);

    let first = |curve: &[HealingSample], side: usize| curve[0].field_rms_mm[side];
    let last = |curve: &[HealingSample], side: usize| curve.last().unwrap().field_rms_mm[side];
    assert!(
        last(&turnover, 0) < first(&turnover, 0) * 0.1
            && last(&turnover, 1) < first(&turnover, 1) * 0.1,
        "ordinary material turnover should remove at least 90% of the injected field error"
    );
    assert!(
        locked_slide
            .iter()
            .all(|sample| sample.cumulative_phase_m == [0.0, 0.0])
    );
    assert!(
        one_side_stationary
            .iter()
            .all(|sample| sample.cumulative_phase_m[0] == 0.0)
    );
    assert!(one_side_stationary.last().unwrap().cumulative_phase_m[1] > 3.9);
    assert!(
        last(&one_side_stationary, 1) < last(&one_side_stationary, 0),
        "the turning-over side should heal further than the non-turning side"
    );
}
