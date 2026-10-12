//! The battlefield half: the struck tank on open ground, seen through the shooter's sight, and the
//! hit as the world shows it — the tracer's flight, the impact flash, the mark left on the face,
//! and the round's onward flight when the record has it leave. The victim scenario watches from
//! behind the struck tank instead.
//!
//! Sizes scale from the round, never from the viewer: the tracer is as wide as the calibre and as
//! long as the distance it covers in [`STREAK_S`]; the flash and the mark scale with the calibre.

use bevy::camera::Hdr;
use bevy::post_process::bloom::Bloom;
use bevy::prelude::*;
use bevy::ui::IsDefaultUiCamera;

use super::material::GlowMaterial;
use super::view::Views;
use super::{Library, PLAYLIST, Record, Run, ShotRun};
use crate::render_policy::{CameraProfile, LightProfile, VisualScope};

/// The staged range from shooter to target (m), inside the spec's 600–1000 m.
pub const SHOOTER_DISTANCE_M: f32 = 700.0;
/// The sight's vertical field (deg): a staging choice. In a 16:9 frame it spans 43.5 m across at
/// [`SHOOTER_DISTANCE_M`] (DERIVED: 2·700·tan(atan(tan 1°·16/9))), so a 3.7 m-wide hull front fills
/// about a twelfth of the width and a 6.3 m side about a seventh.
const SIGHT_FOV_DEG: f32 = 2.0;
/// The victim's chase view: vertical field (deg) and its offset behind and above the hull origin.
const CHASE_FOV_DEG: f32 = 45.0;
const CHASE_EYE: Vec3 = Vec3::new(2.5, 3.6, 11.0);
const CHASE_LOOK: Vec3 = Vec3::new(0.0, 1.2, -2.0);
/// The tracer's streak: the distance the round covers in this exposure (s).
const STREAK_S: f32 = 1.0 / 60.0;
/// The tracer is hidden this close to the eye (m), where the sight looks along it.
const TRACER_NEAR_M: f32 = 40.0;
/// The impact flash: radius in calibres, rise and fall (s).
const FLASH_CALIBRES: f32 = 4.0;
const FLASH_RISE_S: f32 = 0.04;
const FLASH_FALL_S: f32 = 0.30;
/// The round's onward flight after the impact is drawn this long (s).
const ONWARD_S: f32 = 0.35;
/// How many shots one run fires at most.
const MAX_SHOTS: usize = 4;

/// The tracer's emission (linear, above 1 for the bloom; an unlit material shows its base colour
/// and nothing else) and the flash's colour.
const TRACER: LinearRgba = LinearRgba::rgb(60.0, 22.0, 6.0);
const FLASH: LinearRgba = LinearRgba::rgb(1.0, 0.62, 0.3);

pub const SKY: Color = Color::srgb(0.66, 0.73, 0.80);
const GROUND: Color = Color::srgb(0.30, 0.31, 0.24);

#[derive(Component)]
pub struct MainCamera;

#[derive(Component)]
pub(super) struct Tracer(usize);

#[derive(Component)]
pub(super) struct Flash(usize);

#[derive(Component)]
pub(super) struct Mark(usize);

#[derive(Component)]
pub(super) struct Ground;

pub fn spawn(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut glows: ResMut<Assets<GlowMaterial>>,
) {
    commands.spawn((
        Camera3d::default(),
        Camera {
            order: 0,
            ..default()
        },
        Hdr,
        Bloom::NATURAL,
        Projection::Perspective(PerspectiveProjection {
            fov: SIGHT_FOV_DEG.to_radians(),
            far: 5000.0,
            ..default()
        }),
        Transform::from_xyz(0.0, 2.0, -SHOOTER_DISTANCE_M).looking_at(Vec3::Y, Vec3::Y),
        CameraProfile::BattlefieldThirdPerson,
        IsDefaultUiCamera,
        MainCamera,
    ));
    commands.spawn((
        crate::world::sun_light(),
        crate::world::sun_transform(),
        LightProfile::BattlefieldSun,
    ));
    commands.spawn((
        Mesh3d(meshes.add(Plane3d::default().mesh().size(4000.0, 4000.0))),
        MeshMaterial3d(materials.add(StandardMaterial {
            base_color: GROUND,
            perceptual_roughness: 1.0,
            reflectance: 0.0,
            ..default()
        })),
        Transform::IDENTITY,
        Ground,
    ));
    let tracer_mesh = meshes.add(Capsule3d::new(0.5, 1.0));
    let flash_mesh = meshes.add(Sphere::new(1.0));
    let mark_mesh = meshes.add(Circle::new(1.0));
    for shot in 0..MAX_SHOTS {
        commands.spawn((
            Mesh3d(tracer_mesh.clone()),
            MeshMaterial3d(materials.add(StandardMaterial {
                base_color: Color::LinearRgba(TRACER),
                unlit: true,
                ..default()
            })),
            Transform::IDENTITY,
            Visibility::Hidden,
            VisualScope::WORLD_EFFECT,
            Tracer(shot),
        ));
        commands.spawn((
            Mesh3d(flash_mesh.clone()),
            MeshMaterial3d(glows.add(flash_material(1.0))),
            Transform::IDENTITY,
            Visibility::Hidden,
            VisualScope::WORLD_EFFECT,
            Flash(shot),
        ));
        commands.spawn((
            Mesh3d(mark_mesh.clone()),
            MeshMaterial3d(materials.add(StandardMaterial {
                base_color: Color::srgb(0.03, 0.03, 0.03),
                perceptual_roughness: 1.0,
                ..default()
            })),
            Transform::IDENTITY,
            Visibility::Hidden,
            VisualScope::WORLD_EFFECT,
            Mark(shot),
        ));
    }
}

