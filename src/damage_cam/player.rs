//! The damage cam's view half: plays a [`PenetrationReport`] as a four-beat clip over the struck
//! tank's view glb. A pure consumer — it reads the report and the glb, never the sim.
//!
//! Clip time (the playhead, seconds) is not record time: the approach maps onto the impact beat,
//! record `t >= 0` onto the traverse window, and consequences onto the consequence window, by
//! [`Timeline`]. The tank sits at the world origin, so hull-local record positions are world
//! positions.
//!
//! Mounted by `bin/damage_cam_demo`, whose labels use Bevy's default font (ASCII only).

use std::collections::{HashMap, HashSet};

use bevy::camera::ScalingMode;
use bevy::camera::visibility::VisibilitySystems;
use bevy::pbr::{ExtendedMaterial, MaterialExtension};
use bevy::prelude::*;
use bevy::render::render_resource::{AsBindGroup, ShaderType};
use bevy::shader::ShaderRef;
use bevy::time::Real;
use bevy::world_serialization::{WorldAsset, WorldAssetRoot};

use super::{ConsequenceState, PenetrationReport, Role, SpallBurst, Verdict};
use crate::spec::TankSpec;

/// Scenario files, relative to the asset root.
pub const SCENARIO_DIR: &str = "dev/damage_cam";
/// `(label, file)` in key order: `1`..`5`.
pub const SCENARIOS: [(&str, &str); 5] = [
    ("through", "through.ron"),
    ("ricochet", "ricochet.ron"),
    ("stopped", "stopped.ron"),
    ("cookoff", "cookoff.ron"),
    ("overpen", "overpen.ron"),
];

/// The demo's tank fixture: the view the clips are drawn over and the spec that names its collision
/// proxies. The record carries no vehicle id yet.
const TANK_SPEC: &str = "tiger_1/tiger_1.tank.ron";

/// Beat boundaries in clip seconds.
const IMPACT_END: f32 = 0.5;
const REVEAL_END: f32 = 1.2;
const TRAVERSE_END: f32 = 2.7;
const HOLD_START: f32 = 4.0;
/// The approach is drawn over `[0, CONTACT_AT]`; contact, incoming vector and verdict appear here.
const CONTACT_AT: f32 = 0.4;
/// Consequences are spread over this window in record-time order.
const CONSEQUENCE_FROM: f32 = 2.8;
const CONSEQUENCE_TO: f32 = 3.6;
/// One sim tick of record time: the path's stripe period.
const TICK_S: f32 = 1.0 / 64.0;
/// The spall speed law `cos(1.92 theta)` (Verolme-Szymczak) is read no further off axis than this
/// (rad) and never below this fraction of the shell's residual speed.
const FRAGMENT_THETA_MAX: f32 = std::f32::consts::FRAC_PI_4;
const FRAGMENT_SPEED_FLOOR: f32 = 0.1;
/// Minimum axial length (m) drawn for a burst's cone envelope.
const CONE_MIN_LENGTH_M: f32 = 0.3;
/// Path points past the shell's departure from the armour (a ricochet's deflection leg, a
/// through-and-through's exit leg) count toward framing only this far (m) from where it left;
/// points between plates are never clamped.
const FRAME_REACH_M: f32 = 3.0;
/// Section view: below this incidence (deg) the plane of incidence is spanned by the shot line and
/// world up instead of the plate normal.
const SECTION_MIN_INCIDENCE_DEG: f32 = 3.0;
/// The section fits the shot line from this far (m) before the first entry to this far past the
/// last exit or embed, with this many calibres of margin across it.
const SECTION_LEAD_M: f32 = 1.0;
const SECTION_MARGIN_CALIBRES: f32 = 3.0;
/// The section eye's stand-off (m) from the plane.
const SECTION_EYE_M: f32 = 12.0;
/// World up projected into the plane serves as the view's up only above this length.
const SECTION_UP_MIN: f32 = 0.2;
/// Fragments fade out over this distance (m) from the plane in section view.
const SECTION_FADE_M: f32 = 0.5;

const TRACER: Color = Color::srgb(1.0, 0.85, 0.2);
const TRACER_ALT: Color = Color::srgb(0.95, 0.95, 0.9);
const ENTRY: Color = Color::srgb(0.2, 1.0, 0.3);
const OVERMATCH: Color = Color::srgb(1.0, 0.2, 1.0);
const EXIT: Color = Color::srgb(1.0, 0.2, 0.2);
const SPAN: Color = Color::srgb(1.0, 0.45, 0.1);
const RICOCHET: Color = Color::srgb(0.3, 0.8, 1.0);
const CONE: Color = Color::srgb(0.35, 0.37, 0.42);
const FRAGMENT_FAINT: Color = Color::srgba(0.6, 0.62, 0.68, 0.35);
const FRAGMENT_HIT: Color = Color::srgb(1.0, 0.4, 0.1);
const AMBER: Color = Color::srgb(1.0, 0.65, 0.1);
const LOST: Color = Color::srgb(1.0, 0.1, 0.1);
/// The flat colour of steel exposed by the section cut.
const CUT: Color = Color::srgb(0.80, 0.82, 0.86);

pub fn demo_plugin(app: &mut App) {
    let scenarios = load_scenarios();
    // An optional first argument names the starting scenario; a second one, `section`, starts in
    // section view.
    let start = std::env::args()
        .nth(1)
        .and_then(|arg| SCENARIOS.iter().position(|(label, _)| *label == arg))
        .unwrap_or(0);
    let section = SectionView {
        on: std::env::args().nth(2).is_some_and(|arg| arg == "section"),
        restore: None,
    };
    app.add_plugins((
        crate::quit::plugin,
        crate::spec::plugin,
        MaterialPlugin::<SectionMaterial>::default(),
    ))
    .insert_resource(ClearColor(Color::srgb(0.09, 0.1, 0.12)))
    .insert_resource(ActiveClip::new(start, &scenarios))
    .insert_resource(scenarios)
    .insert_resource(section)
    .init_resource::<Playhead>()
    .init_resource::<CamLaw>()
    .init_resource::<ViewReady>()
    .init_resource::<CutMaterials>()
    .add_systems(Startup, (spawn_scene, spawn_hud))
    .add_systems(
        Update,
        (
            handle_keys,
            advance_playhead,
            (rebuild_internals, rebuild_labels).run_if(resource_changed::<ActiveClip>),
            tint_internals,
            place_shell,
            section_camera
                .run_if(resource_changed::<SectionView>.or_else(resource_changed::<ActiveClip>)),
            drive_camera,
            draw_clip,
            update_hud,
            update_cards,
            update_labels,
            update_plate_labels,
        )
            .chain(),
    )
    // Scene meshes spawn in `SpawnScene`, after `Update`; classifying them here, before
    // visibility propagates, keeps a proxy or an unghosted hull off their first rendered frame.
    .add_systems(
        PostUpdate,
        (prepare_view, apply_cut, apply_ghost)
            .chain()
            .before(VisibilitySystems::VisibilityPropagate),
    );
}

/// Set once [`prepare_view`] has classified a mesh under the [`TankView`] root; the playhead holds
/// at zero until then so loading does not eat the impact beat.
#[derive(Resource, Default)]
struct ViewReady(bool);

