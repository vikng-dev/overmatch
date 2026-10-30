//! Graphics presets: the one table of tier → values, and the PRESET row's label derived from it.
//!
//! **A preset is never stored.** [`Preset::of`] names the tier whose table equals the current values
//! of the rows it drives, or `None` — the row's CUSTOM. [`Preset::apply`] writes every one of those
//! rows at once, into [`Settings`], so choosing a tier goes through the same single writer as any
//! other row (`apply_settings`) and is saved the same way. Only the rows reach `video.ron`, so a
//! saved preset cannot disagree with its rows, and editing any driven row reads CUSTOM by itself.
//!
//! **Scope: [`Look`] is the whole of what a preset drives** — the rows that trade frame time for
//! picture. Window mode, display, vsync, frame cap, UI scale and every GAMEPLAY row are not fields
//! of it, so [`Preset::apply`] has no way to write them.
//!
//! **The values are provisional** (owner, #159): each tier is to become a hardware promise ("a PC
//! of class X runs the 30-tank battle at 120 fps on preset Y") and is retuned against the night
//! battery. Retuning is an edit to [`Preset::look`] alone; the tests pin class invariants
//! only — every value on its row's ladder, every row non-decreasing in cost from LOW to ULTRA, the
//! tiers pairwise distinct — never a tier's particular values.

use super::{
    BloomLevel, MsaaLevel, PixelBudget, RenderScaleLevel, Settings, ShadowCascades, ShadowDistance,
    ShadowResolution, TrackPrintRange, TrackPrintRelief,
};

/// A named graphics tier, cheapest first.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Preset {
    Low,
    Medium,
    High,
    Ultra,
}

/// The rows a preset drives, one field each.
#[derive(Clone, Copy, PartialEq, Debug)]
pub(crate) struct Look {
    pub(crate) render_scale: RenderScaleLevel,
    pub(crate) lod_pixel_budget: PixelBudget,
    pub(crate) shadow_distance: ShadowDistance,
    pub(crate) shadow_resolution: ShadowResolution,
    pub(crate) shadow_cascades: ShadowCascades,
    pub(crate) msaa: MsaaLevel,
    pub(crate) bloom: BloomLevel,
    pub(crate) track_prints: TrackPrintRelief,
    pub(crate) track_print_range: TrackPrintRange,
}

/// How many rows [`Look`] drives: the length of [`Look::ranks`].
pub(crate) const DRIVEN_ROWS: usize = 9;

impl Look {
    /// The driven rows' current values.
    pub(crate) fn of(settings: &Settings) -> Self {
        Self {
            render_scale: settings.render_scale,
            lod_pixel_budget: settings.lod_pixel_budget,
            shadow_distance: settings.shadow_distance,
            shadow_resolution: settings.shadow_resolution,
            shadow_cascades: settings.shadow_cascades,
            msaa: settings.msaa,
            bloom: settings.bloom,
            track_prints: settings.track_prints,
            track_print_range: settings.track_print_range,
        }
    }

    /// Write every driven row, and nothing else.
    fn write(self, settings: &mut Settings) {
        settings.render_scale = self.render_scale;
        settings.lod_pixel_budget = self.lod_pixel_budget;
        settings.shadow_distance = self.shadow_distance;
        settings.shadow_resolution = self.shadow_resolution;
        settings.shadow_cascades = self.shadow_cascades;
        settings.msaa = self.msaa;
        settings.bloom = self.bloom;
        settings.track_prints = self.track_prints;
        settings.track_print_range = self.track_print_range;
    }

    /// Each driven row's rung, counted from the row's cheapest: the position in its ladder's
    /// `ORDER` (every one of those ascends in cost), and for the detail budget the slider stop
    /// counted from the top, because a LARGER pixel budget is the cheaper one. A value off its
    /// ladder (a hand-edited budget) counts as the stop the slider would show for it.
    pub(crate) fn ranks(self) -> [usize; DRIVEN_ROWS] {
        [
            rank(&RenderScaleLevel::ORDER, self.render_scale),
            usize::from(PixelBudget::STOPS - 1 - self.lod_pixel_budget.stop()),
            rank(&ShadowDistance::ORDER, self.shadow_distance),
            rank(&ShadowResolution::ORDER, self.shadow_resolution),
            rank(&ShadowCascades::ORDER, self.shadow_cascades),
            rank(&MsaaLevel::ORDER, self.msaa),
            rank(&BloomLevel::ORDER, self.bloom),
            rank(&TrackPrintRelief::ORDER, self.track_prints),
            rank(&TrackPrintRange::ORDER, self.track_print_range),
        ]
    }