/// Place the sight, the ground, and each shot's tracer, flash and mark at the run's clock.
pub fn animate(
    run: Res<Run>,
    library: Res<Library>,
    views: Res<Views>,
    mut camera: Query<(&mut Transform, &mut Projection), With<MainCamera>>,
    mut ground: Query<&mut Transform, (With<Ground>, Without<MainCamera>)>,
    mut tracers: Query<
        (&Tracer, &mut Transform, &mut Visibility),
        (Without<MainCamera>, Without<Ground>),
    >,
    mut flashes: Query<
        (
            &Flash,
            &mut Transform,
            &mut Visibility,
            &MeshMaterial3d<GlowMaterial>,
        ),
        (Without<MainCamera>, Without<Ground>, Without<Tracer>),
    >,
    mut marks: Query<
        (&Mark, &mut Transform, &mut Visibility),
        (
            Without<MainCamera>,
            Without<Ground>,
            Without<Tracer>,
            Without<Flash>,
        ),
    >,
    mut glows: ResMut<Assets<GlowMaterial>>,
) {
    if let (Some(y), Ok(mut transform)) = (views.ground_y, ground.single_mut()) {
        transform.translation.y = y;
    }
    let victim = PLAYLIST[run.entry].victim;
    let eye = if let (Some(first), Ok((mut transform, mut projection))) =
        (run.shots.first(), camera.single_mut())
    {
        let report = &library.records[first.record].report;
        let (want, fov) = if victim {
            (
                Transform::from_translation(CHASE_EYE).looking_at(CHASE_LOOK, Vec3::Y),
                CHASE_FOV_DEG,
            )
        } else {
            let target = Vec3::from(report.impact.point);
            let up = if (target - first.muzzle).normalize().y.abs() > 0.95 {
                Vec3::NEG_Z
            } else {
                Vec3::Y
            };
            (
                Transform::from_translation(first.muzzle).looking_at(target, up),
                SIGHT_FOV_DEG,
            )
        };
        if *transform != want {
            *transform = want;
        }
        if let Projection::Perspective(p) = projection.as_mut()
            && p.fov != fov.to_radians()
        {
            p.fov = fov.to_radians();
        }
        want.translation
    } else {
        Vec3::ZERO
    };
    let t = run.t;
    for (tracer, mut transform, mut visibility) in &mut tracers {
        let Some(shot) = run.shots.get(tracer.0) else {
            visibility.set_if_neq(Visibility::Hidden);
            continue;
        };
        let record = &library.records[shot.record];
        let speed = record.report.projectile.speed_mps.max(1.0);
        let calibre = record.plan.calibre;
        match tracer_head(shot, record, t) {
            Some((head, direction)) if head.distance(eye) > TRACER_NEAR_M => {
                let length = speed * STREAK_S;
                *transform = Transform::from_translation(head - direction * length * 0.5)
                    .with_rotation(Quat::from_rotation_arc(Vec3::Y, direction))
                    .with_scale(Vec3::new(calibre, length / 2.0, calibre));
                visibility.set_if_neq(Visibility::Inherited);
            }
            _ => {
                visibility.set_if_neq(Visibility::Hidden);
            }
        }
    }
    for (flash, mut transform, mut visibility, material) in &mut flashes {
        let Some(shot) = run.shots.get(flash.0) else {
            visibility.set_if_neq(Visibility::Hidden);
            continue;
        };
        let record = &library.records[shot.record];
        let since = t - shot.impact;
        if !(0.0..FLASH_FALL_S).contains(&since) {
            visibility.set_if_neq(Visibility::Hidden);
            continue;
        }
        let rise = (since / FLASH_RISE_S).min(1.0);
        let fall = 1.0 - (since / FLASH_FALL_S);
        let radius = FLASH_CALIBRES * record.plan.calibre * rise;
        *transform = Transform::from_translation(Vec3::from(record.report.impact.point))
            .with_scale(Vec3::splat(radius.max(1e-4)));
        if let Some(mut m) = glows.get_mut(&material.0) {
            *m = flash_material(fall * fall);
        }
        visibility.set_if_neq(Visibility::Inherited);
    }
    for (mark, mut transform, mut visibility) in &mut marks {
        let Some(shot) = run.shots.get(mark.0) else {
            visibility.set_if_neq(Visibility::Hidden);
            continue;
        };
        if t < shot.impact {
            visibility.set_if_neq(Visibility::Hidden);
            continue;
        }
        let plan = &library.records[shot.record].plan;
        *transform = face_mark(
            plan.impact_point,
            plan.impact_normal,
            plan.line.direction_at(plan.contact_arc - 1e-3),
            plan.calibre,
        );
        visibility.set_if_neq(Visibility::Inherited);
    }
}

