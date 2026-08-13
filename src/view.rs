//! THE LIVE VIEW, once: the facts every screen-space ladder selects through, and the projection
//! that turns a world-space deviation into metres of switch distance (ADR-0033 §9).
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

use bevy::prelude::*;

/// Relative field-of-view change that must accumulate before the facts move.
///
/// The optic toggle is a 6.5× jump (π/4 → 0.12 rad), so this never gates a real view change; what
/// it gates is a magnification slider being dragged, where a rewrite per frame would walk every
/// LOD entity on both ladders for a sub-pixel difference.
pub(crate) const FOV_HYSTERESIS: f32 = 0.10;

/// What the player's view IS, right now: the single 3-D camera's vertical field, and the pixel
/// height the main pass actually renders at.
///
/// There is exactly one 3-D camera in the game (`camera::spawn_camera`) and the gunner optic swaps
/// its `Projection` fov in place, so the live view is a single pair rather than a set — which is
/// what makes one shared resource the right shape and not an over-generalisation.
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

/// THE ONE READER of the live view. Moves [`ViewFacts`] only when the view actually moves, so a
/// consumer behind `resource_changed` rebuilds at human rate (module doc).
pub(crate) fn track_view_facts(
    camera: Query<&Projection, With<Camera3d>>,
    windows: Query<&Window>,
    scale: Option<Res<crate::render_scale::RenderScale>>,
    mut facts: ResMut<ViewFacts>,
) {
    let Ok(Projection::Perspective(projection)) = camera.single() else {
        return;
    };
    let Ok(window) = windows.single() else {
        return;
    };
    let wanted = ViewFacts::new(
        projection.fov,
        ViewFacts::rendered_height_px(Some(window), scale.as_deref()),
    );
    let field_moved = (wanted.vfov_rad - facts.vfov_rad).abs()
        > FOV_HYSTERESIS * facts.vfov_rad.max(f32::MIN_POSITIVE);
    if !field_moved && wanted.height_px == facts.height_px {
        return;
    }
    // Inside the dead band the field is HELD, not adopted: adopting it would let a slider creep
    // the value one sub-threshold step at a time and mint a range-table slot for each.
    *facts = ViewFacts {
        vfov_rad: if field_moved {
            wanted.vfov_rad
        } else {
            facts.vfov_rad
        },
        ..wanted
    };
    info!(
        "view: {:.4} rad × {:.0} px — every LOD ladder reselects",
        facts.vfov_rad, facts.height_px,
    );
}

/// Mount the live view. Every windowed composition needs it; a headless one has no view to read.
pub(crate) fn plugin(app: &mut App) {
    app.init_resource::<ViewFacts>()
        .add_systems(Update, track_view_facts);
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

    /// THE ONE READER, and the dead band on the field.
    ///
    /// A resize moves the facts on the frame it happens. A field nudge INSIDE [`FOV_HYSTERESIS`]
    /// leaves them alone: bevy retains a permanent range-table slot per distinct `VisibilityRange`,
    /// so a magnification slider that adopted every sub-threshold step would leak one slot per
    /// frame it was dragged.
    ///
    /// THE BAND DELAYS, IT DOES NOT DEADLOCK, and that is the half worth pinning. Every comparison
    /// is against the HELD value, never against the last frame's request — so a slider dragged in
    /// sub-threshold steps accumulates against what is wired and lands the moment the total crosses
    /// the band. A dead band measured from the request instead would let a slow drag walk the view
    /// anywhere while the ladders stayed selected for the field the session started in.
    #[test]
    fn the_facts_move_on_a_resize_and_hold_inside_the_field_dead_band() {
        let mut app = App::new();
        app.add_plugins(plugin);
        let world = app.world_mut();
        let window = world.spawn(Window::default()).id();
        let camera = world
            .spawn((
                Camera3d::default(),
                Projection::Perspective(PerspectiveProjection {
                    fov: std::f32::consts::FRAC_PI_4,
                    ..default()
                }),
            ))
            .id();
        app.update();
        app.world_mut()
            .get_mut::<Window>(window)
            .expect("the test window")
            .resolution
            .set_physical_resolution(2560, 1440);
        app.update();
        let facts = *app.world().resource::<ViewFacts>();
        assert_eq!(facts.height_px, 1440.0, "a resize is a human-rate move");
        assert_eq!(facts.vfov_rad, std::f32::consts::FRAC_PI_4);

        let nudge = |app: &mut App, fov: f32| {
            let mut lens = app
                .world_mut()
                .get_mut::<Projection>(camera)
                .expect("the test camera");
            let Projection::Perspective(projection) = lens.as_mut() else {
                panic!("the test camera must carry a perspective projection");
            };
            projection.fov = fov;
            app.update();
            app.world().resource::<ViewFacts>().vfov_rad
        };
        // Half a band: nothing moves, and the field is HELD at the wired value.
        let commander = std::f32::consts::FRAC_PI_4;
        assert_eq!(
            nudge(&mut app, commander * (1.0 + FOV_HYSTERESIS * 0.5)),
            commander,
        );
        // A second step of the same size. Measured against the HELD value the total is now past
        // the band, so it lands — the drag is delayed, not swallowed.
        let crossed = commander * (1.0 + FOV_HYSTERESIS * 1.025);
        assert_eq!(nudge(&mut app, crossed), crossed);
        // The optic toggle is 6.5× and cannot be gated by any dead band worth having.
        assert_eq!(
            nudge(&mut app, crate::camera::GUNNER_FOV_FALLBACK),
            crate::camera::GUNNER_FOV_FALLBACK,
        );
    }
}
