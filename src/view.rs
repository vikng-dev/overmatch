//! Shared live-view facts and exact screen-error projection (ADR-0033 §9).
//!
//! [`PlayerView`] declares the camera every reader uses. Counting `Camera3d` cannot identify it:
//! mirrors, spotters, overlays and debug cameras make that query ambiguous. `Single` validation
//! skips the system on zero/multiple matches; `Query::single` returns an error that a reader can
//! silently discard. Either can freeze a ladder in whichever direction its last selection took.
//! Multiple declarations instead produce [`ViewError`].
//!
//! [`ViewFacts`] has no `Default`; its resource is absent until a valid measurement. Substituting
//! a plausible narrow-optic/1080 px view would let an unread camera drive every ladder unnoticed.
//! Consumers defer explicitly: `geometry_lod` and `track::link_view` delay binding; `terrain_lod`
//! starts finest and rewrites when facts arrive. No consumer invents missing measurements.
//!
//! [`track_view_facts`] alone reads projection, target and `render_scale::RenderScale` for these
//! facts, avoiding disagreements between terrain and certified-geometry readers. Budgets belong
//! to consumers: [`crate::terrain_lod::TERRAIN_LOD_BUDGET_PX`] for terrain and
//! `settings::LodPixelBudget` for tank chains. [`ViewProfile`] pairs one budget with shared facts.
//!
//! Bevy retains each distinct `VisibilityRange` table entry for the app's lifetime (ADR-0033 §11).
//! Update facts at resolution/render-scale/optic changes; [`FOV_HYSTERESIS`] accumulates slider
//! changes against the held field so a drag cannot mint an entry per frame.

use bevy::camera::RenderTarget;
use bevy::ecs::error::BevyError;
use bevy::prelude::*;
use bevy::window::PrimaryWindow;

/// The player's camera: game orbit or sandbox free-fly, never an overlay/UI/spotter camera.
/// Separate from [`crate::render_policy::CameraProfile`]: channel policy must not be imposed on
/// every composition merely to identify its view; sandbox overlays use raw layers.
///
/// Inserting the marker supplies `Camera3d`, whose required `Projection` and `Camera` →
/// `Transform`/`GlobalTransform` serve both this reader and `track::link_view`. The composition
/// retains these required components, so either reader counts every declaration.
/// Cardinality is a world invariant: zero defers reading; multiple declarations are a
/// contradiction reported by [`ViewError`].
#[derive(Component, Clone, Copy, PartialEq, Eq, Debug, Default)]
#[require(Camera3d)]
pub(crate) struct PlayerView;

/// Declaration cardinality or unsupported projection. [`PlayerView`]'s required camera components
/// rule out an unreadable bare marker at insertion. Startup absence is gated by
/// `any_with_component::<PlayerView>` ([`plugin`]); reaching these errors in the scheduled reader
/// is a composition bug. Bevy's default `Severity::Panic` stops the app (ADR-0011), rather than
/// letting ladders silently hold their last selection.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum ViewError {
    /// Nothing declares the view: no entity carries [`PlayerView`] at all.
    NoPlayerView,
    /// Conflicting declarations; retain their count to diagnose the composition.
    ManyPlayerViews(usize),
    /// Orthographic/custom views have no perspective half-angle for [`ViewFacts::sub_pixel_distance_m`].
    UnsupportedProjection,
}

impl ViewError {
    /// Translate failed singleton resolution into declaration count. [`PlayerView`]'s retained
    /// camera-component invariant makes both readers' rows equal the declarations.
    pub(crate) fn resolving(declarations: usize) -> Self {
        match declarations {
            0 => Self::NoPlayerView,
            many => Self::ManyPlayerViews(many),
        }
    }
}