/// The tracer's head and direction of travel at run time `t`, in the battlefield's frame, or
/// `None` while the round is not in flight. The battlefield's frame is the record's hull frame:
/// `view::prepare` places the tank view with its hull node at the origin, unrotated.
pub(super) fn tracer_head(shot: &ShotRun, record: &Record, t: f32) -> Option<(Vec3, Vec3)> {
    let plan = &record.plan;
    let impact = Vec3::from(record.report.impact.point);
    let speed = record.report.projectile.speed_mps.max(1.0);
    if t >= shot.fire && t < shot.impact {
        let f = (t - shot.fire) / (shot.impact - shot.fire);
        Some((
            shot.muzzle.lerp(impact, f),
            (impact - shot.muzzle).normalize(),
        ))
    } else if t >= shot.impact && t < shot.impact + ONWARD_S {
        onward(plan, speed).map(|(from, direction, onward_speed)| {
            (
                from + direction * onward_speed * (t - shot.impact),
                direction,
            )
        })
    } else {
        None
    }
}

/// Where and how fast the round flies on after the struck tank, if the record has it leave: off
/// the face of its bounce, or out along its path's last leg, at the striking speed scaled by the
/// last leg's `(v/v₀)²`.
fn onward(plan: &super::plan::Plan, speed: f32) -> Option<(Vec3, Vec3, f32)> {
    let speed = speed * plan.line.energy.last().copied().unwrap_or(1.0).sqrt();
    if let Some(bounce) = &plan.bounce {
        return Some((bounce.point, bounce.outgoing, speed));
    }
    if plan.stop.is_some() || plan.line.points.len() < 3 {
        return None;
    }
    let end = plan.line.length();
    Some((
        plan.line.at(plan.end_arc),
        plan.line.direction_at(end),
        speed,
    ))
}

/// The flash: a soft ball, brightest where it faces the eye and fading to nothing at its limb.
fn flash_material(level: f32) -> GlowMaterial {
    let peak = 8.0 * level;
    GlowMaterial::additive(FLASH, peak).with_rim(peak, -peak, 0.6)
}

/// A flat elliptical mark on a face: a calibre across, stretched along the line's track on the face
/// by `1 / cos(incidence)` (the owner's marks law), lifted 2 mm off it.
pub fn face_mark(point: Vec3, normal: Vec3, travel: Vec3, calibre: f32) -> Transform {
    let normal = normal.normalize_or(Vec3::Y);
    let along = (travel - normal * travel.dot(normal))
        .try_normalize()
        .unwrap_or_else(|| normal.any_orthonormal_vector());
    let cos = (-travel).dot(normal).abs().max(0.2);
    let across = normal.cross(along);
    let rotation = Quat::from_mat3(&Mat3::from_cols(along, across, normal));
    Transform::from_translation(point + normal * 0.002)
        .with_rotation(rotation)
        .with_scale(Vec3::new(calibre * 0.5 / cos, calibre * 0.5, 1.0))
}

#[cfg(test)]
mod tests {
    use super::super::plan::REST_DEPTH_M;
    use super::super::{Settings, ShotRun};
    use super::*;

    /// Tolerances: the angle (deg) between the tracer's travel and the record's leg, and the
    /// distance (m) of its head off that leg's line. A bounce's recorded outgoing direction and the
    /// marched leg after it differ by 0.34° on the recorded ricochet (MEASURED), under the angle.
    const ANGLE_TOL_DEG: f32 = 0.5;
    const POSITION_TOL_M: f32 = 1.0e-3;
    /// The run clock is sampled at this step (s).
    const SAMPLE_S: f32 = 1.0e-3;

