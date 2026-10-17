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

use super::WorldViewer;
use super::demo::{Library, PLAYLIST, Record, Run, ShotRun};
use super::material::GlowMaterial;
use super::view::Views;
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

/// The demo's battlefield camera: the shooter's sight, the victim's chase view, the kill cam.
#[derive(Component)]
#[require(WorldViewer)]
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
/// Place the main camera when no kill cam owns it: the shooter's sight, or the victim's chase
/// view. The camera has one writer per run; [`animate`] reads the pose it is left with.
pub fn place_camera(
    run: Res<Run>,
    library: Res<Library>,
    mut commands: Commands,
    mut camera: Query<(Entity, &mut Transform, &mut Projection, &CameraProfile), With<MainCamera>>,
) {
    let (Some(first), Ok((entity, mut transform, mut projection, profile))) =
        (run.shots.first(), camera.single_mut())
    else {
        return;
    };
    // The battlefield view, whatever profile a kill cam left installed.
    if *profile != CameraProfile::BattlefieldThirdPerson {
        commands
            .entity(entity)
            .insert(CameraProfile::BattlefieldThirdPerson);
    }
    let report = &library.records[first.record].report;
    let (want, fov) = if PLAYLIST[run.entry].victim {
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
}

/// Place the ground, and each shot's tracer, flash and mark at the run's clock; the tracer is
/// culled near the main camera's final eye, wherever its writer put it.
pub fn animate(
    run: Res<Run>,
    library: Res<Library>,
    views: Res<Views>,
    camera: Query<&Transform, With<MainCamera>>,
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
    let eye = camera
        .single()
        .map(|transform| transform.translation)
        .unwrap_or(Vec3::ZERO);
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

/// Where and how fast the round flies on after the struck tank, if the record has it leave: from
/// the point the record has it leave (the far end of its last crossing, or the face of its last
/// bounce), along the recorded leg that leaves that point, at the striking speed scaled by that
/// leg's `(v/v₀)²`. A round the record has come to rest flies on nowhere, whatever it bounced off
/// on the way.
fn onward(plan: &super::plan::Plan, speed: f32) -> Option<(Vec3, Vec3, f32)> {
    let (at, arc) = plan.left_at?;
    let leg = plan.line.leg_after(arc);
    let energy = plan.line.energy.get(leg).copied().unwrap_or(1.0);
    Some((at, plan.line.leg_direction(leg), speed * energy.sqrt()))
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
    use super::super::demo::{Settings, ShotRun};
    use super::super::plan::Plan;
    use super::*;
    use crate::damage_cam::RoundEnd;

    /// The tracer is culled near the main camera's final eye, wherever its writer put it: on a
    /// kill run a camera on the tracer's path hides it, one far from it shows it.
    #[test]
    fn the_tracer_is_culled_at_the_final_eye() {
        let library = Library::load();
        let settings = Settings::default();
        let entry = (0..PLAYLIST.len())
            .find(|&entry| Run::new(entry, &library, &settings, true).kill.is_some())
            .expect("a kill entry");
        let run = Run::new(entry, &library, &settings, true);
        let shot = run.shots[0].clone();
        let t = 0.5 * (shot.fire + shot.impact);
        let (head, _) = tracer_head(&shot, &library.records[shot.record], t).unwrap();
        for (eye, drawn) in [
            (head + Vec3::Y * 5.0, false),
            (head + Vec3::Y * 500.0, true),
        ] {
            let mut app = App::new();
            let mut run = Run::new(entry, &library, &settings, true);
            run.t = t;
            app.insert_resource(Library::load())
                .insert_resource(run)
                .insert_resource(super::super::view::Views::fixture())
                .insert_resource(Assets::<GlowMaterial>::default())
                .add_systems(Update, animate);
            app.world_mut()
                .spawn((Transform::from_translation(eye), MainCamera));
            let tracer = app
                .world_mut()
                .spawn((Tracer(0), Transform::IDENTITY, Visibility::Hidden))
                .id();
            app.update();
            let visibility = *app.world().get::<Visibility>(tracer).unwrap();
            assert_eq!(visibility == Visibility::Inherited, drawn, "eye {eye}");
        }
    }

    /// The tracer flies the record's own legs (the approach along origin → impact, the onward flight
    /// along the leg leaving the point the record has the round leave), so the only gap between the
    /// two is f32 rounding. The angle (deg) between the tracer's travel and its leg: DERIVED, a
    /// direction normalised in f32 carries ~1e-7 rad of rounding, which 1e-3° (1.7e-5 rad) bounds
    /// with margin.
    const ANGLE_TOL_DEG: f32 = 1.0e-3;
    /// How far (m) the head may sit off its leg's line, or short of where it should be: DERIVED, the
    /// f32 spacing at the staging range ([`SHOOTER_DISTANCE_M`], 700 m) is 6.1e-5 m, and the
    /// approach's lerp over that range carries a few such steps.
    const POSITION_TOL_M: f32 = 1.0e-3;
    /// The run clock is sampled at this step (s).
    const SAMPLE_S: f32 = 1.0e-3;

    /// A tracer: its head and direction of travel at run time `t`, as [`tracer_head`] states them.
    type Head = fn(&ShotRun, &Record, f32) -> Option<(Vec3, Vec3)>;

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

    /// Where the record has the round leave the tank and the recorded leg that leaves that point,
    /// read off the record alone; `None` when it rests.
    fn leaving(
        report: &crate::damage_cam::PenetrationReport,
        path: &[Vec3],
    ) -> Option<(Vec3, Vec3)> {
        let RoundEnd::Left { at } = report.end else {
            return None;
        };
        let at = Vec3::from(at);
        let k = path
            .iter()
            .position(|p| p.distance(at) < 1.0e-5)
            .expect("where the round left is a point of its path");
        let leg = match path.get(k + 1) {
            Some(next) => *next - at,
            None => at - path[k - 1],
        };
        Some((at, leg))
    }

    /// Why the tracer `head` of `shot` departs from its record's path, if it does.
    fn departures(shot: &ShotRun, record: &Record, head: Head) -> Vec<String> {
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
        // The round flies from the shooter's muzzle where the record names one; the march's own
        // start otherwise.
        let first_leg = path[1] - report.muzzle.map_or(path[0], Vec3::from);
        if entry.distance(Vec3::from(report.impact.point)) > POSITION_TOL_M {
            out.push(format!(
                "impact {:?} is not the path's entry {entry}",
                report.impact.point
            ));
        }
        let leaving = leaving(report, &path);
        if let Some((at, _)) = leaving {
            match head(shot, record, shot.impact) {
                Some((start, _)) if start.distance(at) <= POSITION_TOL_M => {}
                other => out.push(format!(
                    "at the impact the tracer is at {other:?}, not where the record has the round \
                     leave, {at}"
                )),
            }
        }
        let (mut approach, mut onward) = (0, 0);
        let mut progress: Option<f32> = None;
        let mut t = shot.fire;
        while t < shot.impact + 1.0 {
            if let Some((h, direction)) = head(shot, record, t) {
                if t < shot.impact {
                    approach += 1;
                    let angle = angle_deg(direction, first_leg);
                    let off = off_line(h, entry, first_leg);
                    let beyond = (h - entry).dot(first_leg.normalize());
                    if angle > ANGLE_TOL_DEG || off > POSITION_TOL_M || beyond > POSITION_TOL_M {
                        out.push(format!(
                            "t {t:.3}: approach {angle:.4} deg and {off:.5} m off the first leg, \
                             {beyond:.5} m past the entry"
                        ));
                        break;
                    }
                } else {
                    onward += 1;
                    let Some((at, leg)) = leaving else {
                        out.push(format!(
                            "t {t:.3}: flies on at {h} although the record has the round rest"
                        ));
                        break;
                    };
                    let angle = angle_deg(direction, leg);
                    let along = (h - at).dot(leg.normalize());
                    let off = off_line(h, at, leg);
                    let allowed = POSITION_TOL_M + along.abs() * ANGLE_TOL_DEG.to_radians().sin();
                    if angle > ANGLE_TOL_DEG || off > allowed {
                        out.push(format!(
                            "t {t:.3}: onward {angle:.4} deg and {off:.5} m off the leg leaving \
                             {at}"
                        ));
                        break;
                    }
                    if along < -POSITION_TOL_M {
                        out.push(format!(
                            "t {t:.3}: onward {along:.4} m behind where the round left, {at}"
                        ));
                        break;
                    }
                    if progress.is_some_and(|before| along <= before) {
                        out.push(format!("t {t:.3}: the onward tracer does not advance"));
                        break;
                    }
                    progress = Some(along);
                }
            }
            t += SAMPLE_S;
        }
        if approach == 0 {
            out.push("no tracer before the impact".to_owned());
        }
        if leaving.is_some() && onward < 2 {
            out.push("the record has the round leave the tank and no tracer flies on".to_owned());
        }
        out
    }

    /// Every shot the playlist fires, with its record.
    fn shots(library: &Library) -> Vec<(ShotRun, &Record)> {
        let settings = Settings::default();
        let mut out = Vec::new();
        for entry in 0..PLAYLIST.len() {
            let run = Run::new(entry, library, &settings, true);
            for shot in run.shots {
                let record = &library.records[shot.record];
                out.push((shot, record));
            }
        }
        out
    }

    /// The battlefield's tracer flies the record's path, in the record's hull frame (the
    /// battlefield's, [`tracer_head`]), for every shot the playlist fires: before the impact along
    /// the path's first leg into its entry, which is the record's impact point; after it only when
    /// the record has the round leave the tank, and then from the point it left (at the impact
    /// itself), along the recorded leg leaving that point, never behind it, advancing every sample.
    #[test]
    fn the_world_tracer_flies_the_records_path() {
        let library = Library::load();
        let mut failures = Vec::new();
        for (shot, record) in shots(&library) {
            for why in departures(&shot, record, tracer_head) {
                failures.push(format!("{}: {why}", record.stem));
            }
        }
        assert!(failures.is_empty(), "{failures:#?}");
    }

    /// The onward flight's direction at `t`, or `None` outside it.
    fn onward_direction(shot: &ShotRun, record: &Record, t: f32) -> Option<Vec3> {
        let (_, direction) = tracer_head(shot, record, t)?;
        (t >= shot.impact).then_some(direction)
    }

    fn stationary(shot: &ShotRun, record: &Record, t: f32) -> Option<(Vec3, Vec3)> {
        match onward_direction(shot, record, t) {
            Some(direction) => Some((record.plan.left_at?.0, direction)),
            None => tracer_head(shot, record, t),
        }
    }

    fn backward(shot: &ShotRun, record: &Record, t: f32) -> Option<(Vec3, Vec3)> {
        let (head, direction) = tracer_head(shot, record, t)?;
        match onward_direction(shot, record, t) {
            Some(_) => Some((2.0 * record.plan.left_at?.0 - head, direction)),
            None => Some((head, direction)),
        }
    }

    /// The onward flight shifted `calibres` calibres along its own line: 20 is the drawn span's
    /// run-out for a small round ([`super::super::plan::REACH_CALIBRES`]).
    fn shifted(shot: &ShotRun, record: &Record, t: f32, calibres: f32) -> Option<(Vec3, Vec3)> {
        let (head, direction) = tracer_head(shot, record, t)?;
        let by = calibres * record.plan.calibre;
        match onward_direction(shot, record, t) {
            Some(_) => Some((head + direction * by, direction)),
            None => Some((head, direction)),
        }
    }

    /// Starts past where the round left.
    fn late(shot: &ShotRun, record: &Record, t: f32) -> Option<(Vec3, Vec3)> {
        shifted(shot, record, t, super::super::plan::REACH_CALIBRES)
    }

    /// Flies the right line and speed from behind where the round left: the tank side of the exit.
    fn behind(shot: &ShotRun, record: &Record, t: f32) -> Option<(Vec3, Vec3)> {
        shifted(shot, record, t, -super::super::plan::REACH_CALIBRES)
    }

    /// The tracer check itself catches an onward tracer that stands still, one placed backward on
    /// the same line, one that flies from the tank side of the point the round left, and one that
    /// starts past it, on every shot that leaves.
    #[test]
    fn the_tracer_check_catches_a_still_backward_or_late_tracer() {
        let library = Library::load();
        let mut leaving = 0;
        for (shot, record) in shots(&library) {
            if record.plan.left_at.is_none() {
                continue;
            }
            leaving += 1;
            for (name, head) in [
                ("still", stationary as Head),
                ("backward", backward as Head),
                ("behind", behind as Head),
                ("late", late as Head),
            ] {
                assert!(
                    !departures(&shot, record, head).is_empty(),
                    "{}: a {name} tracer passes",
                    record.stem
                );
            }
        }
        assert!(leaving > 0, "the playlist has a round that leaves");
    }

    /// A round the record has come to rest flies on nowhere, wherever its path ends: a stop a
    /// fraction of a millimetre inside a component's face, or one after a bounce. Every recorded
    /// path is restated as one that rests in the struck tank's first component.
    #[test]
    fn a_round_the_record_rests_flies_on_nowhere() {
        let library = Library::load();
        let asset = crate::damage_cam::shapes::TankAsset::load(&library.vehicle).unwrap();
        let mut bounced = 0;
        for record in &library.records {
            let mut report = record.report.clone();
            report.end = RoundEnd::Rest {
                volume: report.components[0].name.clone(),
            };
            let plan = Plan::new(&report, &asset.shapes(&report.servo_deg));
            let speed = report.projectile.speed_mps;
            assert!(plan.stop.is_some(), "{}: rests", record.stem);
            assert!(onward(&plan, speed).is_none(), "{}: flies on", record.stem);
            bounced += usize::from(plan.bounce.is_some());
        }
        assert!(bounced > 0, "a record that bounced is restated");
    }
}