impl core::fmt::Display for ViewError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::NoPlayerView => write!(
                f,
                "no player view: nothing in this composition declares one — the view is declared, \
                 never discovered, so put `PlayerView` on the camera the player looks through",
            ),
            Self::ManyPlayerViews(views) => write!(
                f,
                "{views} entities declare `PlayerView`: exactly one is the player's view — a \
                 mirror, spotter, overlay or debug camera is not one and must not declare itself \
                 one",
            ),
            Self::UnsupportedProjection => write!(
                f,
                "the player's view carries a non-perspective projection: every screen-space ladder \
                 projects a metre through a perspective half-angle, which no other projection has",
            ),
        }
    }
}

impl core::error::Error for ViewError {}

/// Resolution: relative FOV dead band prevents subpixel slider changes from walking both LOD
/// ladders each frame. DERIVED optic toggle π/4 → 0.12 rad is 6.5×, well beyond the band.
pub(crate) const FOV_HYSTERESIS: f32 = 0.10;

/// Declared camera's vertical FOV and actual main-pass pixel height. One camera changes optic
/// projection in place, so one shared pair suffices regardless of other cameras in the world.
/// No `Default`: absence before [`track_view_facts`]'s first measurement is resource absence,
/// never a guessed value consumers could mistake for a measurement.
#[derive(Resource, Clone, Copy, PartialEq, Debug)]
pub(crate) struct ViewFacts {
    /// Vertical field of view, radians.
    pub(crate) vfov_rad: f32,
    /// Rendered target height, pixels (window/image physical height × render scale).
    pub(crate) height_px: f32,
}

impl ViewFacts {
    /// Admit finite positive height and FOV in `(0, π)`; otherwise return `None`.
    /// Zero/negative height means an unsized window or zero scale, not a tiny view: using it would
    /// collapse thresholds onto the bounding radius and show coarse geometry first.
    /// Invalid FOV corrupts the divisor: NaN ranges draw nothing, negative angles invert chains,
    /// and ≥π is outside the perspective domain. `spec::TankSpec::validate` gates authored FOV;
    /// this also guards debug tools and partially initialised cameras.
    /// Callers hold measured facts ([`Self::settled`]) or remain absent, never substitute a view.
    pub(crate) fn new(vfov_rad: f32, height_px: f32) -> Option<Self> {
        // `NaN` fails every comparison and is refused by both.
        (vfov_rad > 0.0
            && vfov_rad < core::f32::consts::PI
            && height_px.is_finite()
            && height_px > 0.0)
            .then_some(Self {
                vfov_rad,
                height_px,
            })
    }

    /// Pure settling rule for [`track_view_facts`], independent of world/query/error handling.
    /// Return self if unchanged; the caller writes only differences to keep change ticks human-rate.
    /// Hold FOV inside the dead band: adopting each small request would leak range-table entries.
    /// Compare against the held field so subthreshold steps accumulate and eventually cross it.
    /// Invalid requests ([`Self::new`]) hold the last measurement without inventing a substitute.
    pub(crate) fn settled(self, vfov_rad: f32, height_px: f32) -> Self {
        let Some(wanted) = Self::new(vfov_rad, height_px) else {
            return self;
        };
        let field_moved = (wanted.vfov_rad - self.vfov_rad).abs()
            > FOV_HYSTERESIS * self.vfov_rad.max(f32::MIN_POSITIVE);
        Self {
            vfov_rad: if field_moved {
                wanted.vfov_rad
            } else {
                self.vfov_rad
            },
            ..wanted
        }
    }

    /// Window physical height × render scale; window height alone would select for undrawn pixels.
    pub(crate) fn rendered_height_px(
        window: Option<&Window>,
        scale: Option<&crate::render_scale::RenderScale>,
    ) -> f32 {
        window.map_or(0.0, |window| {
            window.physical_height() as f32 * scale.map_or(1.0, |scale| scale.0)
        })
    }

    /// The distance (metres) at which a world-space deviation of `dev_m` projects to exactly
    /// `budget_px` pixels through this view.
    ///
    /// ```text
    /// D = dev_m · height_px / (2 · tan(vfov / 2) · budget_px)
    /// ```
    ///
    /// Exact (ADR-0033 §9), matching `scripts/lod/config.py::switch_distance_m` so certificate and
    /// runtime agree. DERIVED: the small-angle `vfov` divisor is 5.5 % wrong at the commander field.
    pub(crate) fn sub_pixel_distance_m(self, dev_m: f32, budget_px: f32) -> f32 {
        if dev_m <= 0.0 {
            return 0.0;
        }
        dev_m * self.height_px / (2.0 * (self.vfov_rad / 2.0).tan() * budget_px)
    }
}

