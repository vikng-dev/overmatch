//! THE LIVE VIEW, once: the facts every screen-space ladder selects through, and the projection
//! that turns a world-space deviation into metres of switch distance (ADR-0033 §9).
//!
//! # The view is DECLARED, not discovered
//!
//! [`PlayerView`] marks the camera the player looks through, and every reader of "the view" filters
//! on it. Nothing infers it from the shape of the world: `single()` over `With<Camera3d>` answers
//! "is there exactly one 3-D camera", a fact about the archetypes, and reading a domain fact out of
//! it holds only while the game happens to spawn one camera. A mirror, a spotter, an overlay or a
//! debug camera then makes the answer WRONG — and both shapes that used to ask it answered wrong
//! QUIETLY. A `Single<…, With<Camera3d>>` parameter fails its validation on two matches exactly as
//! on zero, and bevy SKIPS a system whose parameters do not validate: no log, no panic (that was
//! `track::link_view`'s selector). A `Query::single()` returns a `Result` and skips nothing — the
//! skip was the reader's own `let Ok(…) = … else { return }` (that was this module's). Either way
//! the ladder FREEZES at the value it last selected, and a freeze has NO DIRECTION: it is whatever
//! the view happened to be when the reader stopped running, held against whatever the view does
//! next. Under the declaration the ambiguity cannot be expressed: a second camera is not a player
//! view unless someone declares it one, and two declarations are a contradiction the reader refuses
//! out loud ([`ViewError`]).
//!
//! # There is no default view
//!
//! The freeze was only half of that fault. The other half was that a PLAUSIBLE WRONG NUMBER was
//! sitting in the resource to freeze on: [`ViewFacts`] had a `Default` — the narrow optic at 1080 px
//! — so a composition whose reader never ran still handed every ladder a view, and every ladder
//! selected through it without knowing it was a guess.
//!
//! So there is no way to construct a view nobody measured. `ViewFacts` has no `Default`, the
//! resource is ABSENT until the declared view has been read once, and [`ViewFacts::new`] answers
//! `None` for inputs that are not measurements (an unsized window, a `NaN` field). Every consumer
//! therefore spells the absence — `Option<Res<…>>` or a `resource_exists` gate — and answers it by
//! DEFERRING: `geometry_lod` does not bind a chain, `track::link_view` does not bind the shoe
//! template, `terrain_lod` spawns its tiles at their FINEST level and rewrites on the first frame
//! with facts. None of them substitutes a number of its own; that would be this bug again, one
//! module further down.
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
/// DECLARING IT MAKES A CAMERA. `#[require(Camera3d)]`, so "a `PlayerView` on something that is not
/// a camera" is not a fault a reader has to report — it is a state nobody can build. `Camera3d`
/// pulls the `Projection` this module reads and (through `Camera`) the `Transform`/`GlobalTransform`
/// `track::link_view` measures from, which is what makes a count of the rows either reader matches
/// the same number as a count of the declarations (see [`ViewError::resolving`]).
///
/// EXACTLY ONE per app — the one thing about the declaration that is NOT expressible in the type
/// system, because cardinality is a property of the world and not of an entity. Zero is a
/// composition that has not declared its view, and has no facts to read rather than wrong ones; two
/// is a contradiction, and the readers refuse it out loud ([`ViewError`]).
#[derive(Component, Clone, Copy, PartialEq, Eq, Debug, Default)]
#[require(Camera3d)]
pub(crate) struct PlayerView;