    /// How many rungs apart two looks are, summed over the driven rows.
    pub(crate) fn distance(self, other: Self) -> usize {
        self.ranks()
            .into_iter()
            .zip(other.ranks())
            .map(|(a, b)| a.abs_diff(b))
            .sum()
    }
}

/// `value`'s position in `order`; every ladder lists every value it can hold, so the fallback is
/// unreachable from a parsed file.
fn rank<T: PartialEq>(order: &[T], value: T) -> usize {
    order
        .iter()
        .position(|entry| *entry == value)
        .unwrap_or_default()
}

impl Preset {
    pub(crate) const ORDER: [Preset; 4] =
        [Preset::Low, Preset::Medium, Preset::High, Preset::Ultra];

    /// ASCII only — it reaches `Text`.
    pub(crate) const fn label(self) -> &'static str {
        match self {
            Preset::Low => "LOW",
            Preset::Medium => "MEDIUM",
            Preset::High => "HIGH",
            Preset::Ultra => "ULTRA",
        }
    }

    /// **The table.** The owner's provisional values on #159, except the detail budget, which that
    /// table does not list: LOW's 2 px is the issue body's ASSUMED value, and the other tiers keep
    /// the 1 px default (ASSUMED).
    pub(crate) const fn look(self) -> Look {
        match self {
            Preset::Low => Look {
                render_scale: RenderScaleLevel::Percent75,
                lod_pixel_budget: PixelBudget(2.0),
                shadow_distance: ShadowDistance::M350,
                shadow_resolution: ShadowResolution::X2048,
                shadow_cascades: ShadowCascades::Two,
                msaa: MsaaLevel::Off,
                bloom: BloomLevel::Low,
                track_prints: TrackPrintRelief::Lighting,
                track_print_range: TrackPrintRange::Middle,
            },
            Preset::Medium => Look {
                render_scale: RenderScaleLevel::Percent100,
                lod_pixel_budget: PixelBudget(1.0),
                shadow_distance: ShadowDistance::M350,
                shadow_resolution: ShadowResolution::X2048,
                shadow_cascades: ShadowCascades::Three,
                msaa: MsaaLevel::X2,
                bloom: BloomLevel::Normal,
                track_prints: TrackPrintRelief::Parallax,
                track_print_range: TrackPrintRange::Far,
            },
            Preset::High => Look {
                render_scale: RenderScaleLevel::Percent100,
                lod_pixel_budget: PixelBudget(1.0),
                shadow_distance: ShadowDistance::M350,
                shadow_resolution: ShadowResolution::X4096,
                shadow_cascades: ShadowCascades::Three,
                msaa: MsaaLevel::X4,
                bloom: BloomLevel::Normal,
                track_prints: TrackPrintRelief::SunShadow,
                track_print_range: TrackPrintRange::Far,
            },
            Preset::Ultra => Look {
                render_scale: RenderScaleLevel::Percent100,
                lod_pixel_budget: PixelBudget(1.0),
                shadow_distance: ShadowDistance::M700,
                shadow_resolution: ShadowResolution::X4096,
                shadow_cascades: ShadowCascades::Four,
                msaa: MsaaLevel::X4,
                bloom: BloomLevel::Normal,
                track_prints: TrackPrintRelief::SunShadow,
                track_print_range: TrackPrintRange::Farthest,
            },
        }
    }

    /// The tier whose table equals `settings`' driven rows exactly, or `None` — CUSTOM.
    pub(crate) fn of(settings: &Settings) -> Option<Preset> {
        let look = Look::of(settings);
        Self::ORDER.into_iter().find(|tier| tier.look() == look)
    }

    /// Write this tier's every row into `settings`, and nothing else.
    pub(crate) fn apply(self, settings: &mut Settings) {
        self.look().write(settings);
    }

    /// The PRESET row's step. On a tier it moves `delta` tiers, saturating at both ends like every
    /// row. On CUSTOM either arrow lands on the NEAREST tier — fewest rungs moved over the driven
    /// rows ([`Look::distance`]), so the first press changes as little as it can — and a tie goes
    /// the arrow's way (right to the dearer tier, left to the cheaper).
    pub(crate) fn step(settings: &mut Settings, delta: i32) {
        let tier = match Self::of(settings) {
            Some(tier) => step_tier(tier, delta),
            None => Self::nearest(Look::of(settings), delta),
        };
        tier.apply(settings);
    }

    /// The tier nearest `look`, a tie broken toward `delta`'s direction.
    pub(crate) fn nearest(look: Look, delta: i32) -> Preset {
        let toward = |index: usize| {
            if delta > 0 {
                Self::ORDER.len() - index
            } else {
                index
            }
        };
        Self::ORDER
            .into_iter()
            .enumerate()
            .min_by_key(|&(index, tier)| (tier.look().distance(look), toward(index)))
            .map_or(Preset::Low, |(_, tier)| tier)
    }
}

