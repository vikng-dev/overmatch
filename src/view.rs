//! THE LIVE VIEW, once: the facts every screen-space ladder selects through, and the projection
//! that turns a world-space deviation into metres of switch distance (ADR-0033 §9).
//!
//! # The view is DECLARED, not discovered
//!
//! [`PlayerView`] marks the camera the player looks through, and every reader of "the view" filters
//! on it. Nothing infers it from the shape of the world: `single()` over `With<Camera3d>` answers
//! "is there exactly one 3-D camera", a fact about the archetypes, and reading a domain fact out of
//! it holds only while the game happens to spawn one camera. A mirror, a spotter, an overlay or a
//! debug camera then makes the answer WRONG, and bevy's `single()` reports wrong by SKIPPING — no
//! log, no panic, the ladders frozen at the last value they held, which is the one direction LOD
//! must never fail in. Under the declaration that ambiguity cannot be expressed: a second camera is
//! not a player view unless someone declares it one, and two declarations are a contradiction the
//! reader refuses out loud ([`ViewError`]).
//!
//! # One reader, many consumers
//!
//! [`ViewFacts`] is the only thing in the tree that reads a `Projection`, a `Window` and
//! `render_scale::RenderScale` to answer "what does the player's view look like right now". Both
//! LOD ladders — `terrain_lod`'s tiles and `geometry_lod`'s certified chains — are CONSUMERS of it.
//! They were two independent readers of the same three components until they disagreed about the
//! projection itself, which is the class of drift a shared fact removes by construction.
//!
//! What is NOT shared is the pixel budget. A budget is a tuning knob per consumer (terrain ships
//! [`crate::terrain_lod::TERRAIN_LOD_BUDGET_PX`]; the tank chain spends the player's
//! `settings::LodPixelBudget`), so a consumer pairs the shared facts with its own budget into a
//! [`ViewProfile`] and derives metres from that.
//!
//! # Human-rate, with hysteresis
//!
//! bevy retains a permanent range-table slot per distinct `VisibilityRange` value for the lifetime
//! of the app, so a threshold that moves per frame is a slow leak (ADR-0033 §11). The facts
//! therefore move only when the view actually moves: a resolution change, a render-scale rung, an
//! optic toggle — and the field carries a relative dead band ([`FOV_HYSTERESIS`]) so a dragged
//! magnification slider cannot mint a slot per frame.

use bevy::ecs::error::BevyError;
use bevy::prelude::*;
use bevy::window::PrimaryWindow;

/// THE CAMERA THE PLAYER LOOKS THROUGH — the one declaration every reader of the live view
/// resolves against (module doc).
///
/// Put it on the `Camera3d` a composition presents to its player: the game's orbit camera, each
/// sandbox's free-fly camera. Put it on nothing else. It is deliberately a bare marker and not
/// [`crate::render_policy::CameraProfile`] — a profile says which CHANNELS a camera draws, which a
/// sandbox's overlay/UI rig has its own (raw-layer) answer to, so making the profile do double duty
/// would force a render-channel policy onto every composition that has a player.
///
/// EXACTLY ONE per app. Zero is a composition that has not declared its view; two is a
/// contradiction — see [`ViewError`].
#[derive(Component, Clone, Copy, PartialEq, Eq, Debug, Default)]
pub(crate) struct PlayerView;

/// What can be wrong with the declaration — the value form of every failure the readers used to
/// express as a silent `return`.
///
/// None of these is a transient and none of them is "not yet": a reader is SCHEDULED behind
/// `any_with_component::<PlayerView>` (see [`plugin`]), so startup's genuine absence never reaches
/// one. Reaching one means the declaration itself is wrong, which is a bug in a composition rather
/// than a condition of a frame — so they carry bevy's default `Severity::Panic` and fail the build
/// that introduced them, exactly as a missing model contract does (ADR-0011). A ladder that quietly
/// held its last value is what this replaces.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum ViewError {
    /// A declared view that carries nothing to read — a `PlayerView` on something that is not a
    /// camera. (The scheduling gate is what makes "no camera yet" a different thing entirely.)
    NoPlayerView,
    /// More than one camera declares itself the player's view. The count is the diagnosis: it says
    /// how many, so the composition that added the second one is findable.
    ManyPlayerViews(usize),
    /// The player's view is not a perspective one. Every screen-space ladder projects a metre
    /// through a perspective half-angle ([`ViewFacts::sub_pixel_distance_m`]); an orthographic or
    /// custom projection has no such angle and no distance to select at.
    UnsupportedProjection,
}