/// What can be wrong with the declaration — the value form of every failure the readers used to
/// express as a silent `return`.
///
/// What is left of it, that is: "the marker is on something that is not a camera" is not here,
/// because [`PlayerView`] requires `Camera3d` and that state cannot be built. These two are the
/// CARDINALITY of the declaration, which no component requirement can constrain, and one value the
/// declared camera can legitimately carry and no ladder can select through.
///
/// None of them is a transient and none of them is "not yet": a reader is SCHEDULED behind
/// `any_with_component::<PlayerView>` (see [`plugin`]), so startup's genuine absence never reaches
/// one. Reaching one means the declaration itself is wrong, which is a bug in a composition rather
/// than a condition of a frame — so they carry bevy's default `Severity::Panic` and stop the app on
/// the frame they are reached, exactly as a missing model contract does (ADR-0011). A ladder that
/// quietly held its last value is what this replaces.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum ViewError {
    /// Nothing declares the view: no entity carries [`PlayerView`] at all.
    NoPlayerView,
    /// More than one entity declares itself the player's view. The count is the diagnosis: it says
    /// how many, so the composition that added the second one is findable.
    ManyPlayerViews(usize),
    /// The player's view is not a perspective one. Every screen-space ladder projects a metre
    /// through a perspective half-angle ([`ViewFacts::sub_pixel_distance_m`]); an orthographic or
    /// custom projection has no such angle and no distance to select at.
    UnsupportedProjection,
}