/// Shared [`ViewFacts`] plus a consumer-owned budget. No `Default`: `geometry_lod` publishes this
/// resource only after facts exist; gated ladders therefore always select through a measured view.
#[derive(Resource, Clone, Copy, PartialEq, Debug)]
pub(crate) struct ViewProfile {
    pub(crate) facts: ViewFacts,
    /// Screen-space error budget, pixels: the on-screen size a deviation is allowed to project to
    /// before the next-finer level must take over.
    pub(crate) budget_px: f32,
}

/// Authored one-pixel budget when player settings are absent, matching the corpus's
/// `scripts/lod/config.py::REFERENCE_VIEW`. A tuning default, not a substitute view measurement.
pub(crate) const DEFAULT_BUDGET_PX: f32 = 1.0;

impl ViewProfile {
    /// Shared facts at a positive finite `budget_px`; callers choose any authored default.
    pub(crate) fn of(facts: ViewFacts, budget_px: f32) -> Self {
        assert!(
            budget_px.is_finite() && budget_px > 0.0,
            "LOD consumers supply a positive finite pixel budget from admitted settings or their authored default"
        );
        Self { facts, budget_px }
    }

    /// Distance where `dev_m` fits the budget, plus radius slack: `VisibilityRange` measures an
    /// anchor (origin or AABB centre with `use_aabb`), but the surface can be one radius nearer.
    pub(crate) fn switch_distance_m(self, dev_m: f32, radius_m: f32) -> f32 {
        self.facts.sub_pixel_distance_m(dev_m, self.budget_px) + radius_m
    }
}

/// Sole live-fact reader/writer: resolves the declaration, delegates arithmetic to
/// [`ViewFacts::settled`] and reports contradictions. [`plugin`] schedules legitimate startup
/// absence. A missing primary window or image supplies no height: hold measured height if any,
/// otherwise remain without facts; never invent it. Window-backed compositions present the primary window.
pub(crate) fn track_view_facts(
    mut commands: Commands,
    views: Query<(&Projection, Option<&RenderTarget>), With<PlayerView>>,
    window: Query<&Window, With<PrimaryWindow>>,
    images: Option<Res<Assets<Image>>>,
    scale: Option<Res<crate::render_scale::RenderScale>>,
    facts: Option<ResMut<ViewFacts>>,
) -> Result<(), BevyError> {
    // Translate query cardinality into the declared-view diagnosis.
    let Ok((projection, target)) = views.single() else {
        return Err(ViewError::resolving(views.iter().count()).into());
    };
    let Projection::Perspective(projection) = projection else {
        return Err(ViewError::UnsupportedProjection.into());
    };
    // An image-backed view selects geometry for that image's physical pixels. The hidden
    // window's size can differ from its offscreen target and cannot define this view's LOD.
    let measured = match target {
        Some(RenderTarget::Image(target)) => images
            .as_deref()
            .and_then(|images| images.get(&target.handle))
            .map_or(0.0, |image| {
                image.height() as f32 * scale.as_deref().map_or(1.0, |scale| scale.0)
            }),
        _ => ViewFacts::rendered_height_px(window.single().ok(), scale.as_deref()),
    };
    let held = facts.as_deref().copied();
    let wanted = match held {
        // Missing image, unsized window or surface teardown under `ExitCondition::DontExit`:
        // hold measured height (e.g. 1440/2160 px). A smaller substitute shortens every threshold
        // by that ratio, showing coarse geometry nearer than its certificate permits.
        Some(held) => held.settled(
            projection.fov,
            if measured > 0.0 {
                measured
            } else {
                held.height_px
            },
        ),
        // No prior measurement: defer consumers until window and projection are measurable.
        None => match ViewFacts::new(projection.fov, measured) {
            Some(first) => first,
            None => return Ok(()),
        },
    };
    if held == Some(wanted) {
        return Ok(());
    }
    match facts {
        Some(mut facts) => *facts = wanted,
        None => commands.insert_resource(wanted),
    }
    info!(
        "view: {:.4} rad × {:.0} px — every LOD ladder reselects",
        wanted.vfov_rad, wanted.height_px,
    );
    Ok(())
}