/// `tier` moved `delta` places in [`Preset::ORDER`], clamped to the ends.
fn step_tier(tier: Preset, delta: i32) -> Preset {
    let index = rank(&Preset::ORDER, tier) as i32;
    let last = Preset::ORDER.len() as i32 - 1;
    Preset::ORDER[(index + delta).clamp(0, last) as usize]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::{
        DebugPanel, DisplaySelection, FrameCap, KillCam, PenCamCorner, ReduceMotion,
        UiScalePercent, VsyncMode, WindowModeSetting,
    };

    /// Settings with every row a preset must NOT drive moved off its default, so a write to any of
    /// them shows.
    fn every_undriven_row_off_default() -> Settings {
        let settings = Settings {
            vsync: VsyncMode::Off,
            window_mode: WindowModeSetting::Fullscreen,
            display: DisplaySelection::Display2,
            frame_cap: FrameCap(144),
            ui_scale: UiScalePercent(125),
            debug_panel: DebugPanel::Off,
            pen_cam_corner: PenCamCorner::ThroughOnly,
            kill_cam: KillCam::Off,
            reduce_motion: ReduceMotion::On,
            ..Settings::default()
        };
        assert_eq!(
            Look::of(&settings),
            Look::of(&Settings::default()),
            "the premise: only undriven rows moved",
        );
        settings
    }

    /// **Every tier's every value is a rung its row can show.** The PRESET row writes these into
    /// rows the player then walks with the arrows, and a value off its ladder is one the row can
    /// render but never step back to (`ui::step_in` resolves it to the bottom rung, and the budget
    /// slider snaps it on first touch).
    #[test]
    fn every_tier_value_is_on_its_rows_ladder() {
        for tier in Preset::ORDER {
            let look = tier.look();
            assert!(
                RenderScaleLevel::ORDER.contains(&look.render_scale),
                "{tier:?}"
            );
            assert_eq!(
                look.lod_pixel_budget.step(0),
                look.lod_pixel_budget,
                "{tier:?}: the detail budget must sit on a slider stop",
            );
            assert!(
                ShadowDistance::ORDER.contains(&look.shadow_distance),
                "{tier:?}"
            );
            assert!(
                ShadowResolution::ORDER.contains(&look.shadow_resolution),
                "{tier:?}"
            );
            assert!(
                ShadowCascades::ORDER.contains(&look.shadow_cascades),
                "{tier:?}"
            );
            assert!(MsaaLevel::ORDER.contains(&look.msaa), "{tier:?}");
            assert!(BloomLevel::ORDER.contains(&look.bloom), "{tier:?}");
            assert!(
                TrackPrintRelief::ORDER.contains(&look.track_prints),
                "{tier:?}"
            );
            assert!(
                TrackPrintRange::ORDER.contains(&look.track_print_range),
                "{tier:?}"
            );
        }
    }

    /// **Every driven row is non-decreasing in cost from LOW to ULTRA** (owner, #159): a dearer
    /// tier never buys a cheaper rung of any row, so stepping the PRESET row right never trades one
    /// row's look away for another's. And the tiers are pairwise distinct, or two tiers would claim
    /// the same values and the label would name whichever is listed first.
    #[test]
    fn every_driven_row_is_non_decreasing_from_low_to_ultra() {
        for pair in Preset::ORDER.windows(2) {
            let (cheaper, dearer) = (pair[0].look().ranks(), pair[1].look().ranks());
            for (row, (a, b)) in cheaper.iter().zip(dearer).enumerate() {
                assert!(
                    *a <= b,
                    "{:?} -> {:?}: driven row {row} drops from rung {a} to {b}",
                    pair[0],
                    pair[1],
                );
            }
        }
        for (i, a) in Preset::ORDER.iter().enumerate() {
            for b in &Preset::ORDER[i + 1..] {
                assert_ne!(a.look(), b.look(), "{a:?} and {b:?} share one table");
            }
        }
    }

    /// **The label is derived: the tier whose table matches exactly, else CUSTOM.** Applying a tier
    /// reads that tier whatever the undriven rows hold, and a value between two rungs matches no
    /// table. The one-rung edit of every row, through the row itself, is
    /// `ui::tests::editing_a_driven_row_reads_custom_and_any_other_row_keeps_the_tier`.
    #[test]
    fn the_label_is_the_matching_tier_else_custom() {
        for base in [Settings::default(), every_undriven_row_off_default()] {
            for tier in Preset::ORDER {
                let mut settings = base;
                tier.apply(&mut settings);
                assert_eq!(Preset::of(&settings), Some(tier));
            }
        }
        // A hand-edited budget between two stops matches no table, however close.
        let mut settings = Settings::default();
        Preset::High.apply(&mut settings);
        settings.lod_pixel_budget = PixelBudget(1.2);
        assert_eq!(Preset::of(&settings), None, "an off-ladder value is CUSTOM");
    }

    /// **Choosing a tier never changes a row it does not drive.** Restoring the driven rows after
    /// the apply must give back the starting settings exactly — fail-closed, so a field added to
    /// [`Settings`] later that `apply` touched would fail here without being listed.
    #[test]
    fn choosing_a_tier_never_changes_an_undriven_row() {
        let base = every_undriven_row_off_default();
        for tier in Preset::ORDER {
            let mut settings = base;
            tier.apply(&mut settings);
            Look::of(&base).write(&mut settings);
            assert_eq!(settings, base, "{tier:?} wrote a row it does not drive");
        }
    }

    /// The PRESET row's walk: tier to tier, saturating at both ends, and from CUSTOM onto the
    /// nearest tier — the one a single deviation came from, whichever way the arrow points, when
    /// that tier is strictly nearest.
    #[test]
    fn stepping_walks_the_tiers_and_custom_lands_on_the_nearest() {
        let mut settings = Settings::default();
        Preset::Low.apply(&mut settings);
        Preset::step(&mut settings, -1);
        assert_eq!(
            Preset::of(&settings),
            Some(Preset::Low),
            "the floor saturates"
        );
        for want in &Preset::ORDER[1..] {
            Preset::step(&mut settings, 1);
            assert_eq!(Preset::of(&settings), Some(*want));
        }
        Preset::step(&mut settings, 1);
        assert_eq!(
            Preset::of(&settings),
            Some(Preset::Ultra),
            "the top saturates"
        );

        for tier in Preset::ORDER {
            let mut custom = Settings::default();
            tier.apply(&mut custom);
            // One rung off on the render-scale row, toward whichever end has room.
            let ladder = RenderScaleLevel::ORDER;
            let at = rank(&ladder, custom.render_scale);
            custom.render_scale = ladder[if at + 1 < ladder.len() {
                at + 1
            } else {
                at - 1
            }];
            if Preset::of(&custom).is_some() {
                continue; // The deviation landed on another tier's table: not CUSTOM.
            }
            let look = Look::of(&custom);
            let distance = tier.look().distance(look);
            let strictly_nearest = Preset::ORDER
                .into_iter()
                .filter(|other| *other != tier)
                .all(|other| other.look().distance(look) > distance);
            for delta in [-1, 1] {
                let mut stepped = custom;
                Preset::step(&mut stepped, delta);
                let landed = Preset::of(&stepped);
                assert!(
                    landed.is_some(),
                    "a step from CUSTOM must land on a named tier"
                );
                if strictly_nearest {
                    assert_eq!(landed, Some(tier), "{delta}: one rung off {tier:?}");
                }
            }
        }
    }

    /// The rung distance the CUSTOM step minimises: zero to itself, one per rung on any one row.
    #[test]
    fn the_distance_counts_rungs() {
        let high = Preset::High.look();
        assert_eq!(high.distance(high), 0);
        let one_off = Look {
            msaa: MsaaLevel::X2,
            ..high
        };
        assert_eq!(high.distance(one_off), 1);
        let budget_off = Look {
            lod_pixel_budget: high.lod_pixel_budget.step(1),
            ..high
        };
        assert_eq!(high.distance(budget_off), 1, "a budget stop is one rung");
    }
}