impl ViewError {
    /// The domain fact behind a declaration that did not resolve to exactly one, `declarations`
    /// being how many rows matched. THE ONE translation from query shape to domain in the tree.
    ///
    /// The rows ARE the declarations, in both readers: this one filters `With<PlayerView>` for a
    /// `Projection` and `track::link_view`'s selector for a `GlobalTransform`, and the marker
    /// requires the `Camera3d` that carries both — so neither can count a subset of the other's
    /// and report a number that is not the number of declarations.
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
///
/// NO `Default`, and as a RESOURCE it is absent until [`track_view_facts`] has read the declared
/// view once (module doc). Every value of this type is a pair of measurements; there is no value of
/// it meaning "nobody has looked yet", because that state belongs to the resource's absence and a
/// consumer that can read a guess will.
#[derive(Resource, Clone, Copy, PartialEq, Debug)]
pub(crate) struct ViewFacts {
    /// Vertical field of view, radians.
    pub(crate) vfov_rad: f32,
    /// Rendered height of the main pass, pixels (window physical height × render scale).
    pub(crate) height_px: f32,
}

impl ViewFacts {
    /// Live facts, or NONE — both inputs are UNTRUSTED, and inputs that are not measurements do not
    /// make a view.
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
    /// than an authored view (a debug tool, a half-initialised projection) cannot reach a ladder.
    ///
    /// Answering `None` rather than substituting is the whole point: the substitute WAS a view, it
    /// looked exactly like a measured one, and every consumer selected through it. The caller's two
    /// honest answers are "hold what is already wired" ([`Self::settled`]) and "there are no facts
    /// yet" (the resource stays absent) — and neither is a number this module invented.
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
    /// measured against the HELD field, so sub-threshold steps accumulate and land the moment their
    /// total crosses it: the band DELAYS, it does not deadlock.
    ///
    /// A REQUEST THAT IS NOT A VIEW ([`Self::new`]) settles onto nothing: what is wired stays, which
    /// is the last thing actually measured rather than a constant standing in for one.
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
///
/// It inherits [`ViewFacts`]' absence: no `Default`, and `geometry_lod`'s resource does not exist
/// until there are facts to compose it from. A ladder gated on it is a ladder that has been told
/// what the view is.
#[derive(Resource, Clone, Copy, PartialEq, Debug)]
pub(crate) struct ViewProfile {
    /// The live view, shared.
    pub(crate) facts: ViewFacts,
    /// Screen-space error budget, pixels: the on-screen size a deviation is allowed to project to
    /// before the next-finer level must take over.
    pub(crate) budget_px: f32,
}

/// The budget a ladder spends when the player's setting is not there to read, and the one the
/// shipped corpus was cut against (`scripts/lod/config.py::REFERENCE_VIEW`): one pixel of
/// screen-space error. An AUTHORED default for a tuning knob — not a stand-in for a measurement,
/// which is why this constant exists and [`ViewFacts`] has none.
pub(crate) const DEFAULT_BUDGET_PX: f32 = 1.0;

impl ViewProfile {
    /// The shared facts, spent at `budget_px`. A non-positive or non-finite budget collapses every
    /// distance onto the bounding radius, so it falls back to [`DEFAULT_BUDGET_PX`].
    pub(crate) fn of(facts: ViewFacts, budget_px: f32) -> Self {
        Self {
            facts,
            budget_px: if budget_px.is_finite() && budget_px > 0.0 {
                budget_px
            } else {
                DEFAULT_BUDGET_PX
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
/// for the same reason the camera is the declared view. An absent one is not an error and not a new
/// fact either: it is an absent HEIGHT, so a session that has facts HOLDS them and a session that
/// has none stays without.
///
/// IT IS ALSO THE ONE WRITER — the resource does not exist until this system inserts it, and after
/// that nothing else touches it.
pub(crate) fn track_view_facts(
    mut commands: Commands,
    views: Query<&Projection, With<PlayerView>>,
    window: Query<&Window, With<PrimaryWindow>>,
    scale: Option<Res<crate::render_scale::RenderScale>>,
    facts: Option<ResMut<ViewFacts>>,
) -> Result<(), BevyError> {
    // `single()` answers a QUERY-SHAPE question ("is there exactly one row?"); the count restates
    // its failure as the domain fact this reader actually needs.
    let Ok(projection) = views.single() else {
        return Err(ViewError::resolving(views.iter().count()).into());
    };
    let Projection::Perspective(projection) = projection else {
        return Err(ViewError::UnsupportedProjection.into());
    };
    let measured = ViewFacts::rendered_height_px(window.single().ok(), scale.as_deref());
    let held = facts.as_deref().copied();
    let wanted = match held {
        // AN ABSENT WINDOW IS AN ABSENT HEIGHT, AND AN ABSENT HEIGHT IS HELD. Zero primary windows
        // (a surface torn down at exit under `ExitCondition::DontExit`) or an unsized one reads as
        // `0.0`, and settling on any stand-in for it would overwrite a measured 1440 or 2160 — every
        // switch distance short by the ratio, coarse geometry NEARER the eye than its certificate,
        // the one direction LOD must not fail in.
        Some(held) => held.settled(
            projection.fov,
            if measured > 0.0 {
                measured
            } else {
                held.height_px
            },
        ),
        // THE FIRST READ has nothing to hold and nothing to guess with: an unsized window or a
        // hostile projection simply leaves the session without facts for another frame, and every
        // consumer defers rather than binding to a number nobody measured.
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

/// Mount the live view. Every windowed composition needs it; a headless one has no view to read.
///
/// The reader is GATED ON THE DECLARATION existing, which is how "not yet" is expressed: a
/// composition's camera spawns in `Startup` and the frames before it are simply frames this system
/// is not in. What is left inside it is genuinely a bug, and says so.
///
/// The gate is SCHEDULING, not correctness. A composition that never declares a view at all is
/// indistinguishable, to this gate, from one whose camera has not landed yet — which is exactly why
/// [`ViewFacts`] has no `Default` and no `init_resource` here: the reader that never runs leaves the
/// resource ABSENT, and there is no plausible wrong view for a ladder to select through while
/// nobody notices.
pub(crate) fn plugin(app: &mut App) {
    app.add_systems(
        Update,
        track_view_facts.run_if(any_with_component::<PlayerView>),
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A MEASURED view — the only kind there is. Every fixture here states real numbers, so an
    /// `expect` is the honest reading of [`ViewFacts::new`] rather than a fallback in disguise.
    fn measured(vfov_rad: f32, height_px: f32) -> ViewFacts {
        ViewFacts::new(vfov_rad, height_px).expect("a measured view")
    }

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

    /// AN INPUT THAT IS NOT A MEASUREMENT IS NOT A VIEW. It used to be one — the fallback made a
    /// `NaN` field into the narrow optic and an unsized window into 1080 px, and the result was
    /// indistinguishable from a view somebody actually looked through. There is no such value now:
    /// the constructor answers `None` and the caller says what it does about that.
    ///
    /// The BUDGET is the one thing that still falls back, and it is not of the same kind: a pixel
    /// budget is an authored knob with a documented default ([`DEFAULT_BUDGET_PX`]), not a claim
    /// about the player's screen.
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
        for budget in [-1.0, 0.0, f32::NAN, f32::INFINITY] {
            let view = ViewProfile::of(measured(0.12, 1440.0), budget);
            assert_eq!(view.budget_px, DEFAULT_BUDGET_PX);
            assert!(view.switch_distance_m(0.01, 0.5).is_finite());
        }
        // A zero-deviation level is the source surface: it owns the camera, not the radius alone.
        assert_eq!(measured(0.12, 1080.0).sub_pixel_distance_m(0.0, 1.0), 0.0);
    }

    /// AN UNSIZED WINDOW IS AN ABSENT ONE. bevy reports zero physical height before it has sized
    /// the surface, and a zero render scale says the same thing; read literally either collapses
    /// every switch distance onto the bounding radius and puts the COARSEST level of every ladder a
    /// bounding radius from the camera on the frames the player first sees.
    #[test]
    fn an_unsized_window_is_not_a_zero_pixel_viewport() {
        assert_eq!(ViewFacts::rendered_height_px(None, None), 0.0);
        assert!(ViewFacts::new(0.5, ViewFacts::rendered_height_px(None, None)).is_none());
    }

    /// THE DEAD BAND ON THE FIELD, tested where it lives: arithmetic, no world.
    ///
    /// A field nudge INSIDE [`FOV_HYSTERESIS`] leaves the facts alone — bevy retains a permanent
    /// range-table slot per distinct `VisibilityRange`, so a magnification slider that adopted every
    /// sub-threshold step would leak one slot per frame it was dragged.
    ///
    /// THE BAND DELAYS, IT DOES NOT DEADLOCK, and that is the half worth pinning: the comparison is
    /// against the HELD value, so a slider dragged in sub-threshold steps ACCUMULATES against what
    /// is wired and lands the moment the total crosses the band.
    ///
    /// What this cannot pin is the alternative — a band measured from the LAST REQUEST, which would
    /// let a slow drag walk the view anywhere while the ladders stayed at the field the session
    /// started in. [`ViewFacts::settled`] is handed a held value and a request and nothing else, so
    /// there is no last request for a mutant to measure from; that property belongs to the
    /// SIGNATURE, and the accumulation below is what remains to assert.
    #[test]
    fn the_field_dead_band_delays_a_drag_and_never_deadlocks_it() {
        let commander = std::f32::consts::FRAC_PI_4;
        let held = measured(commander, 1440.0);
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
        // A request that is not a view settles onto nothing: what is WIRED stays, and what is wired
        // is the last field actually measured rather than a constant standing in for one.
        assert_eq!(held.settled(f32::NAN, 1440.0), held);
        assert_eq!(held.settled(commander, 0.0), held);
    }

    /// A perspective projection at `fov`, the only thing the test cameras below differ in.
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

    /// The live facts, or their absence — the whole of what a consumer can see.
    fn facts_of(app: &App) -> Option<ViewFacts> {
        app.world().get_resource::<ViewFacts>().copied()
    }

    /// DECLARING THE VIEW MAKES A CAMERA. `PlayerView` requires `Camera3d`, so "the marker is on
    /// something that is not a camera" is a state nobody can build — which is what lets both
    /// readers count their own filtered rows and mean the number of DECLARATIONS by it.
    ///
    /// The mutant is `#[require(Camera3d)]` deleted: a bare marker then resolves as a declaration
    /// this module cannot read a field off, while `track::link_view`'s selector reads the OTHER
    /// declaration's pose, and the report says "2 cameras" about one camera and one marker.
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

    /// A crowd of cameras where ONE carries the declaration — the regression that started this.
    /// `single()` over `With<Camera3d>` fails on TWO exactly as it fails on zero, and the reader
    /// answered that failure with its own `let … else { return }`: every LOD ladder then held
    /// whatever it last selected, with nothing in the log. The sandboxes, which mount three 3-D
    /// cameras and two, never ran this reader at all — so they held the old default's 0.12 rad, the
    /// narrowest field and therefore the FINEST geometry, over-detailed rather than under-detailed.
    /// That direction was the accident of which value the freeze caught, not a property of freezing.
    ///
    /// So: three 3-D cameras, one declaration, and the facts must be the DECLARED camera's — not
    /// the first, not the nearest. The decoys carry a field of their own, distinct from the declared
    /// one and from `Camera3d`'s required π/4, or a reader taking the first row would answer right
    /// by coincidence and this would pin nothing.
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

    /// THE READER SETTLES AGAINST WHAT IS WIRED, across frames, with a live camera and a live
    /// window — the half [`ViewFacts::settled`]'s arithmetic cannot pin on its own.
    ///
    /// The mutant is the first-read branch taken every frame (`ViewFacts::new(fov, height)` in
    /// place of `held.settled(…)`), which is what the deleted `ViewFacts::default()` used to be a
    /// second version of: it drops the dead band entirely and rewrites the resource on every frame
    /// of a magnification drag — the permanent `VisibilityRange` slot per distinct value that
    /// ADR-0033 §11 forbids.
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

    /// A WINDOW THAT GOES AWAY IS AN ABSENT HEIGHT, AND AN ABSENT HEIGHT IS HELD.
    ///
    /// The client runs under `ExitCondition::DontExit`, so the surface can be despawned with the
    /// app still updating. The mutant takes [`ViewFacts::rendered_height_px`]'s `0.0` as a height —
    /// under the deleted `Default` it wrote 1080 over a settled 1440, every switch distance a third
    /// short, coarse geometry nearer the eye than its certificate.
    ///
    /// HELD, NOT FROZEN: the second mutant hands the `0.0` straight to
    /// [`ViewFacts::settled`], whose own guard bounces the whole call and takes the FIELD down with
    /// the height. The optic toggle below still has to land.
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

    /// NOT YET IS NOT AN ERROR, and NEVER IS NOT A WRONG NUMBER. A composition spawns its camera in
    /// `Startup`; the frames before it are frames this reader is not scheduled in, so they are
    /// neither a skip nor a report — and a composition that never declares a view at all reaches
    /// the same silence, which is safe only because there is nothing to read while it lasts.
    ///
    /// Two mutants. The run condition deleted: the reader runs with no declaration and REPORTS,
    /// which panics through bevy's fallback handler. And an `init_resource::<ViewFacts>()` back in
    /// [`plugin`]: the facts exist without anyone having looked, and every ladder selects through a
    /// number the app invented — the bug the whole no-`Default` shape removes.
    #[test]
    fn a_composition_with_no_declared_view_has_no_facts_rather_than_invented_ones() {
        let mut app = App::new();
        app.add_plugins(plugin);
        app.world_mut().spawn((Window::default(), PrimaryWindow));
        app.world_mut().spawn(Camera3d::default());
        app.update();
        assert_eq!(facts_of(&app), None);
    }

    /// AN UNSIZED WINDOW ON THE FIRST FRAME leaves the session without facts, rather than with a
    /// guess: bevy reports zero physical height until it has sized the surface, and there is
    /// nothing wired yet to hold. The ladders bind on the frame the window is real.
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

    /// EVERY FAILURE IS A VALUE, and each one is producible. The mutant this catches is the
    /// original itself: a reader that answered either of these with `return` passed every test in
    /// this file, because a silent skip is indistinguishable from a frame where nothing moved. The
    /// second mutant is a reader that reports AND writes: the window here is 1440 px, so any write
    /// at all leaves facts behind, and each case asserts none were.
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

    /// FACTS ALREADY SETTLED SURVIVE A DECLARATION THAT BREAKS UNDER THEM. The case above starts
    /// from nothing, so it cannot tell "wrote nothing" from "wrote and then removed"; this one
    /// settles 0.5 rad × 1440 px first and then breaks the declaration two ways.
    ///
    /// The mutant is a reader that clears or rewrites the resource before it reports — the ladders
    /// would then reselect against a value the report says is unreadable, which is the freeze back
    /// with an extra step.
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

    /// THE REPORT IS LOUD IN A REAL SCHEDULE, not merely a value the caller could drop. bevy's
    /// fallback handler takes a `BevyError` at its default severity, which is `Panic`: the build
    /// that mounts a second player view dies on the frame it does, exactly as a missing model
    /// contract does (ADR-0011). The silently-skipping version this replaces `update()`d forever.
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