/// Mount in every windowed composition; headless apps have no view. Gate on [`PlayerView`] so
/// startup before its camera exists is legitimate absence. A composition that never declares one
/// reaches the same gate; keeping facts absent (no `Default`/`init_resource`) prevents guessed LOD.
/// The gate controls scheduling, not declaration correctness.
pub(crate) fn plugin(app: &mut App) {
    app.add_systems(
        Update,
        // LOD facts must consume this frame's applied render scale and window mode; both
        // geometry ladders derive their distances from these facts in the same Update.
        track_view_facts
            .after(crate::settings::ApplySettings)
            .run_if(any_with_component::<PlayerView>),
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Fixture for a MEASURED view: valid [`ViewFacts::new`] inputs, never a fallback.
    fn measured(vfov_rad: f32, height_px: f32) -> ViewFacts {
        ViewFacts::new(vfov_rad, height_px).expect("a measured view")
    }

    /// ADR-0033 §9 requires exact projection. DERIVED `2·tan(f/2) ≥ f`: the small-angle shortcut
    /// overestimates distance by 0.12 % at the gunner field, 5.5 % at the commander field.
    #[test]
    fn the_projection_is_exact_and_never_the_small_angle_shortcut() {
        for (fov, claimed) in [
            (0.12_f32, 0.0012_f32),
            (std::f32::consts::FRAC_PI_4, 0.0548),
        ] {
            let view = measured(fov, 2160.0);
            let exact = 0.05 * 2160.0 / (2.0 * (fov / 2.0).tan());
            let derived = view.sub_pixel_distance_m(0.05, 1.0);
            assert!(
                (derived - exact).abs() < 1e-3,
                "fov {fov}: D = dev_m × height_px / (2·tan(vfov/2)·budget_px), got {derived}",
            );
            let small_angle = 0.05 * 2160.0 / fov;
            assert!(
                derived < small_angle,
                "fov {fov}: the small-angle shortcut must not be what is wired",
            );
            let overshoot = (small_angle - exact) / exact;
            assert!(
                (overshoot - claimed).abs() < 0.001,
                "fov {fov}: the shortcut overshoots by {overshoot}, documented as {claimed}",
            );
        }
    }

    /// A HALVED BUDGET doubles every distance; a HALVED HEIGHT halves the deviation term. Both
    /// terms are linear, and the radius slack rides outside them.
    #[test]
    fn the_budget_and_the_height_scale_the_deviation_term() {
        let radius = 0.5;
        let base = ViewProfile::of(measured(0.12, 2160.0), 1.0);
        let term = base.switch_distance_m(0.01, radius) - radius;
        let halved_budget = ViewProfile::of(measured(0.12, 2160.0), 0.5);
        assert!((halved_budget.switch_distance_m(0.01, radius) - radius - 2.0 * term).abs() < 1e-2);
        let halved_height = ViewProfile::of(measured(0.12, 1080.0), 1.0);
        assert!((halved_height.switch_distance_m(0.01, radius) - radius - 0.5 * term).abs() < 1e-2);
    }

    /// Invalid FOV/height cannot masquerade as measurements (e.g. narrow optic/1080 px).
    #[test]
    fn an_input_that_is_not_a_measurement_is_not_a_view() {
        for (fov, height) in [
            (f32::NAN, 1440.0),
            (0.0, 1440.0),
            (-0.5, 1440.0),
            (core::f32::consts::PI, 1440.0),
            (0.12, 0.0),
            (0.12, -1.0),
            (0.12, f32::INFINITY),
            (0.12, f32::NAN),
        ] {
            assert!(
                ViewFacts::new(fov, height).is_none(),
                "{fov} rad × {height} px is not a view anything may select through",
            );
        }
        // A zero-deviation level is the source surface: it owns the camera, not the radius alone.
        assert_eq!(measured(0.12, 1080.0).sub_pixel_distance_m(0.0, 1.0), 0.0);
    }

    /// LOD consumers must not silently replace invalid internal budgets with another selection
    /// policy. A removed constructor assertion lets every invalid fixture reach a profile.
    #[test]
    fn invalid_lod_budgets_are_bugs_in_the_consumer() {
        // Fixture measurements and invalid budgets; independent of any consumer's chosen taste.
        let facts = measured(0.12, 1440.0);
        for budget in [-1.0, 0.0, f32::NEG_INFINITY, f32::NAN, f32::INFINITY] {
            assert!(
                std::panic::catch_unwind(|| ViewProfile::of(facts, budget)).is_err(),
                "invalid internal budget {budget} must be refused"
            );
        }
    }

    /// Subthreshold FOV nudges hold the field to avoid permanent `VisibilityRange` table growth.
    /// Accumulation must cross [`FOV_HYSTERESIS`] eventually. The signature carries held value and
    /// request, not last request, ruling out a band that follows a slow drag and never lands.
    #[test]
    fn the_field_dead_band_delays_a_drag_and_never_deadlocks_it() {
        let commander = std::f32::consts::FRAC_PI_4;
        let held = measured(commander, 1440.0);
        // A resize is a human-rate move and carries no band at all.
        assert_eq!(held.settled(commander, 2160.0).height_px, 2160.0);
        // Half a band holds all facts, avoiding a resource change tick.
        let half = held.settled(commander * (1.0 + FOV_HYSTERESIS * 0.5), 1440.0);
        assert_eq!(half, held, "inside the band the field is held, not adopted");
        // Further motion accumulates against held FOV and crosses the band.
        let crossed = commander * (1.0 + FOV_HYSTERESIS * 1.025);
        assert_eq!(half.settled(crossed, 1440.0).vfov_rad, crossed);
        // The optic toggle is 6.5x and cannot be gated by any dead band worth having.
        assert_eq!(
            held.settled(crate::camera::GUNNER_FOV_FALLBACK, 1440.0)
                .vfov_rad,
            crate::camera::GUNNER_FOV_FALLBACK,
        );
        // Invalid requests retain measured facts.
        assert_eq!(held.settled(f32::NAN, 1440.0), held);
        assert_eq!(held.settled(commander, 0.0), held);
    }

    fn perspective(fov: f32) -> Projection {
        Projection::Perspective(PerspectiveProjection { fov, ..default() })
    }

    /// Resize the window to a 16:9 surface `height_px` tall — the fact the reader actually reads.
    fn resize(world: &mut World, window: Entity, height_px: u32) {
        world
            .get_mut::<Window>(window)
            .expect("the test window")
            .resolution
            .set_physical_resolution(height_px * 16 / 9, height_px);
    }

    /// The composition every system-level test here starts from: the live view mounted, a primary
    /// window at `height_px`, and ONE camera declaring itself the player's view at `fov`.
    fn declared_view_app(fov: f32, height_px: u32) -> (App, Entity, Entity) {
        let mut app = App::new();
        app.add_plugins(plugin);
        let world = app.world_mut();
        let window = world.spawn((Window::default(), PrimaryWindow)).id();
        resize(world, window, height_px);
        let camera = world.spawn((perspective(fov), PlayerView)).id();
        (app, camera, window)
    }

    fn facts_of(app: &App) -> Option<ViewFacts> {
        app.world().get_resource::<ViewFacts>().copied()
    }

    /// Both LOD ladders must select for the pixels actually drawn. A hidden window with a
    /// different backing scale must not change a tank's mesh when the image target is unchanged.
    #[test]
    fn an_offscreen_view_measures_its_image_instead_of_the_hidden_window() {
        // ASSUMED fixture dimensions and scale; the invariant is target independence from window.
        let (mut app, camera, window) = declared_view_app(std::f32::consts::FRAC_PI_4, 1440);
        let mut images = Assets::<Image>::default();
        let target = crate::dev_capture::offscreen_target(&mut images, UVec2::new(1280, 720), 2.0);
        app.insert_resource(images)
            .insert_resource(crate::render_scale::RenderScale(0.5));
        app.world_mut()
            .entity_mut(camera)
            .insert(RenderTarget::Image(target));
        app.update();
        let wanted = measured(std::f32::consts::FRAC_PI_4, 360.0);
        assert_eq!(facts_of(&app), Some(wanted));
        resize(app.world_mut(), window, 720);
        app.update();
        assert_eq!(facts_of(&app), Some(wanted));
    }

    /// Settings must be fresh before LOD facts read the render scale. Ambiguity detection makes
    /// removing the production order fail even if the executor happens to pick the desired order.
    #[test]
    fn lod_facts_read_the_current_frames_applied_settings() {
        use bevy::ecs::schedule::{LogLevel, ScheduleBuildSettings};

        fn apply(mut scale: ResMut<crate::render_scale::RenderScale>) {
            scale.0 = 0.5; // ASSUMED fixture scale.
        }
        let (mut app, _, _) = declared_view_app(std::f32::consts::FRAC_PI_4, 1440);
        app.insert_resource(crate::render_scale::RenderScale(1.0))
            .add_systems(Update, apply.in_set(crate::settings::ApplySettings))
            .edit_schedule(Update, |schedule| {
                schedule.set_build_settings(ScheduleBuildSettings {
                    ambiguity_detection: LogLevel::Error,
                    ..default()
                });
            });
        app.update();
        assert_eq!(
            facts_of(&app),
            Some(measured(std::f32::consts::FRAC_PI_4, 720.0))
        );
    }

    /// Marker insertion must supply both readers' required camera components. Removing
    /// `#[require(Camera3d)]` lets an unreadable bare declaration corrupt cardinality reports.
    #[test]
    fn the_declaration_carries_the_camera_every_reader_needs() {
        let mut world = World::new();
        let declared = world.spawn(PlayerView).id();
        let declared = world.entity(declared);
        assert!(declared.contains::<Camera3d>(), "the marker makes a camera");
        assert!(
            matches!(
                declared.get::<Projection>(),
                Some(Projection::Perspective(_))
            ),
            "with the field this module projects a metre through",
        );
        assert!(
            declared.contains::<GlobalTransform>(),
            "and the pose `track::link_view` measures a belt's distance from",
        );
    }

    /// Three 3-D cameras, one declaration: read its facts, never the first/nearest camera.
    /// Decoy FOVs differ from the declaration and required π/4, preventing accidental success.
    /// Counting cameras instead would freeze selection; a guessed 0.12 rad view would over-detail
    /// the sandboxes with two/three cameras. Freeze direction depends only on the value held.
    #[test]
    fn the_declared_view_is_read_out_of_a_crowd_of_cameras() {
        let commander = std::f32::consts::FRAC_PI_4;
        let (mut app, _, _) = declared_view_app(commander, 1440);
        // The overlay and UI rigs a sandbox mounts: 3-D cameras, no player behind either.
        let world = app.world_mut();
        world.spawn((Camera3d::default(), perspective(1.2)));
        world.spawn((Camera3d::default(), perspective(0.4)));
        app.update();
        assert_eq!(
            facts_of(&app),
            Some(measured(commander, 1440.0)),
            "the declared view is the view, however many cameras stand beside it",
        );
    }

    /// Production reader must settle against held facts across frames, not call `ViewFacts::new`
    /// each time. That mutant drops the dead band and leaks permanent `VisibilityRange` entries
    /// on every magnification-drag frame (ADR-0033 §11); pure arithmetic tests cannot catch wiring.
    #[test]
    fn the_facts_move_on_a_resize_and_hold_inside_the_field_dead_band() {
        let commander = std::f32::consts::FRAC_PI_4;
        let (mut app, camera, window) = declared_view_app(commander, 1440);
        app.update();
        assert_eq!(
            facts_of(&app),
            Some(measured(commander, 1440.0)),
            "the first frame with a real window and camera is the first read",
        );

        // A RESIZE is human-rate and carries no band: the height follows it on the next frame.
        resize(app.world_mut(), window, 2160);
        app.update();
        assert_eq!(facts_of(&app), Some(measured(commander, 2160.0)));

        // A 5 % magnification nudge, INSIDE the band around the wired π/4: held.
        let nudge = |app: &mut App, fov: f32| {
            *app.world_mut()
                .get_mut::<Projection>(camera)
                .expect("the declared view's projection") = perspective(fov);
            app.update();
        };
        nudge(&mut app, commander * 1.05);
        assert_eq!(
            facts_of(&app),
            Some(measured(commander, 2160.0)),
            "inside the band the WIRED field is held, and the band is measured around it",
        );

        // And the optic toggle, 6.5x, still lands on the very next frame.
        nudge(&mut app, crate::camera::GUNNER_FOV_FALLBACK);
        assert_eq!(
            facts_of(&app),
            Some(measured(crate::camera::GUNNER_FOV_FALLBACK, 2160.0)),
        );
    }

    /// Surface teardown under `ExitCondition::DontExit` holds height while still accepting FOV
    /// changes. DERIVED counterexample: replacing 1440 with 1080 px shortens thresholds by a quarter,
    /// exposing coarse geometry too near. Passing zero directly to [`ViewFacts::settled`] would
    /// reject the whole request and incorrectly hold FOV too.
    #[test]
    fn a_window_that_goes_away_holds_the_height_it_settled_on() {
        let commander = std::f32::consts::FRAC_PI_4;
        let (mut app, camera, window) = declared_view_app(commander, 1440);
        app.update();
        app.world_mut().entity_mut(window).despawn();
        app.update();
        assert_eq!(
            facts_of(&app),
            Some(measured(commander, 1440.0)),
            "a despawned surface is a frame with no height, not a shorter view",
        );

        let gunner = 0.12;
        app.world_mut()
            .entity_mut(camera)
            .insert(perspective(gunner));
        app.update();
        assert_eq!(
            facts_of(&app),
            Some(measured(gunner, 1440.0)),
            "the height is what the missing window costs, not the field",
        );
    }

    /// Before Startup's camera, or forever without a declaration, no facts exist to mislead LOD.
    /// Removing the run condition panics through Bevy's fallback handler; adding
    /// `init_resource::<ViewFacts>()` invents a view before measurement. Both mutants must fail.
    #[test]
    fn a_composition_with_no_declared_view_has_no_facts_rather_than_invented_ones() {
        let mut app = App::new();
        app.add_plugins(plugin);
        app.world_mut().spawn((Window::default(), PrimaryWindow));
        app.world_mut().spawn(Camera3d::default());
        app.update();
        assert_eq!(facts_of(&app), None);
    }

    /// Without a measurable window, first read leaves facts absent; adding a sized surface enables LOD.
    #[test]
    fn a_first_frame_with_no_measurable_window_produces_no_facts() {
        let mut app = App::new();
        app.add_plugins(plugin);
        app.world_mut()
            .spawn((perspective(std::f32::consts::FRAC_PI_4), PlayerView));
        app.update();
        assert_eq!(facts_of(&app), None, "no window is no height, and no view");
        let window = app
            .world_mut()
            .spawn((Window::default(), PrimaryWindow))
            .id();
        resize(app.world_mut(), window, 1440);
        app.update();
        assert_eq!(
            facts_of(&app),
            Some(measured(std::f32::consts::FRAC_PI_4, 1440.0)),
        );
    }

    /// Every declaration fault reports without writing. A silent return looks like an unchanged
    /// frame; reporting plus writing also fails because a measurable 1440 px window is present.
    #[test]
    fn each_way_the_declaration_can_be_wrong_is_reported_rather_than_skipped() {
        use bevy::ecs::system::RunSystemOnce;

        let read = |spawn: &dyn Fn(&mut World)| -> BevyError {
            let mut app = App::new();
            app.add_plugins(plugin);
            let world = app.world_mut();
            let window = world.spawn((Window::default(), PrimaryWindow)).id();
            resize(world, window, 1440);
            spawn(world);
            let read: Result<(), BevyError> = app
                .world_mut()
                .run_system_once(track_view_facts)
                .expect("the reader runs");
            let reported = read.expect_err("a wrong declaration is a report, never a frame");
            assert_eq!(
                facts_of(&app),
                None,
                "a reader that cannot resolve the view writes nothing: {reported}",
            );
            reported
        };

        // No declaration at all: the scheduling gate keeps this off a real frame.
        let err = read(&|_| {});
        assert!(
            err.to_string().contains("no player view"),
            "the report names the fault: {err}",
        );

        // Two declarations: a contradiction, and the count is the diagnosis.
        let err = read(&|world| {
            world.spawn(PlayerView);
            world.spawn(PlayerView);
        });
        assert!(
            err.to_string().contains("2 entities declare"),
            "the report counts the declarations: {err}",
        );

        // A projection with no perspective half-angle to project a metre through — the one wrong
        // state that IS a value, since the declared camera legitimately carries a `Projection`.
        let err = read(&|world| {
            world.spawn((
                PlayerView,
                Projection::Orthographic(OrthographicProjection::default_3d()),
            ));
        });
        assert!(
            err.to_string().contains("non-perspective"),
            "the report names the fault: {err}",
        );

        // And the shape of the thing being pinned: the counts, as values.
        assert_eq!(ViewError::resolving(0), ViewError::NoPlayerView);
        assert_eq!(ViewError::resolving(3), ViewError::ManyPlayerViews(3));
    }

    /// Existing 0.5 rad × 1440 px facts survive either declaration fault. Starting with facts
    /// distinguishes "wrote nothing" from "cleared then reported", which first-read tests cannot.
    #[test]
    fn a_declaration_that_breaks_leaves_the_settled_facts_alone() {
        use bevy::ecs::system::RunSystemOnce;

        let broken = |break_it: &dyn Fn(&mut World)| -> Option<ViewFacts> {
            let (mut app, camera, _) = declared_view_app(0.5, 1440);
            app.update();
            let settled = facts_of(&app).expect("the declared view settles");
            assert_eq!(settled, measured(0.5, 1440.0));
            let world = app.world_mut();
            break_it(world);
            let read: Result<(), BevyError> = world
                .run_system_once(track_view_facts)
                .expect("the reader runs");
            read.expect_err("a wrong declaration is a report, never a frame");
            let _ = camera;
            facts_of(&app)
        };

        // A second declaration arrives mid-session.
        assert_eq!(
            broken(&|world| {
                world.spawn(PlayerView);
            }),
            Some(measured(0.5, 1440.0)),
            "the contradiction is reported, the last measured view is left standing",
        );

        // The declared camera's projection is swapped for one with no half-angle.
        assert_eq!(
            broken(&|world| {
                let declared = world
                    .query_filtered::<Entity, With<PlayerView>>()
                    .single(world)
                    .expect("the declared view");
                world.entity_mut(declared).insert(Projection::Orthographic(
                    OrthographicProjection::default_3d(),
                ));
            }),
            Some(measured(0.5, 1440.0)),
            "the projection is reported, the last measured view is left standing",
        );
    }

    /// Production scheduling must panic on a second declaration through Bevy's default
    /// `BevyError` severity, not merely return a value a caller could drop (ADR-0011).
    #[test]
    #[should_panic(expected = "entities declare `PlayerView`")]
    fn a_second_declaration_stops_the_app_rather_than_the_ladders() {
        let mut app = App::new();
        app.add_plugins(plugin);
        let world = app.world_mut();
        world.spawn((Window::default(), PrimaryWindow));
        world.spawn(PlayerView);
        world.spawn(PlayerView);
        app.update();
    }
}