impl ViewError {
    /// The domain fact behind a query that did not resolve to exactly one declared view, `views`
    /// being how many it matched. THE ONE translation from query shape to domain in the tree.
    pub(crate) fn resolving(views: usize) -> Self {
        match views {
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
                "no player view: a `PlayerView` is declared but carries no camera to read — the \
                 view is declared, never discovered, so put the marker on the `Camera3d` the \
                 player looks through",
            ),
            Self::ManyPlayerViews(views) => write!(
                f,
                "{views} cameras declare `PlayerView`: exactly one camera is the player's view — a \
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

/// Relative field-of-view change that must accumulate before the facts move.
///
/// The optic toggle is a 6.5× jump (π/4 → 0.12 rad), so this never gates a real view change; what
/// it gates is a magnification slider being dragged, where a rewrite per frame would walk every
/// LOD entity on both ladders for a sub-pixel difference.
pub(crate) const FOV_HYSTERESIS: f32 = 0.10;

/// What the player's view IS, right now: the [`PlayerView`] camera's vertical field, and the pixel
/// height the main pass actually renders at.
///
/// Exactly one camera is declared the player's view and the gunner optic swaps its `Projection` fov
/// in place, so the live view is a single pair rather than a set — which is what makes one shared
/// resource the right shape and not an over-generalisation. Note where that "one" comes from: the
/// declaration, not a count of the cameras in the world.
#[derive(Resource, Clone, Copy, PartialEq, Debug)]
pub(crate) struct ViewFacts {
    /// Vertical field of view, radians.
    pub(crate) vfov_rad: f32,
    /// Rendered height of the main pass, pixels (window physical height × render scale).
    pub(crate) height_px: f32,
}

impl Default for ViewFacts {
    /// The pre-window guess: the narrowest authored field and a modest height. A narrow field
    /// demands the finest geometry, so the first frames are over-detailed rather than
    /// under-detailed, and [`track_view_facts`] replaces this on the first frame with a real
    /// window and camera.
    fn default() -> Self {
        Self {
            vfov_rad: crate::camera::GUNNER_FOV_FALLBACK,
            height_px: 1080.0,
        }
    }
}

impl ViewFacts {
    /// Live facts, with both inputs treated as UNTRUSTED.
    ///
    /// A NON-POSITIVE height is not a small window, it is an ABSENT one — a window bevy has not
    /// sized yet at Startup, or a zero render scale. Taken literally it collapses every switch
    /// distance onto the bounding radius and puts the COARSEST level a bounding radius from the
    /// camera on the frames the player first sees, which is the exact opposite of the conservative
    /// direction.
    ///
    /// A FIELD outside `(0, π)` is worse, because it is a DIVISOR: `NaN` produces `NaN` range
    /// boundaries, which compare false against every distance — the world simply stops being drawn
    /// — a negative one inverts every chain, and π or more has no perspective half-angle to be the
    /// field of. `spec::TankSpec::validate` rejects such a sheet outright over the same interval,
    /// so this is the second line: a camera whose projection has been written by anything other
    /// than an authored view (a debug tool, a half-initialised projection) still leaves both
    /// ladders with a usable view instead of an invisible world.
    ///
    /// Both fall back to the default's value, and the next frame with sane inputs corrects it.
    /// Silently, on purpose: this is a per-frame view-layer read, and a fail-loud here would panic
    /// the client on a transient.
    pub(crate) fn new(vfov_rad: f32, height_px: f32) -> Self {
        let default = Self::default();
        Self {
            vfov_rad: if vfov_rad > 0.0 && vfov_rad < core::f32::consts::PI {
                vfov_rad
            } else {
                // `NaN` fails both comparisons and lands here with everything else out of range.
                default.vfov_rad
            },
            height_px: if height_px.is_finite() && height_px > 0.0 {
                height_px
            } else {
                default.height_px
            },
        }
    }

    /// THE WHOLE INTENDED BEHAVIOUR OF [`track_view_facts`], as arithmetic: the facts these become
    /// for a view of `vfov_rad` rendered at `height_px`, dead band included. Pure — no world, no
    /// query, no `Option`, so the rule is testable without an `App` and the system that calls it is
    /// left with nothing but resolving the view and reporting what it could not.
    ///
    /// Returns SELF when nothing moved, which is what keeps the resource's change ticks at human
    /// rate: the caller writes only a value that differs.
    ///
    /// Inside the dead band the field is HELD, not adopted — adopting it would let a slider creep
    /// the value one sub-threshold step at a time and mint a range-table slot for each. The band is
    /// measured against the HELD field, never against the last request, so sub-threshold steps
    /// accumulate and land the moment their total crosses it: the band DELAYS, it does not deadlock.
    pub(crate) fn settled(self, vfov_rad: f32, height_px: f32) -> Self {
        let wanted = Self::new(vfov_rad, height_px);
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

    /// The pixel height the main pass renders at: the window's PHYSICAL height through the render
    /// scale. THE one expression of it — `render_scale` draws the 3-D pass into that fraction of
    /// the window, so a ladder that read the window alone would be selecting for a view nobody is
    /// looking at.
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
    /// EXACT, not small-angle. The shortcut divides by `vfov` rather than by `2·tan(vfov/2)`, which
    /// is 5.5 % wrong at the commander field — ADR-0033 §9 puts the exact form in the doctrine, and
    /// `scripts/lod/config.py::switch_distance_m` is the same expression on the build side, so the
    /// metres a certificate was cut against and the metres the runtime selects at come out of one
    /// formula.
    pub(crate) fn sub_pixel_distance_m(self, dev_m: f32, budget_px: f32) -> f32 {
        if dev_m <= 0.0 {
            return 0.0;
        }
        dev_m * self.height_px / (2.0 * (self.vfov_rad / 2.0).tan() * budget_px)
    }
}

/// One consumer's selection view: the shared [`ViewFacts`] plus THAT consumer's pixel budget.
///
/// The split is the point. The facts are a property of the session and have one writer; the budget
/// is a tuning knob and each ladder owns its own (see the module doc).
#[derive(Resource, Clone, Copy, PartialEq, Debug)]
pub(crate) struct ViewProfile {
    /// The live view, shared.
    pub(crate) facts: ViewFacts,
    /// Screen-space error budget, pixels: the on-screen size a deviation is allowed to project to
    /// before the next-finer level must take over.
    pub(crate) budget_px: f32,
}

impl Default for ViewProfile {
    fn default() -> Self {
        Self {
            facts: ViewFacts::default(),
            budget_px: 1.0,
        }
    }
}

impl ViewProfile {
    /// The shared facts, spent at `budget_px`. A non-positive or non-finite budget collapses every
    /// distance onto the bounding radius, so it falls back exactly as the facts do.
    pub(crate) fn of(facts: ViewFacts, budget_px: f32) -> Self {
        Self {
            facts,
            budget_px: if budget_px.is_finite() && budget_px > 0.0 {
                budget_px
            } else {
                Self::default().budget_px
            },
        }
    }

    /// The distance beyond which `dev_m` fits inside this profile's pixel budget, plus a bounding
    /// radius as slack.
    ///
    /// The radius term is there because `VisibilityRange` measures the camera to an ANCHOR — the
    /// entity origin, or the AABB centre under `use_aabb` — while the deviation lives on the
    /// SURFACE, which can be one bounding radius nearer the camera than the point the runtime
    /// tested.
    pub(crate) fn switch_distance_m(self, dev_m: f32, radius_m: f32) -> f32 {
        self.facts.sub_pixel_distance_m(dev_m, self.budget_px) + radius_m
    }
}

/// THE ONE READER of the live view: a thin adapter over [`ViewFacts::settled`] that resolves the
/// declared view and hands back what it could not.
///
/// Three jobs the one `let Ok(…) else { return }` it replaces had conflated. The ARITHMETIC is in
/// `settled`, where it is testable without a world. The FAILURES are values, at this one boundary,
/// and each is a contradiction in the declaration rather than a state of the frame. "NOT YET" is
/// neither of those and is expressed as SCHEDULING ([`plugin`]) — so nothing here has to encode the
/// startup frames where a composition legitimately has no camera, and nothing here may `return`
/// silently.
///
/// The window is the PRIMARY one — bevy's own declaration of the window a composition presents —
/// for the same reason the camera is the declared view. An absent one is not an error though: it is
/// an absent HEIGHT, which [`ViewFacts::new`] documents as an untrusted input with a conservative
/// fallback.
pub(crate) fn track_view_facts(
    views: Query<&Projection, With<PlayerView>>,
    window: Query<&Window, With<PrimaryWindow>>,
    scale: Option<Res<crate::render_scale::RenderScale>>,
    mut facts: ResMut<ViewFacts>,
) -> Result<(), BevyError> {
    // `single()` answers a QUERY-SHAPE question ("is there exactly one row?"); the count restates
    // its failure as the domain fact this reader actually needs.
    let Ok(projection) = views.single() else {
        return Err(ViewError::resolving(views.iter().count()).into());
    };
    let Projection::Perspective(projection) = projection else {
        return Err(ViewError::UnsupportedProjection.into());
    };
    let height_px = ViewFacts::rendered_height_px(window.single().ok(), scale.as_deref());
    let settled = facts.settled(projection.fov, height_px);
    if settled != *facts {
        *facts = settled;
        info!(
            "view: {:.4} rad × {:.0} px — every LOD ladder reselects",
            facts.vfov_rad, facts.height_px,
        );
    }
    Ok(())
}

/// Mount the live view. Every windowed composition needs it; a headless one has no view to read.
///
/// The reader is GATED ON THE DECLARATION existing, which is the whole handling of "not yet": a
/// composition's camera spawns in `Startup` and the frames before it are simply frames this system
/// is not in. What is left inside it is genuinely a bug, and says so.
pub(crate) fn plugin(app: &mut App) {
    app.init_resource::<ViewFacts>().add_systems(
        Update,
        track_view_facts.run_if(any_with_component::<PlayerView>),
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    /// THE PROJECTION IS EXACT, and the shortcut it replaced is quantified rather than remembered.
    ///
    /// `2·tan(f/2) ≥ f` always, so the small-angle form always returned the LARGER distance — it
    /// held every level nearer the camera than the pixel budget required. It reads 0.12 % long in
    /// the gunner optic and 5.5 % long at the commander field, which is the number ADR-0033 §9
    /// refuses it over. Quantified here rather than remembered: the claim is in the doctrine, so
    /// the arithmetic behind it belongs in the suite.
    #[test]
    fn the_projection_is_exact_and_never_the_small_angle_shortcut() {
        for (fov, claimed) in [
            (0.12_f32, 0.0012_f32),
            (std::f32::consts::FRAC_PI_4, 0.0548),
        ] {
            let view = ViewFacts::new(fov, 2160.0);
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
        let base = ViewProfile::of(ViewFacts::new(0.12, 2160.0), 1.0);
        let term = base.switch_distance_m(0.01, radius) - radius;
        let halved_budget = ViewProfile::of(ViewFacts::new(0.12, 2160.0), 0.5);
        assert!((halved_budget.switch_distance_m(0.01, radius) - radius - 2.0 * term).abs() < 1e-2);
        let halved_height = ViewProfile::of(ViewFacts::new(0.12, 1080.0), 1.0);
        assert!((halved_height.switch_distance_m(0.01, radius) - radius - 0.5 * term).abs() < 1e-2);
    }

    /// UNTRUSTED INPUTS fall back rather than poisoning a divisor — the guard BOTH ladders now sit
    /// behind, so neither can be handed a `NaN` field the other would have rejected.
    #[test]
    fn a_hostile_view_falls_back_to_the_conservative_profile() {
        let default = ViewProfile::default();
        for (fov, height, budget) in [
            (f32::NAN, 1440.0, 1.0),
            (0.0, 1440.0, 1.0),
            (core::f32::consts::PI, 1440.0, 1.0),
            (0.12, 0.0, 1.0),
            (0.12, f32::INFINITY, 1.0),
            (0.12, 1440.0, -1.0),
            (0.12, 1440.0, f32::NAN),
        ] {
            let view = ViewProfile::of(ViewFacts::new(fov, height), budget);
            assert!(view.facts.vfov_rad > 0.0 && view.facts.vfov_rad < core::f32::consts::PI);
            assert!(view.facts.height_px.is_finite() && view.facts.height_px > 0.0);
            assert!(view.budget_px.is_finite() && view.budget_px > 0.0);
            assert!(view.switch_distance_m(0.01, 0.5).is_finite());
        }
        assert_eq!(
            ViewProfile::of(ViewFacts::new(f32::NAN, -1.0), f32::NAN),
            default
        );
        // A zero-deviation level is the source surface: it owns the camera, not the radius alone.
        assert_eq!(ViewFacts::default().sub_pixel_distance_m(0.0, 1.0), 0.0);
    }

    /// AN UNSIZED WINDOW IS AN ABSENT ONE. bevy reports zero physical height before it has sized
    /// the surface, and a zero render scale says the same thing; read literally either collapses
    /// every switch distance onto the bounding radius and puts the COARSEST level of every ladder a
    /// bounding radius from the camera on the frames the player first sees.
    #[test]
    fn an_unsized_window_is_not_a_zero_pixel_viewport() {
        assert_eq!(ViewFacts::rendered_height_px(None, None), 0.0);
        assert_eq!(
            ViewFacts::new(0.5, ViewFacts::rendered_height_px(None, None)).height_px,
            ViewFacts::default().height_px,
        );
    }

    /// THE DEAD BAND ON THE FIELD, tested where it lives: arithmetic, no world.
    ///
    /// A field nudge INSIDE [`FOV_HYSTERESIS`] leaves the facts alone — bevy retains a permanent
    /// range-table slot per distinct `VisibilityRange`, so a magnification slider that adopted every
    /// sub-threshold step would leak one slot per frame it was dragged.
    ///
    /// THE BAND DELAYS, IT DOES NOT DEADLOCK, and that is the half worth pinning. Every comparison
    /// is against the HELD value, never against the last request — so a slider dragged in
    /// sub-threshold steps accumulates against what is wired and lands the moment the total crosses
    /// the band. A dead band measured from the request instead would let a slow drag walk the view
    /// anywhere while the ladders stayed selected for the field the session started in.
    #[test]
    fn the_field_dead_band_delays_a_drag_and_never_deadlocks_it() {
        let commander = std::f32::consts::FRAC_PI_4;
        let held = ViewFacts::new(commander, 1440.0);
        // A resize is a human-rate move and carries no band at all.
        assert_eq!(held.settled(commander, 2160.0).height_px, 2160.0);
        // Half a band: the field is HELD at the wired value, and nothing else moves either — a
        // settling that returns SELF is what keeps the resource's change ticks at human rate.
        let half = held.settled(commander * (1.0 + FOV_HYSTERESIS * 0.5), 1440.0);
        assert_eq!(half, held, "inside the band the field is held, not adopted");
        // A second step of the same size, measured against the HELD value: the total is now past
        // the band, so it lands. The drag is delayed, not swallowed.
        let crossed = commander * (1.0 + FOV_HYSTERESIS * 1.025);
        assert_eq!(half.settled(crossed, 1440.0).vfov_rad, crossed);
        // The optic toggle is 6.5x and cannot be gated by any dead band worth having.
        assert_eq!(
            held.settled(crate::camera::GUNNER_FOV_FALLBACK, 1440.0)
                .vfov_rad,
            crate::camera::GUNNER_FOV_FALLBACK,
        );
        // An untrusted field inside the band is still refused: the fallback is the value settled
        // ON, not a reason to hold. (`NaN` fails the band comparison, so holding would be silent.)
        assert_eq!(
            held.settled(f32::NAN, 1440.0).vfov_rad,
            ViewFacts::default().vfov_rad,
        );
    }

    /// A crowd of cameras where ONE carries the declaration — the regression that started this.
    /// `single()` over `With<Camera3d>` fails on TWO exactly as it fails on zero, and a failed
    /// `single()` inside a system is a SILENT SKIP: every LOD ladder then held whatever it last
    /// selected, pointed the forbidden way (too coarse for the actual view), with nothing in the
    /// log. The sandboxes, which mount three 3-D cameras and two, never ran this reader at all.
    ///
    /// So: three 3-D cameras, one declaration, and the facts must be the DECLARED camera's — not
    /// the first, not the nearest, not the default the resource started at.
    #[test]
    fn the_declared_view_is_read_out_of_a_crowd_of_cameras() {
        let mut app = App::new();
        app.add_plugins(plugin);
        let world = app.world_mut();
        let window = world.spawn((Window::default(), PrimaryWindow)).id();
        world
            .get_mut::<Window>(window)
            .expect("the test window")
            .resolution
            .set_physical_resolution(2560, 1440);
        // The overlay and UI rigs a sandbox mounts: 3-D cameras, no player behind either.
        world.spawn(Camera3d::default());
        world.spawn(Camera3d::default());
        world.spawn((
            Camera3d::default(),
            Projection::Perspective(PerspectiveProjection {
                fov: std::f32::consts::FRAC_PI_4,
                ..default()
            }),
            PlayerView,
        ));
        app.update();
        let facts = *app.world().resource::<ViewFacts>();
        assert_eq!(
            facts.vfov_rad,
            std::f32::consts::FRAC_PI_4,
            "the declared view is the view, however many cameras stand beside it",
        );
        assert_eq!(facts.height_px, 1440.0);
    }

    /// NOT YET IS NOT AN ERROR. A composition spawns its camera in `Startup`; the frames before it
    /// are frames this reader is not scheduled in, so they are neither a skip nor a report — and
    /// the facts are still the conservative default every ladder starts against.
    #[test]
    fn a_composition_without_a_declared_view_is_scheduling_not_an_error() {
        let mut app = App::new();
        app.add_plugins(plugin);
        app.world_mut().spawn((Window::default(), PrimaryWindow));
        app.world_mut().spawn(Camera3d::default());
        app.update();
        assert_eq!(*app.world().resource::<ViewFacts>(), ViewFacts::default());
    }

    /// EVERY FAILURE IS A VALUE, and each one is producible. The mutant this catches is the
    /// original itself: a reader that answered any of these with `return` passed every test in this
    /// file, because a silent skip is indistinguishable from a frame where nothing moved.
    #[test]
    fn each_way_the_declaration_can_be_wrong_is_reported_rather_than_skipped() {
        use bevy::ecs::system::RunSystemOnce;

        let read = |spawn: &dyn Fn(&mut World)| -> Result<(), BevyError> {
            let mut app = App::new();
            app.add_plugins(plugin);
            let world = app.world_mut();
            world.spawn((Window::default(), PrimaryWindow));
            spawn(world);
            app.world_mut()
                .run_system_once(track_view_facts)
                .expect("the reader runs")
        };

        // A declaration on something that is not a camera: nothing to read.
        let err = read(&|world| {
            world.spawn(PlayerView);
        })
        .expect_err("a declaration with no camera under it is a bug, not a frame");
        assert!(
            err.to_string().contains("no player view"),
            "the report names the fault: {err}",
        );

        // Two declarations: a contradiction, and the count is the diagnosis.
        let err = read(&|world| {
            world.spawn((Camera3d::default(), PlayerView));
            world.spawn((Camera3d::default(), PlayerView));
        })
        .expect_err("two player views is a contradiction");
        assert!(
            err.to_string().contains("2 cameras declare"),
            "the report counts them: {err}",
        );

        // A projection with no perspective half-angle to project a metre through.
        let err = read(&|world| {
            world.spawn((
                Camera3d::default(),
                Projection::Orthographic(OrthographicProjection::default_3d()),
                PlayerView,
            ));
        })
        .expect_err("an orthographic player view has no field to select through");
        assert!(
            err.to_string().contains("non-perspective"),
            "the report names the fault: {err}",
        );

        // And the shape of the thing being pinned: the same three variants, as values.
        assert_eq!(ViewError::resolving(0), ViewError::NoPlayerView);
        assert_eq!(ViewError::resolving(3), ViewError::ManyPlayerViews(3));
    }

    /// THE REPORT IS LOUD IN A REAL SCHEDULE, not merely a value the caller could drop. bevy's
    /// fallback handler takes a `BevyError` at its default severity, which is `Panic`: the build
    /// that mounts a second player view dies on the frame it does, exactly as a missing model
    /// contract does (ADR-0011). The silently-skipping version this replaces `update()`d forever.
    #[test]
    #[should_panic(expected = "cameras declare `PlayerView`")]
    fn a_second_declaration_stops_the_app_rather_than_the_ladders() {
        let mut app = App::new();
        app.add_plugins(plugin);
        let world = app.world_mut();
        world.spawn((Window::default(), PrimaryWindow));
        world.spawn((Camera3d::default(), PlayerView));
        world.spawn((Camera3d::default(), PlayerView));
        app.update();
    }
}