/// Every scenario, loaded once at startup. A malformed clip is a broken fixture, so it panics.
#[derive(Resource)]
struct Scenarios(Vec<(&'static str, PenetrationReport)>);

fn load_scenarios() -> Scenarios {
    let dir = crate::assets::asset_root().join(SCENARIO_DIR);
    Scenarios(
        SCENARIOS
            .iter()
            .map(|(label, file)| {
                let report = PenetrationReport::load(&dir.join(file))
                    .unwrap_or_else(|e| panic!("damage cam scenario failed to load: {e}"));
                (*label, report)
            })
            .collect(),
    )
}

/// The clip on the stage, with everything derived from it once.
#[derive(Resource)]
struct ActiveClip {
    index: usize,
    report: PenetrationReport,
    timeline: Timeline,
    framing: Framing,
    section: SectionPlane,
    /// One per [`PenetrationReport::spall`] entry, in order.
    bursts: Vec<BurstTiming>,
}

impl ActiveClip {
    fn new(index: usize, scenarios: &Scenarios) -> Self {
        let report = scenarios.0[index].1.clone();
        let bursts = burst_timings(&report);
        Self {
            index,
            timeline: Timeline::new(&report, &bursts),
            framing: Framing::new(&report),
            section: SectionPlane::new(&report),
            bursts,
            report,
        }
    }
}

/// Clip seconds, on its own clock: real delta times `rate`, stopped at [`HOLD_START`].
#[derive(Resource)]
struct Playhead {
    clip: f32,
    rate: f32,
    paused: bool,
    /// Hold-orbit angle (rad) about the framed centre; zero outside the hold.
    orbit: f32,
}

impl Default for Playhead {
    fn default() -> Self {
        Self {
            clip: 0.0,
            rate: 1.0,
            paused: false,
            orbit: 0.0,
        }
    }
}

/// Clip seconds one scrub key press moves the playhead.
const SCRUB_S: f32 = 0.1;
/// Hold-orbit rate, rad per real second.
const ORBIT_RAD_S: f32 = 0.25;

/// The camera law at the plate, through the reveal and traverse beats.
#[derive(Resource, Default, Clone, Copy, PartialEq, Eq)]
enum CamLaw {
    /// The framed oblique x-ray view.
    #[default]
    Cut,
    /// Follows the shell along the path through the hull.
    Ride,
}

impl CamLaw {
    fn label(self) -> &'static str {
        match self {
            CamLaw::Cut => "CUT",
            CamLaw::Ride => "RIDE",
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Beat {
    Impact,
    Reveal,
    Traverse,
    Consequence,
    Hold,
}

impl Beat {
    fn at(clip: f32) -> Self {
        if clip < IMPACT_END {
            Beat::Impact
        } else if clip < REVEAL_END {
            Beat::Reveal
        } else if clip < TRAVERSE_END {
            Beat::Traverse
        } else if clip < HOLD_START {
            Beat::Consequence
        } else {
            Beat::Hold
        }
    }

    fn label(self) -> &'static str {
        match self {
            Beat::Impact => "impact",
            Beat::Reveal => "reveal",
            Beat::Traverse => "traverse",
            Beat::Consequence => "consequence",
            Beat::Hold => "hold",
        }
    }
}

/// The map between record time and clip time.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Timeline {
    /// Record time of the first path point; `<= 0`.
    approach_start: f32,
    /// Record time of the last path point, plate, burst or fragment arrival; `> 0`.
    span: f32,
    consequence_from: f32,
    consequence_to: f32,
}

impl Timeline {
    fn new(report: &PenetrationReport, bursts: &[BurstTiming]) -> Self {
        let approach_start = report.path.first().map_or(0.0, |p| p.t).min(0.0);
        let span = report
            .path
            .iter()
            .map(|p| p.t)
            .chain(report.plates.iter().map(|p| p.t))
            .chain(report.spall.iter().map(|b| b.t))
            .chain(bursts.iter().flat_map(|b| b.fragments.iter().copied()))
            .fold(1.0e-4, f32::max);
        let consequence_from = report
            .consequences
            .iter()
            .map(|c| c.t)
            .fold(f32::INFINITY, f32::min);
        let consequence_to = report
            .consequences
            .iter()
            .map(|c| c.t)
            .fold(f32::NEG_INFINITY, f32::max);
        Self {
            approach_start,
            span,
            consequence_from,
            consequence_to,
        }
    }

    /// Clip time at which record time `t` is reached.
    fn clip_of(&self, t: f32) -> f32 {
        if t < 0.0 {
            CONTACT_AT * (t - self.approach_start) / -self.approach_start
        } else {
            REVEAL_END + (TRAVERSE_END - REVEAL_END) * (t / self.span).min(1.0)
        }
    }

    /// Record time drawn up to at clip time `clip`.
    fn cutoff(&self, clip: f32) -> f32 {
        if clip < CONTACT_AT && self.approach_start < 0.0 {
            self.approach_start * (1.0 - clip / CONTACT_AT)
        } else if clip < REVEAL_END {
            0.0
        } else if clip < TRAVERSE_END {
            self.span * (clip - REVEAL_END) / (TRAVERSE_END - REVEAL_END)
        } else {
            f32::INFINITY
        }
    }

    /// Clip time at which the consequence at record time `t` lands.
    fn consequence_clip(&self, t: f32) -> f32 {
        let range = self.consequence_to - self.consequence_from;
        if range <= 0.0 {
            CONSEQUENCE_FROM
        } else {
            CONSEQUENCE_FROM
                + (CONSEQUENCE_TO - CONSEQUENCE_FROM) * (t - self.consequence_from) / range
        }
    }
}

/// Fragment speed as a fraction of the shell's residual speed, `theta` rad off the burst axis:
/// `cos(1.92 theta)`, read no further than [`FRAGMENT_THETA_MAX`] and floored at
/// [`FRAGMENT_SPEED_FLOOR`]. On axis the fragment keeps pace with the shell.
fn fragment_speed_fraction(theta: f32) -> f32 {
    (1.92 * theta.min(FRAGMENT_THETA_MAX))
        .cos()
        .max(FRAGMENT_SPEED_FLOOR)
}

/// The shell's speed along the path segment that contains record time `t`; the contact speed
/// when no segment does.
fn path_speed_at(report: &PenetrationReport, t: f32) -> f32 {
    report
        .path
        .windows(2)
        .find(|pair| pair[0].t <= t && t <= pair[1].t && pair[1].t > pair[0].t)
        .map(|pair| {
            Vec3::from(pair[0].point).distance(Vec3::from(pair[1].point)) / (pair[1].t - pair[0].t)
        })
        .unwrap_or(report.projectile.speed_mps)
}

/// When a burst's pieces reach their ends, in record seconds, from the shell's residual speed at
/// the burst and [`fragment_speed_fraction`] of each piece's angle off the axis.
struct BurstTiming {
    /// Axial length (m) of the cone envelope: the farthest fragment's projection on the axis.
    length: f32,
    /// The envelope's tip, on axis.
    nose: f32,
    /// The envelope's end circle, along a generator at the cone's half-angle.
    rim: f32,
    /// One per [`SpallBurst::fragments`] entry, in order.
    fragments: Vec<f32>,
}

impl BurstTiming {
    fn new(report: &PenetrationReport, burst: &SpallBurst) -> Self {
        let origin = Vec3::from(burst.origin);
        let axis = Vec3::from(burst.axis).normalize_or(Vec3::Z);
        let v_exit = path_speed_at(report, burst.t);
        let arrival = |distance: f32, theta: f32| {
            burst.t + distance / (v_exit * fragment_speed_fraction(theta))
        };
        let length = burst
            .fragments
            .iter()
            .map(|f| (Vec3::from(f.end) - origin).dot(axis))
            .fold(CONE_MIN_LENGTH_M, f32::max);
        let half = burst.half_angle_deg.to_radians();
        let fragments = burst
            .fragments
            .iter()
            .map(|f| {
                let offset = Vec3::from(f.end) - origin;
                match offset.try_normalize() {
                    Some(dir) => arrival(offset.length(), axis.angle_between(dir)),
                    None => burst.t,
                }
            })
            .collect();
        Self {
            length,
            nose: arrival(length, 0.0),
            rim: arrival(length / half.cos(), half),
            fragments,
        }
    }
}

fn burst_timings(report: &PenetrationReport) -> Vec<BurstTiming> {
    report
        .spall
        .iter()
        .map(|burst| BurstTiming::new(report, burst))
        .collect()
}

/// The oblique x-ray view: bounds of the post-contact path and the affected internals, seen from
/// the impact side.
#[derive(Clone, Copy, Debug)]
struct Framing {
    center: Vec3,
    /// Unit vector from `center` toward the eye.
    view_dir: Vec3,
    distance: f32,
}

impl Framing {
    fn new(report: &PenetrationReport) -> Self {
        let (lo, hi) = bounds(&framed_points(report));
        let radius = ((hi - lo).length() * 0.5).max(1.2);
        let normal = Vec3::from(report.impact.normal);
        let side = Vec3::new(normal.x, 0.0, normal.z)
            .try_normalize()
            .unwrap_or(Vec3::NEG_Z);
        let view_dir =
            (Quat::from_rotation_y(40f32.to_radians()) * side + Vec3::Y * 0.75).normalize();
        // Fits a sphere of `radius` inside the default 45 deg vertical field of view.
        let distance = radius / (std::f32::consts::FRAC_PI_8).sin() * 1.1;
        Self {
            center: (lo + hi) * 0.5,
            view_dir,
            distance,
        }
    }

    /// The framed view, turned `orbit` rad about the vertical through the centre.
    fn eye(&self, orbit: f32) -> Transform {
        let dir = Quat::from_rotation_y(orbit) * self.view_dir;
        Transform::from_translation(self.center + dir * self.distance)
            .looking_at(self.center, Vec3::Y)
    }
}

/// What the oblique view bounds: the impact, the post-contact path with its departure leg
/// clamped to [`FRAME_REACH_M`], and the affected internals' boxes.
fn framed_points(report: &PenetrationReport) -> Vec<Vec3> {
    let affected: HashSet<&str> = report
        .spall
        .iter()
        .flat_map(|b| &b.fragments)
        .flat_map(|f| &f.hits)
        .map(|h| h.volume.as_str())
        .chain(report.consequences.iter().map(|c| c.volume.as_str()))
        .collect();
    let departure = departure(report);
    let mut points = vec![Vec3::from(report.impact.point)];
    points.extend(report.path.iter().filter(|p| p.t >= 0.0).map(|p| {
        let point = Vec3::from(p.point);
        match departure {
            Some((left_at, anchor)) if p.t > left_at => {
                anchor + (point - anchor).clamp_length_max(FRAME_REACH_M)
            }
            _ => point,
        }
    }));
    for internal in report
        .internals
        .iter()
        .filter(|i| affected.contains(i.name.as_str()))
    {
        let (center, half) = (
            Vec3::from(internal.center),
            Vec3::from(internal.half_extents),
        );
        points.extend([center - half, center + half]);
    }
    points
}

/// Where the shell left the armour for good (the ricochet point, or the last plate's exit) and
/// the record time of the path point there; `None` when the path ends in material.
fn departure(report: &PenetrationReport) -> Option<(f32, Vec3)> {
    let anchor = Vec3::from(match report.impact.verdict {
        Verdict::Ricochet => report.impact.point,
        _ => report.plates.last()?.exit?,
    });
    let left_at = report
        .path
        .iter()
        .min_by(|a, b| {
            let gap = |p: &super::PathPoint| Vec3::from(p.point).distance_squared(anchor);
            gap(a).total_cmp(&gap(b))
        })?
        .t;
    Some((left_at, anchor))
}

fn bounds(points: &[Vec3]) -> (Vec3, Vec3) {
    points.iter().fold((Vec3::MAX, Vec3::MIN), |(lo, hi), p| {
        (lo.min(*p), hi.max(*p))
    })
}

/// The plane of incidence through the impact (spanned by the shot line and the struck plate's
/// normal, so the band shows at true thickness) and the orthographic eye that films it side-on.
#[derive(Clone, Copy, Debug)]
struct SectionPlane {
    point: Vec3,
    /// Unit normal; the eye sits on this side. Above the plane, or on its +X side when it is
    /// vertical.
    normal: Vec3,
    /// Looks along `-normal`, up being world up projected into the plane, or the shot line's
    /// in-plane perpendicular when the plane is near horizontal.
    eye: Transform,
    /// The fitted viewport, world units.
    width: f32,
    height: f32,
}

impl SectionPlane {
    fn new(report: &PenetrationReport) -> Self {
        let point = Vec3::from(report.impact.point);
        let incoming = Vec3::from(report.impact.incoming).normalize_or(Vec3::NEG_Z);
        let span = if report.impact.incidence_deg < SECTION_MIN_INCIDENCE_DEG {
            Vec3::Y
        } else {
            Vec3::from(report.impact.normal)
        };
        let mut normal = incoming
            .cross(span)
            .try_normalize()
            .unwrap_or_else(|| incoming.any_orthonormal_vector());
        if normal.y < -1.0e-3 || (normal.y.abs() <= 1.0e-3 && normal.x < 0.0) {
            normal = -normal;
        }
        let up = Vec3::Y.reject_from_normalized(normal);
        let up = if up.length() >= SECTION_UP_MIN {
            up.normalize()
        } else {
            normal.cross(incoming).normalize_or(Vec3::Z)
        };
        let look = Transform::IDENTITY.looking_to(-normal, up);
        let (right, up) = (*look.right(), *look.up());
        let first = report.plates.first().map_or(point, |p| Vec3::from(p.entry));
        let last = report
            .plates
            .last()
            .map_or(point, |p| Vec3::from(p.exit.unwrap_or(p.entry)));
        let along = (last - first).try_normalize().unwrap_or(incoming);
        let across = normal.cross(along).normalize_or(up)
            * (report.projectile.caliber_mm * 1.0e-3 * SECTION_MARGIN_CALIBRES);
        let ends = [
            first - along * SECTION_LEAD_M,
            last + along * SECTION_LEAD_M,
        ];
        let (lo, hi) = ends
            .iter()
            .flat_map(|end| [*end + across, *end - across])
            .map(|corner| Vec2::new((corner - point).dot(right), (corner - point).dot(up)))
            .fold((Vec2::MAX, Vec2::MIN), |(lo, hi), c| (lo.min(c), hi.max(c)));
        let centre = (lo + hi) * 0.5;
        let eye = Transform::from_translation(
            point + right * centre.x + up * centre.y + normal * SECTION_EYE_M,
        )
        .looking_to(-normal, up);
        Self {
            point,
            normal,
            eye,
            width: hi.x - lo.x,
            height: hi.y - lo.y,
        }
    }

    /// Signed distance from the plane, positive on the eye's side.
    fn height_of(&self, p: Vec3) -> f32 {
        (p - self.point).dot(self.normal)
    }

    fn projection(&self) -> Projection {
        Projection::Orthographic(OrthographicProjection {
            scaling_mode: ScalingMode::AutoMin {
                min_width: self.width,
                min_height: self.height,
            },
            ..OrthographicProjection::default_3d()
        })
    }
}

/// Section view (key `X`): the armour cut along [`SectionPlane`], filmed by its eye.
#[derive(Resource, Default)]
struct SectionView {
    on: bool,
    /// The clip camera's projection before the cut, put back when the view turns off.
    restore: Option<Projection>,
}

type SectionMaterial = ExtendedMaterial<StandardMaterial, SectionCut>;

/// The cut's half of [`SectionMaterial`]: the plane and the flat colour of the steel it exposes.
/// Bindings start at 100, bevy's convention for a material extension.
#[derive(Asset, AsBindGroup, Reflect, Debug, Clone)]
struct SectionCut {
    #[uniform(100)]
    params: SectionCutParams,
}

/// The uniform block `section_cut.wgsl` reads.
#[derive(ShaderType, Reflect, Debug, Clone, Copy, Default)]
struct SectionCutParams {
    /// xyz: the unit normal toward the eye, w: `normal . point`; a fragment with
    /// `normal . position > w` is on the eye's side and discarded.
    plane: Vec4,
    cut_color: Vec4,
}

impl MaterialExtension for SectionCut {
    fn fragment_shader() -> ShaderRef {
        "shaders/section_cut.wgsl".into()
    }
}

/// A view mesh wearing the cut material in place of its [`GhostSwap`] one.
#[derive(Component)]
struct Cut;

/// Cut materials by the original material they extend, made on first use.
#[derive(Resource, Default)]
struct CutMaterials(HashMap<AssetId<StandardMaterial>, Handle<SectionMaterial>>);

#[derive(Component)]
struct ClipCamera;

/// The tank view's scene root.
#[derive(Component)]
struct TankView;

#[derive(Resource)]
struct TankFixture {
    spec: Handle<TankSpec>,
}

/// A view mesh that swaps between its own material and the ghost.
#[derive(Component)]
struct GhostSwap {
    original: Handle<StandardMaterial>,
}

/// A view mesh already classified by [`prepare_view`].
#[derive(Component)]
struct Prepared;

#[derive(Resource)]
struct ClipMaterials {
    ghost: Handle<StandardMaterial>,
    crew: Handle<StandardMaterial>,
    module: Handle<StandardMaterial>,
    ammo: Handle<StandardMaterial>,
    engine: Handle<StandardMaterial>,
    damaged: Handle<StandardMaterial>,
}

impl ClipMaterials {
    fn role(&self, role: Role) -> &Handle<StandardMaterial> {
        match role {
            Role::Crew => &self.crew,
            Role::Module => &self.module,
            Role::Ammo => &self.ammo,
            Role::Engine => &self.engine,
        }
    }
}

/// One record internal drawn as a box, by its index in [`PenetrationReport::internals`].
#[derive(Component)]
struct InternalBox(usize);

#[derive(Component)]
struct StatusText;

#[derive(Component)]
struct VerdictText;

#[derive(Component)]
struct PlateCard;

fn spawn_scene(
    mut commands: Commands,
    asset_server: Res<AssetServer>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    clip: Res<ActiveClip>,
    mut gizmo_store: ResMut<GizmoConfigStore>,
) {
    let scene: Handle<WorldAsset> =
        asset_server.load(GltfAssetLabel::Scene(0).from_asset(crate::tank::TIGER_GLB_PATH));
    commands.spawn((WorldAssetRoot(scene), Transform::IDENTITY, TankView));
    // The game's main-gun round as `ballistics::view` dresses it: same asset, unscaled.
    let shell: Handle<WorldAsset> =
        asset_server.load(GltfAssetLabel::Scene(0).from_asset("shell/shell.glb"));
    commands.spawn((
        WorldAssetRoot(shell),
        Transform::IDENTITY,
        Visibility::Hidden,
        ShellModel,
    ));
    commands.insert_resource(TankFixture {
        spec: asset_server.load(TANK_SPEC),
    });
    commands.spawn((Camera3d::default(), exterior_eye(&clip.report), ClipCamera));
    commands.spawn((crate::world::sun_light(), crate::world::sun_transform()));
    commands.spawn((
        Mesh3d(meshes.add(Plane3d::default().mesh().size(40.0, 40.0))),
        MeshMaterial3d(materials.add(StandardMaterial {
            base_color: Color::srgb(0.22, 0.23, 0.2),
            perceptual_roughness: 0.95,
            ..default()
        })),
    ));
    let translucent = |color: Color| StandardMaterial {
        base_color: color,
        alpha_mode: AlphaMode::Blend,
        unlit: true,
        ..default()
    };
    commands.insert_resource(ClipMaterials {
        ghost: materials.add(StandardMaterial {
            base_color: Color::srgba(0.62, 0.64, 0.68, 0.12),
            alpha_mode: AlphaMode::Blend,
            ..default()
        }),
        crew: materials.add(translucent(Color::srgba(0.45, 0.7, 1.0, 0.35))),
        module: materials.add(translucent(Color::srgba(0.6, 0.62, 0.66, 0.3))),
        ammo: materials.add(translucent(Color::srgba(0.95, 0.85, 0.35, 0.35))),
        engine: materials.add(translucent(Color::srgba(0.55, 0.7, 0.5, 0.3))),
        damaged: materials.add(translucent(AMBER.with_alpha(0.6))),
    });
    let (config, _) = gizmo_store.config_mut::<DefaultGizmoConfigGroup>();
    config.line.width = 2.5;
}

fn spawn_hud(mut commands: Commands) {
    commands.spawn((
        StatusText,
        Text::new(""),
        TextFont {
            font_size: FontSize::Px(15.0),
            ..default()
        },
        TextColor(Color::srgb(0.85, 0.87, 0.95)),
        Node {
            position_type: PositionType::Absolute,
            top: Val::Px(10.0),
            left: Val::Px(12.0),
            ..default()
        },
    ));
    commands
        .spawn(Node {
            position_type: PositionType::Absolute,
            top: Val::Px(10.0),
            width: Val::Percent(100.0),
            justify_content: JustifyContent::Center,
            ..default()
        })
        .with_children(|row| {
            row.spawn((
                VerdictText,
                Text::new(""),
                TextFont {
                    font_size: FontSize::Px(22.0),
                    ..default()
                },
                TextColor(SPAN),
                TextLayout::justify(Justify::Center),
                Visibility::Hidden,
            ));
        });
    commands.spawn((
        PlateCard,
        Text::new(""),
        TextFont {
            font_size: FontSize::Px(15.0),
            ..default()
        },
        TextColor(Color::srgb(0.85, 0.87, 0.95)),
        Node {
            position_type: PositionType::Absolute,
            bottom: Val::Px(12.0),
            left: Val::Px(12.0),
            padding: UiRect::all(Val::Px(8.0)),
            ..default()
        },
        BackgroundColor(Color::srgba(0.0, 0.0, 0.0, 0.45)),
        Visibility::Hidden,
    ));
    commands.spawn((
        Text::new(
            "1-5 scenario   R replay   Space pause   Left/Right scrub   C camera   X section   Esc quit",
        ),
        TextFont {
            font_size: FontSize::Px(14.0),
            ..default()
        },
        TextColor(Color::srgb(0.7, 0.72, 0.78)),
        Node {
            position_type: PositionType::Absolute,
            bottom: Val::Px(12.0),
            right: Val::Px(12.0),
            ..default()
        },
    ));
}

/// Respawn the internal boxes for the active clip.
fn rebuild_internals(
    mut commands: Commands,
    clip: Res<ActiveClip>,
    materials: Option<Res<ClipMaterials>>,
    mut meshes: ResMut<Assets<Mesh>>,
    boxes: Query<Entity, With<InternalBox>>,
) {
    let Some(materials) = materials else {
        return;
    };
    for entity in &boxes {
        commands.entity(entity).despawn();
    }
    for (index, internal) in clip.report.internals.iter().enumerate() {
        let size = Vec3::from(internal.half_extents) * 2.0;
        commands.spawn((
            InternalBox(index),
            Mesh3d(meshes.add(Cuboid::from_size(size))),
            MeshMaterial3d(materials.role(internal.role).clone()),
            Transform::from_translation(Vec3::from(internal.center)),
        ));
    }
}

/// Classify each view mesh once its spec has loaded: collision proxies and the volumes the record
/// draws itself are hidden, everything else may ghost.
fn prepare_view(
    mut commands: Commands,
    fixture: Res<TankFixture>,
    specs: Res<Assets<TankSpec>>,
    scenarios: Res<Scenarios>,
    candidates: Query<
        (Entity, &MeshMaterial3d<StandardMaterial>),
        (With<Mesh3d>, Without<Prepared>, Without<InternalBox>),
    >,
    parents: Query<&ChildOf>,
    names: Query<&Name>,
    roots: Query<(), With<TankView>>,
    shells: Query<(), With<ShellModel>>,
    mut ready: ResMut<ViewReady>,
) {
    let Some(spec) = specs.get(&fixture.spec) else {
        return;
    };
    let hidden: HashSet<&str> = spec
        .colliders
        .iter()
        .map(String::as_str)
        .chain(
            scenarios
                .0
                .iter()
                .flat_map(|(_, report)| &report.internals)
                .map(|internal| internal.name.as_str()),
        )
        .collect();
    for (entity, material) in &candidates {
        let mut hide = false;
        let mut in_view = false;
        for node in std::iter::once(entity).chain(parents.iter_ancestors(entity)) {
            if shells.contains(node) {
                break;
            }
            if roots.contains(node) {
                in_view = true;
                break;
            }
            hide |= names
                .get(node)
                .is_ok_and(|name| hidden.contains(name.as_str()));
        }
        if !in_view {
            continue;
        }
        if !ready.0 {
            ready.0 = true;
        }
        if hide {
            commands
                .entity(entity)
                .insert((Prepared, Visibility::Hidden));
        } else {
            commands.entity(entity).insert((
                Prepared,
                GhostSwap {
                    original: material.0.clone(),
                },
            ));
        }
    }
}

/// The hull is solid through the impact beat and a ghost from the reveal on.
fn apply_ghost(
    playhead: Res<Playhead>,
    materials: Option<Res<ClipMaterials>>,
    mut meshes: Query<(&GhostSwap, &mut MeshMaterial3d<StandardMaterial>)>,
) {
    let Some(materials) = materials else {
        return;
    };
    let ghost = playhead.clip >= IMPACT_END;
    for (swap, mut material) in &mut meshes {
        let want = if ghost {
            &materials.ghost
        } else {
            &swap.original
        };
        if material.0 != *want {
            material.0 = want.clone();
        }
    }
}

/// While section view is on, every ghost-swap mesh wears the cut material: its own material
/// extended by the plane, culling nothing. Off, it gets its own material back for [`apply_ghost`]
/// to settle.
fn apply_cut(
    mut commands: Commands,
    section: Res<SectionView>,
    clip: Res<ActiveClip>,
    standard: Res<Assets<StandardMaterial>>,
    mut cut_materials: ResMut<Assets<SectionMaterial>>,
    mut cache: ResMut<CutMaterials>,
    uncut: Query<(Entity, &GhostSwap), Without<Cut>>,
    cut: Query<(Entity, &GhostSwap), With<Cut>>,
) {
    if !section.on {
        for (entity, swap) in &cut {
            commands
                .entity(entity)
                .remove::<(Cut, MeshMaterial3d<SectionMaterial>)>()
                .insert(MeshMaterial3d(swap.original.clone()));
        }
        return;
    }
    let plane = &clip.section;
    let cut = CUT.to_linear();
    let params = SectionCutParams {
        plane: plane.normal.extend(plane.normal.dot(plane.point)),
        cut_color: Vec4::new(cut.red, cut.green, cut.blue, cut.alpha),
    };
    if clip.is_changed() || section.is_changed() {
        for handle in cache.0.values() {
            if let Some(mut material) = cut_materials.get_mut(handle) {
                material.extension.params = params;
            }
        }
    }
    for (entity, swap) in &uncut {
        let handle = cache
            .0
            .entry(swap.original.id())
            .or_insert_with(|| {
                let mut base = standard.get(&swap.original).cloned().unwrap_or_default();
                base.cull_mode = None;
                base.double_sided = true;
                cut_materials.add(SectionMaterial {
                    base,
                    extension: SectionCut { params },
                })
            })
            .clone();
        commands
            .entity(entity)
            .remove::<MeshMaterial3d<StandardMaterial>>()
            .insert((Cut, MeshMaterial3d(handle)));
    }
}

/// A `Damaged` internal's box turns amber once its consequence lands.
fn tint_internals(
    playhead: Res<Playhead>,
    clip: Res<ActiveClip>,
    materials: Option<Res<ClipMaterials>>,
    mut boxes: Query<(&InternalBox, &mut MeshMaterial3d<StandardMaterial>)>,
) {
    let Some(materials) = materials else {
        return;
    };
    for (internal_box, mut material) in &mut boxes {
        let Some(internal) = clip.report.internals.get(internal_box.0) else {
            continue;
        };
        let state = landed_state(&clip.report, &clip.timeline, playhead.clip, &internal.name);
        let want = if state == Some(ConsequenceState::Damaged) {
            &materials.damaged
        } else {
            materials.role(internal.role)
        };
        if material.0 != *want {
            material.0 = want.clone();
        }
    }
}

const SCENARIO_KEYS: [KeyCode; 5] = [
    KeyCode::Digit1,
    KeyCode::Digit2,
    KeyCode::Digit3,
    KeyCode::Digit4,
    KeyCode::Digit5,
];

fn handle_keys(
    keys: Res<ButtonInput<KeyCode>>,
    scenarios: Res<Scenarios>,
    mut clip: ResMut<ActiveClip>,
    mut playhead: ResMut<Playhead>,
    mut law: ResMut<CamLaw>,
    mut section: ResMut<SectionView>,
    mut exit: MessageWriter<AppExit>,
) {
    if keys.just_pressed(KeyCode::KeyC) {
        *law = match *law {
            CamLaw::Cut => CamLaw::Ride,
            CamLaw::Ride => CamLaw::Cut,
        };
    }
    if keys.just_pressed(KeyCode::KeyX) {
        section.on = !section.on;
    }
    for (index, key) in SCENARIO_KEYS.iter().enumerate() {
        if keys.just_pressed(*key) && index < scenarios.0.len() {
            *clip = ActiveClip::new(index, &scenarios);
            playhead.clip = 0.0;
        }
    }
    if keys.just_pressed(KeyCode::KeyR) {
        playhead.clip = 0.0;
    }
    if keys.just_pressed(KeyCode::Space) {
        playhead.paused = !playhead.paused;
    }
    if keys.just_pressed(KeyCode::ArrowLeft) {
        playhead.clip = (playhead.clip - SCRUB_S).max(0.0);
    }
    if keys.just_pressed(KeyCode::ArrowRight) {
        playhead.clip = (playhead.clip + SCRUB_S).min(HOLD_START);
    }
    if keys.just_pressed(KeyCode::Escape) {
        exit.write(AppExit::Success);
    }
}

fn advance_playhead(time: Res<Time<Real>>, ready: Res<ViewReady>, mut playhead: ResMut<Playhead>) {
    if !ready.0 {
        return;
    }
    if playhead.clip < HOLD_START {
        playhead.orbit = 0.0;
    }
    if playhead.paused {
        return;
    }
    let step = time.delta_secs() * playhead.rate;
    if playhead.clip >= HOLD_START {
        playhead.orbit += step * ORBIT_RAD_S;
    } else {
        playhead.clip = (playhead.clip + step).min(HOLD_START);
    }
}

/// The impact beat looks at the contact point from outside, off the line of fire.
fn exterior_eye(report: &PenetrationReport) -> Transform {
    let point = Vec3::from(report.impact.point);
    let incoming = Vec3::from(report.impact.incoming);
    let side = incoming.cross(Vec3::Y).try_normalize().unwrap_or(Vec3::X);
    let eye = point - incoming * 6.0 + side * 3.5 + Vec3::Y * 1.8;
    Transform::from_translation(eye).looking_at(point, Vec3::Y)
}

/// Behind and above the shell head at record time `cutoff`, looking along its travel.
fn ride_eye(report: &PenetrationReport, cutoff: f32) -> Transform {
    let (head, dir) = shell_head(report, cutoff);
    Transform::from_translation(head - dir * 1.2 + Vec3::Y * 0.35).looking_at(head + dir, Vec3::Y)
}

/// The shell head at record time `cutoff` and its unit travel direction along the last segment of
/// non-zero length; `impact.incoming` when there is none.
fn shell_head(report: &PenetrationReport, cutoff: f32) -> (Vec3, Vec3) {
    let path = path_until(report, cutoff);
    let head = path.last().map_or(Vec3::from(report.impact.point), |p| p.1);
    let dir = path
        .iter()
        .rev()
        .map(|p| head - p.1)
        .find(|d| d.length_squared() > 1.0e-6)
        .map_or(Vec3::from(report.impact.incoming), Vec3::normalize);
    (head, dir)
}

/// The shell model's root, kept out of the ghost swap.
#[derive(Component)]
struct ShellModel;

/// Place the shell model at the head, model `-Z` along travel (the game's projectile convention,
/// `looking_to(travel, Y)`), hidden until the playhead starts.
fn place_shell(
    playhead: Res<Playhead>,
    clip: Res<ActiveClip>,
    ready: Res<ViewReady>,
    mut shell: Single<(&mut Transform, &mut Visibility), With<ShellModel>>,
) {
    let (transform, visibility) = &mut *shell;
    let (head, dir) = shell_head(&clip.report, clip.timeline.cutoff(playhead.clip));
    **transform = Transform::from_translation(head).looking_to(dir, Vec3::Y);
    **visibility = shown(ready.0);
}

/// Exterior through the impact beat, then the camera law until the consequence beat cuts to the
/// framed oblique, which orbits in the hold. Section view overrides them all with its own eye.
fn drive_camera(
    playhead: Res<Playhead>,
    clip: Res<ActiveClip>,
    law: Res<CamLaw>,
    section: Res<SectionView>,
    mut camera: Single<&mut Transform, With<ClipCamera>>,
) {
    let now = playhead.clip;
    **camera = match (section.on, Beat::at(now), *law) {
        (true, _, _) => clip.section.eye,
        (_, Beat::Impact, _) => exterior_eye(&clip.report),
        (_, Beat::Reveal | Beat::Traverse, CamLaw::Ride) => {
            ride_eye(&clip.report, clip.timeline.cutoff(now))
        }
        _ => clip.framing.eye(playhead.orbit),
    };
}

/// Entering section view keeps the clip camera's projection and fits the orthographic one to the
/// clip's section; leaving puts the kept projection back.
fn section_camera(
    mut section: ResMut<SectionView>,
    clip: Res<ActiveClip>,
    mut projection: Single<&mut Projection, With<ClipCamera>>,
) {
    if section.on {
        if section.restore.is_none() {
            section.restore = Some(projection.clone());
        }
        **projection = clip.section.projection();
    } else if let Some(previous) = section.restore.take() {
        **projection = previous;
    }
}

/// Record path up to `cutoff`, with the last segment interpolated to it.
fn path_until(report: &PenetrationReport, cutoff: f32) -> Vec<(f32, Vec3)> {
    let mut out = Vec::new();
    for pair in report.path.windows(2) {
        let (a, b) = (pair[0], pair[1]);
        if out.is_empty() && a.t <= cutoff {
            out.push((a.t, Vec3::from(a.point)));
        }
        if b.t <= cutoff {
            out.push((b.t, Vec3::from(b.point)));
        } else {
            if a.t < cutoff {
                let s = (cutoff - a.t) / (b.t - a.t);
                out.push((cutoff, Vec3::from(a.point).lerp(Vec3::from(b.point), s)));
            }
            break;
        }
    }
    out
}

/// Split each segment at tick boundaries and alternate two colours per tick.
fn draw_striped(gizmos: &mut Gizmos, points: &[(f32, Vec3)]) {
    for pair in points.windows(2) {
        let ((t0, p0), (t1, p1)) = (pair[0], pair[1]);
        let mut t = t0;
        while t < t1 {
            let tick = (t / TICK_S).floor();
            let next = ((tick + 1.0) * TICK_S).min(t1);
            let at = |time: f32| p0.lerp(p1, (time - t0) / (t1 - t0));
            let color = if (tick as i64).rem_euclid(2) == 0 {
                TRACER
            } else {
                TRACER_ALT
            };
            gizmos.line(at(t), at(next), color);
            t = next;
        }
    }
}

fn draw_clip(
    mut gizmos: Gizmos,
    playhead: Res<Playhead>,
    clip: Res<ActiveClip>,
    section: Res<SectionView>,
) {
    let (report, timeline, now) = (&clip.report, &clip.timeline, playhead.clip);
    let plane = section.on.then_some(&clip.section);
    let cutoff = timeline.cutoff(now);
    let path = path_until(report, cutoff);
    draw_striped(&mut gizmos, &path);
    let head = path.last().map(|p| p.1);
    if now >= CONTACT_AT {
        draw_impact(&mut gizmos, report);
    }
    for plate in report
        .plates
        .iter()
        .filter(|p| now >= timeline.clip_of(p.t))
    {
        let entry = Vec3::from(plate.entry);
        let entry_color = if plate.overmatched { OVERMATCH } else { ENTRY };
        gizmos.sphere(Isometry3d::from_translation(entry), 0.035, entry_color);
        if plate.overmatched {
            let facing = Quat::from_rotation_arc(Vec3::Z, Vec3::from(report.impact.normal));
            let radius = report.projectile.caliber_mm * 1.5e-3;
            gizmos.circle(Isometry3d::new(entry, facing), radius, OVERMATCH);
        }
        if let (Some(exit), Some(head)) = (plate.exit, head) {
            let exit = Vec3::from(exit);
            let through = plate_progress(entry, exit, head);
            gizmos.line(entry, entry.lerp(exit, through), SPAN);
            if through >= 1.0 {
                gizmos.sphere(Isometry3d::from_translation(exit), 0.035, EXIT);
            }
        }
        if let (Some(plane), Some(head)) = (plane, head) {
            draw_section_plate(&mut gizmos, report, plate, plane, head);
        }
    }
    let fade = |p: Vec3| {
        plane.map_or(1.0, |plane| {
            1.0 - (plane.height_of(p).abs() / SECTION_FADE_M).min(1.0)
        })
    };
    for (burst, timing) in report
        .spall
        .iter()
        .zip(&clip.bursts)
        .filter(|(b, _)| cutoff >= b.t)
    {
        let start = timeline.clip_of(burst.t);
        let progress = |arrival: f32| {
            let end = timeline.clip_of(arrival);
            if end <= start {
                1.0
            } else {
                ((now - start) / (end - start)).clamp(0.0, 1.0)
            }
        };
        draw_burst(&mut gizmos, burst, timing, progress, fade);
    }
    draw_internals(&mut gizmos, report, timeline, now);
}

/// The section's reading of one plate: the chord as a bar with end ticks (an embed's chord runs
/// from the entry to the shell head; a ricochet has none) and a bar of calibre length across the
/// shot line just before the entry, so overmatch reads against the band. The record drives every
/// length; the mesh only supplies the band.
fn draw_section_plate(
    gizmos: &mut Gizmos,
    report: &PenetrationReport,
    plate: &super::Plate,
    plane: &SectionPlane,
    head: Vec3,
) {
    let entry = Vec3::from(plate.entry);
    let incoming = Vec3::from(report.impact.incoming);
    let calibre = report.projectile.caliber_mm * 1.0e-3;
    let chord = match plate.exit {
        Some(exit) => {
            let exit = Vec3::from(exit);
            Some((exit, (exit - entry).normalize_or(incoming)))
        }
        None if report.impact.verdict == Verdict::Ricochet => None,
        None => {
            let along = (head - entry).normalize_or(incoming);
            let depth = (head - entry)
                .dot(along)
                .clamp(0.0, plate.chord_mm * 1.0e-3);
            Some((entry + along * depth, along))
        }
    };
    let along = chord.map_or(incoming, |(_, along)| along);
    let across = plane.normal.cross(along).normalize_or(Vec3::Y);
    if let Some((end, _)) = chord {
        let reached = entry.lerp(end, plate_progress(entry, end, head));
        let tick = across * calibre * 0.25;
        gizmos.line(entry, reached, SPAN);
        gizmos.line(entry - tick, entry + tick, SPAN);
        if reached.distance_squared(end) < 1.0e-8 {
            gizmos.line(end - tick, end + tick, SPAN);
        }
    }
    let bar = entry - along * calibre * 0.4;
    let half = across * calibre * 0.5;
    gizmos.line(bar - half, bar + half, TRACER);
}

/// How far through the crossing `entry`→`exit` the shell head is, as its clamped projection.
fn plate_progress(entry: Vec3, exit: Vec3, head: Vec3) -> f32 {
    let span = exit - entry;
    let length_squared = span.length_squared();
    if length_squared <= 0.0 {
        return 1.0;
    }
    ((head - entry).dot(span) / length_squared).clamp(0.0, 1.0)
}

fn draw_impact(gizmos: &mut Gizmos, report: &PenetrationReport) {
    let point = Vec3::from(report.impact.point);
    let incoming = Vec3::from(report.impact.incoming);
    let normal = Vec3::from(report.impact.normal);
    let color = match report.impact.verdict {
        Verdict::Ricochet => RICOCHET,
        _ if report.plates.first().is_some_and(|p| p.overmatched) => OVERMATCH,
        _ => ENTRY,
    };
    gizmos.sphere(Isometry3d::from_translation(point), 0.06, color);
    gizmos.arrow(point - incoming * 1.5, point - incoming * 0.08, TRACER);
    gizmos.line(point, point + normal * 0.35, Color::srgb(0.8, 0.82, 0.86));
}

/// The cone as an envelope (the axis to the nose, generators and the end circle at the rim's own
/// reach) and every fragment at its own progress, faint unless it hit something. `progress` maps
/// a record arrival time to the fraction of the way there at the playhead; `fade` scales a
/// fragment's alpha by where it ends.
fn draw_burst(
    gizmos: &mut Gizmos,
    burst: &SpallBurst,
    timing: &BurstTiming,
    progress: impl Fn(f32) -> f32,
    fade: impl Fn(Vec3) -> f32,
) {
    let origin = Vec3::from(burst.origin);
    let axis = Vec3::from(burst.axis).normalize_or(Vec3::Z);
    let facing = Quat::from_rotation_arc(Vec3::Z, axis);
    let nose = origin + axis * timing.length * progress(timing.nose);
    let reach = timing.length * progress(timing.rim);
    let rim = reach * burst.half_angle_deg.to_radians().tan();
    let ring = origin + axis * reach;
    gizmos.line(origin, nose, CONE);
    gizmos.circle(Isometry3d::new(ring, facing), rim, CONE);
    for k in 0..6 {
        let angle = k as f32 * std::f32::consts::TAU / 6.0;
        let offset = facing * Vec3::new(angle.cos(), angle.sin(), 0.0) * rim;
        gizmos.line(origin, ring + offset, CONE);
    }
    for (fragment, arrival) in burst.fragments.iter().zip(&timing.fragments) {
        let grow = progress(*arrival);
        let full = Vec3::from(fragment.end);
        let alpha = fade(full);
        let end = origin.lerp(full, grow);
        if fragment.hits.is_empty() {
            let color = FRAGMENT_FAINT.with_alpha(FRAGMENT_FAINT.alpha() * alpha);
            gizmos.line(origin, end, color);
        } else {
            gizmos.line(origin, end, FRAGMENT_HIT.with_alpha(alpha));
            if grow >= 1.0 {
                gizmos.sphere(
                    Isometry3d::from_translation(end),
                    0.03,
                    EXIT.with_alpha(alpha),
                );
            }
        }
    }
}

/// The latest consequence for each internal that has landed by `now`.
fn landed_state(
    report: &PenetrationReport,
    timeline: &Timeline,
    now: f32,
    name: &str,
) -> Option<ConsequenceState> {
    report
        .consequences
        .iter()
        .filter(|c| c.volume == name && now >= timeline.consequence_clip(c.t))
        .map(|c| c.state)
        .next_back()
}

fn draw_internals(gizmos: &mut Gizmos, report: &PenetrationReport, timeline: &Timeline, now: f32) {
    for internal in &report.internals {
        let center = Vec3::from(internal.center);
        let size = Vec3::from(internal.half_extents) * 2.0;
        let cube = |scale: f32| Transform::from_translation(center).with_scale(size * scale);
        match landed_state(report, timeline, now, &internal.name) {
            None => {}
            Some(ConsequenceState::Damaged) => gizmos.cube(cube(1.0), AMBER),
            Some(ConsequenceState::Lost) => gizmos.cube(cube(1.06), LOST),
            Some(ConsequenceState::Cookoff) => {
                gizmos.cube(cube(1.06), LOST);
                gizmos.sphere(Isometry3d::from_translation(center), 0.45, SPAN);
            }
        }
    }
}

fn update_hud(
    playhead: Res<Playhead>,
    clip: Res<ActiveClip>,
    law: Res<CamLaw>,
    section: Res<SectionView>,
    ready: Res<ViewReady>,
    scenarios: Res<Scenarios>,
    mut status: Single<&mut Text, With<StatusText>>,
) {
    let beat = if ready.0 {
        Beat::at(playhead.clip).label()
    } else {
        "loading"
    };
    let synthetic = if clip.report.synthetic {
        "\nSYNTHETIC"
    } else {
        ""
    };
    status.0 = format!(
        "t {:.2} s  {}{}\nscenario {}/{} {}\ncam {}{}{synthetic}",
        playhead.clip,
        beat,
        if playhead.paused { "  [paused]" } else { "" },
        clip.index + 1,
        scenarios.0.len(),
        scenarios.0[clip.index].0,
        law.label(),
        if section.on { "  SECTION" } else { "" },
    );
}

fn verdict_word(verdict: Verdict) -> (&'static str, Color) {
    match verdict {
        Verdict::Through => ("THROUGH", SPAN),
        Verdict::Stopped => ("STOPPED", Color::srgb(0.45, 0.6, 0.85)),
        Verdict::Ricochet => ("RICOCHET", RICOCHET),
    }
}

fn verdict_card(report: &PenetrationReport) -> String {
    let (word, _) = verdict_word(report.impact.verdict);
    let p = &report.projectile;
    format!(
        "{word}\n{}  {:.0} mm  {:.1} kg  {:.0} m/s  incidence {:.0} deg",
        p.name, p.caliber_mm, p.mass_kg, p.speed_mps, report.impact.incidence_deg
    )
}

fn plate_card(report: &PenetrationReport) -> String {
    let mut card = String::from("PLATES");
    for plate in &report.plates {
        card.push_str(&format!(
            "\n{}\n  chord {:.0} mm\n  steel-equiv {:.0} mm\n  cost {:.0} mm\n  overmatched {}",
            plate.name,
            plate.chord_mm,
            plate.steel_equiv_mm,
            plate.cost_mm,
            if plate.overmatched { "yes" } else { "no" },
        ));
    }
    card
}

/// The verdict appears at contact, the plate card from the reveal on.
fn update_cards(
    playhead: Res<Playhead>,
    clip: Res<ActiveClip>,
    mut verdict: Single<
        (&mut Text, &mut TextColor, &mut Visibility),
        (With<VerdictText>, Without<PlateCard>),
    >,
    mut plates: Single<(&mut Text, &mut Visibility), (With<PlateCard>, Without<VerdictText>)>,
) {
    let (text, color, visibility) = &mut *verdict;
    text.0 = verdict_card(&clip.report);
    color.0 = verdict_word(clip.report.impact.verdict).1;
    **visibility = shown(playhead.clip >= CONTACT_AT);
    let (text, visibility) = &mut *plates;
    text.0 = plate_card(&clip.report);
    **visibility = shown(playhead.clip >= IMPACT_END);
}

fn shown(visible: bool) -> Visibility {
    if visible {
        Visibility::Inherited
    } else {
        Visibility::Hidden
    }
}

/// One label per consequence, by its index in [`PenetrationReport::consequences`].
#[derive(Component)]
struct ConsequenceLabel(usize);

/// One label per plate, by its index in [`PenetrationReport::plates`]; section view only.
#[derive(Component)]
struct PlateLabel(usize);

fn rebuild_labels(
    mut commands: Commands,
    clip: Res<ActiveClip>,
    labels: Query<Entity, Or<(With<ConsequenceLabel>, With<PlateLabel>)>>,
) {
    for entity in &labels {
        commands.entity(entity).despawn();
    }
    for (index, plate) in clip.report.plates.iter().enumerate() {
        commands.spawn((
            PlateLabel(index),
            Text::new(format!(
                "{}\nchord {:.0} mm  steel {:.0} mm  cost {:.0} mm",
                plate.name, plate.chord_mm, plate.steel_equiv_mm, plate.cost_mm
            )),
            TextFont {
                font_size: FontSize::Px(14.0),
                ..default()
            },
            TextColor(SPAN),
            Node {
                position_type: PositionType::Absolute,
                ..default()
            },
            Visibility::Hidden,
        ));
    }
    for (index, consequence) in clip.report.consequences.iter().enumerate() {
        let (word, color) = match consequence.state {
            ConsequenceState::Damaged => ("damaged", AMBER),
            ConsequenceState::Lost => ("lost", LOST),
            ConsequenceState::Cookoff => ("cookoff", LOST),
        };
        let function = consequence.function.as_deref().unwrap_or("");
        commands.spawn((
            ConsequenceLabel(index),
            Text::new(format!("{} {word}\n{function}", consequence.volume)),
            TextFont {
                font_size: FontSize::Px(14.0),
                ..default()
            },
            TextColor(color),
            Node {
                position_type: PositionType::Absolute,
                ..default()
            },
            Visibility::Hidden,
        ));
    }
}

/// Each label sits over its internal's top face once its consequence lands.
fn update_labels(
    playhead: Res<Playhead>,
    clip: Res<ActiveClip>,
    camera: Single<(&Camera, &Transform), With<ClipCamera>>,
    mut labels: Query<(&ConsequenceLabel, &mut Node, &mut Visibility)>,
) {
    // The clip camera is unparented, so its `Transform` is its world pose as of this frame.
    let (camera, transform) = *camera;
    let camera_transform = &GlobalTransform::from(*transform);
    for (label, mut node, mut visibility) in &mut labels {
        let Some(consequence) = clip.report.consequences.get(label.0) else {
            continue;
        };
        let landed = playhead.clip >= clip.timeline.consequence_clip(consequence.t);
        let anchor = clip
            .report
            .internal(&consequence.volume)
            .map(|internal| Vec3::from(internal.center) + Vec3::Y * internal.half_extents[1]);
        let screen = anchor.and_then(|a| camera.world_to_viewport(camera_transform, a).ok());
        match screen {
            Some(screen) if landed => {
                node.left = Val::Px(screen.x + 8.0);
                node.top = Val::Px(screen.y - 8.0);
                *visibility = Visibility::Inherited;
            }
            _ => *visibility = Visibility::Hidden,
        }
    }
}

/// In section view, each plate's label sits by its entry once the shell has reached it.
fn update_plate_labels(
    playhead: Res<Playhead>,
    clip: Res<ActiveClip>,
    section: Res<SectionView>,
    camera: Single<(&Camera, &Transform), With<ClipCamera>>,
    mut labels: Query<(&PlateLabel, &mut Node, &mut Visibility)>,
) {
    let (camera, transform) = *camera;
    let camera_transform = &GlobalTransform::from(*transform);
    for (label, mut node, mut visibility) in &mut labels {
        let Some(plate) = clip.report.plates.get(label.0) else {
            continue;
        };
        let reached = section.on && playhead.clip >= clip.timeline.clip_of(plate.t);
        let screen = camera
            .world_to_viewport(camera_transform, Vec3::from(plate.entry))
            .ok();
        match screen {
            Some(screen) if reached => {
                node.left = Val::Px(screen.x + 12.0);
                node.top = Val::Px(screen.y + 12.0);
                *visibility = Visibility::Inherited;
            }
            _ => *visibility = Visibility::Hidden,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::{Fragment, Impact, PathPoint, Projectile};
    use super::*;

    fn scenario(file: &str) -> PenetrationReport {
        PenetrationReport::load(&crate::assets::asset_root().join(SCENARIO_DIR).join(file)).unwrap()
    }

    fn through() -> PenetrationReport {
        scenario("through.ron")
    }

    /// Inside `lo - margin ..= hi + margin` on every axis.
    fn within(point: Vec3, (lo, hi): (Vec3, Vec3), margin: f32) -> bool {
        point.cmpge(lo - margin).all() && point.cmple(hi + margin).all()
    }

    #[test]
    fn departure_leg_counts_toward_framing_only_within_reach() {
        let short = scenario("overpen.ron");
        let mut long = short.clone();
        let exit = Vec3::from(short.plates.last().unwrap().exit.unwrap());
        let end = long.path.last_mut().unwrap();
        let leg = (Vec3::from(end.point) - exit).normalize();
        end.point = (exit + leg * 20.0).into();
        end.t += 18.0 / 700.0;
        let envelope = bounds(&framed_points(&short));
        let (lo, hi) = bounds(&framed_points(&long));
        assert!(within(lo, envelope, FRAME_REACH_M) && within(hi, envelope, FRAME_REACH_M));
        let unclamped = Vec3::from(long.path.last().unwrap().point);
        assert!(!within(unclamped, envelope, FRAME_REACH_M));
    }

    /// A straight, constant-speed path with one burst half-way along it, for the timing law.
    fn straight_burst(fragment_end: [f32; 3]) -> PenetrationReport {
        PenetrationReport {
            synthetic: true,
            projectile: Projectile {
                name: "fixture".into(),
                caliber_mm: 88.0,
                mass_kg: 10.0,
                speed_mps: 900.0,
            },
            impact: Impact {
                point: [0.0; 3],
                normal: [0.0, 0.0, -1.0],
                incoming: [0.0, 0.0, 1.0],
                incidence_deg: 0.0,
                verdict: Verdict::Through,
            },
            plates: vec![],
            path: vec![
                PathPoint {
                    t: 0.0,
                    point: [0.0; 3],
                },
                PathPoint {
                    t: 0.001,
                    point: [0.0, 0.0, 0.6],
                },
            ],
            spall: vec![SpallBurst {
                t: 0.0005,
                origin: [0.0, 0.0, 0.3],
                axis: [0.0, 0.0, 1.0],
                half_angle_deg: 30.0,
                fragments: vec![Fragment {
                    end: fragment_end,
                    hits: vec![],
                }],
            }],
            consequences: vec![],
            internals: vec![],
        }
    }

    /// The plane of incidence holds the shot line and the struck normal; the eye looks straight
    /// along its normal from the normal's side, up in the plane, and fits the shot's span.
    #[test]
    fn section_plane_holds_the_shot_and_the_normal() {
        for (_, file) in SCENARIOS {
            let report = scenario(file);
            let plane = SectionPlane::new(&report);
            let incoming = Vec3::from(report.impact.incoming).normalize();
            let normal = Vec3::from(report.impact.normal);
            assert!(plane.normal.is_normalized(), "{file}");
            assert!(plane.normal.dot(incoming).abs() < 1.0e-3, "{file}");
            if report.impact.incidence_deg >= SECTION_MIN_INCIDENCE_DEG {
                assert!(plane.normal.dot(normal).abs() < 1.0e-3, "{file}");
            }
            assert!(plane.normal.y >= -1.0e-3, "{file}: eye below the plane");
            assert!(plane.height_of(plane.eye.translation) > 0.0, "{file}");
            assert!(
                (*plane.eye.forward() + plane.normal).length() < 1.0e-4,
                "{file}"
            );
            assert!(plane.eye.up().dot(plane.normal).abs() < 1.0e-4, "{file}");
            assert!(plane.height_of(Vec3::from(report.impact.point)).abs() < 1.0e-6);
            let inside = |p: Vec3| {
                let d = p - plane.eye.translation;
                d.dot(*plane.eye.right()).abs() <= plane.width * 0.5 + 1.0e-4
                    && d.dot(*plane.eye.up()).abs() <= plane.height * 0.5 + 1.0e-4
            };
            assert!(inside(Vec3::from(report.impact.point)), "{file}");
            for plate in &report.plates {
                assert!(inside(Vec3::from(plate.entry)), "{file}: {}", plate.name);
                if let Some(exit) = plate.exit {
                    assert!(inside(Vec3::from(exit)), "{file}: {}", plate.name);
                }
            }
            assert!(
                plane.width.max(plane.height) >= 2.0 * SECTION_LEAD_M,
                "{file}"
            );
        }
    }

    /// Near-parallel incidence spans the plane with world up instead of the normal.
    #[test]
    fn section_plane_falls_back_to_world_up_when_incidence_is_flat() {
        let mut report = straight_burst([0.0, 0.0, 0.6]);
        report.impact.incidence_deg = 1.0;
        report.impact.normal = [0.0, 0.0, -1.0];
        let plane = SectionPlane::new(&report);
        assert!(plane.normal.dot(Vec3::Y).abs() < 1.0e-6);
        assert!(plane.normal.dot(Vec3::Z).abs() < 1.0e-6);
        assert!(plane.normal.x > 0.0);
    }

    #[test]
    fn fragment_speed_falls_off_axis_to_a_floor() {
        let at = |deg: f32| fragment_speed_fraction(deg.to_radians());
        assert_eq!(at(0.0), 1.0);
        assert!((at(15.0) - 0.88).abs() <= 0.01);
        assert!((at(30.0) - 0.54).abs() <= 0.01);
        assert!((at(40.0) - 0.23).abs() <= 0.01);
        for deg in [44.0, 45.0, 60.0, 90.0, 180.0] {
            assert_eq!(at(deg), FRAGMENT_SPEED_FLOOR, "{deg} deg");
        }
    }

    #[test]
    fn on_axis_fragment_keeps_pace_with_the_shell() {
        let report = straight_burst([0.0, 0.0, 0.6]);
        assert!((path_speed_at(&report, 0.0005) - 600.0).abs() < 1.0e-3);
        assert_eq!(path_speed_at(&report, 0.002), 900.0);
        let timing = &burst_timings(&report)[0];
        assert!((timing.fragments[0] - 0.001).abs() < 1.0e-6);
        assert!((timing.nose - 0.001).abs() < 1.0e-6);
        assert!(timing.rim > timing.nose);
        let timeline = Timeline::new(&report, &burst_timings(&report));
        assert!((timeline.clip_of(timing.fragments[0]) - TRAVERSE_END).abs() < 1.0e-4);
    }

    #[test]
    fn off_axis_fragment_lags_and_extends_the_span() {
        let report = straight_burst([0.0, 0.3, 0.6]);
        let timing = &burst_timings(&report)[0];
        assert!(timing.fragments[0] > 0.001);
        let timeline = Timeline::new(&report, &burst_timings(&report));
        assert_eq!(timeline.span, timing.fragments[0]);
    }

    #[test]
    fn timeline_maps_contact_and_span_onto_the_beats() {
        let report = through();
        let timeline = Timeline::new(&report, &burst_timings(&report));
        assert_eq!(timeline.clip_of(timeline.approach_start), 0.0);
        assert_eq!(timeline.clip_of(0.0), REVEAL_END);
        assert_eq!(timeline.clip_of(timeline.span), TRAVERSE_END);
        assert_eq!(timeline.cutoff(CONTACT_AT), 0.0);
        assert_eq!(timeline.cutoff(TRAVERSE_END), f32::INFINITY);
    }

    #[test]
    fn plate_span_follows_the_head() {
        let (entry, exit) = (Vec3::ZERO, Vec3::Z * 0.1);
        assert_eq!(plate_progress(entry, exit, entry), 0.0);
        assert_eq!(plate_progress(entry, exit, Vec3::Z * 0.05), 0.5);
        assert_eq!(plate_progress(entry, exit, Vec3::Z * 2.0), 1.0);
        assert_eq!(plate_progress(entry, exit, Vec3::NEG_Z), 0.0);
    }

    #[test]
    fn path_until_interpolates_the_open_segment() {
        let report = through();
        let mid = (report.path[2].t + report.path[3].t) * 0.5;
        let path = path_until(&report, mid);
        assert_eq!(path.len(), 4);
        assert_eq!(path.last().unwrap().0, mid);
        assert!(path_until(&report, report.path[0].t - 1.0).is_empty());
    }
}