    fn angle_deg(a: Vec3, b: Vec3) -> f32 {
        a.normalize()
            .dot(b.normalize())
            .clamp(-1.0, 1.0)
            .acos()
            .to_degrees()
    }

    /// The distance of `p` from the line through `on` along `direction`.
    fn off_line(p: Vec3, on: Vec3, direction: Vec3) -> f32 {
        let d = direction.normalize();
        let v = p - on;
        (v - d * v.dot(d)).length()
    }

    /// Why the tracer of `shot` departs from its record's path, if it does.
    fn departures(shot: &ShotRun, record: &Record) -> Vec<String> {
        let report = &record.report;
        let mut path: Vec<Vec3> = Vec::new();
        for node in &report.path {
            let p = Vec3::from(node.point);
            if path.last().is_none_or(|last| last.distance(p) > 1.0e-6) {
                path.push(p);
            }
        }
        let mut out = Vec::new();
        let entry = path[1];
        let first_leg = path[1] - path[0];
        let last = path[path.len() - 1];
        let leg_start = path[path.len() - 2];
        let last_leg = last - leg_start;
        if entry.distance(Vec3::from(report.impact.point)) > POSITION_TOL_M {
            out.push(format!(
                "impact {:?} is not the path's entry {entry}",
                report.impact.point
            ));
        }
        let rested_in_plate = report.plates.last().is_some_and(|p| p.exit.is_none());
        let rested_in_box = report.internals.iter().any(|b| {
            let rotation = Quat::from_array(b.rotation).normalize();
            let local = rotation.inverse() * (last - Vec3::from(b.center));
            (local.abs() - Vec3::from(b.half_extents)).max_element() < -REST_DEPTH_M
        });
        let leaves = !rested_in_plate && !rested_in_box;
        let (mut approach, mut onward) = (0, 0);
        let mut t = shot.fire;
        while t < shot.impact + 1.0 {
            if let Some((head, direction)) = tracer_head(shot, record, t) {
                if t < shot.impact {
                    approach += 1;
                    let angle = angle_deg(direction, first_leg);
                    let off = off_line(head, entry, first_leg);
                    let beyond = (head - entry).dot(first_leg.normalize());
                    if angle > ANGLE_TOL_DEG || off > POSITION_TOL_M || beyond > POSITION_TOL_M {
                        out.push(format!(
                            "t {t:.3}: approach {angle:.2} deg and {off:.4} m off the first leg, \
                             {beyond:.4} m past the entry"
                        ));
                        break;
                    }
                } else {
                    onward += 1;
                    if !leaves {
                        out.push(format!(
                            "t {t:.3}: flies on at {head} although the path ends at rest at {last}"
                        ));
                        break;
                    }
                    let angle = angle_deg(direction, last_leg);
                    let off = off_line(head, last, last_leg);
                    let allowed = POSITION_TOL_M
                        + head.distance(leg_start) * ANGLE_TOL_DEG.to_radians().sin();
                    if angle > ANGLE_TOL_DEG || off > allowed {
                        out.push(format!(
                            "t {t:.3}: onward {angle:.2} deg and {off:.4} m off the last leg"
                        ));
                        break;
                    }
                }
            }
            t += SAMPLE_S;
        }
        if approach == 0 {
            out.push("no tracer before the impact".to_owned());
        }
        if leaves && onward == 0 {
            out.push("the path leaves the tank and no tracer flies on".to_owned());
        }
        out
    }

    /// The battlefield's tracer flies the record's path, in the record's hull frame (the
    /// battlefield's, [`tracer_head`]), for every shot the playlist fires: before
    /// the impact along the path's first leg (within [`ANGLE_TOL_DEG`]) into its entry, which is
    /// the record's impact point (within [`POSITION_TOL_M`]); after it only when the path leaves
    /// the tank — its last point inside no recorded box and no plate it rested in — and then along
    /// the path's last leg.
    #[test]
    fn the_world_tracer_flies_the_records_path() {
        let library = Library::load();
        let settings = Settings::default();
        let mut failures = Vec::new();
        for entry in 0..PLAYLIST.len() {
            let run = Run::new(entry, &library, &settings, true);
            for shot in &run.shots {
                let record = &library.records[shot.record];
                for why in departures(shot, record) {
                    failures.push(format!("{}: {why}", record.stem));
                }
            }
        }
        assert!(failures.is_empty(), "{failures:#?}");
    }
}
