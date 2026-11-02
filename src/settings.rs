//! Player-facing graphics settings: the persisted model, its on-disk home, and the sole renderer
//! configuration owner.
//!
//! # Three writers: two player intent, one reality
//!
//! | Writer | Writes [`Settings`] | Writes the file |
//! |---|---|---|
//! | The settings page (`settings::ui`) | yes | **yes** — every change saves |
//! | [`observe_window_placement`] (OS fullscreen toggle) | yes — window mode only | **yes** — the green button is player intent |
//! | [`normalize_vsync`] (a conclusive capability probe answer) | yes — vsync only | **yes** — see [`Settings::effective_vsync`] |
//!
//! Normalization spends a stored choice only on a positive capability report, never a failed probe
//! ([`PresentCaps`]). A portable file can name a vsync rung this surface cannot present.
//!
//! The table is exhaustive: values express player intent or conclusive capability normalization.
//!
//! Only an explicit [`SaveSettings`] writes the file; saving on exit could launder an incidental
//! resource value into the player's config.
//!
//! # Forward compatibility
//!
//! Fields default when absent; serde ignores unknown keys. Adding or removing a setting therefore
//! needs no migration in either direction.
//!
//! * Rename a field: `#[serde(alias = "old_name")]`, without a [`SETTINGS_VERSION`] bump.
//! * Change its type while retaining its on-disk shape: `#[serde(from/into)]`, without a bump.
//!   If the shape cannot hold the type, use a new key and a read-only legacy field
//!   ([`VsyncMode`] documents the bool-to-three-rung case).
//! * Change its meaning under the same name: bump the version.
//!
//! Future files are refused and moved aside so a downgrade cannot overwrite them. See `store` and
//! `.agents/docs/design/display/persistence-brief-2026-07.md` for the mechanism and evidence.

use bevy::light::{CascadeShadowConfig, CascadeShadowConfigBuilder, DirectionalLightShadowMap};
use bevy::post_process::bloom::Bloom;
use bevy::prelude::*;
use bevy::render::view::Msaa;
use bevy::ui::UiScale;
use bevy::window::{
    Monitor, MonitorSelection, PresentMode, PrimaryMonitor, PrimaryWindow, WindowMode,
    WindowPosition,
};
use serde::{Deserialize, Serialize};

use crate::render_policy::{CameraProfile, LightProfile};

/// The display row's offered rungs and OS names, read from [`Monitor`] entities. Decides what the
/// row offers, never what [`DisplaySelection`] means. Mounted by its sole consumer, [`ui::plugin`].
mod displays;
/// The end-of-frame frame-rate limiter — armed only by [`VsyncMode::Off`] plus a non-off
/// [`FrameCap`]. Pure std timing; see its doc for the coarse-sleep-then-spin shape.
mod limiter;
/// The graphics presets: the one tier -> values table, and the PRESET row's label derived from
/// it. Never persisted — only the rows a preset writes are; see its doc.
mod preset;
/// The one-shot present-mode capability probe: creates a throwaway wgpu surface for the primary
/// window IN THE RENDER WORLD, reads `Surface::get_capabilities().present_modes`, and shuttles the
/// result into the main-world [`PresentCaps`]. Probe, don't guess — no `cfg(target_os)` anywhere.
mod probe;
/// Persistence location, filename, format, write safety and tolerance. Its doc explains the
/// `video.ron` split and the choice over crates and `bevy_settings` (research brief §1).
mod store;
/// The page itself. Private to this module: composition roots reach it only through [`plugin`],
/// which is what makes mounting the page without its entry declarer unrepresentable.
mod ui;
/// The page's root, which the UI-ownership audit counts as an owner (`net::ui_ownership`).
#[cfg(test)]
pub(crate) use ui::SettingsCard;

/// The schema generation. See the module doc's policy: renames use `#[serde(alias)]` and do NOT
/// bump this; only a field whose MEANING changed under the same name does.
const SETTINGS_VERSION: u32 = 1;

/// The DEFAULT cascade count — [`ShadowCascades::default`]'s value, and the ONE number the whole
/// shadow ladder is priced against.
///
/// MEASURED on an M4 (10 GPU cores, two idling Tigers, release, vsync off, 20 s settled windows):
/// each cascade costs ~1.16 ms; the 1024 → 4096 map-size axis spans 0.27 ms; the 150 → 1000 m
/// distance axis costs ~0.71 ms at 3 cascades / 4096 with track shadow proxies. At 4 cascades /
/// 2048 that distance axis cost ~1.17 ms. Count dominates; dropping one cascade pays for both
/// crispness and reach. The shipped envelope spends only 0.30 ms of that distance axis
/// ([`ShadowDistance`]); [`SHADOW_FIRST_CASCADE_FAR_BOUND_M`] keeps the picture crisp at 3 cascades.
///
/// Runtime count changes require registry bevy_light >= 0.19.1: PR #24807 resizes every pooled
/// thread slot before each view's parallel walk. Stock 0.19.0 can panic on growth; shrinking is
/// benign. Mechanism and validation: `upstream/bevy-cascade-count-stale-local-parallel.md`.
///
/// # Runtime-length audit
///
/// Bevy's per-frame `Local`s outlive frames: config-sized buffers must resize even on idle threads.
/// Audit of the settings [`apply_settings`] writes against Bevy 0.19's pooled `Local<Parallel<…>>`:
///
/// | Site | Element | Positionally indexed by a config length? |
/// |---|---|---|
/// | `bevy_light` `check_dir_light_mesh_visibility` | `Vec<Vec<Entity>>` | yes — cascade count; every slot resized since 0.19.1 |
/// | same system, `defer_visible_entities_queue` | `Vec<Entity>` | no (flat append) |
/// | `bevy_light` point/spot visibility | `[Vec<Entity>; 6]`, `Vec<Entity>` | no (cubemap faces are a fixed 6) |
/// | `bevy_camera` visibility + ranges | `TypeIdMap<Vec<Entity>>`, `Vec<(Entity, u32)>` | no (keyed / flat) |
/// | `bevy_pbr` material + mesh instance queues | `Vec<Entity>`, `Vec<(Entity, …)>` | no (flat append) |
///
/// So the cascade count is the ONLY value of this shape, and no other row can reach it:
///
/// * **MSAA** — `Msaa` is an `ExtractComponent`, re-extracted every frame; the one `Local` on its
///   path (`prepare_view_targets`' `main_texture_atomics`) is a HashMap *keyed by* a tuple
///   containing `Msaa`, so a change is a different key, not a stale length. Textures come from
///   `TextureCache` by descriptor and pipelines are specialized on a key carrying the sample count,
///   both derived from the same extracted value in the same frame. Bevy's own `check_msaa` mutates
///   this component at runtime, so it is a supported operation, not one we invented.
/// * **Bloom** — `Bloom` is an `ExtractComponent`, re-extracted every frame. `prepare_bloom_textures`
///   takes the mip chain from `TextureCache` by a descriptor built from that frame's
///   `max_mip_dimension`, and `prepare_bloom_bind_groups` sizes its bind groups from that frame's
///   `BloomTexture::mip_count`; nothing on the path is a `Local` (bevy_post_process 0.19.1
///   `bloom/mod.rs`).
/// * **Render scale** — `MainPassResolutionOverride` is reconciled onto the render-world view every
///   frame from an extracted resource, and it feeds only per-frame values: the pass scissor rect and
///   `View::main_pass_viewport`. Nothing is sized from it — view targets, depth and prepass textures
///   are all still allocated at the full `physical_target_size` (see [`crate::render_scale`]'s
///   honest-limits section) — so there is no length for a stale `Local` to index with.
pub(crate) const SHADOW_CASCADES: usize = 3;

/// First cascade far bound, metres, explicitly replacing the builder's character-scale 10.0.
///
/// Bevy's geometric split is `first · (maximum/first)^(i/(n−1))`: at fixed count, raising `first`
/// flattens the ladder instead of spending its crispness within a room-sized near field.
///
/// DERIVED screen pixels per shadow texel at each cascade's near bound (its worst point),
/// at 16:9 / 45° vertical / 1964 px tall:
///
/// | config | worst px per texel |
/// |---|---|
/// | 4 casc, first 10, 150 m, 2048 (old default) | 6.08 |
/// | 3 casc, first 10, 1000 m, 4096 | 12.23 |
/// | 3 casc, first 40, 1000 m, 4096 | 6.12 |
/// | 3 casc, first 10, 350 m, 4096 | 7.24 |
/// | **3 casc, first 40, 350 m, 4096 (default)** | **3.64** |
///
/// DERIVED: at 1000 m, 40 m restores crispness to within 0.7% of the old default at 6.7× its reach;
/// at the shipped 350 m, it is 1.7× crisper at 2.3× the reach. The near texel grows 0.83 → 1.66 cm,
/// still under two centimetres on a 3 m tank. A short envelope alone does not fix a bad split:
/// 350 m at first=10 still costs 7.24 px/texel versus 3.64 at first=40.
///
/// Coupled to [`SHADOW_CASCADES`] through the exponent: recompute the table if the count changes.
const SHADOW_FIRST_CASCADE_FAR_BOUND_M: f32 = 40.0;

/// Cascade overlap as a fraction of the previous far bound. Bevy's default 0.2 is explicit so
/// the builder is not a silent author of the picture (see [`SHADOW_FIRST_CASCADE_FAR_BOUND_M`]).
/// It hides boundary seams and spends near-bound density through `(1 − overlap)` in that table.
/// No measurement supports changing it.
const SHADOW_CASCADE_OVERLAP: f32 = 0.2;

/// Directional shadow cascade count at fixed [`ShadowResolution`] and [`ShadowDistance`]. Each
/// cascade costs a render pass; fewer alias more near the camera. MEASURED M4 cost is owned by
/// [`SHADOW_CASCADES`], whose doc also explains the bevy_light >= 0.19.1 runtime-growth requirement.
///
/// No `Off` (owned by [`ShadowDistance::Off`]) or `1`: a single full-distance cascade falls below
/// the offered quality floor; 2 is the budget rung.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default, Serialize, Deserialize)]
pub(crate) enum ShadowCascades {
    Two,
    /// **The shipped default**, pinned as [`SHADOW_CASCADES`] — 3 rather than bevy's 4 because the
    /// fourth cascade costs ~1.16 ms and buys back less than [`SHADOW_FIRST_CASCADE_FAR_BOUND_M`]
    /// gives away for nothing.
    #[default]
    Three,
    Four,
}

impl ShadowCascades {
    /// `CascadeShadowConfigBuilder::num_cascades`; the default uses [`SHADOW_CASCADES`] so the
    /// constant and ladder cannot drift.
    pub(crate) const fn count(self) -> usize {
        match self {
            ShadowCascades::Two => 2,
            ShadowCascades::Three => SHADOW_CASCADES,
            ShadowCascades::Four => 4,
        }
    }

    /// ASCII only — it reaches `Text`; quotes the count (see [`ShadowDistance::label`]).
    pub(crate) const fn label(self) -> &'static str {
        match self {
            ShadowCascades::Two => "2",
            ShadowCascades::Three => "3",
            ShadowCascades::Four => "4",
        }
    }

    pub(crate) const ORDER: [ShadowCascades; 3] = [
        ShadowCascades::Two,
        ShadowCascades::Three,
        ShadowCascades::Four,
    ];
}

/// How far directional shadows are drawn before they stop — the far bound of the last cascade, with
/// the cascade COUNT its own row ([`ShadowCascades`]).
///
/// Separate from [`ShadowResolution`] because their costs are independent: distance widens each
/// cascade's area; resolution buys texels at fixed area. Coupled presets force players wanting
/// reach to buy 4K maps too, and hide which axis causes a frame-rate complaint.
///
/// Quality is coupled: a cascade's texel is `frustum_diameter / map_size`, so distance grows it and resolution shrinks
/// it, and the artefact that texel size drives is not blur — it is **peter-panning**, the shadow
/// sliding off the object that casts it. `bevy_pbr` offsets the shadow lookup by
/// `shadow_normal_bias · √2 · cascade_texel` along the surface normal (see [`crate::world`]'s
/// `SUN_SHADOW_NORMAL_BIAS`); DERIVED detachment is **3.68 × the cascade's texel**, in metres.
/// Raising the distance without raising the resolution therefore buys far shadows that float:
///
/// | far cascade | texel | detachment |
/// |---|---|---|
/// | 150 m @ 2048 (the old default) | 12.4 cm | 0.46 m |
/// | **350 m @ 4096 (the default)** | **14.5 cm** | **0.53 m** |
/// | 700 m @ 4096 | 29 cm | 1.06 m |
/// | 1000 m @ 4096 (the top rung) | 41 cm | 1.5 m |
/// | 1000 m @ 2048 | 82 cm | 3.0 m — **a whole Tiger** |
///
/// That is why the distance default and the resolution default moved together, and it is the reason
/// a player dragging this row to 1000 m on a 1024 map gets something visibly worse than the same
/// row at 350 m. The rows stay separate because their COSTS are separable; the coupling is a fact
/// about the picture that the labels cannot express, so it is written down here instead.
///
/// # Why the default is the floor rung
///
/// MEASURED frame p50 and DERIVED px/texel on an M4 at 3 cascades / 4096 / 40 m first bound, two
/// idling tanks with track shadow proxies, against shadows-off controls agreeing to 0.157 ms:
///
/// | envelope | frame p50 | px per texel (lower = crisper) |
/// |---|---|---|
/// | 150 m (retired ceiling) | 14.56 ms | 2.44 |
/// | **350 m** | **14.86 ms** | **3.64** |
/// | 700 m | 15.38 ms | 5.12 |
/// | 1000 m | 15.27 ms | 6.11 |
///
/// Two things fall out. The cost SATURATES: 700 and 1000 m are 0.11 ms apart, inside the session's
/// own 0.23 ms repeatability, because past ~700 m the cascade already encloses the whole 1 km map.
/// And crispness degrades monotonically with reach — under a geometric split a longer envelope
/// spreads the same texels over more world, so 1000 m is 1.7× blurrier than 350 m for 0.41 ms more.
/// Every column therefore argues the same way, which is what makes this an easy default: shorter is
/// cheaper AND crisper, right down to the point where it starts cutting real shadows off.
///
/// That point is the floor, and it is physics, not taste (see the migration note: a 17° sun over
/// 100 m of relief ⇒ ~327 m of self-shadowing). 350 m is the first rung above it, and it is also the
/// LAST one worth paying for: past it the envelope reaches into sky this world has nothing to cast
/// into, so the extra metres buy reach nobody can see while costing texel density everybody can.
/// The 150 m row is in the table to show what the floor is protecting — it is cheaper and crisper
/// still, and it is exactly the setting whose cut-off shadows started this work. The longer rungs
/// stay on the ladder for other hardware and other worlds, not as the recommendation.
///
/// `Off` remains provisional pending the 15v15 frame-headroom measurement: shadows carry gameplay
/// information. The competitive-integrity decision lives in
/// `.agents/docs/design/display/shadow-presets.md`; downstream code must not assume its outcome.
///
/// # Legacy distance aliases
///
/// DERIVED: at the world's 17° sun, shadows run 3.27× caster height; 100 m relief needs ~327 m.
/// The 350 m floor avoids cutting ridge shadows off partway through a valley. Legacy 100/150/300 m
/// values all fall below it; Bevy's stock 150 m envelope is for character-scale scenes.
///
/// **`M100`/`M150`/`M300` are gone as VARIANTS and survive as `#[serde(alias)]`s** on the nearest
/// surviving rung, so an existing `video.ron` loads without error and lands on a real ladder
/// position. Migrating beats keeping them representable-but-hidden: `ui::step_in` treats a current
/// value it cannot find in `ORDER` as index 0, so an off-ladder rung would send the next press of
/// the right arrow to `Off` — a stored value the page cannot walk out of is worse than a stored
/// value the page silently rounds. All three land on `M350` (|300 − 350| = 50 beats |300 − 700| =
/// 400), which does mean a player who chose 100 m for frame rate is moved UP; that is accepted
/// because the same release drops a cascade, and the 150 → 350 m they are moved across costs
/// ~0.30 ms (table above) against the ~1.16 ms that one fewer cascade gives back — see
/// [`SHADOW_CASCADES`]. `M350` being the DEFAULT as well as the migration target means the common
/// case is a file that lands where a fresh install would.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default, Serialize, Deserialize)]
pub(crate) enum ShadowDistance {
    /// Nothing casts: [`apply_settings`] clears `DirectionalLight::shadow_maps_enabled`. This is the
    /// whole shadow budget in one A/B, and the rung whose future is undecided (see the type doc).
    Off,
    /// **The shipped default**, and the physical floor for this world's sun — see the migration note
    /// above for the 3.27× that produces it, and for why the three retired rungs all land here.
    /// Chosen over the longer rungs on the measurement in the type doc's envelope table: everything
    /// past ~350 m reaches into sky this world cannot shadow, so the extra metres are paid for and
    /// not seen.
    #[serde(alias = "M100", alias = "M150", alias = "M300")]
    #[default]
    M350,
    M700,
    /// This is the rung at which a 1 km map casts whole and the draw volume stops growing —
    /// MEASURED on that map: the cost curve saturates around
    /// 700 m for exactly that reason (700 and 1000 m are within the session's own repeatability).
    /// The type doc's envelope table makes this a top rung rather than the default. Only affordable together with [`ShadowResolution::X4096`] — at 2048 it
    /// detaches a far shadow by a whole tank.
    M1000,
}

impl ShadowDistance {
    /// The far bound of the last cascade, metres — `None` for [`ShadowDistance::Off`], which is not
    /// a distance at all. Returning an `Option` rather than `0.0` is what keeps a caller from
    /// building a degenerate zero-metre cascade config and calling it "off".
    pub(crate) const fn distance_m(self) -> Option<f32> {
        match self {
            ShadowDistance::Off => None,
            ShadowDistance::M350 => Some(350.0),
            ShadowDistance::M700 => Some(700.0),
            ShadowDistance::M1000 => Some(1000.0),
        }
    }

    /// Whether anything casts at all — the `DirectionalLight::shadow_maps_enabled` this maps to.
    pub(crate) const fn casts(self) -> bool {
        self.distance_m().is_some()
    }

    /// ASCII only — it reaches `Text`. Metres, rather than LOW/MEDIUM/HIGH, let players compare the
    /// cutoff with the sight picture.
    pub(crate) const fn label(self) -> &'static str {
        match self {
            ShadowDistance::Off => "OFF",
            ShadowDistance::M350 => "350 M",
            ShadowDistance::M700 => "700 M",
            ShadowDistance::M1000 => "1000 M",
        }
    }

    /// Ascending in cost, so the settings page's right arrow always moves toward more shadow.
    ///
    /// **Every representable value must appear here** — see the type doc's migration note for the
    /// `step_in` dead-end that an off-ladder variant would create. `the_retired_distance_rungs_migrate`
    /// pins it.
    pub(crate) const ORDER: [ShadowDistance; 4] = [
        ShadowDistance::Off,
        ShadowDistance::M350,
        ShadowDistance::M700,
        ShadowDistance::M1000,
    ];
}

/// Cascade map edge length: crispness at a given [`ShadowDistance`]. Costs are separable; quality
/// is coupled through peter-panning (see [`ShadowDistance`]).
///
/// MEASURED on an M4: 1024 → 4096 costs 0.27 ms, versus ~1.16 ms per cascade
/// ([`ShadowCascades`]); render-pass count dominates texel count on that tiled GPU.
///
/// No `Off`: [`ShadowDistance::Off`] owns it, avoiding contradictory rows.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default, Serialize, Deserialize)]
pub(crate) enum ShadowResolution {
    X1024,
    /// Bevy's `DirectionalLightShadowMap::default().size`. DERIVED detachment: ~1.07 m at the
    /// shipped 350 m envelope and ~3 m at [`ShadowDistance::M1000`], making this a budget rung.
    X2048,
    /// **The shipped default.** 0.27 ms for 4× the texels — the cheapest quality on this page, and
    /// what holds far-shadow detachment to half a metre at the shipped envelope (see
    /// [`ShadowDistance`]'s table). It is also what makes the longer rungs usable at all: without it
    /// [`ShadowDistance::M1000`] would peter-pan by a whole tank.
    #[default]
    X4096,
}

impl ShadowResolution {
    /// Texels per cascade edge. Powers of two — `bevy_light`'s `validate_shadow_map_size` rounds
    /// anything else up and warns every launch.
    pub(crate) const fn shadow_map_size(self) -> usize {
        match self {
            ShadowResolution::X1024 => 1024,
            ShadowResolution::X2048 => 2048,
            ShadowResolution::X4096 => 4096,
        }
    }

    /// ASCII only — it reaches `Text`. Quotes texels, as [`ShadowDistance::label`] quotes metres.
    pub(crate) const fn label(self) -> &'static str {
        match self {
            ShadowResolution::X1024 => "1024",
            ShadowResolution::X2048 => "2048",
            ShadowResolution::X4096 => "4096",
        }
    }

    /// Ascending in cost, like every other ladder here.
    pub(crate) const ORDER: [ShadowResolution; 3] = [
        ShadowResolution::X1024,
        ShadowResolution::X2048,
        ShadowResolution::X4096,
    ];
}

/// Multisample level. `Off` IS offered here, unlike shadows: MSAA smooths edges, it does not reveal
/// or conceal anything — a player who turns it off sees the same tanks in the same places.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default, Serialize, Deserialize)]
pub(crate) enum MsaaLevel {
    Off,
    X2,
    /// **The shipped default** — bevy installs `Msaa::Sample4` as a required component of `Camera`.
    #[default]
    X4,
}

impl MsaaLevel {
    pub(crate) const fn to_msaa(self) -> Msaa {
        match self {
            MsaaLevel::Off => Msaa::Off,
            MsaaLevel::X2 => Msaa::Sample2,
            MsaaLevel::X4 => Msaa::Sample4,
        }
    }

    /// ASCII only — it reaches `Text`.
    pub(crate) const fn label(self) -> &'static str {
        match self {
            MsaaLevel::Off => "OFF",
            MsaaLevel::X2 => "2X",
            MsaaLevel::X4 => "4X",
        }
    }

    pub(crate) const ORDER: [MsaaLevel; 3] = [MsaaLevel::Off, MsaaLevel::X2, MsaaLevel::X4];
}

/// How finely the battlefield camera's bloom is drawn: the glow that makes a tracer read as a
/// tracer (its emissive rides far above 1.0 and only this halo shows it — `ballistics::view`),
/// and the glow around muzzle flashes and the sun's highlights. Applied as bevy's
/// `Bloom::max_mip_dimension`, the height in texels of the largest texture in the blur chain.
///
/// **No `Off`.** Tracers and muzzle flashes must stay visible under every graphics option (#159's
/// never-list), and they are drawn through this glow; LOW keeps every halo and draws it coarser.
///
/// The cost, DERIVED from bevy_post_process 0.19.1 `prepare_bloom_textures`: the chain's top
/// texture is `max_mip_dimension` texels tall (and the view's aspect wide), and the chain has
/// `ilog2(max_mip_dimension) - 1` mips, so LOW's 256 holds a quarter of NORMAL's top-level texels
/// and one mip fewer, and both chains end at the same 4-texel-tall mip, so the widest halo is the
/// same size on screen. The full-resolution composite onto the view is unchanged. MEASURED: none.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default, Serialize, Deserialize)]
pub(crate) enum BloomLevel {
    /// Half NORMAL's resolution on each axis.
    Low,
    /// **The shipped default**: `Bloom::NATURAL`'s own `max_mip_dimension`, 512.
    #[default]
    Normal,
}

impl BloomLevel {
    /// The `Bloom::max_mip_dimension` this rung applies.
    pub(crate) fn max_mip_dimension(self) -> u32 {
        match self {
            BloomLevel::Low => Bloom::NATURAL.max_mip_dimension / 2,
            BloomLevel::Normal => Bloom::NATURAL.max_mip_dimension,
        }
    }

    /// ASCII only — it reaches `Text`.
    pub(crate) const fn label(self) -> &'static str {
        match self {
            BloomLevel::Low => "LOW",
            BloomLevel::Normal => "NORMAL",
        }
    }

    /// Ascending in cost, like every other ladder here.
    pub(crate) const ORDER: [BloomLevel; 2] = [BloomLevel::Low, BloomLevel::Normal];
}

/// The fraction of the window the 3D world is rendered at, before being upscaled to fill it. See
/// [`crate::render_scale`] for the mechanism and its honest limits; this type is only the ladder.
///
/// **Why a ladder and not a resolution list.** The research brief's central finding is that a
/// resolution dropdown is the macOS blur trap: `CGDisplayCopyAllDisplayModes` returns modes for two
/// different regions of the same panel in one unfilterable list, and shipped titles (Tomb Raider,
/// No Man's Sky, Stray, Riven) pick the wrong one and get a squashed, resampled picture. We never
/// enumerate modes, never mode-set a display, and let the drawable stay the window's real size — so
/// we are immune to that class by construction, and the only thing left to expose is how much of
/// that drawable the 3D pass fills. WoW's "Render Scale" is the same model.
///
/// **Why these five rungs.** 50% and 100% are exact: at half the panel resolution macOS's
/// WindowServer quadruples pixels with no interpolation at all, so those two are the sharp ones. The
/// three in between trade sharpness for fill rate through a bilinear filter, and exist because the
/// jump from 100% to 50% is a very large step to be the only choice.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default, Serialize, Deserialize)]
pub(crate) enum RenderScaleLevel {
    Percent50,
    Percent67,
    Percent75,
    Percent85,
    /// **The shipped default** — native, and the path on which [`crate::render_scale`] inserts no
    /// override and schedules no upscale pass at all.
    #[default]
    Percent100,
}

impl RenderScaleLevel {
    /// The linear fraction of the window's physical size each axis is rendered at. Area cost scales
    /// with the SQUARE of this, which is what makes 75% a ~44% cut in main-pass pixels.
    pub(crate) const fn fraction(self) -> f32 {
        match self {
            RenderScaleLevel::Percent50 => 0.50,
            RenderScaleLevel::Percent67 => 0.67,
            RenderScaleLevel::Percent75 => 0.75,
            RenderScaleLevel::Percent85 => 0.85,
            RenderScaleLevel::Percent100 => 1.0,
        }
    }

    /// ASCII only — it reaches `Text`.
    pub(crate) const fn label(self) -> &'static str {
        match self {
            RenderScaleLevel::Percent50 => "50%",
            RenderScaleLevel::Percent67 => "67%",
            RenderScaleLevel::Percent75 => "75%",
            RenderScaleLevel::Percent85 => "85%",
            RenderScaleLevel::Percent100 => "100%",
        }
    }

    /// Ascending, so the settings page's right arrow always moves toward more pixels.
    pub(crate) const ORDER: [RenderScaleLevel; 5] = [
        RenderScaleLevel::Percent50,
        RenderScaleLevel::Percent67,
        RenderScaleLevel::Percent75,
        RenderScaleLevel::Percent85,
        RenderScaleLevel::Percent100,
    ];
}

/// Whether frames are locked to the display refresh — a THREE-rung ladder since V2 added
/// [`VsyncMode::Fast`].
///
/// # The v1→v2 field migration, recorded once
///
/// v1 shipped this as `vsync: bool` (later a two-rung enum bridged through
/// `#[serde(from/into = "bool")]`), and `video.ron` files carrying `vsync: false` are on players'
/// disks now. A `bool` has exactly two values, so the third rung CANNOT live under that field name —
/// writing `vsync: Fast` would fail a v1 build's whole parse and reset every other setting with it.
/// Per the module doc's policy this is therefore a NEW field, `vsync_mode` (see
/// [`Settings::vsync`]), and [`SETTINGS_VERSION`] does not move:
///
/// * **reading old files**: the retired `vsync: bool` key is still read through
///   [`Settings::legacy_vsync`] and absorbed by [`Settings::absorb_legacy_vsync`] — a player's
///   saved `vsync: false` still lands on [`VsyncMode::Off`];
/// * **old build reading a new file**: `vsync_mode` is an unknown key to a v1 build, so it is
///   skipped and vsync comes up at its default (ON). That dropped-key downgrade is the documented
///   cost of the rename, accepted because ON is the safe rung everywhere.
///
/// # Which rungs a machine actually gets
///
/// The ladder is capability-gated by [`PresentCaps`], the probe result — see `probe`. OFF needs
/// `Immediate` (macOS Metal has it; Wayland refuses it), FAST needs `Mailbox` (Wayland has it;
/// Metal does not). ON is `Fifo`, which every surface supports. The page offers only the rungs the
/// surface reports; [`Settings::present_mode`] maps the probed-only modes to
/// [`PresentMode::AutoNoVsync`] whenever the probe has not confirmed them, so even a config file
/// carried over from another machine can never ask the backend for a mode it lacks.
///
/// A carried-over file can still NAME a rung this surface lacks, though, and that is a different
/// problem: see [`Settings::effective_vsync`] (what such a rung resolves to, for every consumer)
/// and [`normalize_vsync`] (which writes the resolution back once the probe lands).
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default, Serialize, Deserialize)]
pub(crate) enum VsyncMode {
    /// Uncapped, may tear, lowest latency — [`PresentMode::Immediate`] where the surface supports
    /// it. The ONE rung that arms the frame-cap row.
    Off,
    /// Uncapped without tearing — [`PresentMode::Mailbox`] where the surface supports it (NVIDIA
    /// calls this shape "Fast Sync").
    Fast,
    /// **The shipped default** — [`PresentMode::Fifo`], universally supported and traditionally
    /// exactly what "VSync On" means.
    #[default]
    On,
}

impl VsyncMode {
    /// ASCII only — it reaches `Text`.
    pub(crate) const fn label(self) -> &'static str {
        match self {
            VsyncMode::Off => "OFF",
            VsyncMode::Fast => "FAST",
            VsyncMode::On => "ON",
        }
    }

    /// Ascending in cost, like every other ladder here — the right arrow moves toward more waiting
    /// per frame.
    pub(crate) const ORDER: [VsyncMode; 3] = [VsyncMode::Off, VsyncMode::Fast, VsyncMode::On];
}

/// Surface capabilities from `probe`. Only [`PresentCaps::Reported`] is evidence a rung is absent:
/// normalizing on a failed probe would permanently replace a valid stored FAST/OFF with ON.
///
/// Surface creation can fail; wgpu 29 also returns an empty present-mode list on a failed query or
/// an adapter-incompatible surface. A presentable surface reports at least `Fifo`, so `probe` maps
/// both failures to [`PresentCaps::Unavailable`].
///
/// Both unknown states offer every rung and leave [`Settings::effective_vsync`] unchanged.
/// [`Settings::present_mode`] uses self-negotiating [`PresentMode::AutoNoVsync`] for uncapped rungs
/// until a positive report permits concrete `Mailbox`/`Immediate`, preventing backend panics.
#[derive(Resource, Clone, Copy, PartialEq, Eq, Debug, Default)]
pub(crate) enum PresentCaps {
    /// Boot state: no surface answer yet; used deliberately by both windowed roots.
    #[default]
    Unprobed,
    /// Terminal probe failure: surface creation failed or the capability list was empty. No retry;
    /// no claim about surface support. See the type doc.
    Unavailable,
    /// The two gated modes, from a nonempty capability list only. Both flags false means a real
    /// `[Fifo]` or `[Fifo, FifoRelaxed]` answer: ON only, unlike an empty list.
    Reported { immediate: bool, mailbox: bool },
}

impl PresentCaps {
    /// The `receive_probe` run condition: unavailable stops polling just as a report does.
    pub(crate) const fn answered(self) -> bool {
        !matches!(self, PresentCaps::Unprobed)
    }

    /// Whether `Immediate` was positively reported. Unknown is false: `present_mode` requires evidence
    /// before emitting a concrete mode.
    pub(crate) const fn immediate(self) -> bool {
        matches!(
            self,
            PresentCaps::Reported {
                immediate: true,
                ..
            }
        )
    }

    /// `PresentMode::Mailbox` was POSITIVELY reported. See [`PresentCaps::immediate`].
    pub(crate) const fn mailbox(self) -> bool {
        matches!(self, PresentCaps::Reported { mailbox: true, .. })
    }

    /// Unknown states offer every rung; reported lists gate them. FAST requires `Mailbox` specifically:
    /// using `Immediate` would duplicate OFF on Metal.
    pub(crate) const fn offers(self, mode: VsyncMode) -> bool {
        match self {
            PresentCaps::Unprobed | PresentCaps::Unavailable => true,
            PresentCaps::Reported { immediate, mailbox } => match mode {
                VsyncMode::On => true,
                VsyncMode::Fast => mailbox,
                VsyncMode::Off => immediate,
            },
        }
    }

    /// Resolve an offered rung to itself, otherwise to the universal [`VsyncMode::On`] (`Fifo`).
    /// Unknown states offer everything, so [`normalize_vsync`] cannot act on a guess.
    ///
    /// ON is chosen over an adjacent rung deliberately: Wayland could turn unsupported OFF into FAST,
    /// but FAST promises no tearing and no frame cap. Choosing it would guess which part of OFF the
    /// player wanted; ON promises only what every surface can honour.
    pub(crate) const fn resolve(self, mode: VsyncMode) -> VsyncMode {
        if self.offers(mode) {
            mode
        } else {
            VsyncMode::On
        }
    }
}

/// Windowed vs fullscreen. Two rungs on purpose: the fullscreen offered is winit's BORDERLESS
/// fullscreen (`WindowMode::BorderlessFullscreen`), which on macOS is native Spaces fullscreen via
/// `NSWindow toggleFullScreen` — the same thing the green traffic-light button does. Exclusive
/// (`WindowMode::Fullscreen`) is deliberately unrepresentable here: bevy PANICS on an exclusive
/// mode whose monitor cannot be resolved, which would make a saved config unlaunchable once that
/// monitor is unplugged, and borderless is what every modern display stack actually wants.
///
/// The monitor selection comes from [`DisplaySelection`] and defaults to
/// [`MonitorSelection::Current`], none of which can panic for borderless:
/// `bevy_winit::select_monitor` merely warns and passes `None` (= current) through.
///
/// **The monitor rides THIS field, not `Window::position`** — see [`DisplaySelection`]'s fullscreen
/// section. A fullscreen window ignores `set_outer_position`, and bevy picks a borderless window's
/// monitor out of `Window::mode` alone (vendored bevy_winit-0.19.0/src/system.rs:326-334), so the
/// display row has to reach fullscreen through here or not at all.
///
/// **The row REFLECTS the OS, it does not own it** — see `observe_window_placement`: the player can
/// toggle fullscreen with the green button, and the stored value follows reality rather than
/// fighting it. That writer computes this mapping with the SAME resolved display this one does,
/// which is load-bearing rather than tidy — see `observe_window_placement` for the clobber it prevents.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default, Serialize, Deserialize)]
pub(crate) enum WindowModeSetting {
    /// **The shipped default** — the window the game has always opened with.
    #[default]
    Windowed,
    Fullscreen,
}

impl WindowModeSetting {
    /// The `Window::mode` this asks for on `monitor` — the ONE mapping, used at boot window
    /// description, by `apply_settings` and by `observe_window_placement` alike.
    ///
    /// `monitor` is a MonitorSelection rather than a [`DisplaySelection`] because the two callers
    /// derive it differently and must be free to: a running app resolves against the live monitor
    /// list ([`DisplaySelection::monitor_on`]), while the boot roots may only name what window
    /// CREATION can resolve ([`DisplaySelection::at_window_creation`]). `None` is "no opinion",
    /// which for a borderless window means the display it is already on.
    pub(crate) const fn to_window_mode(self, monitor: Option<MonitorSelection>) -> WindowMode {
        match self {
            WindowModeSetting::Windowed => WindowMode::Windowed,
            WindowModeSetting::Fullscreen => WindowMode::BorderlessFullscreen(match monitor {
                Some(monitor) => monitor,
                None => MonitorSelection::Current,
            }),
        }
    }

    /// The setting that DESCRIBES an observed OS state — the reflect-don't-fight direction.
    pub(crate) const fn from_fullscreen(fullscreen: bool) -> Self {
        if fullscreen {
            WindowModeSetting::Fullscreen
        } else {
            WindowModeSetting::Windowed
        }
    }

    /// ASCII only — it reaches `Text`.
    pub(crate) const fn label(self) -> &'static str {
        match self {
            WindowModeSetting::Windowed => "WINDOWED",
            WindowModeSetting::Fullscreen => "FULLSCREEN",
        }
    }

    pub(crate) const ORDER: [WindowModeSetting; 2] =
        [WindowModeSetting::Windowed, WindowModeSetting::Fullscreen];
}

/// Whether the online client draws its debug card (ping, jitter, frame rate, frame time, version)
/// in a corner during a match. On by default during the beta, so a tester's report carries the
/// numbers. Read only by `net::debug_hud`; the offline root has no card.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default, Serialize, Deserialize)]
pub(crate) enum DebugPanel {
    #[default]
    On,
    Off,
}

impl DebugPanel {
    pub(crate) const ORDER: [DebugPanel; 2] = [DebugPanel::Off, DebugPanel::On];

    /// ASCII only — it reaches `Text`.
    pub(crate) const fn label(self) -> &'static str {
        match self {
            DebugPanel::On => "ON",
            DebugPanel::Off => "OFF",
        }
    }

    pub(crate) const fn shown(self) -> bool {
        matches!(self, DebugPanel::On)
    }
}

/// Which of the player's own hits open the pen cam's corner: every one, penetrations only, or
/// none. Read by `damage_cam::pen_cam::game`.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default, Serialize, Deserialize)]
pub(crate) enum PenCamCorner {
    #[default]
    All,
    ThroughOnly,
    Off,
}

impl PenCamCorner {
    pub(crate) const ORDER: [PenCamCorner; 3] = [
        PenCamCorner::All,
        PenCamCorner::ThroughOnly,
        PenCamCorner::Off,
    ];

    /// ASCII only — it reaches `Text`.
    pub(crate) const fn label(self) -> &'static str {
        match self {
            PenCamCorner::All => "ALL",
            PenCamCorner::ThroughOnly => "THROUGH ONLY",
            PenCamCorner::Off => "OFF",
        }
    }

    /// Whether a hit of verdict `verdict` opens the corner.
    pub(crate) fn opens(self, verdict: crate::damage_cam::ShotVerdict) -> bool {
        match self {
            PenCamCorner::All => true,
            PenCamCorner::ThroughOnly => verdict == crate::damage_cam::ShotVerdict::Penetration,
            PenCamCorner::Off => false,
        }
    }
}

/// Whether the player's own knockout by a recorded shot plays the kill cam before the death screen.
/// Off, the death screen comes at once, the killer card still naming what the record names. Read by
/// `damage_cam::pen_cam::game::killcam`.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default, Serialize, Deserialize)]
pub(crate) enum KillCam {
    #[default]
    On,
    Off,
}

impl KillCam {
    pub(crate) const ORDER: [KillCam; 2] = [KillCam::On, KillCam::Off];

    /// ASCII only — it reaches `Text`.
    pub(crate) const fn label(self) -> &'static str {
        match self {
            KillCam::On => "ON",
            KillCam::Off => "OFF",
        }
    }

    pub(crate) const fn on(self) -> bool {
        matches!(self, KillCam::On)
    }
}

/// Whether the pen cam's views hold their camera still and land their events with short fades
/// instead of travelling.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default, Serialize, Deserialize)]
pub(crate) enum ReduceMotion {
    #[default]
    Off,
    On,
}

impl ReduceMotion {
    pub(crate) const ORDER: [ReduceMotion; 2] = [ReduceMotion::Off, ReduceMotion::On];

    /// ASCII only — it reaches `Text`.
    pub(crate) const fn label(self) -> &'static str {
        match self {
            ReduceMotion::Off => "OFF",
            ReduceMotion::On => "ON",
        }
    }

    pub(crate) const fn on(self) -> bool {
        matches!(self, ReduceMotion::On)
    }
}

/// How the track prints' relief is drawn (cosmetic terrain marks, `track_prints`): lighting only
/// (the depth bends the ground's normal), parallax (the ground's texture shifts with the depth, so
/// a rut reads at low angles), or parallax with the sun's shadow marched across the rut. Every
/// level darkens and roughens the trodden ground the same; only the relief's cost differs, and only
/// on print pixels within [`TrackPrintRange`]. Read by `track_prints::sync_material`.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default, Serialize, Deserialize)]
pub(crate) enum TrackPrintRelief {
    Lighting,
    Parallax,
    #[default]
    SunShadow,
}

impl TrackPrintRelief {
    pub(crate) const ORDER: [TrackPrintRelief; 3] = [
        TrackPrintRelief::Lighting,
        TrackPrintRelief::Parallax,
        TrackPrintRelief::SunShadow,
    ];

    /// ASCII only — it reaches `Text`.
    pub(crate) const fn label(self) -> &'static str {
        match self {
            TrackPrintRelief::Lighting => "LIGHTING",
            TrackPrintRelief::Parallax => "PARALLAX",
            TrackPrintRelief::SunShadow => "SUN SHADOW",
        }
    }

    /// The level the shader branches on: 0 lighting only, 1 parallax, 2 parallax and sun.
    pub(crate) const fn level(self) -> u8 {
        match self {
            TrackPrintRelief::Lighting => 0,
            TrackPrintRelief::Parallax => 1,
            TrackPrintRelief::SunShadow => 2,
        }
    }
}

/// How far from the eye the track prints' relief is drawn before it fades to lighting only. The
/// near store reaches at least 39 m ahead of the eye in every direction (`track_prints::store`), so
/// the longest rung is the store's own reach. Read by `track_prints::sync_material`.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default, Serialize, Deserialize)]
pub(crate) enum TrackPrintRange {
    Near,
    Middle,
    #[default]
    Far,
    Farthest,
}

impl TrackPrintRange {
    pub(crate) const ORDER: [TrackPrintRange; 4] = [
        TrackPrintRange::Near,
        TrackPrintRange::Middle,
        TrackPrintRange::Far,
        TrackPrintRange::Farthest,
    ];

    /// The distance, m. ASSUMED: 30 m by default, the far end of the range a print must read at.
    pub(crate) const fn metres(self) -> f32 {
        match self {
            TrackPrintRange::Near => 10.0,
            TrackPrintRange::Middle => 20.0,
            TrackPrintRange::Far => 30.0,
            TrackPrintRange::Farthest => 40.0,
        }
    }

    /// ASCII only — it reaches `Text`.
    pub(crate) const fn label(self) -> &'static str {
        match self {
            TrackPrintRange::Near => "10 M",
            TrackPrintRange::Middle => "20 M",
            TrackPrintRange::Far => "30 M",
            TrackPrintRange::Farthest => "40 M",
        }
    }
}

/// Which monitor the window is put on — the row that stops a two-display machine opening the game
/// wherever the window manager last felt like.
///
/// # It takes TWO fields to move a window, because bevy reads two
///
/// * **Windowed** — `Window::position = WindowPosition::Centered(selection)`.
/// * **Borderless fullscreen** — `Window::mode = BorderlessFullscreen(selection)`, through
///   [`WindowModeSetting::to_window_mode`].
///
/// Fullscreen ignores `set_outer_position`: Bevy resolves its monitor from `Window::mode` alone,
/// never the adjacent position (bevy_winit-0.19.1/src/system.rs:326-334). A fullscreen display
/// choice written only into position therefore leaves the window on `Current`'s fallback monitor.
///
/// The two are applied differently on purpose, and [`apply_settings`] states which is which:
/// the MODE is level-triggered (it is also written by `observe_window_placement`, so it must be
/// re-asserted rather than fired once), the POSITION is edge-triggered (winit rewrites it out from
/// under us — see the reconciler).
///
/// # The boot window must NOT name an indexed display, and that is not an optimisation
///
/// `Index` cannot be resolved when the window is CREATED. Bevy creates the primary window from
/// `resumed` (vendored bevy_winit-0.19.0/src/state.rs:173-181) but only populates `WinitMonitors`
/// from `about_to_wait` (state.rs:458-464), so `select_monitor`'s `Index(n) => monitors.nth(n)`
/// (bevy_winit-0.19.0/src/winit_windows.rs:522) sees an EMPTY list and answers `None`. Creation
/// then goes fullscreen on `Fullscreen::Borderless(None)` (winit_windows.rs:73-86) — the wrong
/// display — and `winit_window_position` likewise warns and declines to place a windowed window
/// (winit_windows.rs:493-497).
///
/// **The trap is what happens NEXT, and it is why the boot mode is deliberately weaker than the
/// stored setting.** Bevy caches the window it was ASKED for, verbatim and unresolved:
/// `CachedWindow(window.clone())` (bevy_winit-0.19.0/src/system.rs:88). Reissuing fullscreen is
/// then gated on the component DIFFERING from that cache — `Changed<Window>` plus
/// `if window.mode != cache.mode` (system.rs:305-326). So a boot window described with
/// `BorderlessFullscreen(Index(1))` produces a cache saying `Index(1)`, and when [`apply_settings`]
/// resolves the same `Index(1)` at `Startup` the value is EQUAL — the guarded write is skipped,
/// `Changed` never fires, bevy never reissues, and the real winit window sits on creation's
/// fallback monitor for the rest of the session. Naming the display twice is worse than naming it
/// once.
///
/// So [`DisplaySelection::at_window_creation`] emits only what creation can actually honour:
/// `Primary` (handed to `select_monitor` as a direct argument rather than through the empty list)
/// resolves there and is passed through; every indexed rung is passed as "no opinion", so the boot
/// mode is `Current`, the cache says `Current`, and `Startup`'s resolved `Index(n)` is a genuine
/// CHANGE that bevy acts on. The visible cost is honest and small: an indexed display opens where
/// the window manager put it and moves once, immediately.
///
/// # Why a stored selection is NEVER normalised back to the file
///
/// [`normalize_vsync`] exists because a vsync rung a surface lacks is a lie the file should stop
/// telling. A monitor is the opposite case: displays are HOT-PLUGGED, so "display 2 is not here" is
/// a statement about this minute, not about the machine. Persisting the fallback would spend the
/// player's choice the first time they undocked, and they would have to re-pick it every time they
/// plugged back in. So [`DisplaySelection::resolve`] is a read-side fallback only — it logs, it
/// never writes.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default, Serialize, Deserialize)]
pub(crate) enum DisplaySelection {
    /// **The shipped default** — nothing is written, so the window manager places the window
    /// exactly as it always has. The one rung that is not a decision, and the reason adding this
    /// row changes no existing install's behaviour.
    #[default]
    Auto,
    /// The OS's own main display. The only rung that would ALSO resolve at window-creation time
    /// (`select_monitor` takes `primary_monitor` as a direct argument rather than through the
    /// not-yet-populated list), and the one a capture harness should reach for when it just wants
    /// the built-in panel.
    Primary,
    /// `MonitorSelection::Index(0)`. **One-based in the label, zero-based on the wire**: every
    /// desktop OS numbers displays from 1, and `Index` counts from 0.
    Display1,
    Display2,
    Display3,
}

impl DisplaySelection {
    /// Persisted indexed-monitor capacity, not the offered [`displays::DisplayLadder`]. A fourth
    /// monitor cannot borrow another display's variant. Widen by adding variants; narrowing would
    /// make a stored `Display3` unparseable and reset the whole video file.
    const INDEXED: usize = 3;

    /// The monitor this rung names, or `None` for [`DisplaySelection::Auto`] — which is not a
    /// monitor at all, and is what makes "do not manage the window's position" representable.
    pub(crate) const fn monitor(self) -> Option<MonitorSelection> {
        match self {
            DisplaySelection::Auto => None,
            DisplaySelection::Primary => Some(MonitorSelection::Primary),
            DisplaySelection::Display1 => Some(MonitorSelection::Index(0)),
            DisplaySelection::Display2 => Some(MonitorSelection::Index(1)),
            DisplaySelection::Display3 => Some(MonitorSelection::Index(2)),
        }
    }

    /// The zero-based index into the attached-monitor list this rung asks for, or `None` for the
    /// two rungs that name no index.
    const fn index(self) -> Option<usize> {
        match self {
            DisplaySelection::Auto | DisplaySelection::Primary => None,
            DisplaySelection::Display1 => Some(0),
            DisplaySelection::Display2 => Some(1),
            DisplaySelection::Display3 => Some(2),
        }
    }

    /// The rung naming the zero-based attached-monitor `index`, or `None` past [`Self::INDEXED`].
    /// The inverse of [`DisplaySelection::index`], and the only place the ladder learns which
    /// variant a monitor's list position stores.
    const fn at_index(index: usize) -> Option<Self> {
        match index {
            0 => Some(DisplaySelection::Display1),
            1 => Some(DisplaySelection::Display2),
            2 => Some(DisplaySelection::Display3),
            _ => None,
        }
    }

    /// Boot selection: only `Primary` resolves before `WinitMonitors` is populated. Indexed rungs
    /// return no opinion (`Current`); naming one would poison Bevy's cache and prevent the Startup
    /// correction. See the type doc.
    pub(crate) const fn at_window_creation(self) -> Option<MonitorSelection> {
        match self {
            DisplaySelection::Primary => Some(MonitorSelection::Primary),
            // `Auto` genuinely has no opinion; the indexed rungs have one that creation cannot act
            // on and must not cache.
            _ => None,
        }
    }

    /// The monitor selection this rung asks for on `displays` — the ONE mapping a RUNNING app
    /// writes into `Window::mode` and `Window::position` alike. `None` is "leave placement alone"
    /// ([`DisplaySelection::Auto`]).
    ///
    /// **The primary rung is emitted as `MonitorSelection::Entity`, not `Primary`**, whenever bevy
    /// has actually marked a monitor. That is a deliberate hardening rather than a rename, because
    /// bevy's `PrimaryMonitor` marker is not a live fact — see [`AttachedDisplays::primary`] for how
    /// it goes stale. `Entity` is resolved by `find_entity` against the monitor list
    /// (bevy_winit-0.19.0/src/winit_windows.rs:523, `winit_monitors.rs:29-35`), so it answers
    /// deterministically for any monitor that is still attached and never consults the
    /// `primary_monitor()` that Wayland always answers `None` to. The bare `Primary` is emitted only
    /// in the state where nothing has been learned at all, where it is the honest request.
    pub(crate) fn monitor_on(self, displays: AttachedDisplays) -> Option<MonitorSelection> {
        match self.resolve(displays) {
            DisplaySelection::Auto => None,
            // After `resolve`, this rung means "the primary" only where a marker exists or where
            // nothing is known yet; both are covered by preferring the entity when there is one.
            DisplaySelection::Primary => Some(
                displays
                    .primary
                    .map_or(MonitorSelection::Primary, MonitorSelection::Entity),
            ),
            indexed => indexed.monitor(),
        }
    }

    /// The `Window::position` this rung asks for on `displays`. `Automatic` for
    /// [`DisplaySelection::Auto`], which bevy answers with "the window manager handles it" and no
    /// move at all (`winit_window_position`, vendored
    /// bevy_winit-0.19.0/src/winit_windows.rs:458-461).
    pub(crate) fn window_position_on(self, displays: AttachedDisplays) -> WindowPosition {
        self.monitor_on(displays)
            .map_or(WindowPosition::Automatic, WindowPosition::Centered)
    }

    /// The rung actually honoured on `displays` — itself, or a rung this machine can really reach.
    ///
    /// Two substitutions, and the second is not the obvious one:
    ///
    /// * a `DisplayN` naming a display that is not attached becomes the primary rung;
    /// * **the primary rung itself becomes `Display1` where no primary is IDENTIFIABLE.** winit
    ///   documents `ActiveEventLoop::primary_monitor` as "Returns `None` if it can't identify any
    ///   monitor as a primary one", and then, verbatim: "**Wayland / Web:** Always returns `None`"
    ///   (vendored winit-0.30.13/src/event_loop.rs:405-411). Bevy hands `MonitorSelection::Primary`
    ///   straight to that optional handle (`Primary => primary_monitor`,
    ///   bevy_winit-0.19.0/src/winit_windows.rs:521), so on Wayland both the explicit PRIMARY rung
    ///   AND the unplugged-display fallback above would have resolved to `None` — a warning and a
    ///   window that never moves. `Index(0)` is the documented substitute: it always exists when
    ///   anything is attached, and it is what "the first display" means on a stack that refuses to
    ///   rank them.
    ///
    /// [`AttachedDisplays::primary`] is what makes that decidable rather than a `cfg`: bevy inserts
    /// `PrimaryMonitor` only on the entity whose handle matched `primary_monitor()`
    /// (bevy_winit-0.19.0/src/system.rs:217-219), so "monitors exist and none is marked" IS winit's
    /// `None`, observed rather than guessed at per platform. It is a LAGGING observation, not a live
    /// one — see that field for the hot-plug limitation and what [`DisplaySelection::monitor_on`]
    /// does about it.
    ///
    /// **An EMPTY display list is the identity**, and that is the same tri-state discipline
    /// [`PresentCaps`] is built around: no [`Monitor`] entities means the list has not been
    /// synchronised yet (or there is no winit at all — every bare-`App` test), which is "we could
    /// not learn", not "there are no displays". Falling back on that would let a headless frame
    /// decide a player's row.
    pub(crate) fn resolve(self, displays: AttachedDisplays) -> Self {
        if displays.count == 0 {
            return self;
        }
        let wants_primary = match self {
            DisplaySelection::Auto => return DisplaySelection::Auto,
            DisplaySelection::Primary => true,
            indexed => indexed.index().is_some_and(|index| index >= displays.count),
        };
        if !wants_primary {
            return self;
        }
        if displays.primary.is_some() {
            DisplaySelection::Primary
        } else {
            // Index(0): always attached, and the only honest reading of "primary" where the window
            // system declines to name one — or where bevy's marker has gone stale (see
            // `AttachedDisplays::primary`).
            DisplaySelection::Display1
        }
    }

    /// The rung's name when nothing has been learned about the machine's monitors. ASCII only — it
    /// reaches `Text`. One-based, per the variant docs.
    ///
    /// **This is the fallback, not the label a player normally reads.** A variant cannot name a
    /// panel: `DISPLAY 2` says only where a rung sits in a list. Once bevy has synchronised the
    /// monitors, [`displays::DisplayLadder`] names every rung from the display itself and only
    /// [`DisplaySelection::Auto`] still renders from here.
    const fn label(self) -> &'static str {
        match self {
            DisplaySelection::Auto => "AUTO",
            DisplaySelection::Primary => "PRIMARY",
            DisplaySelection::Display1 => "DISPLAY 1",
            DisplaySelection::Display2 => "DISPLAY 2",
            DisplaySelection::Display3 => "DISPLAY 3",
        }
    }

    /// Every variant, in ladder order: `Auto` first because it is the default and the "leave it
    /// alone" rung; the rest ascend so the right arrow always walks further down the display list.
    ///
    /// This is the ladder ONLY where nothing has been learned about the machine's monitors — see
    /// [`displays::DisplayLadder`], which offers a rung per attached display instead. Everywhere
    /// else it is the variant sweep: the persistence round-trip walks it to prove every rung an old
    /// file can hold still parses.
    pub(crate) const ORDER: [DisplaySelection; 2 + DisplaySelection::INDEXED] = [
        DisplaySelection::Auto,
        DisplaySelection::Primary,
        DisplaySelection::Display1,
        DisplaySelection::Display2,
        DisplaySelection::Display3,
    ];
}

/// What this machine's display list looks like RIGHT NOW — the fact
/// [`DisplaySelection::resolve`] is resolved against, gathered once by [`read_displays`] so the two
/// window writers cannot form different opinions of it.
///
/// Both fields are observations of bevy's own [`Monitor`] entities rather than platform guesses;
/// see [`DisplaySelection::resolve`] for what each one decides.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub(crate) struct AttachedDisplays {
    /// How many displays are attached. **ZERO means "not synchronised yet"**, never "no displays" —
    /// the tri-state discipline again.
    pub(crate) count: usize,
    /// Bevy's `PrimaryMonitor` entity, if still attached: the primary when it was first seen.
    ///
    /// `create_monitors` skips known monitors before inserting the marker and never reassigns or
    /// removes it. Only spawning a newly seen primary or despawning a removed monitor changes it.
    /// Hot-plug can therefore leave the new primary unmarked or an obsolete primary marked.
    /// Source: bevy_winit-0.19.1/src/system.rs:184-190,217-236 (`create_monitors`).
    ///
    /// [`DisplaySelection::monitor_on`] emits `MonitorSelection::Entity` so even a stale marker names
    /// a real attached display, avoiding Wayland's unresolvable `Primary` request. After the marked
    /// primary is unplugged, PRIMARY means the first display until Bevy can mark a new one. This
    /// Bevy-side sync limitation cannot be resolved from these observations.
    pub(crate) primary: Option<Entity>,
}

/// Gather once so `apply_settings`, `observe_window_placement` and the display ladder resolve
/// identically (see the observer's doc for the clobber a disagreement causes). Require `Monitor`
/// beside the primary marker: `find_entity` cannot resolve an entity outside the monitor list.
fn read_displays(
    monitors: &Query<(), With<Monitor>>,
    primaries: &Query<Entity, (With<Monitor>, With<PrimaryMonitor>)>,
) -> AttachedDisplays {
    AttachedDisplays {
        count: monitors.iter().count(),
        primary: primaries.iter().next(),
    }
}

/// The screen-space error budget the LOD chain is allowed to spend, in PIXELS: the largest
/// silhouette deviation a simplified mesh may project to before the next-finer level has to be
/// used. Lower is crisper and costs more triangles.
///
/// [`LodPixelBudget`] carries the applied value; `geometry_lod` reads it.
///
/// # The contract for whoever wires the consumer
///
/// Consumers recompute their `VisibilityRange` thresholds FROM this number when it changes — the
/// budget is a screen-space quantity, so a threshold derived from it is only valid for the fov,
/// resolution and render scale it was derived under. Recomputing is a HUMAN-RATE event (a settings
/// row moved, a window resized, a render-scale rung changed), not a per-frame one: read
/// [`LodPixelBudget`] behind `resource_changed` (plus whatever view-parameter change detection the
/// consumer needs) and rebuild the ladder there, never in a per-entity loop.
///
/// Stored on disk as the bare `f32` (`#[serde(transparent)]`), so `lod_pixel_budget: 2.0` is what a
/// player finds in `video.ron`. A hand-edited value outside the ladder is honoured as written and
/// clamped only at the absurd ends by [`PixelBudget::pixels`] — the same shape [`FrameCap`] has.
///
/// Nonfinite values are refused during deserialization and reported through the corrupt-file
/// outcome. NaN would break Settings' reflexive equality and defeat the page's no-op write guards.
/// Finite off-ladder edits remain exact until the player touches the control.
#[derive(Clone, Copy, PartialEq, Debug, Serialize)]
#[serde(transparent)]
pub(crate) struct PixelBudget(pub(crate) f32);

impl<'de> Deserialize<'de> for PixelBudget {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let budget = f32::deserialize(deserializer)?;
        if !budget.is_finite() {
            return Err(serde::de::Error::invalid_value(
                serde::de::Unexpected::Float(f64::from(budget)),
                &"a finite lod_pixel_budget; replace NaN or infinity with a finite number",
            ));
        }
        Ok(Self(budget))
    }
}

impl Default for PixelBudget {
    fn default() -> Self {
        Self(1.0)
    }
}

impl PixelBudget {
    /// A budget under a pixel is below what a human eyeball can resolve on the silhouette, and one
    /// over eight is a visibly popping model — the two ends of the range worth offering.
    pub(crate) const MIN: f32 = 0.5;
    pub(crate) const MAX: f32 = 8.0;
    /// The ladder step. Half a pixel, so the low end — where the difference is actually visible —
    /// is finely divided rather than jumping straight from 0.5 to 1.
    const STEP: f32 = 0.5;
    /// Number of discrete stops, MIN..=MAX inclusive — `1 + (MAX - MIN) / STEP`, written out
    /// because a float-to-integer cast is not something to do in a `const` item. The assertion
    /// after this impl pins it against the three constants above.
    const STOPS: u16 = 16;

    /// The budget in screen pixels. The clamp is the guard against a hand-edited
    /// `lod_pixel_budget: 0.0` demanding an infinitely fine ladder (or a negative one inverting the
    /// comparison every consumer will write).
    ///
    /// Parsed values are finite; a nonfinite value constructed in code is a bug. A NaN would make
    /// every consumer comparison false and silently select no LOD.
    pub(crate) fn pixels(self) -> f32 {
        assert!(
            self.0.is_finite(),
            "PixelBudget comes from finite preset data or checked settings input"
        );
        self.0.clamp(Self::MIN, Self::MAX)
    }

    /// ASCII only — it reaches `Text`. Honest pixels, like every other value on this page.
    pub(crate) fn label(self) -> String {
        format!("{:.1} PX", self.pixels())
    }

    /// The nearest slider stop for the current value. A hand-edited off-ladder value (1.2) resolves
    /// to its nearest stop the moment the player TOUCHES the control, and not before.
    fn stop(self) -> u16 {
        (((self.pixels() - Self::MIN) / Self::STEP).round() as u16).min(Self::STOPS - 1)
    }

    fn from_stop(stop: u16) -> Self {
        Self(Self::MIN + f32::from(stop.min(Self::STOPS - 1)) * Self::STEP)
    }

    /// One keyboard step along the ladder, saturating at both ends like every other row.
    pub(crate) fn step(self, delta: i32) -> Self {
        let stop = (i32::from(self.stop()) + delta).clamp(0, i32::from(Self::STOPS) - 1);
        Self::from_stop(stop as u16)
    }

    /// Where the slider handle sits, `0.0..=1.0`.
    pub(crate) fn fraction(self) -> f32 {
        f32::from(self.stop()) / f32::from(Self::STOPS - 1)
    }

    /// The value a drag to `fraction` lands on, snapped to the ladder.
    pub(crate) fn from_fraction(fraction: f32) -> Self {
        let stops = f32::from(Self::STOPS - 1);
        Self::from_stop((fraction.clamp(0.0, 1.0) * stops).round() as u16)
    }
}

const _: () = assert!(
    PixelBudget::STOPS as f32 == 1.0 + (PixelBudget::MAX - PixelBudget::MIN) / PixelBudget::STEP
);

/// Applied screen-space error budget, pixels, written by [`apply_settings`]. Consumers depend on
/// this small resource rather than the persisted intent. Its change triggers human-rate
/// `VisibilityRange` rebuilds under [`PixelBudget`]'s recompute contract.
#[derive(Resource, Clone, Copy, PartialEq, Debug)]
pub(crate) struct LodPixelBudget(pub(crate) f32);

impl Default for LodPixelBudget {
    fn default() -> Self {
        Self(PixelBudget::default().pixels())
    }
}

/// The frame-rate cap: `0` = off, anything else a target FPS honoured by `limiter`. Only ACTIVE
/// while the EFFECTIVE rung is [`VsyncMode::Off`] (see [`Settings::effective_vsync`]) — with any
/// present-mode wait in play the display is already the cap,
/// and two competing limiters make a stutter machine (see [`Settings::frame_limit_period`], the one
/// place that conjunction is decided).
///
/// Stored on disk as the bare `u16` (`#[serde(transparent)]`), so `frame_cap: 144` is what a player
/// finds in `video.ron`. A hand-edited value is honoured as written while merely OUT of the UI
/// ladder (144 renders and caps as 144); only [`FrameCap::fps`]'s clamp bounds truly absurd values.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default, Serialize, Deserialize)]
#[serde(transparent)]
pub(crate) struct FrameCap(pub(crate) u16);

impl FrameCap {
    pub(crate) const OFF: FrameCap = FrameCap(0);
    /// The UI ladder: OFF, then MIN..=MAX in STEP increments.
    pub(crate) const MIN_FPS: u16 = 30;
    pub(crate) const MAX_FPS: u16 = 240;
    const STEP_FPS: u16 = 10;
    /// Number of discrete stops on the slider: OFF plus every STEP from MIN to MAX inclusive.
    const STOPS: u16 = 2 + (Self::MAX_FPS - Self::MIN_FPS) / Self::STEP_FPS;

    /// The cap in frames per second, or `None` for off. The clamp is the guard against a
    /// hand-edited `frame_cap: 5` starving the game (or `60000` meaning "spin forever").
    pub(crate) fn fps(self) -> Option<u16> {
        (self.0 != 0).then(|| self.0.clamp(Self::MIN_FPS, Self::MAX_FPS))
    }

    /// The frame period this cap asks for, or `None` for off.
    pub(crate) fn period(self) -> Option<std::time::Duration> {
        self.fps()
            .map(|fps| std::time::Duration::from_secs_f64(1.0 / f64::from(fps)))
    }

    /// ASCII only — it reaches `Text`.
    pub(crate) fn label(self) -> String {
        match self.fps() {
            None => "OFF".to_string(),
            Some(fps) => format!("{fps} FPS"),
        }
    }

    /// The nearest slider stop for the current value (`0` = OFF). A hand-edited off-ladder value
    /// (144) resolves to its nearest stop (140) the moment the player TOUCHES the control, and not
    /// before.
    fn stop(self) -> u16 {
        match self.fps() {
            None => 0,
            Some(fps) => 1 + (fps - Self::MIN_FPS + Self::STEP_FPS / 2) / Self::STEP_FPS,
        }
    }

    fn from_stop(stop: u16) -> Self {
        if stop == 0 {
            Self::OFF
        } else {
            Self(Self::MIN_FPS + (stop.min(Self::STOPS - 1) - 1) * Self::STEP_FPS)
        }
    }

    /// One keyboard step along the ladder, saturating at both ends like every other row.
    pub(crate) fn step(self, delta: i32) -> Self {
        let stop = (i32::from(self.stop()) + delta).clamp(0, i32::from(Self::STOPS) - 1);
        Self::from_stop(stop as u16)
    }

    /// Where the slider handle sits, `0.0..=1.0` (OFF is the far left).
    pub(crate) fn fraction(self) -> f32 {
        f32::from(self.stop()) / f32::from(Self::STOPS - 1)
    }

    /// The value a drag to `fraction` lands on — the inverse of [`FrameCap::fraction`], snapped to
    /// the ladder.
    pub(crate) fn from_fraction(fraction: f32) -> Self {
        let stops = f32::from(Self::STOPS - 1);
        Self::from_stop((fraction.clamp(0.0, 1.0) * stops).round() as u16)
    }
}

/// UI scale, in percent — a multiplier on bevy's [`UiScale`] resource, so every `Val::Px` in the
/// HUD and menus scales while the 3D world (and [`RenderScaleLevel`]) is untouched.
///
/// Stored on disk as the bare `u16` (`#[serde(transparent)]`): `ui_scale: 125`. Values outside the
/// ladder are clamped by [`UiScalePercent::factor`] — a hand-edited `ui_scale: 500` must not make
/// the settings page itself unreachable, which is also why the range is deliberately modest.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
#[serde(transparent)]
pub(crate) struct UiScalePercent(pub(crate) u16);

impl Default for UiScalePercent {
    fn default() -> Self {
        Self(100)
    }
}

impl UiScalePercent {
    pub(crate) const MIN: u16 = 75;
    pub(crate) const MAX: u16 = 150;
    const STEP: u16 = 5;
    const STOPS: u16 = 1 + (Self::MAX - Self::MIN) / Self::STEP;

    /// The multiplier written to [`UiScale`]. Clamped — see the type doc.
    pub(crate) fn factor(self) -> f32 {
        f32::from(self.0.clamp(Self::MIN, Self::MAX)) / 100.0
    }

    /// ASCII only — it reaches `Text`.
    pub(crate) fn label(self) -> String {
        format!("{}%", self.0.clamp(Self::MIN, Self::MAX))
    }

    fn stop(self) -> u16 {
        (self.0.clamp(Self::MIN, Self::MAX) - Self::MIN + Self::STEP / 2) / Self::STEP
    }

    fn from_stop(stop: u16) -> Self {
        Self(Self::MIN + stop.min(Self::STOPS - 1) * Self::STEP)
    }

    /// One keyboard step along the ladder, saturating at both ends.
    pub(crate) fn step(self, delta: i32) -> Self {
        let stop = (i32::from(self.stop()) + delta).clamp(0, i32::from(Self::STOPS) - 1);
        Self::from_stop(stop as u16)
    }

    /// Where the slider handle sits, `0.0..=1.0`.
    pub(crate) fn fraction(self) -> f32 {
        f32::from(self.stop()) / f32::from(Self::STOPS - 1)
    }

    /// The value a drag to `fraction` lands on, snapped to the ladder.
    pub(crate) fn from_fraction(fraction: f32) -> Self {
        let stops = f32::from(Self::STOPS - 1);
        Self::from_stop((fraction.clamp(0.0, 1.0) * stops).round() as u16)
    }
}

/// Read the retired v1 `vsync: bool` key: a bare bool in the file, `Some(bool)` in the shadow
/// field. See [`Settings::legacy_vsync`] for why this exists (RON refuses a bare value into an
/// `Option`).
fn deserialize_legacy_vsync<'de, D>(deserializer: D) -> Result<Option<bool>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    bool::deserialize(deserializer).map(Some)
}

/// The sparse-write predicate. Every skipped field's [`Settings::default`] must equal its type
/// default; `settings_defaults_are_their_types_defaults` guards against accidentally persisting
/// a default or omitting a choice.
fn is_default<T: Default + PartialEq>(value: &T) -> bool {
    *value == T::default()
}

/// The persisted player settings — and, as a `Resource`, the live truth the renderer is reconciled
/// against by [`apply_settings`]. Nothing else may write `Msaa`, `CascadeShadowConfig`,
/// `DirectionalLightShadowMap`, `Window::present_mode` or a battlefield camera's
/// `Bloom::max_mip_dimension`.
///
/// **The file is a SPARSE DIFF against the current defaults**, which is what the per-field
/// `skip_serializing_if` buys. Writing every field would freeze today's defaults into every player's
/// file: `shadows: Medium` on disk is indistinguishable from a deliberate choice, so a shipped
/// default could never be improved for existing players again. (That is the bug `television` had to
/// write a 318-line config-archaeology system to undo — reconstructing every default it ever
/// shipped from git history to guess which values were choices.) RimWorld's `Scribe_Values.Look`
/// takes the same shortcut, and its `forceSave` flag is why [`Settings::version`] is the one field
/// exempt: a fully-default file must still carry a version stamp, or it probes as version 0.
#[derive(Resource, Clone, Copy, PartialEq, Debug, Serialize, Deserialize)]
#[serde(default)]
pub(crate) struct Settings {
    /// See [`SETTINGS_VERSION`]. ALWAYS written — never given a `skip_serializing_if`.
    pub(crate) version: u32,
    /// How far shadows are drawn, or [`ShadowDistance::Off`]. Together with `shadow_resolution` this
    /// REPLACED a single coupled `shadows` preset; a `video.ron` still carrying `shadows:` hits
    /// serde's unknown-field tolerance and lands both of these on their defaults, which is exactly
    /// the module doc's stated remove-a-field policy and is why [`SETTINGS_VERSION`] did not move.
    #[serde(skip_serializing_if = "is_default")]
    pub(crate) shadow_distance: ShadowDistance,
    #[serde(skip_serializing_if = "is_default")]
    pub(crate) shadow_resolution: ShadowResolution,
    /// The cascade count — runtime changes require bevy_light >= 0.19.1; see
    /// [`ShadowCascades`] and [`SHADOW_CASCADES`].
    #[serde(skip_serializing_if = "is_default")]
    pub(crate) shadow_cascades: ShadowCascades,
    #[serde(skip_serializing_if = "is_default")]
    pub(crate) msaa: MsaaLevel,
    /// See [`BloomLevel`].
    #[serde(skip_serializing_if = "is_default")]
    pub(crate) bloom: BloomLevel,
    /// Persisted as `vsync_mode` — a NEW key, because the retired `vsync` key was a `bool` and the
    /// ladder grew a third rung. See [`VsyncMode`]'s migration note.
    #[serde(rename = "vsync_mode", skip_serializing_if = "is_default")]
    pub(crate) vsync: VsyncMode,
    /// The retired v1 `vsync: bool` key — READ for back-compat, NEVER written. Folded into
    /// [`Settings::vsync`] by [`Settings::absorb_legacy_vsync`] (which `store::parse` calls), after
    /// which this is always `None`. Not a live setting: nothing outside the parse path may read it.
    ///
    /// The `deserialize_with` is load-bearing: the file carries a BARE bool (`vsync: false`), and
    /// RON will not read a bare value into an `Option` (it demands `Some(false)`) — the shim reads
    /// the bool and wraps it, while an absent key still takes the `default` `None`.
    #[serde(
        rename = "vsync",
        default,
        skip_serializing,
        deserialize_with = "deserialize_legacy_vsync"
    )]
    pub(crate) legacy_vsync: Option<bool>,
    /// The fraction of the window the 3D pass renders at — applied by writing
    /// [`crate::render_scale::RenderScale`], which the render app extracts.
    #[serde(skip_serializing_if = "is_default")]
    pub(crate) render_scale: RenderScaleLevel,
    /// Windowed / borderless fullscreen — see [`WindowModeSetting`], including the reflect-don't-
    /// fight rule for OS-side toggles.
    #[serde(skip_serializing_if = "is_default")]
    pub(crate) window_mode: WindowModeSetting,
    /// Which monitor the window is centred on — see [`DisplaySelection`], including why the boot
    /// window is NOT described with it and why an absent display is never written back.
    #[serde(skip_serializing_if = "is_default")]
    pub(crate) display: DisplaySelection,
    /// The LOD screen-space error budget in pixels — see [`PixelBudget`]. Applied by writing
    /// [`LodPixelBudget`], which `geometry_lod` reads.
    #[serde(skip_serializing_if = "is_default")]
    pub(crate) lod_pixel_budget: PixelBudget,
    /// The frame-rate cap, active only under [`VsyncMode::Off`] — see [`FrameCap`].
    #[serde(skip_serializing_if = "is_default")]
    pub(crate) frame_cap: FrameCap,
    /// UI scale percent — a multiplier on [`UiScale`]; see [`UiScalePercent`].
    #[serde(skip_serializing_if = "is_default")]
    pub(crate) ui_scale: UiScalePercent,
    /// The online debug card — see [`DebugPanel`].
    #[serde(skip_serializing_if = "is_default")]
    pub(crate) debug_panel: DebugPanel,
    /// Which of the player's hits open the pen cam's corner — see [`PenCamCorner`].
    #[serde(skip_serializing_if = "is_default")]
    pub(crate) pen_cam_corner: PenCamCorner,
    /// See [`KillCam`].
    #[serde(skip_serializing_if = "is_default")]
    pub(crate) kill_cam: KillCam,
    /// See [`ReduceMotion`].
    #[serde(skip_serializing_if = "is_default")]
    pub(crate) reduce_motion: ReduceMotion,
    /// See [`TrackPrintRelief`].
    #[serde(skip_serializing_if = "is_default")]
    pub(crate) track_prints: TrackPrintRelief,
    /// See [`TrackPrintRange`].
    #[serde(skip_serializing_if = "is_default")]
    pub(crate) track_print_range: TrackPrintRange,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            version: SETTINGS_VERSION,
            shadow_distance: ShadowDistance::default(),
            shadow_resolution: ShadowResolution::default(),
            shadow_cascades: ShadowCascades::default(),
            msaa: MsaaLevel::default(),
            bloom: BloomLevel::default(),
            vsync: VsyncMode::default(),
            legacy_vsync: None,
            render_scale: RenderScaleLevel::default(),
            window_mode: WindowModeSetting::default(),
            display: DisplaySelection::default(),
            lod_pixel_budget: PixelBudget::default(),
            frame_cap: FrameCap::default(),
            ui_scale: UiScalePercent::default(),
            debug_panel: DebugPanel::default(),
            pen_cam_corner: PenCamCorner::default(),
            kill_cam: KillCam::default(),
            reduce_motion: ReduceMotion::default(),
            track_prints: TrackPrintRelief::default(),
            track_print_range: TrackPrintRange::default(),
        }
    }
}

impl Settings {
    /// Effective vsync shared by present mode, frame-cap gate and page greying. Otherwise a Windows/
    /// Vulkan file naming FAST can show FAST over tearing Metal `Immediate`, or a stored Wayland OFF
    /// can arm a second limiter over compositor-paced presentation.
    ///
    /// Only a reported list resolves a rung; unknown states preserve player intent, while
    /// [`Settings::present_mode`] negotiates safely without concrete uncapped modes.
    ///
    /// [`normalize_vsync`] persists the resolution. A file roamed between Metal and Vulkan thus
    /// forgets FAST on the laptop's first launch: accepted over values whose meaning depends on
    /// which machine last read them.
    pub(crate) const fn effective_vsync(self, caps: PresentCaps) -> VsyncMode {
        caps.resolve(self.vsync)
    }

    /// The present mode this configuration asks for, GIVEN what the surface is known to support.
    /// Split out so the window can be BUILT with it (before the surface is ever configured, with
    /// [`PresentCaps::Unprobed`]) and so [`apply_settings`] reuses the one mapping.
    ///
    /// **The safety property, restated for the three-rung ladder.** On Metal, wgpu-hal's `Mailbox`
    /// and `FifoRelaxed` reach an `unreachable!()` at surface-configure time — they PANIC if a
    /// surface is actually configured with them (wgpu-hal metal `adapter.rs`, `surface.rs`). This
    /// mapping therefore emits `Mailbox`/`Immediate` ONLY when the probe has positively reported
    /// them in the surface's own capability list; in every other state the uncapped rungs answer
    /// [`PresentMode::AutoNoVsync`], whose fallback chain bevy filters against the real
    /// capabilities and terminates at the universal `Fifo`. `AutoVsync` stays unreachable — its
    /// first fallback choice is the panicking `FifoRelaxed`, and `Fifo` says what "on" means
    /// without negotiation.
    pub(crate) const fn present_mode(self, caps: PresentCaps) -> PresentMode {
        match self.effective_vsync(caps) {
            VsyncMode::On => PresentMode::Fifo,
            VsyncMode::Fast => {
                if caps.mailbox() {
                    PresentMode::Mailbox
                } else {
                    PresentMode::AutoNoVsync
                }
            }
            VsyncMode::Off => {
                if caps.immediate() {
                    PresentMode::Immediate
                } else {
                    PresentMode::AutoNoVsync
                }
            }
        }
    }

    /// Limiter period, or `None` when disarmed. Gate on [`Settings::effective_vsync`], as the page
    /// does, so an unsupported stored OFF cannot arm the limiter even before [`normalize_vsync`]
    /// writes the correction.
    pub(crate) fn frame_limit_period(self, caps: PresentCaps) -> Option<std::time::Duration> {
        match self.effective_vsync(caps) {
            VsyncMode::Off => self.frame_cap.period(),
            VsyncMode::Fast | VsyncMode::On => None,
        }
    }

    /// Fold the retired v1 `vsync: bool` key into the live [`VsyncMode`] — called by `store::parse`
    /// right after deserialization, so the legacy key never escapes the persistence seam.
    ///
    /// The rule: the legacy key is honoured only while the NEW key holds its default. v1 files can
    /// only ever carry `vsync: false` (the sparse writer skipped the default `true`), and a file
    /// with both keys was hand-edited — an explicit non-default `vsync_mode` is the newer, more
    /// specific statement, so it wins.
    pub(crate) fn absorb_legacy_vsync(mut self) -> Self {
        if let Some(legacy) = self.legacy_vsync.take()
            && self.vsync == VsyncMode::default()
        {
            self.vsync = if legacy {
                VsyncMode::On
            } else {
                VsyncMode::Off
            };
        }
        self
    }
}

// --- applying -----------------------------------------------------------------------------------

/// Request that the CURRENT [`Settings`] be written to disk — the ONLY thing that writes the file
/// (see the module doc's writer table).
#[derive(Message)]
pub(crate) struct SaveSettings;

/// Ordering handle for the reconcilers, so a UI or knob write in the same frame is applied after it
/// lands rather than a frame later.
#[derive(SystemSet, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct ApplySettings;

/// What the boot read found, carried from before `LogPlugin` existed so it can be reported once the
/// app is up. See [`store::LoadNote`] for why this is data rather than a log line.
#[derive(Resource)]
pub(crate) struct StoreReport(store::LoadNote);

/// Last save failure, if any; retained until a later successful save so closing/reopening the page
/// cannot turn a refused write into a success notice.
#[derive(Resource, Default)]
struct SaveStatus(Option<store::SaveError>);

/// Load settings before `DefaultPlugins` in each windowed root, returning them for primary
/// `Window` construction as well as inserting them. The present mode must precede surface
/// configuration to avoid a visible first-frame reconfigure; queued window-mode/size research
/// fields also require creation-time application.
///
/// Insert values and report together: [`plugin`] finds the values already present, and
/// [`report_store_load`] logs the report at Startup, after `LogPlugin` installs a subscriber.
///
/// Missing/corrupt files yield defaults with a note. [`WindowModeSetting`] rules out exclusive
/// fullscreen on an unresolvable monitor, so no row needs an escape hatch outside the UI.
pub(crate) fn load_at_boot(app: &mut App) -> Settings {
    let path = store::StorePath::default();
    let store::Load { settings, note } = store::load(path.0.as_deref());
    app.insert_resource(path);
    app.insert_resource(settings);
    app.insert_resource(StoreReport(note));
    settings
}

/// Which pause surface the settings page is the content of — the ONE thing the two windowed roots
/// do differently, and therefore the one thing [`plugin`] takes as a parameter.
///
/// It is a parameter rather than a system each root remembers to add because forgetting the
/// declarer is silent: the page mounts, never becomes visible, and nothing logs. Making the entry
/// part of mounting the page makes that state **unrepresentable**.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum PageEntry {
    /// Single-player: the page is up exactly while `AppState::Paused`, which already owns the Esc
    /// toggle, the cursor release and the physics freeze.
    PauseState,
    /// Net client: the page is the CONTENT of `Overlay::Menu`'s scrim, which already blocks input,
    /// frees the cursor and is priority-gated against the connect/death screens.
    OverlayMenu,
}

/// The whole player-facing display feature: the model, its reconcilers, the render-scale render-app
/// half, the settings page, and the page's entry declarer. Mounted by both windowed roots; NEVER by
/// the headless server, which has no camera, no window and no shadow maps to configure.
///
/// [`crate::render_scale::plugin`] is mounted HERE rather than beside this one in each root because
/// [`apply_settings`] is the one writer of the resource it owns: a root that mounted the settings
/// page without it would silently apply every row except render scale. (It is safe to mount without
/// a render app — its render-half early-returns — but no path in this tree does that.)
pub(crate) fn plugin(entry: PageEntry) -> impl Plugin {
    move |app: &mut App| {
        app.add_plugins((crate::render_scale::plugin, probe::plugin, ui::plugin))
            .init_resource::<Settings>()
            .init_resource::<store::StorePath>()
            // Omitting this required resource panics on the first reconcile; it cannot just skip a row.
            .init_resource::<LodPixelBudget>()
            // The window-placement state both window writers share — see [`WindowPlacement`] for
            // why an edge memory cannot be a `Local` here.
            .init_resource::<WindowPlacement>()
            .add_message::<SaveSettings>()
            // `Startup` applies the loaded file to a world that has just spawned its camera and sun;
            // after that the reconcilers are change-driven, so they cost a resource-tick comparison
            // per frame and nothing else.
            .add_systems(
                Startup,
                (apply_settings.in_set(ApplySettings), report_store_load),
            )
            .add_systems(
                Update,
                (
                    // BEFORE the page (and therefore before this frame's edits): the observed OS
                    // state is this frame's baseline, which player input then overrides.
                    observe_window_placement.before(ui::DeclareSettingsPage),
                    // Same shape, same reason, for the capability side: the probe's answer is the
                    // baseline the page renders and the player then edits. Runs only on a caps
                    // change, which after boot means exactly once — when the probe lands.
                    normalize_vsync
                        .before(ApplySettings)
                        .before(ui::DeclareSettingsPage)
                        .run_if(resource_changed::<PresentCaps>),
                    apply_settings
                        .in_set(ApplySettings)
                        // Also on a capability-probe arrival: FAST/OFF may upgrade from the
                        // negotiated `AutoNoVsync` to the probed `Mailbox`/`Immediate`.
                        .run_if(
                            resource_changed::<Settings>.or_else(resource_changed::<PresentCaps>),
                        ),
                    save_on_request.after(ApplySettings),
                ),
            )
            // The frame-rate limiter runs LAST in the frame, after everything that could still do
            // work — see `limiter` for the coarse-sleep-then-spin shape and its measurement.
            .add_systems(Last, limiter::limit_frame_rate);
        // Both declarers land in `DeclareSettingsPage`, which the page's input/refresh chain runs
        // `.after` — see that set's doc for the dropped-keypress bug the ordering exists to stop.
        match entry {
            PageEntry::PauseState => {
                app.add_systems(
                    Update,
                    ui::declare_from_pause_state.in_set(ui::DeclareSettingsPage),
                );
            }
            PageEntry::OverlayMenu => {
                app.add_systems(
                    Update,
                    ui::declare_from_overlay_menu
                        // After the reconciled overlay set, like every other consumer of it, so the
                        // same frame's declarations have landed.
                        .after(crate::overlay::OverlaySet::Toggle)
                        .in_set(ui::DeclareSettingsPage),
                );
            }
        }
    }
}

/// "An external capture harness has PINNED this window; settings must not manage it." Inserted by
/// every hidden run (`run_window::hide`: a documentation capture, `OVERMATCH_WINDOW=hidden` or
/// `offscreen`), whose window is deliberately `Windowed` + invisible: [`apply_settings`]
/// re-applying a persisted fullscreen at runtime would order that window front on macOS regardless
/// of `visible: false`, defeating the no-focus-steal guarantee the creation-time fields bought.
///
/// It lives HERE, not beside the harness, because of the layering gate
/// (`src/net/tests/net_boundary.rs`): the sim/settings layer may never name the netcode layer —
/// single-player is a runtime mode with no netcode mounted — so the netcode-side harness inserts
/// a settings-OWNED marker rather than settings reading a netcode-owned one. The marker states a
/// window/presentation contract, which is settings' domain; who pins the window is the
/// inserter's business.
#[derive(Resource)]
pub(crate) struct CaptureWindowPinned;

/// Every public cascade field participates: an overlap-only change must reach the renderer.
fn reconcile_shadow_config(config: &mut CascadeShadowConfig, rebuilt: CascadeShadowConfig) {
    if config.bounds != rebuilt.bounds
        || config.overlap_proportion != rebuilt.overlap_proportion
        || config.minimum_distance != rebuilt.minimum_distance
    {
        info!(
            "render_cost: cascades applied bounds={:?} overlap={} minimum={}",
            rebuilt.bounds, rebuilt.overlap_proportion, rebuilt.minimum_distance
        );
        *config = rebuilt;
    }
}

/// Reconcile the rendering world to [`Settings`]. The ONE writer of every knob below, which is what
/// makes "the page and the picture cannot disagree" true by construction rather than by discipline.
#[expect(
    clippy::too_many_arguments,
    reason = "the one writer of every rendering knob: each parameter is one knob's world target"
)]
fn apply_settings(
    settings: Res<Settings>,
    shadow_controls: Option<Res<crate::render_cost::ShadowControls>>,
    mut shadow_map: ResMut<DirectionalLightShadowMap>,
    // A camera or light with a profile that is not the battlefield's (the pen cam's inset, masks
    // and key lights) keeps its own: the masks must not multisample, and the stage casts nothing.
    // `Bloom` is optional: `SPIKE_ABLATE=bloom` removes it (`render_cost`).
    mut cameras: Query<(&mut Msaa, Option<&mut Bloom>, Option<&CameraProfile>), With<Camera3d>>,
    mut lights: Query<(
        &mut DirectionalLight,
        &mut CascadeShadowConfig,
        Option<&LightProfile>,
    )>,
    // Optional because a headless/simulate root has no primary window at all — the present mode is
    // then simply nothing to apply, not a missing dependency.
    window: Option<Single<&mut Window, With<PrimaryWindow>>>,
    // Required: `plugin` mounts `render_scale::plugin` on every path to this system.
    mut render_scale: ResMut<crate::render_scale::RenderScale>,
    // What the surface reported (or an unknown state, before/instead of that) — the gate on the
    // uncapped modes.
    caps: Res<PresentCaps>,
    // bevy_ui's global multiplier. Present on every windowed root (it comes with `UiPlugin`); a
    // bare-`App` test must init it, same as the shadow map above.
    mut ui_scale: ResMut<UiScale>,
    // Present exactly on a hidden capture run (see `CaptureWindowPinned`) — the window sub-block
    // below must not run for it.
    capture_pinned: Option<Res<CaptureWindowPinned>>,
    // The applied half of the LOD budget row, read by `geometry_lod` — see `PixelBudget`.
    mut lod_budget: ResMut<LodPixelBudget>,
    // The attached displays, as `bevy_winit::create_monitors` synchronised them. COUNTED, never
    // ordered: `MonitorSelection::Index` indexes bevy's own `WinitMonitors` vec, so this system
    // only needs how many rungs of the ladder are real — plus whichever monitor bevy last marked
    // primary (`read_displays`).
    monitors: Query<(), With<Monitor>>,
    primaries: Query<Entity, (With<Monitor>, With<PrimaryMonitor>)>,
    // The placement state both window writers share — the position edge, and the round trip this
    // system yields the mode to. NOT a `Local`: see [`WindowPlacement`].
    mut placement: ResMut<WindowPlacement>,
) {
    let msaa = settings.msaa.to_msaa();
    let bloom_mips = settings.bloom.max_mip_dimension();
    for (mut camera_msaa, bloom, profile) in &mut cameras {
        if profile.is_none_or(|profile| profile.draws_battlefield()) {
            camera_msaa.set_if_neq(msaa);
            // Compared by hand — `Bloom` derives no `PartialEq`.
            if let Some(mut bloom) = bloom
                && bloom.max_mip_dimension != bloom_mips
            {
                bloom.max_mip_dimension = bloom_mips;
            }
        }
    }

    // Resolution applies even while distance is Off; distance and count share the config rebuild.
    let size = settings.shadow_resolution.shadow_map_size();
    // Compare before writing: `DirectionalLightShadowMap` is extracted to the render world every
    // frame, and touching it through `ResMut` unconditionally would mark it changed forever. (Hand
    // written rather than `set_if_neq` — the bevy type derives no `PartialEq`.)
    if shadow_map.size != size {
        shadow_map.size = size;
    }
    let distance = settings.shadow_distance;
    for (mut light, mut config, profile) in &mut lights {
        if profile.is_some_and(|profile| !profile.lights_battlefield()) {
            continue;
        }
        // Off keeps the last valid config: nothing samples it while casting is disabled, and a
        // zero-metre rebuild degenerates. Pending counts apply when casting resumes; no repair is needed.
        if let Some(maximum_distance) = distance.distance_m() {
            // Runtime count changes require bevy_light >= 0.19.1 (see `SHADOW_CASCADES`).
            // Split parameters are explicit: the stock 10 m first bound halves far-field crispness on a
            // 1 km map (`SHADOW_FIRST_CASCADE_FAR_BOUND_M`). Only `minimum_distance` remains Bevy's
            // default (0.1 m near clip), which no page row should move.
            let mut rebuilt = CascadeShadowConfigBuilder {
                num_cascades: settings.shadow_cascades.count(),
                maximum_distance,
                first_cascade_far_bound: SHADOW_FIRST_CASCADE_FAR_BOUND_M,
                overlap_proportion: SHADOW_CASCADE_OVERLAP,
                ..default()
            }
            .build();
            if let Some(controls) = shadow_controls.as_deref() {
                controls.configure_cascades(&mut rebuilt);
            }
            reconcile_shadow_config(&mut config, rebuilt);
        }
        let enabled = distance.casts();
        if light.shadow_maps_enabled != enabled {
            light.shadow_maps_enabled = enabled;
        }
    }

    // Preserve every window field on a hidden run; see `CaptureWindowPinned` for the macOS
    // fullscreen focus hazard. Nobody watches its present mode either.
    if capture_pinned.is_none()
        && let Some(window) = window
    {
        let mut window = window.into_inner();
        window.present_mode = settings.present_mode(*caps);
        let displays = read_displays(&monitors, &primaries);
        // Both window fields below are derived through `monitor_on`, so the mode and the position
        // can never name different displays.
        let monitor = settings.display.monitor_on(displays);

        // Mode is level-triggered, including its display: fullscreen ignores position. The OS observer
        // can write mode without changing Settings, so reassert it to retain the selected monitor.
        // Guard the write to avoid a needless change tick.
        //
        // This also corrects boot's `Current` to a resolved `Index(n)`: Bevy reissues only a change
        // from its cached creation request (`DisplaySelection::at_window_creation`).
        //
        // Yield during `RoundTrip::Leaving`; reasserting fullscreen would cancel the transition and
        // prevent the observed edge that releases re-entry.
        if !placement.round_trip.owns_the_mode() {
            let mode = settings.window_mode.to_window_mode(monitor);
            if window.mode != mode {
                window.mode = mode;
            }
        }

        // Position is edge-triggered on the stored (mode, display) pair, with launch as the first edge.
        // Winit rewrites it to `At(..)` on every Moved event; a level write would re-centre a dragged
        // window whenever an unrelated setting or probe changed. Mode belongs in the pair so leaving
        // fullscreen applies the position ignored there.
        //
        // Read Settings, never Window::mode: a monitor round trip passes through Windowed without
        // changing the row and must not spend the placement edge.
        let applied = (settings.window_mode, settings.display);
        if placement.last_applied.replace(applied) != Some(applied) {
            let resolved = settings.display.resolve(displays);
            if resolved != settings.display {
                warn!(
                    "settings: {} is not reachable ({} display(s) present, primary {}) -> {} \
                     (not saved — displays come back)",
                    settings.display.label(),
                    displays.count,
                    if displays.primary.is_some() {
                        "marked"
                    } else {
                        "unidentifiable"
                    },
                    resolved.label(),
                );
            }
            // Fullscreen ignores `set_outer_position`; defer placement until the player leaves it.
            if settings.window_mode == WindowModeSetting::Windowed {
                let position = settings.display.window_position_on(displays);
                if window.position != position {
                    window.position = position;
                }
            }
        }
    }

    // Guarded by hand — bevy's `UiScale` derives no `PartialEq`.
    let factor = settings.ui_scale.factor();
    if ui_scale.0 != factor {
        ui_scale.0 = factor;
    }

    // Avoid marking the extracted resource changed on an unchanged value.
    render_scale.set_if_neq(crate::render_scale::RenderScale(
        settings.render_scale.fraction(),
    ));

    // Consumers rebuild `VisibilityRange` on change; an unconditional write would rebuild every
    // frame. See `PixelBudget`.
    lod_budget.set_if_neq(LodPixelBudget(settings.lod_pixel_budget.pixels()));
}

/// Persist an unsupported vsync rung's effective value; see [`Settings::effective_vsync`] for
/// the roaming-file tradeoff. Runs on caps insertion and probe arrival only: later UI edits
/// can reach only offered rungs.
///
/// [`PresentCaps::resolve`] is identity on both unknown states, so equality prevents a failed
/// probe from spending a stored FAST/OFF. Only a reported list can cause a save.
fn normalize_vsync(
    caps: Res<PresentCaps>,
    mut settings: ResMut<Settings>,
    mut save: MessageWriter<SaveSettings>,
) {
    let effective = settings.effective_vsync(*caps);
    if settings.vsync == effective {
        return;
    }
    info!(
        "settings: vsync {} is not supported by this surface -> {} (saved)",
        settings.vsync.label(),
        effective.label(),
    );
    settings.vsync = effective;
    save.write(SaveSettings);
}

/// Shared state for the two window writers. `Local` cannot hold the edge: Startup and Update
/// register separate `apply_settings` instances and would place twice at boot (MEASURED as
/// unreachable-display warnings about a second apart). The round trip also must be shared:
/// [`observe_window_placement`] drives it while [`apply_settings`] yields mode.
#[derive(Resource, Default)]
struct WindowPlacement {
    /// The `(mode, display)` pair the POSITION write is edge-triggered on — see [`apply_settings`].
    /// `None` is "never applied", and that first application IS an edge: it is the launch placement.
    last_applied: Option<(WindowModeSetting, DisplaySelection)>,
    round_trip: RoundTrip,
}

/// Move an already-fullscreen window to its target by leaving and re-entering fullscreen.
///
/// winit's macOS `set_fullscreen` has no Borderless -> Borderless arm, but commits the target
/// and calls `setFrameOrigin` before that no-op. Winit's state then agrees with the requested
/// monitor while the real window stays elsewhere; Bevy's equality guard prevents repair by
/// rewriting the mode. Requests during a Space transition are deferred and replay into the same
/// missing arm. Mechanism and rejected alternatives:
/// `upstream/winit-macos-borderless-to-borderless-fullscreen-noop.md`.
///
/// Each asynchronous half takes about 1-2 s; release re-entry only on the observed fullscreen
/// exit, never a level, retry or frame count. Re-entry honours the current target.
///
/// Arm on observed disagreement, covering boot's unresolved indexed display, hot-plug
/// (`PrimaryMonitor` is never reassigned), and a window winit has lost track of.
///
/// A failed attempt settles in [`RoundTrip::Stranded`], reported once and never retried: a
/// standing mismatch must not flap between Spaces. A changed row, changed resolved target or
/// observed agreement re-arms it.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
enum RoundTrip {
    /// Nothing in flight and nothing spent: a disagreement observed now is acted on.
    #[default]
    Idle,
    /// `Windowed` has been written and the observed fullscreen edge is what releases the second
    /// write. The target is deliberately NOT carried: the re-entry honours whatever the row says on
    /// the frame it is written, so a change made mid-transition costs nothing and interrupts
    /// nothing. While the machine is here the window is passing through a state no row asked for,
    /// which is why [`Settings`] is not written and [`apply_settings`] yields the mode.
    Leaving,
    /// The re-entry has been written for this target; where the window landed is not known yet.
    Spent(DisplaySelection),
    /// A completed trip did not put the window on this target. Said once, then held.
    Stranded(DisplaySelection),
}

/// What the OS says about the window right now — the only input [`RoundTrip`] may sequence on.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
struct ObservedPlacement {
    /// Whether the OS has the window in fullscreen.
    fullscreen: bool,
    /// Whether the monitor the window is REALLY on is the one the display row resolves to. `None`
    /// is "could not be learned" — the tri-state discipline [`PresentCaps`] is built around, and it
    /// moves nothing: a target that cannot be resolved is not a disagreement. Two states reach it,
    /// both real: a window winit has not mapped to a screen yet answers no `current_monitor()` (see
    /// `frame_cost::record_monitor`), and a machine on which bevy has marked no primary monitor
    /// leaves the PRIMARY rung unresolvable.
    on_target: Option<bool>,
}

/// What [`RoundTrip::step`] asks the window writer to do this frame.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum PlacementStep {
    /// Leave `Window::mode` alone.
    Hold,
    /// Write `WindowMode::Windowed` — step one. TRANSITIONAL: [`Settings`] is not touched, so no row
    /// says windowed and the position edge stays unspent.
    LeaveFullscreen,
    /// Write the resolved `BorderlessFullscreen` — step two, the `None -> Some` winit implements.
    ReenterFullscreen,
    /// Write nothing, and say once that the window is still not where the row asks. The trip is over
    /// and it did not land; settling here rather than retrying is what makes the rule terminate.
    Settle,
}

impl RoundTrip {
    /// Advance on this frame's observation and say what to write. The whole decision rule.
    ///
    /// `target` is the RESOLVED rung ([`DisplaySelection::resolve`]) rather than the stored one, so
    /// an unreachable rung and the rung it falls back to are one attempt, not two — and a hot-plug
    /// that changes what the stored rung resolves to is a genuinely new target, which is exactly
    /// what it is.
    fn step(
        &mut self,
        setting: WindowModeSetting,
        target: DisplaySelection,
        observed: ObservedPlacement,
    ) -> PlacementStep {
        if setting != WindowModeSetting::Fullscreen {
            // Abandon the trip: its pending write would restore fullscreen the player no longer wants.
            *self = RoundTrip::Idle;
            return PlacementStep::Hold;
        }
        if *self == RoundTrip::Leaving {
            if observed.fullscreen {
                return PlacementStep::Hold;
            }
            *self = RoundTrip::Spent(target);
            return PlacementStep::ReenterFullscreen;
        }
        if !observed.fullscreen {
            // Not fullscreen yet: preserve state through asynchronous re-entry, or the trip would re-arm
            // against its own transition and flap.
            return PlacementStep::Hold;
        }
        match observed.on_target {
            // "Could not learn" is not a disagreement, and it is not evidence against one either.
            None => PlacementStep::Hold,
            Some(true) => {
                *self = RoundTrip::Idle;
                PlacementStep::Hold
            }
            // The completed trip missed its target.
            Some(false) if *self == RoundTrip::Spent(target) => {
                *self = RoundTrip::Stranded(target);
                PlacementStep::Settle
            }
            Some(false) if *self == RoundTrip::Stranded(target) => PlacementStep::Hold,
            Some(false) => {
                *self = RoundTrip::Leaving;
                PlacementStep::LeaveFullscreen
            }
        }
    }

    /// Whether the window is in the transitional state the trip owns — the frames on which
    /// [`apply_settings`] must yield `Window::mode` rather than re-assert the row's fullscreen over
    /// a half-done trip.
    const fn owns_the_mode(self) -> bool {
        matches!(self, RoundTrip::Leaving)
    }
}

/// Observe where the window REALLY is — fullscreen or not, and on which monitor — and reconcile
/// both halves. The "observe external changes" half of the window-mode row, and the only owner of
/// fullscreen monitor placement.
///
/// On macOS the green traffic-light button
/// toggles native fullscreen without bevy hearing about it: `Window::mode` goes stale, and a page
/// that trusted it would either lie or fight the OS (writing the stale mode back would kick the
/// player straight out of the fullscreen they just asked for).
///
/// **Edge-triggered, deliberately.** The system acts only when the observed state CHANGES between
/// frames, never on a standing mismatch. A level-triggered version would fight winit's deferred
/// transitions: while macOS animates into fullscreen, a commanded mode can sit "not yet real" for
/// up to a second (winit parks it in `target_fullscreen`), and reconciling against that snapshot
/// would revert the player's choice and then oscillate. An edge is only ever produced by the OS
/// actually switching, so following edges follows truth.
///
/// Reaches winit through the same `WINIT_WINDOWS` thread-local + `NonSendMarker` pattern as
/// `branding::set_window_icon` (the marker is load-bearing: off the main thread the thread-local
/// is empty). Headless roots never mount this plugin; a windowed root before window creation just
/// finds no winit window and returns.
///
/// This is a second WRITER of the settings file (via [`SaveSettings`]) beyond the page — accepted
/// because the green button IS player intent, exactly as deliberate as a row click, and a player
/// who fullscreens the game expects it to come back fullscreen.
///
/// NOT gated on [`CaptureWindowPinned`], deliberately, unlike [`apply_settings`]'s
/// window sub-block: the first observation is a baseline, not an edge, so a hidden capture window
/// booting `Windowed` writes nothing — and with the apply-side skip in place nothing ever commands
/// that window fullscreen, so the OS edge this system needs cannot occur on a capture run. It
/// therefore cannot spend the player's persisted fullscreen on a capture client's `Windowed` state.
/// The round trip below is covered by the same fact from the other side: it arms only on a window
/// the OS reports as ALREADY fullscreen, which a capture window never is.
///
/// # Why it resolves the DISPLAY row too
///
/// This writer must produce the same `Window::mode` [`apply_settings`] would, monitor included,
/// because there is one path on which its write is the LAST word: the player picks FULLSCREEN on
/// the settings page, `apply_settings` writes `BorderlessFullscreen(Index(n))`, the OS completes
/// the transition, and this system sees the edge — at which point the observed state already equals
/// the stored one, so `Settings` is NOT written, so `apply_settings` (which runs on
/// `resource_changed::<Settings>`) does not run again to correct anything. A `Current` written here
/// would stand, and bevy would re-apply fullscreen on whatever monitor the window happened to be
/// on. Hence the same [`read_displays`] + [`DisplaySelection::resolve`] the reconciler uses.
///
/// # Why it also watches WHICH monitor
///
/// Tracking fullscreen-or-not alone is what let a disagreement stand forever: winit accepts a
/// monitor change on an already-fullscreen window, commits it to its own state, and never moves the
/// window ([`RoundTrip`]). Nothing downstream can notice, because every component and every cache
/// then reads as correct. So the real monitor is observed beside the real fullscreen state, and the
/// rule is one comparison against the row: while the row asks for fullscreen and the OS agrees the
/// window is fullscreen, the monitor it is really on must be the one the row resolves to, or
/// [`RoundTrip`] drives it there. Keyed on the DISAGREEMENT and not on the player touching the row,
/// so boot, a hot-plug and a window winit has lost track of are all the same case — see
/// [`RoundTrip`] for the three and for why the repair terminates.
///
/// The target is resolved through `bevy_winit`'s OWN `select_monitor`, so this comparison cannot
/// drift from the monitor `changed_windows` would actually pick.
#[expect(
    clippy::too_many_arguments,
    reason = "a system spanning the window, settings, the save channel, both monitor sources, placement and a local"
)]
fn observe_window_placement(
    _non_send_marker: bevy::ecs::system::NonSendMarker,
    window: Option<Single<(Entity, &mut Window), With<PrimaryWindow>>>,
    mut settings: ResMut<Settings>,
    mut save: MessageWriter<SaveSettings>,
    monitors: Query<(), With<Monitor>>,
    primaries: Query<Entity, (With<Monitor>, With<PrimaryMonitor>)>,
    // Bevy's own monitor-handle list, the one `changed_windows` resolves `Window::mode` through.
    // Present on every root that mounts this plugin — they all carry `WinitPlugin`.
    winit_monitors: Res<bevy::winit::WinitMonitors>,
    mut placement: ResMut<WindowPlacement>,
    mut last_seen: Local<Option<bool>>,
) {
    let Some(window) = window else {
        return;
    };
    let (entity, mut window) = window.into_inner();
    let displays = read_displays(&monitors, &primaries);
    let target = settings.display.monitor_on(displays);
    let Some(observed) = bevy::winit::WINIT_WINDOWS.with_borrow(|winit_windows| {
        winit_windows.get_window(entity).map(|winit_window| {
            let real = winit_window.current_monitor();
            ObservedPlacement {
                fullscreen: winit_window.fullscreen().is_some(),
                on_target: target.and_then(|selection| {
                    // The primary handle is not reachable from a system, and does not need to be:
                    // `monitor_on` emits the bare `MonitorSelection::Primary` only where bevy has
                    // marked no monitor at all, which is precisely the state in which nothing about
                    // the target can be concluded. `select_monitor` answers `None` there, and an
                    // unresolvable target is "could not learn", not a disagreement.
                    let wanted = bevy::winit::select_monitor(
                        &winit_monitors,
                        None,
                        real.clone(),
                        &selection,
                    )?;
                    Some(real.clone()? == wanted)
                }),
            }
        })
    }) else {
        return;
    };
    let previous = last_seen.replace(observed.fullscreen);

    // The trip's own fullscreen edges are this system's writes coming back, not player intent:
    // reflecting them below would save a windowed state nobody asked for, spending the player's
    // persisted fullscreen on the way to honouring their display row.
    let trip_owns_the_window = placement.round_trip.owns_the_mode();
    let resolved = settings.display.resolve(displays);
    let step = placement
        .round_trip
        .step(settings.window_mode, resolved, observed);
    match step {
        PlacementStep::Hold => {}
        PlacementStep::LeaveFullscreen => {
            window.mode = WindowMode::Windowed;
            info!(
                "settings: the window is fullscreen on the wrong display -> leaving fullscreen to \
                 re-enter on {}",
                resolved.label(),
            );
        }
        PlacementStep::ReenterFullscreen => {
            window.mode = settings.window_mode.to_window_mode(target);
            info!("settings: re-entering fullscreen on {}", resolved.label());
        }
        PlacementStep::Settle => warn!(
            "settings: the window is still not on {} after a fullscreen round trip — left where \
             it is (not retried)",
            resolved.label(),
        ),
    }
    if trip_owns_the_window || step != PlacementStep::Hold {
        return;
    }

    // The first observation is a baseline, not an edge — and a same-state frame is nothing at all.
    if previous.is_none_or(|previous| previous == observed.fullscreen) {
        return;
    }
    let reflected = WindowModeSetting::from_fullscreen(observed.fullscreen);
    // Keep `Window::mode` truthful FIRST, whatever the settings say: bevy's change detection can
    // only express "leave fullscreen" later if the component actually says it is fullscreen now.
    // The MONITOR in it is the settings row's, resolved exactly as `apply_settings` resolves it —
    // see this function's doc for the path on which this write is the last word.
    let mode = reflected.to_window_mode(target);
    if window.mode != mode {
        window.mode = mode;
    }
    if settings.window_mode != reflected {
        settings.window_mode = reflected;
        save.write(SaveSettings);
        info!(
            "settings: window mode -> {} (changed outside the settings page)",
            reflected.label()
        );
    }
}

/// Persist on request. Reads the settings AFTER [`ApplySettings`] so what is written is what the
/// player is now looking at.
fn save_on_request(
    mut requests: MessageReader<SaveSettings>,
    settings: Res<Settings>,
    path: Res<store::StorePath>,
    mut status: ResMut<SaveStatus>,
) {
    // Collapse a frame's requests: several rows changed in one frame is still one file write.
    if requests.read().count() == 0 {
        return;
    }
    match store::save(path.0.as_deref(), &settings) {
        Ok(saved_path) => {
            status.0 = None;
            info!("settings: saved {}", saved_path.display());
        }
        Err(error) => {
            warn!("settings: save failed ({error})");
            status.0 = Some(error);
        }
    }
}

/// Name where a superseded file was preserved, or say plainly that it was not. Never silently omits
/// the fact: "we moved your file" and "we could not move your file" are different things to a player
/// looking for it.
fn describe_kept(kept: Option<&std::path::Path>, subject: &str) -> String {
    kept.map_or_else(
        || format!("{subject} could NOT be preserved"),
        |kept| format!("{subject} was kept at {}", kept.display()),
    )
}

/// Report the boot read, once, now that a tracing subscriber exists. Absent when a root inserted no
/// report (tests, the sandboxes) — those simply say nothing.
fn report_store_load(report: Option<Res<StoreReport>>, path: Res<store::StorePath>) {
    let Some(report) = report else {
        return;
    };
    // Named in every branch, because "where do I edit this?" is the first question a complaint
    // raises. `None` only when there is no platform config directory at all.
    let path = path.0.as_ref().map_or_else(
        || "<no config directory>".to_string(),
        |path| path.display().to_string(),
    );
    match &report.0 {
        store::LoadNote::Loaded => info!("settings: loaded {path}"),
        store::LoadNote::FirstLaunch => {
            info!("settings: no {path} yet — first launch, using defaults");
        }
        store::LoadNote::NoLocation => warn!(
            "settings: no platform config directory (is HOME/APPDATA set?) — using defaults, and \
             changes will not persist"
        ),
        store::LoadNote::Unreadable(err) => {
            warn!("settings: cannot read {path} ({err}) — using defaults");
        }
        store::LoadNote::Corrupt { error, kept } => warn!(
            "settings: {path} could not be parsed ({error}) — using defaults. {}",
            describe_kept(kept.as_deref(), "the damaged file"),
        ),
        store::LoadNote::FutureVersion { version, kept } => warn!(
            "settings: {path} is version {version} but this build understands at most \
             {SETTINGS_VERSION} — using defaults rather than guessing what its values mean. {}",
            describe_kept(kept.as_deref(), "it"),
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every capability state the app can actually be in: the two "nothing is known" ones — the
    /// pre-probe boot state and a probe that could not ask — plus the four lists a surface can
    /// report. Enumerated once, so a grid test cannot quietly stop covering a variant.
    fn every_caps_state() -> Vec<PresentCaps> {
        let mut states = vec![PresentCaps::Unprobed, PresentCaps::Unavailable];
        for immediate in [false, true] {
            for mailbox in [false, true] {
                states.push(PresentCaps::Reported { immediate, mailbox });
            }
        }
        states
    }

    /// The pricing runner relies on overlap-only reconciliation even when all bounds agree.
    /// ASSUMED fixture overlap exercises the independent field, not a product recommendation.
    #[test]
    fn overlap_only_shadow_reconciliation_reaches_the_renderer() {
        let mut config = CascadeShadowConfig::default();
        let mut rebuilt = config.clone();
        rebuilt.overlap_proportion = 0.05;
        reconcile_shadow_config(&mut config, rebuilt);
        assert_eq!(config.overlap_proportion, 0.05);
        let mut rebuilt = config.clone();
        rebuilt.minimum_distance = 0.2;
        reconcile_shadow_config(&mut config, rebuilt);
        assert_eq!(config.minimum_distance, 0.2);
    }

    /// The render app relies on `apply_settings` writing [`crate::render_scale::RenderScale`];
    /// a direct UI write would split saved intent from the picture.
    #[test]
    fn the_render_scale_row_is_applied_through_the_one_resource() {
        use crate::render_scale::RenderScale;

        let mut app = App::new();
        app.init_resource::<Settings>()
            .init_resource::<DirectionalLightShadowMap>()
            .init_resource::<RenderScale>()
            .init_resource::<PresentCaps>()
            .init_resource::<UiScale>()
            .init_resource::<LodPixelBudget>()
            .init_resource::<WindowPlacement>()
            .add_systems(Update, apply_settings);
        app.update();
        assert_eq!(
            app.world().resource::<RenderScale>().0,
            1.0,
            "a default config must leave the render app on the native path"
        );

        for level in RenderScaleLevel::ORDER {
            app.world_mut().resource_mut::<Settings>().render_scale = level;
            app.update();
            assert_eq!(
                app.world().resource::<RenderScale>().0,
                level.fraction(),
                "{level:?} did not reach the render-world resource",
            );
        }
    }

    /// **The bloom row reaches the battlefield camera, and only it.** Its value is a field on a
    /// camera component, so the seam is `apply_settings`' profile filter: a pen-cam inset keeps the
    /// bloom it was built with, as it keeps its own MSAA. Every rung is also a value bevy accepts
    /// as a mip-chain height — a power of two, so `ilog2` takes no rounding.
    #[test]
    fn the_bloom_row_reaches_the_battlefield_camera_only() {
        let mut app = App::new();
        app.init_resource::<Settings>()
            .init_resource::<DirectionalLightShadowMap>()
            .init_resource::<crate::render_scale::RenderScale>()
            .init_resource::<PresentCaps>()
            .init_resource::<UiScale>()
            .init_resource::<LodPixelBudget>()
            .init_resource::<WindowPlacement>()
            .add_systems(Update, apply_settings);
        // `Msaa` is spelled out because `bevy_render` registers it as `Camera`'s required component,
        // and this bare app mounts no render plugin; without it no camera matches the query.
        let battlefield = app
            .world_mut()
            .spawn((
                Camera3d::default(),
                Msaa::default(),
                Bloom::NATURAL,
                CameraProfile::BattlefieldThirdPerson,
            ))
            .id();
        let inset = app
            .world_mut()
            .spawn((
                Camera3d::default(),
                Msaa::default(),
                Bloom::NATURAL,
                CameraProfile::PenCamInset,
            ))
            .id();

        for level in BloomLevel::ORDER {
            assert!(level.max_mip_dimension().is_power_of_two(), "{level:?}");
            app.world_mut().resource_mut::<Settings>().bloom = level;
            app.update();
            let mips = |camera| {
                app.world()
                    .get::<Bloom>(camera)
                    .expect("the camera keeps its bloom")
                    .max_mip_dimension
            };
            assert_eq!(mips(battlefield), level.max_mip_dimension(), "{level:?}");
            assert_eq!(
                mips(inset),
                Bloom::NATURAL.max_mip_dimension,
                "{level:?}: the pen cam's inset must keep its own bloom",
            );
        }
    }

    /// Both ladders ascend in cost, and the map sizes are powers of two — otherwise
    /// `bevy_light::validate_shadow_map_size` silently rounds them up and warns every launch.
    #[test]
    fn both_shadow_ladders_ascend_and_the_sizes_are_powers_of_two() {
        let sizes: Vec<usize> = ShadowResolution::ORDER
            .iter()
            .map(|resolution| resolution.shadow_map_size())
            .collect();
        assert!(sizes.iter().all(|size| size.is_power_of_two()), "{sizes:?}");
        assert!(sizes.windows(2).all(|w| w[0] < w[1]), "{sizes:?} ascending");
        // `Off` carries no distance, so the ascent is checked over the rungs that have one.
        let distances: Vec<f32> = ShadowDistance::ORDER
            .iter()
            .filter_map(|distance| distance.distance_m())
            .collect();
        assert_eq!(
            distances.len(),
            ShadowDistance::ORDER.len() - 1,
            "every rung but Off must name a distance"
        );
        assert!(
            distances.windows(2).all(|w| w[0] < w[1]),
            "{distances:?} ascending"
        );
    }

    /// Across two laps of distance × resolution × count, only the count row may change config
    /// length, and only while casting; Off leaves the change pending. That length sizes the
    /// frusta, per-view entity vectors and pooled queues downstream.
    ///
    /// Rebuilds must retain [`SHADOW_FIRST_CASCADE_FAR_BOUND_M`], [`SHADOW_CASCADE_OVERLAP`] and
    /// Bevy's minimum distance; an implicit first bound would silently halve far-field crispness.
    ///
    /// The scheduling-dependent upstream panic is deliberately not reproduced: a test can pass
    /// while no pooled thread misses init. Its validation belongs to
    /// `upstream/bevy-cascade-count-stale-local-parallel.md`.
    #[test]
    fn the_cascade_count_follows_its_row_and_only_its_row() {
        let mut app = App::new();
        app.init_resource::<Settings>()
            .init_resource::<DirectionalLightShadowMap>()
            // A bare App must supply the resource `plugin` normally mounts.
            .init_resource::<crate::render_scale::RenderScale>()
            .init_resource::<PresentCaps>()
            .init_resource::<UiScale>()
            .init_resource::<LodPixelBudget>()
            .init_resource::<WindowPlacement>()
            .add_systems(Update, apply_settings);
        let sun = app
            .world_mut()
            .spawn((
                DirectionalLight {
                    shadow_maps_enabled: true,
                    ..default()
                },
                CascadeShadowConfig::default(),
            ))
            .id();

        let grid = ShadowDistance::ORDER
            .into_iter()
            .flat_map(|distance| {
                ShadowResolution::ORDER
                    .into_iter()
                    .flat_map(move |resolution| {
                        ShadowCascades::ORDER
                            .into_iter()
                            .map(move |cascades| (distance, resolution, cascades))
                    })
            })
            .collect::<Vec<_>>();
        // The reference for the parameters the count rebuild must never move.
        let reference = CascadeShadowConfigBuilder::default().build();
        // What the sun's config was last REBUILT with: the spawn value until the first casting
        // frame, then whatever cascade row was live on the most recent casting frame. A count set
        // while OFF is pending, not applied — that is the deliberate off-branch behaviour.
        let mut applied_count = CascadeShadowConfig::default().bounds.len();
        // Two full laps, so every cell is also reached from the last cell of the previous lap.
        for (distance, resolution, cascades) in grid.iter().copied().chain(grid.iter().copied()) {
            {
                let mut settings = app.world_mut().resource_mut::<Settings>();
                settings.shadow_distance = distance;
                settings.shadow_resolution = resolution;
                settings.shadow_cascades = cascades;
            }
            app.update();
            if distance.casts() {
                applied_count = cascades.count();
            }
            let config = app
                .world()
                .get::<CascadeShadowConfig>(sun)
                .expect("sun carries a cascade config");
            assert_eq!(
                config.bounds.len(),
                applied_count,
                "{distance:?}/{resolution:?}/{cascades:?}: the cascade count must be exactly the \
                 last one the row APPLIED — moved by the row alone, and only on a casting frame",
            );
            assert_eq!(
                config.overlap_proportion, SHADOW_CASCADE_OVERLAP,
                "{cascades:?} must not move the overlap — only the count and the far bound may vary",
            );
            assert_eq!(
                config.minimum_distance, reference.minimum_distance,
                "{cascades:?} must not move the minimum distance — the one field still bevy's",
            );
            if let Some(want) = distance.distance_m() {
                // The stated first bound must survive every rebuild, at every count. It is not
                // bevy's, so `..default()` creeping back over it would silently halve far-field
                // crispness rather than fail anything.
                let first = *config.bounds.first().expect("cascades exist");
                assert!(
                    (first - SHADOW_FIRST_CASCADE_FAR_BOUND_M).abs()
                        <= SHADOW_FIRST_CASCADE_FAR_BOUND_M * 1e-4,
                    "{distance:?}/{cascades:?}: the first cascade must end at \
                     {SHADOW_FIRST_CASCADE_FAR_BOUND_M} m, got {first}",
                );
                // The far bound is `nearest * base^(n-1)` with `base` itself a `powf`, so it
                // reproduces `distance_m` only to float precision (MEASURED: 200.00002 for 200.0). A
                // relative tolerance — the claim is "the envelope moved", not "these bits match".
                let far = *config.bounds.last().expect("cascades exist");
                assert!(
                    (far - want).abs() <= want * 1e-4,
                    "{distance:?} must move the far bound to {want} m, got {far}",
                );
            }
            assert_eq!(
                app.world().resource::<DirectionalLightShadowMap>().size,
                resolution.shadow_map_size(),
                "{resolution:?} must resize the map whatever {distance:?} says — the rows are \
                 independent",
            );
            assert_eq!(
                app.world()
                    .get::<DirectionalLight>(sun)
                    .expect("sun exists")
                    .shadow_maps_enabled,
                distance.casts(),
                "{distance:?} must be the one thing deciding whether anything casts",
            );
        }
    }

    /// **The Metal panic guard, capability-gated.** `Mailbox` and `FifoRelaxed` hit an
    /// `unreachable!()` in wgpu-hal's Metal backend if a surface is CONFIGURED with them, and
    /// `AutoVsync` names `FifoRelaxed` as its first fallback choice. The rule this pins, across the
    /// WHOLE vsync × capability grid: a concrete uncapped mode (`Mailbox`/`Immediate`) may only be
    /// produced when the probe positively reported it; every other cell answers `Fifo` or the
    /// self-negotiating `AutoNoVsync`, and `AutoVsync`/`FifoRelaxed` are never produced at all.
    #[test]
    fn present_modes_are_probe_gated_and_metal_safe() {
        for vsync in VsyncMode::ORDER {
            for caps in every_caps_state() {
                let mode = Settings { vsync, ..default() }.present_mode(caps);
                match mode {
                    PresentMode::Fifo | PresentMode::AutoNoVsync => {}
                    PresentMode::Mailbox => assert!(
                        caps.mailbox(),
                        "vsync={vsync:?} caps={caps:?} produced Mailbox without the probe \
                         confirming it — that is the Metal unreachable!()",
                    ),
                    PresentMode::Immediate => assert!(
                        caps.immediate(),
                        "vsync={vsync:?} caps={caps:?} produced Immediate without the probe \
                         confirming it — that is the Wayland refusal",
                    ),
                    other => panic!(
                        "vsync={vsync:?} caps={caps:?} produced {other:?}, which is never a safe \
                         answer (AutoVsync's first fallback choice panics on Metal)",
                    ),
                }
            }
        }
        // The three rungs at their intended best: ON is Fifo everywhere; FAST/OFF land on their
        // concrete modes exactly when the surface reported them.
        let probed_all = PresentCaps::Reported {
            immediate: true,
            mailbox: true,
        };
        let by_vsync = |vsync| Settings { vsync, ..default() };
        assert_eq!(
            by_vsync(VsyncMode::On).present_mode(probed_all),
            PresentMode::Fifo,
        );
        assert_eq!(
            by_vsync(VsyncMode::Fast).present_mode(probed_all),
            PresentMode::Mailbox,
        );
        assert_eq!(
            by_vsync(VsyncMode::Off).present_mode(probed_all),
            PresentMode::Immediate,
        );
        // And every state where nothing is KNOWN — the boot window description, and a probe that
        // could not ask — is always negotiation, never a gamble.
        for caps in [PresentCaps::Unprobed, PresentCaps::Unavailable] {
            for vsync in [VsyncMode::Fast, VsyncMode::Off] {
                assert_eq!(
                    by_vsync(vsync).present_mode(caps),
                    PresentMode::AutoNoVsync,
                    "{caps:?} must negotiate, not gamble",
                );
            }
        }
    }

    /// Which rungs the page may OFFER, per capability state. FAST requires `Mailbox` specifically
    /// (an Immediate-backed FAST would duplicate OFF on a Metal surface); OFF requires `Immediate`;
    /// ON is unconditional; unprobed offers everything because there is nothing to gate on yet.
    #[test]
    fn the_offered_rungs_follow_the_probe() {
        let offered = |caps: PresentCaps| -> Vec<VsyncMode> {
            VsyncMode::ORDER
                .into_iter()
                .filter(|mode| caps.offers(*mode))
                .collect()
        };
        assert_eq!(offered(PresentCaps::Unprobed), VsyncMode::ORDER.to_vec());
        // A probe that could not ask knows no more than one that has not run: offer everything.
        assert_eq!(offered(PresentCaps::Unavailable), VsyncMode::ORDER.to_vec());
        // A Metal surface: [Fifo, Immediate].
        assert_eq!(
            offered(PresentCaps::Reported {
                immediate: true,
                mailbox: false,
            }),
            vec![VsyncMode::Off, VsyncMode::On],
        );
        // A Wayland surface: [Fifo, Mailbox] — Immediate refused.
        assert_eq!(
            offered(PresentCaps::Reported {
                immediate: false,
                mailbox: true,
            }),
            vec![VsyncMode::Fast, VsyncMode::On],
        );
        // A minimal surface: Fifo only. ON must always survive.
        assert_eq!(
            offered(PresentCaps::Reported {
                immediate: false,
                mailbox: false,
            }),
            vec![VsyncMode::On],
        );
        // Everything reported (a typical Vulkan/X11 or Windows surface): every rung. With this the
        // ladder is pinned for all four Reported shapes plus both non-answers — exhaustively HERE,
        // which is what lets the probe test stick to the present-mode-list distillation.
        assert_eq!(
            offered(PresentCaps::Reported {
                immediate: true,
                mailbox: true,
            }),
            VsyncMode::ORDER.to_vec(),
        );
    }

    /// A portable file's unsupported rung resolves to ON for page, present mode and cap gate.
    /// Otherwise FAST can label tearing Metal output or OFF can arm a second limiter on Wayland.
    #[test]
    fn a_persisted_rung_the_surface_cannot_present_is_normalized() {
        // Metal: [Fifo, Immediate] — no Mailbox, so FAST is dead here.
        let metal = PresentCaps::Reported {
            immediate: true,
            mailbox: false,
        };
        // Wayland: [Fifo, Mailbox] — Immediate refused, so OFF is dead here.
        let wayland = PresentCaps::Reported {
            immediate: false,
            mailbox: true,
        };

        let stored_fast = Settings {
            vsync: VsyncMode::Fast,
            ..default()
        };
        assert_eq!(stored_fast.effective_vsync(metal), VsyncMode::On);
        assert_eq!(
            stored_fast.present_mode(metal),
            PresentMode::Fifo,
            "the mode a resolved-to-ON rung presents with is Fifo, not a negotiated AutoNoVsync",
        );

        let stored_off = Settings {
            vsync: VsyncMode::Off,
            frame_cap: FrameCap(120),
            ..default()
        };
        assert_eq!(stored_off.effective_vsync(wayland), VsyncMode::On);
        assert_eq!(
            stored_off.frame_limit_period(wayland),
            None,
            "a rung the surface cannot present must not arm the limiter",
        );
        // Each rung still survives on a surface that DOES offer it, and pre-probe nothing is
        // resolved away on a guess.
        assert_eq!(stored_fast.effective_vsync(wayland), VsyncMode::Fast);
        assert_eq!(stored_off.effective_vsync(metal), VsyncMode::Off);
        for caps in [PresentCaps::Unprobed, metal, wayland] {
            for vsync in VsyncMode::ORDER {
                let settings = Settings { vsync, ..default() };
                assert!(
                    caps.offers(settings.effective_vsync(caps)),
                    "{vsync:?} on {caps:?} resolved to a rung the surface does not offer",
                );
            }
        }
        assert_eq!(
            Settings::default().effective_vsync(PresentCaps::Unprobed),
            VsyncMode::On,
        );
        for vsync in VsyncMode::ORDER {
            assert_eq!(
                Settings { vsync, ..default() }.effective_vsync(PresentCaps::Unprobed),
                vsync,
                "unprobed must not normalise anything — nothing is known yet",
            );
        }
    }

    /// The write-back half of the same finding: when the probe lands, the STORED rung follows the
    /// effective one and the file is asked to follow it too (no phantom state on disk), while a
    /// supported rung — and the whole pre-probe state, and every rung under a probe that could not
    /// ask — is left alone.
    #[test]
    fn the_probe_arrival_writes_the_normalized_rung_back() {
        let app_with = |vsync: VsyncMode, caps: PresentCaps| {
            let mut app = App::new();
            app.add_message::<SaveSettings>()
                .insert_resource(Settings { vsync, ..default() })
                .insert_resource(caps)
                .add_systems(Update, normalize_vsync);
            app.update();
            app
        };
        let saves = |app: &App| {
            app.world()
                .resource::<bevy::ecs::message::Messages<SaveSettings>>()
                .len()
        };

        let metal = PresentCaps::Reported {
            immediate: true,
            mailbox: false,
        };
        let app = app_with(VsyncMode::Fast, metal);
        assert_eq!(app.world().resource::<Settings>().vsync, VsyncMode::On);
        assert_eq!(
            app.world().resource::<Settings>().vsync.label(),
            "ON",
            "the page renders the rung the surface is actually on",
        );
        assert_eq!(saves(&app), 1, "the correction is persisted, not just live");

        // A supported rung is untouched, and so is a save-less frame.
        let app = app_with(VsyncMode::Off, metal);
        assert_eq!(app.world().resource::<Settings>().vsync, VsyncMode::Off);
        assert_eq!(saves(&app), 0, "a supported rung must not rewrite the file");

        // Pre-probe: nothing is known, so nothing is normalised (and nothing is written).
        let app = app_with(VsyncMode::Fast, PresentCaps::Unprobed);
        assert_eq!(app.world().resource::<Settings>().vsync, VsyncMode::Fast);
        assert_eq!(saves(&app), 0);

        // A probe that could not ask is not a negative answer: no stored rung moves and nothing is
        // written.
        for vsync in VsyncMode::ORDER {
            let app = app_with(vsync, PresentCaps::Unavailable);
            assert_eq!(
                app.world().resource::<Settings>().vsync,
                vsync,
                "a probe that could not ask must not rewrite {vsync:?}",
            );
            assert_eq!(
                saves(&app),
                0,
                "a probe that could not ask must not write the file ({vsync:?})",
            );
        }

        // A surface that answered with a list carrying neither uncapped mode (`[Fifo]`) is a
        // conclusive negative, and does normalise. An EMPTY list is not this — `probe` maps that to
        // `Unavailable` before it can reach here (`probe::an_empty_capability_list_is_a_failed_
        // query_not_an_answer`).
        let app = app_with(
            VsyncMode::Off,
            PresentCaps::Reported {
                immediate: false,
                mailbox: false,
            },
        );
        assert_eq!(app.world().resource::<Settings>().vsync, VsyncMode::On);
        assert_eq!(saves(&app), 1);
    }

    /// The frame cap is armed by exactly one conjunction: a non-off cap AND an EFFECTIVE vsync OFF.
    /// FAST and ON both already wait on the compositor, and two competing limiters make a stutter
    /// machine.
    #[test]
    fn the_frame_cap_arms_only_under_vsync_off() {
        let capped = Settings {
            vsync: VsyncMode::Off,
            frame_cap: FrameCap(120),
            ..default()
        };
        let period = capped
            .frame_limit_period(PresentCaps::Unprobed)
            .expect("off + cap must limit");
        assert!((period.as_secs_f64() - 1.0 / 120.0).abs() < 1e-9);
        for vsync in [VsyncMode::Fast, VsyncMode::On] {
            assert_eq!(
                Settings {
                    vsync,
                    frame_cap: FrameCap(120),
                    ..default()
                }
                .frame_limit_period(PresentCaps::Unprobed),
                None,
                "{vsync:?} must disarm the cap",
            );
        }
        assert_eq!(
            Settings {
                vsync: VsyncMode::Off,
                frame_cap: FrameCap::OFF,
                ..default()
            }
            .frame_limit_period(PresentCaps::Unprobed),
            None,
            "an off cap limits nothing even with vsync off",
        );
    }

    /// The display ladder's mapping onto bevy's own [`MonitorSelection`], pinned against the
    /// vendored enum: `AUTO` names no monitor at all (and is therefore the only rung that leaves
    /// `Window::position` on `Automatic`), the indexed rungs are ZERO-based on the wire while
    /// their labels are one-based, and every representable rung is on `ORDER` — the same
    /// `step_in`-dead-end rule [`ShadowDistance::ORDER`] carries.
    #[test]
    fn the_display_ladder_maps_onto_bevys_monitor_selection() {
        assert_eq!(DisplaySelection::default(), DisplaySelection::Auto);
        assert_eq!(DisplaySelection::Auto.monitor(), None);
        assert_eq!(
            DisplaySelection::Auto.window_position_on(ranked(3)),
            WindowPosition::Automatic,
            "the default rung must leave placement to the window manager — adding this row must \
             not move any existing install's window",
        );
        assert_eq!(
            DisplaySelection::Primary.monitor(),
            Some(MonitorSelection::Primary),
        );
        for (rung, index) in [
            (DisplaySelection::Display1, 0),
            (DisplaySelection::Display2, 1),
            (DisplaySelection::Display3, 2),
        ] {
            assert_eq!(
                rung.monitor(),
                Some(MonitorSelection::Index(index)),
                "{rung:?}: the labels are one-based and `MonitorSelection::Index` is not",
            );
            assert_eq!(
                rung.window_position_on(ranked(3)),
                WindowPosition::Centered(MonitorSelection::Index(index)),
            );
        }
        // The BOOT mapping is deliberately weaker: only the rung window creation can resolve
        // survives, because caching an unresolved one strands the window (see the type doc).
        assert_eq!(
            DisplaySelection::Primary.at_window_creation(),
            Some(MonitorSelection::Primary),
            "the primary handle is passed to select_monitor directly, so it resolves at creation",
        );
        for rung in [
            DisplaySelection::Auto,
            DisplaySelection::Display1,
            DisplaySelection::Display2,
            DisplaySelection::Display3,
        ] {
            assert_eq!(
                rung.at_window_creation(),
                None,
                "{rung:?} must not be named in the boot window — WinitMonitors is still empty, and \
                 bevy caches the unresolved request verbatim",
            );
        }
        assert_eq!(
            DisplaySelection::ORDER,
            [
                DisplaySelection::Auto,
                DisplaySelection::Primary,
                DisplaySelection::Display1,
                DisplaySelection::Display2,
                DisplaySelection::Display3,
            ],
            "every representable rung must be on the ladder, or `ui::step_in` resolves a stored \
             off-ladder value to index 0",
        );
        assert_eq!(DisplaySelection::ORDER.len(), 2 + DisplaySelection::INDEXED);
    }

    /// A machine with `count` displays, one of which bevy has marked as the primary. The entity is
    /// a placeholder because these are PURE resolution tests — only its presence is read; the
    /// app-level tests below use real spawned monitor entities.
    fn ranked(count: usize) -> AttachedDisplays {
        AttachedDisplays {
            count,
            primary: Some(Entity::PLACEHOLDER),
        }
    }

    /// A machine with `count` displays and no `PrimaryMonitor` marker — Wayland, or a hot-plug that
    /// took the marked monitor away.
    fn unranked(count: usize) -> AttachedDisplays {
        AttachedDisplays {
            count,
            primary: None,
        }
    }

    /// **A stored display that is not plugged in.** The fallback is read-side and PRIMARY, and the
    /// unknown state — no `Monitor` entities synchronised — moves nothing, exactly as
    /// [`PresentCaps`]'s two unknown states move nothing. That second half is the load-bearing one:
    /// every bare-`App` test and every frame before `create_monitors` runs reports zero monitors,
    /// and a fallback there would let a headless frame overwrite a player's row.
    #[test]
    fn an_absent_display_falls_back_to_primary_and_an_unknown_count_never_does() {
        // A laptop with the external unplugged: one display attached.
        assert_eq!(
            DisplaySelection::Display2.resolve(ranked(1)),
            DisplaySelection::Primary,
        );
        assert_eq!(
            DisplaySelection::Display3.resolve(ranked(2)),
            DisplaySelection::Primary,
        );
        // Plugged back in: the stored rung is honoured again, which is the whole reason the
        // fallback is never persisted.
        assert_eq!(
            DisplaySelection::Display2.resolve(ranked(2)),
            DisplaySelection::Display2,
        );
        for rung in DisplaySelection::ORDER {
            assert_eq!(
                rung.resolve(AttachedDisplays::default()),
                rung,
                "{rung:?}: an unsynchronised monitor list is not evidence of an absent display",
            );
            assert_eq!(
                rung.resolve(ranked(DisplaySelection::INDEXED)),
                rung,
                "{rung:?}: with every indexed rung attached, nothing falls back",
            );
            // Whatever it resolves to is always something this machine can actually honour.
            let resolved = rung.resolve(ranked(1));
            assert!(
                resolved.index().is_none_or(|index| index < 1),
                "{rung:?} resolved to {resolved:?}, which still names a display that is not here",
            );
        }
        // The two rungs that name no index are never touched by any count.
        for count in 0..4 {
            assert_eq!(
                DisplaySelection::Auto.resolve(ranked(count)),
                DisplaySelection::Auto
            );
            assert_eq!(
                DisplaySelection::Primary.resolve(ranked(count)),
                DisplaySelection::Primary,
            );
        }
    }

    /// Wayland's unidentifiable primary must resolve to the attached first display, not a warning
    /// and no move. Detect through absent `PrimaryMonitor` with a nonempty list, never a platform
    /// guess; see [`DisplaySelection::resolve`].
    #[test]
    fn a_machine_that_cannot_name_a_primary_falls_back_to_the_first_display() {
        let wayland = unranked;
        assert_eq!(
            DisplaySelection::Primary.resolve(wayland(2)),
            DisplaySelection::Display1,
            "the PRIMARY rung must land on something winit can actually resolve",
        );
        assert_eq!(
            DisplaySelection::Display3.resolve(wayland(2)),
            DisplaySelection::Display1,
            "and so must the absent-display fallback, which routes through the same rung",
        );
        // Nothing else moves: an attached indexed rung and AUTO are untouched by the platform.
        assert_eq!(
            DisplaySelection::Display2.resolve(wayland(2)),
            DisplaySelection::Display2,
        );
        assert_eq!(
            DisplaySelection::Auto.resolve(wayland(2)),
            DisplaySelection::Auto,
        );
        // The unknown state still wins over the platform question — nothing is decided on a list
        // that has not been synchronised, whether or not it could rank what it does not have.
        for rung in DisplaySelection::ORDER {
            assert_eq!(rung.resolve(wayland(0)), rung);
        }
        // On a stack that CAN rank displays the substitution never happens, so this cannot silently
        // become the behaviour everywhere.
        assert_eq!(
            DisplaySelection::Primary.resolve(ranked(2)),
            DisplaySelection::Primary,
        );
        // Whatever the platform, the answer always names something reachable.
        for count in 1..4 {
            for marked in [false, true] {
                for rung in DisplaySelection::ORDER {
                    let displays = if marked {
                        ranked(count)
                    } else {
                        unranked(count)
                    };
                    let resolved = rung.resolve(displays);
                    assert!(
                        resolved.index().is_none_or(|index| index < count),
                        "{rung:?} on {count} display(s) (marked={marked}) resolved to \
                         {resolved:?}, which names a display that is not there",
                    );
                    assert!(
                        marked || resolved != DisplaySelection::Primary,
                        "{rung:?} resolved to PRIMARY on a machine with no identifiable primary",
                    );
                }
            }
        }
    }

    /// A bare `App` carrying everything [`apply_settings`] needs, plus a primary window and
    /// `monitors` attached displays (the first of them marked primary, as every ranked platform
    /// does). The window is the real bevy component, which is what `bevy_winit` reads — no winit is
    /// needed to pin what this system WRITES onto it, and none is available in a test binary.
    fn window_app(settings: Settings, monitors: usize) -> App {
        let mut app = App::new();
        app.insert_resource(settings)
            .init_resource::<DirectionalLightShadowMap>()
            .init_resource::<crate::render_scale::RenderScale>()
            .init_resource::<PresentCaps>()
            .init_resource::<UiScale>()
            .init_resource::<LodPixelBudget>()
            .init_resource::<WindowPlacement>()
            .add_systems(Update, apply_settings);
        app.world_mut().spawn((Window::default(), PrimaryWindow));
        spawn_monitors(&mut app, monitors);
        app
    }

    /// The primary window's mode and position, as `bevy_winit` would read them.
    fn window_state(app: &mut App) -> (WindowMode, WindowPosition) {
        let world = app.world_mut();
        let mut windows = world.query_filtered::<&Window, With<PrimaryWindow>>();
        let window = windows.single(world).expect("one primary window");
        (window.mode, window.position)
    }

    /// Model Bevy's boot order: describe/create the window with an empty monitor list, cache that
    /// request verbatim, create monitors, then apply settings at Startup.
    ///
    /// `CachedWindow` is private; `boot_mode` stands for its exact value in the reissue predicate
    /// `window.mode != cache.mode`. Component equality with the target alone misses a stranded
    /// window whose unresolved cached request already looks correct.
    fn booted_window_app(settings: Settings, monitors: usize, boot_mode: WindowMode) -> App {
        let mut app = App::new();
        app.insert_resource(settings)
            .init_resource::<DirectionalLightShadowMap>()
            .init_resource::<crate::render_scale::RenderScale>()
            .init_resource::<PresentCaps>()
            .init_resource::<UiScale>()
            .init_resource::<LodPixelBudget>()
            .init_resource::<WindowPlacement>()
            .add_systems(Update, apply_settings);
        // Step 1-2: the window exists, described with the boot mode, before any monitor does.
        app.world_mut().spawn((
            Window {
                mode: boot_mode,
                ..default()
            },
            PrimaryWindow,
        ));
        // Step 3: the monitor list arrives afterwards.
        spawn_monitors(&mut app, monitors);
        app
    }

    /// `count` attached displays, the first marked `PrimaryMonitor` exactly as `create_monitors`
    /// marks a newly-seen primary. Returns their entities, oldest first.
    fn spawn_monitors(app: &mut App, count: usize) -> Vec<Entity> {
        (0..count)
            .map(|index| {
                let monitor = app
                    .world_mut()
                    .spawn(Monitor {
                        name: Some(format!("display {index}")),
                        physical_height: 1080,
                        physical_width: 1920,
                        physical_position: IVec2::new(1920 * index as i32, 0),
                        refresh_rate_millihertz: Some(60_000),
                        scale_factor: 1.0,
                        video_modes: Vec::new(),
                    })
                    .id();
                if index == 0 {
                    app.world_mut().entity_mut(monitor).insert(PrimaryMonitor);
                }
                monitor
            })
            .collect()
    }

    /// Startup must change mode away from Bevy's cached boot request, not merely name the right
    /// monitor: only a difference triggers reissue. See [`DisplaySelection::at_window_creation`]
    /// for the unresolved-index trap.
    #[test]
    fn a_fullscreen_boot_on_an_indexed_display_changes_the_mode_bevy_cached() {
        let settings = Settings {
            window_mode: WindowModeSetting::Fullscreen,
            display: DisplaySelection::Display2,
            ..default()
        };
        // Exactly what the composition roots put in the boot window.
        let cached = settings
            .window_mode
            .to_window_mode(settings.display.at_window_creation());
        assert_eq!(
            cached,
            WindowMode::BorderlessFullscreen(MonitorSelection::Current),
            "an indexed rung must NOT be named at creation — see `at_window_creation`",
        );

        let mut app = booted_window_app(settings, 3, cached);
        app.update();
        let (mode, _) = window_state(&mut app);
        assert_eq!(
            mode,
            WindowMode::BorderlessFullscreen(MonitorSelection::Index(1)),
            "Startup must resolve the stored display now that the monitors exist",
        );
        assert_ne!(
            mode, cached,
            "THE REGRESSION: the mode must differ from the one bevy cached at creation, or \
             `changed_windows` never reissues fullscreen (system.rs:326) and the real window stays \
             on creation's fallback monitor — which a component-equality assertion cannot see",
        );

        // If creation carries the resolved selection, Startup sees no cache difference and leaves
        // the window stranded despite a correct-looking component.
        let stranded_cache = WindowMode::BorderlessFullscreen(MonitorSelection::Index(1));
        let mut app = booted_window_app(settings, 3, stranded_cache);
        app.update();
        let (mode, _) = window_state(&mut app);
        assert_eq!(
            mode, stranded_cache,
            "the control's premise: naming the display twice produces no change at all",
        );

        // The PRIMARY rung is the one that may be named at creation, because `select_monitor` takes
        // the primary handle as a direct argument rather than through the empty list. It therefore
        // boots correct and needs no correction — but the mode still ends up as a resolvable
        // selection rather than the bare `Primary` (see `monitor_on`'s hardening).
        let primary = Settings {
            window_mode: WindowModeSetting::Fullscreen,
            display: DisplaySelection::Primary,
            ..default()
        };
        let cached = primary
            .window_mode
            .to_window_mode(primary.display.at_window_creation());
        assert_eq!(
            cached,
            WindowMode::BorderlessFullscreen(MonitorSelection::Primary),
        );
        let mut app = booted_window_app(primary, 3, cached);
        app.update();
        let (mode, _) = window_state(&mut app);
        assert!(
            matches!(
                mode,
                WindowMode::BorderlessFullscreen(MonitorSelection::Entity(_))
            ),
            "the primary rung resolves to the marked monitor's entity, got {mode:?}",
        );
    }

    /// A stale primary marker must still name an attached display via `Entity`, not Wayland's
    /// unresolvable `Primary` query. Losing the marker falls back to Index(0). See
    /// [`AttachedDisplays::primary`] for the hot-plug limitation.
    #[test]
    fn the_primary_rung_survives_bevys_stale_monitor_markers() {
        let mut app = App::new();
        let monitors = spawn_monitors(&mut app, 3);
        let displays_now = |app: &mut App| {
            let world = app.world_mut();
            let mut all = world.query_filtered::<(), With<Monitor>>();
            let count = all.iter(world).count();
            let mut marked =
                world.query_filtered::<Entity, (With<Monitor>, With<PrimaryMonitor>)>();
            let primary = marked.iter(world).next();
            AttachedDisplays { count, primary }
        };

        // The ordinary machine: the marker is fresh and the rung resolves through it BY ENTITY.
        let displays = displays_now(&mut app);
        assert_eq!(displays.primary, Some(monitors[0]));
        assert_eq!(
            DisplaySelection::Primary.monitor_on(displays),
            Some(MonitorSelection::Entity(monitors[0])),
            "a marked primary must be spent as the entity bevy marked, not as the platform query \
             that Wayland answers None to",
        );

        // HOT-PLUG: the marked primary is unplugged. Bevy despawns its entity (system.rs:225-236)
        // and — this is the limitation — never marks whichever display inherits the role.
        app.world_mut().entity_mut(monitors[0]).despawn();
        let displays = displays_now(&mut app);
        assert_eq!(displays.count, 2);
        assert_eq!(
            displays.primary, None,
            "the premise: bevy re-marks nothing, so the new primary is invisible to us",
        );
        assert_eq!(
            DisplaySelection::Primary.monitor_on(displays),
            Some(MonitorSelection::Index(0)),
            "with no surviving marker the rung must fall back to a display that certainly exists — \
             the documented residual is that PRIMARY then means 'the first display'",
        );
        assert_ne!(
            DisplaySelection::Primary.monitor_on(displays),
            Some(MonitorSelection::Primary),
            "and it must never be the bare platform query, which is the no-op this all guards",
        );

        // The mirror case — a marker that OUTLIVES the platform's ability to name a primary — is
        // indistinguishable from the healthy one through the ECS, and that is exactly why the
        // entity form is used: it resolves against the monitor list either way. Pinned as an
        // invariant over every state this module can observe.
        for count in 1..4 {
            for displays in [ranked(count), unranked(count)] {
                let monitor = DisplaySelection::Primary.monitor_on(displays);
                assert_ne!(
                    monitor,
                    Some(MonitorSelection::Primary),
                    "{displays:?}: a synchronised monitor list must never leave the rung as the \
                     bare platform query",
                );
                assert!(monitor.is_some(), "{displays:?}");
            }
        }
        // The one state where the bare request IS right: nothing has been learned at all, so there
        // is no entity to prefer and no evidence the platform cannot answer.
        assert_eq!(
            DisplaySelection::Primary.monitor_on(AttachedDisplays::default()),
            Some(MonitorSelection::Primary),
        );
    }

    /// The display row must write fullscreen mode at boot and on a live change, then place the
    /// window when fullscreen is left. Fullscreen ignores position; consuming placement there
    /// would strand the window on exit.
    ///
    /// These component writes are necessary, not sufficient: macOS winit may swallow the monitor
    /// move. [`RoundTrip`] and its tests cover that mechanism.
    #[test]
    fn a_fullscreen_window_follows_the_display_row_and_still_places_itself_after_leaving() {
        // BOOT: a persisted fullscreen on display 2, three displays attached.
        let mut app = window_app(
            Settings {
                window_mode: WindowModeSetting::Fullscreen,
                display: DisplaySelection::Display2,
                ..default()
            },
            3,
        );
        app.update();
        let (mode, _) = window_state(&mut app);
        assert_eq!(
            mode,
            WindowMode::BorderlessFullscreen(MonitorSelection::Index(1)),
            "a persisted fullscreen must boot on the persisted DISPLAY, not on Current",
        );

        // LIVE: the player walks the row while still fullscreen. The mode is level-triggered, so it
        // follows — the edge-triggered position could not have.
        app.world_mut().resource_mut::<Settings>().display = DisplaySelection::Display3;
        app.update();
        let (mode, _) = window_state(&mut app);
        assert_eq!(
            mode,
            WindowMode::BorderlessFullscreen(MonitorSelection::Index(2)),
            "changing the display row while fullscreen must move the window",
        );

        // And an unrelated row must NOT disturb it — the level-triggered write is guarded.
        app.world_mut().resource_mut::<Settings>().msaa = MsaaLevel::Off;
        app.update();
        let (mode, position) = window_state(&mut app);
        assert_eq!(
            mode,
            WindowMode::BorderlessFullscreen(MonitorSelection::Index(2))
        );
        assert_eq!(
            position,
            WindowPosition::Automatic,
            "nothing may write a position while fullscreen — it is ignored, and spending the edge \
             there is what stranded the window on the way back out",
        );

        // LEAVING: the position edge must still be available, because it was never spent.
        app.world_mut().resource_mut::<Settings>().window_mode = WindowModeSetting::Windowed;
        app.update();
        let (mode, position) = window_state(&mut app);
        assert_eq!(mode, WindowMode::Windowed);
        assert_eq!(
            position,
            WindowPosition::Centered(MonitorSelection::Index(2)),
            "leaving fullscreen must place the window on the display the row names",
        );
    }

    /// **The decision rule, exhaustively: only a window that REALLY disagrees with the row is moved.**
    ///
    /// The comparison is against observed reality, never against `Window::mode` — the whole defect
    /// is that the component, bevy's cache and winit's own state all read as correct while the
    /// window sits on the other panel (see [`RoundTrip`]). Both "could not learn" states are pinned
    /// beside the two decidable ones, because an unresolvable target answering like a disagreement
    /// would move a window on evidence nobody gathered.
    #[test]
    fn the_round_trip_fires_only_where_the_window_really_disagrees_with_the_row() {
        let fullscreen = WindowModeSetting::Fullscreen;
        let target = DisplaySelection::Display2;
        let seen = |fullscreen, on_target| ObservedPlacement {
            fullscreen,
            on_target,
        };

        for (observed, expected, claim) in [
            (
                seen(true, Some(false)),
                PlacementStep::LeaveFullscreen,
                "a fullscreen window on the wrong monitor is THE defect — it must round trip",
            ),
            (
                seen(true, Some(true)),
                PlacementStep::Hold,
                "a window already where the row asks must never be disturbed",
            ),
            (
                seen(true, None),
                PlacementStep::Hold,
                "an unresolvable target is not a disagreement — an unmapped window and an \
                 unmarked primary both land here",
            ),
            (
                seen(false, Some(false)),
                PlacementStep::Hold,
                "commanded fullscreen and not arrived: nothing is placed, so nothing disagrees",
            ),
        ] {
            let mut trip = RoundTrip::default();
            assert_eq!(trip.step(fullscreen, target, observed), expected, "{claim}");
        }

        // A row that is not asking for fullscreen is not this machine's business at all.
        let mut trip = RoundTrip::default();
        assert_eq!(
            trip.step(WindowModeSetting::Windowed, target, seen(true, Some(false))),
            PlacementStep::Hold,
            "a windowed row never drives a fullscreen transition",
        );

        // A standing agreement leaves nothing behind — the machine is only ever armed by a real
        // disagreement, so it cannot accumulate state across a session.
        let mut trip = RoundTrip::default();
        for _ in 0..64 {
            assert_eq!(
                trip.step(fullscreen, target, seen(true, Some(true))),
                PlacementStep::Hold
            );
        }
        assert_eq!(trip, RoundTrip::Idle);
    }

    /// Boot, hot-plug and lost winit state all arm on observed disagreement. A hot-plug changing
    /// the resolved target must re-arm even a stranded attempt; otherwise a returning display
    /// could remain unreachable. See [`RoundTrip`].
    #[test]
    fn any_observed_disagreement_arms_the_trip_whatever_produced_it() {
        let fullscreen = WindowModeSetting::Fullscreen;
        let wrong = ObservedPlacement {
            fullscreen: true,
            on_target: Some(false),
        };
        let leaving = ObservedPlacement {
            fullscreen: false,
            on_target: Some(false),
        };

        // BOOT: an indexed rung cannot be named at window creation, so the window is born on the
        // fallback monitor and the `Startup` correction is itself a swallowed no-op. The very first
        // observation of a fullscreen window on the wrong panel is enough.
        let mut trip = RoundTrip::default();
        assert_eq!(
            trip.step(fullscreen, DisplaySelection::Display2, wrong),
            PlacementStep::LeaveFullscreen,
            "a window BORN on the wrong monitor must be repaired without anyone touching the row",
        );

        // HOT-PLUG: settle stranded on one target, then let the display list change what the stored
        // rung resolves to.
        let mut trip = RoundTrip::default();
        assert_eq!(
            trip.step(fullscreen, DisplaySelection::Primary, wrong),
            PlacementStep::LeaveFullscreen
        );
        assert_eq!(
            trip.step(fullscreen, DisplaySelection::Primary, leaving),
            PlacementStep::ReenterFullscreen
        );
        assert_eq!(
            trip.step(fullscreen, DisplaySelection::Primary, wrong),
            PlacementStep::Settle
        );
        assert_eq!(trip, RoundTrip::Stranded(DisplaySelection::Primary));
        assert_eq!(
            trip.step(fullscreen, DisplaySelection::Display1, wrong),
            PlacementStep::LeaveFullscreen,
            "the same stored rung resolving somewhere else is a NEW target — a settled machine \
             must re-arm for it, or a display coming back is the one case never repaired",
        );

        // And a window that drifts into agreement on its own retires the machine, so a later
        // disagreement is judged fresh rather than against a stale attempt.
        let mut trip = RoundTrip::Stranded(DisplaySelection::Display2);
        assert_eq!(
            trip.step(
                fullscreen,
                DisplaySelection::Display2,
                ObservedPlacement {
                    fullscreen: true,
                    on_target: Some(true)
                },
            ),
            PlacementStep::Hold,
        );
        assert_eq!(trip, RoundTrip::Idle);
    }

    /// Wait arbitrarily many transition frames, release re-entry on the observed exit only, and
    /// never re-arm against re-entry or retry a failed trip. These pin the anti-flapping contract
    /// from [`RoundTrip`], including a target change mid-transition.
    #[test]
    fn the_round_trip_is_sequenced_on_the_observed_edge_and_never_re_enters_itself() {
        let fullscreen = WindowModeSetting::Fullscreen;
        let target = DisplaySelection::Display2;
        let wrong = ObservedPlacement {
            fullscreen: true,
            on_target: Some(false),
        };
        let leaving = ObservedPlacement {
            fullscreen: false,
            on_target: Some(false),
        };
        let landed = ObservedPlacement {
            fullscreen: true,
            on_target: Some(true),
        };

        let mut trip = RoundTrip::default();
        assert_eq!(
            trip.step(fullscreen, target, wrong),
            PlacementStep::LeaveFullscreen
        );
        for _ in 0..240 {
            assert_eq!(
                trip.step(fullscreen, target, wrong),
                PlacementStep::Hold,
                "every frame of the macOS transition must be silent — a second write here is \
                 parked by winit and replayed into the arm that does not exist",
            );
        }
        assert_eq!(
            trip.step(fullscreen, target, leaving),
            PlacementStep::ReenterFullscreen,
            "the observed edge is the ONLY thing that releases the second write",
        );
        for _ in 0..240 {
            assert_eq!(
                trip.step(fullscreen, target, leaving),
                PlacementStep::Hold,
                "the way back in is asynchronous too, and looks exactly like a window that is not \
                 fullscreen yet — re-arming on it would flap the window forever",
            );
        }
        assert_eq!(trip.step(fullscreen, target, landed), PlacementStep::Hold);
        assert_eq!(
            trip,
            RoundTrip::default(),
            "landing retires the trip, so a LATER disagreement is judged fresh",
        );

        // A change requested MID-TRANSITION: the half-done trip is never interrupted, and the
        // re-entry then honours whatever the row says at the moment it is written — so the newest
        // choice wins without a second trip being stacked on the first.
        let mut trip = RoundTrip::default();
        assert_eq!(
            trip.step(fullscreen, DisplaySelection::Display2, wrong),
            PlacementStep::LeaveFullscreen
        );
        for _ in 0..64 {
            assert_eq!(
                trip.step(fullscreen, DisplaySelection::Display3, wrong),
                PlacementStep::Hold,
                "a row moved mid-transition must not start a second trip on top of the first",
            );
        }
        assert_eq!(
            trip.step(fullscreen, DisplaySelection::Display3, leaving),
            PlacementStep::ReenterFullscreen,
        );
        assert_eq!(
            trip,
            RoundTrip::Spent(DisplaySelection::Display3),
            "the re-entry is what was actually written, so it is what the trip is spent on",
        );

        // And the row leaving fullscreen mid-trip abandons it: the pending write would put back a
        // fullscreen nobody is asking for any more.
        let mut trip = RoundTrip::default();
        assert_eq!(
            trip.step(fullscreen, target, wrong),
            PlacementStep::LeaveFullscreen
        );
        assert_eq!(
            trip.step(WindowModeSetting::Windowed, target, leaving),
            PlacementStep::Hold
        );
        assert_eq!(trip, RoundTrip::Idle);
    }

    /// A failed round trip emits one Settle, then stays silent across every stranded observation
    /// until the question changes. A standing mismatch must not flap between Spaces; see
    /// [`RoundTrip`]. Drive thousands of frames, including apparent fresh-trip openings.
    #[test]
    fn a_round_trip_that_does_not_land_settles_once_and_never_retries() {
        let fullscreen = WindowModeSetting::Fullscreen;
        let target = DisplaySelection::Display2;
        let wrong = ObservedPlacement {
            fullscreen: true,
            on_target: Some(false),
        };
        let leaving = ObservedPlacement {
            fullscreen: false,
            on_target: Some(false),
        };
        let unknown = ObservedPlacement {
            fullscreen: true,
            on_target: None,
        };

        let mut trip = RoundTrip::default();
        assert_eq!(
            trip.step(fullscreen, target, wrong),
            PlacementStep::LeaveFullscreen
        );
        assert_eq!(
            trip.step(fullscreen, target, leaving),
            PlacementStep::ReenterFullscreen
        );

        // The trip is over and the window is still on the wrong panel — the target vanished
        // mid-flight, or winit swallowed the re-entry too.
        assert_eq!(
            trip.step(fullscreen, target, wrong),
            PlacementStep::Settle,
            "the one frame that reports it",
        );
        assert_eq!(trip, RoundTrip::Stranded(target));

        for frame in 0..4096 {
            // Every observation the stranded state can be handed, including the ones that look like
            // a fresh trip's opening frames.
            let observed = match frame % 3 {
                0 => wrong,
                1 => leaving,
                _ => unknown,
            };
            assert_eq!(
                trip.step(fullscreen, target, observed),
                PlacementStep::Hold,
                "frame {frame} ({observed:?}): a settled machine must write NOTHING and report \
                 nothing again — one attempt and one log line per question, however long the \
                 disagreement stands",
            );
        }
        assert_eq!(
            trip,
            RoundTrip::Stranded(target),
            "and it must still be settled afterwards, not have wandered somewhere re-armable",
        );

        // The only things that re-open the question are a change in it: the target moving (covered
        // by `any_observed_disagreement_arms_the_trip_whatever_produced_it`) or the player leaving
        // fullscreen, after which a fresh fullscreen is a fresh question.
        assert_eq!(
            trip.step(WindowModeSetting::Windowed, target, leaving),
            PlacementStep::Hold
        );
        assert_eq!(trip, RoundTrip::Idle);
        assert_eq!(
            trip.step(fullscreen, target, wrong),
            PlacementStep::LeaveFullscreen,
            "a player who leaves fullscreen and comes back has asked the question again",
        );
    }

    /// A round trip must preserve the row and placement edge while `apply_settings` yields mode.
    /// Otherwise an unrelated edit cancels the transition or recentres the window and strands its
    /// next placement. Reproduce the observer's Leave/Reenter writes; the observer itself needs
    /// a real winit window.
    #[test]
    fn a_window_in_a_round_trip_neither_spends_the_placement_edge_nor_is_overwritten() {
        let mut app = window_app(
            Settings {
                window_mode: WindowModeSetting::Fullscreen,
                display: DisplaySelection::Display2,
                ..default()
            },
            3,
        );
        app.update();
        assert_eq!(
            window_state(&mut app).0,
            WindowMode::BorderlessFullscreen(MonitorSelection::Index(1)),
        );

        let set_window_mode = |app: &mut App, trip: RoundTrip, mode: WindowMode| {
            let world = app.world_mut();
            world.resource_mut::<WindowPlacement>().round_trip = trip;
            let mut windows = world.query_filtered::<&mut Window, With<PrimaryWindow>>();
            windows.single_mut(world).expect("one window").mode = mode;
        };

        // An unrelated row changes while the macOS fullscreen transition is still animating.
        set_window_mode(&mut app, RoundTrip::Leaving, WindowMode::Windowed);
        app.world_mut().resource_mut::<Settings>().msaa = MsaaLevel::Off;
        app.update();
        let (mode, position) = window_state(&mut app);
        assert_eq!(
            mode,
            WindowMode::Windowed,
            "the reconciler must yield the mode to a trip in flight, or the trip is cancelled \
             half-done and the edge it waits for never comes",
        );
        assert_eq!(
            position,
            WindowPosition::Automatic,
            "a window passing through windowed is not a player leaving fullscreen — spending the \
             edge here re-centres their windowed window and strands the next placement",
        );

        // Step two, and the row is still exactly what it always was.
        set_window_mode(
            &mut app,
            RoundTrip::Spent(DisplaySelection::Display2),
            WindowMode::BorderlessFullscreen(MonitorSelection::Index(1)),
        );
        app.world_mut().resource_mut::<Settings>().msaa = MsaaLevel::X4;
        app.update();
        assert_eq!(
            window_state(&mut app),
            (
                WindowMode::BorderlessFullscreen(MonitorSelection::Index(1)),
                WindowPosition::Automatic
            ),
        );
        assert_eq!(
            app.world().resource::<Settings>().window_mode,
            WindowModeSetting::Fullscreen,
            "nothing in a round trip may write the row it is serving",
        );

        // The edge the trip did not spend is still there when the player really leaves.
        app.world_mut().resource_mut::<Settings>().window_mode = WindowModeSetting::Windowed;
        app.update();
        assert_eq!(
            window_state(&mut app).1,
            WindowPosition::Centered(MonitorSelection::Index(1)),
        );
    }

    /// The other half of the edge: once placed, the window STAYS where the player drags it. winit
    /// rewrites `Window::position` to `At(..)` on every `Moved` event, so a level-triggered write
    /// would re-centre the window every time any unrelated row moved.
    #[test]
    fn an_unrelated_row_never_re_centres_a_window_the_player_moved() {
        let mut app = window_app(
            Settings {
                display: DisplaySelection::Display2,
                ..default()
            },
            3,
        );
        app.update();
        assert_eq!(
            window_state(&mut app).1,
            WindowPosition::Centered(MonitorSelection::Index(1)),
            "the launch application",
        );

        // What winit does when the player drags the window (state.rs:381-383).
        let dragged = WindowPosition::At(IVec2::new(37, 21));
        {
            let world = app.world_mut();
            let mut windows = world.query_filtered::<&mut Window, With<PrimaryWindow>>();
            windows.single_mut(world).expect("one window").position = dragged;
        }
        for row in [MsaaLevel::Off, MsaaLevel::X2, MsaaLevel::X4] {
            app.world_mut().resource_mut::<Settings>().msaa = row;
            app.update();
            assert_eq!(
                window_state(&mut app).1,
                dragged,
                "{row:?}: an unrelated row must not drag the window back to the middle",
            );
        }
        // But the row itself still moves it.
        app.world_mut().resource_mut::<Settings>().display = DisplaySelection::Display3;
        app.update();
        assert_eq!(
            window_state(&mut app).1,
            WindowPosition::Centered(MonitorSelection::Index(2)),
        );
    }

    /// The LOD budget row reaches the render world through exactly one door — [`apply_settings`]
    /// writing [`LodPixelBudget`]. `geometry_lod::compose_view_profile` consumes it; this test pins
    /// only the resource write, like `the_render_scale_row_is_applied_through_the_one_resource`.
    #[test]
    fn the_lod_budget_row_is_applied_through_the_one_resource() {
        let mut app = App::new();
        app.init_resource::<Settings>()
            .init_resource::<DirectionalLightShadowMap>()
            .init_resource::<crate::render_scale::RenderScale>()
            .init_resource::<PresentCaps>()
            .init_resource::<UiScale>()
            .init_resource::<LodPixelBudget>()
            .init_resource::<WindowPlacement>()
            .add_systems(Update, apply_settings);
        app.update();
        assert_eq!(
            app.world().resource::<LodPixelBudget>().0,
            1.0,
            "a default config must apply the default budget"
        );

        let mut budget = PixelBudget(PixelBudget::MIN);
        loop {
            app.world_mut().resource_mut::<Settings>().lod_pixel_budget = budget;
            app.update();
            assert_eq!(
                app.world().resource::<LodPixelBudget>().0,
                budget.pixels(),
                "{budget:?} did not reach the resource",
            );
            let next = budget.step(1);
            if next == budget {
                break;
            }
            budget = next;
        }
        assert_eq!(
            budget.pixels(),
            PixelBudget::MAX,
            "the walk ends at the top"
        );

        // A hand-edited absurd value is clamped on the way OUT, so no consumer ever sees a budget
        // it has to defend against.
        app.world_mut().resource_mut::<Settings>().lod_pixel_budget = PixelBudget(-3.0);
        app.update();
        assert_eq!(app.world().resource::<LodPixelBudget>().0, PixelBudget::MIN);
        app.world_mut().resource_mut::<Settings>().lod_pixel_budget = PixelBudget(9999.0);
        app.update();
        assert_eq!(app.world().resource::<LodPixelBudget>().0, PixelBudget::MAX);
    }

    /// Skipped fields must default to their type defaults, or sparse saves freeze defaults or
    /// omit choices. `version` is exempt: always written, defaulting to [`SETTINGS_VERSION`].
    /// This guards the next field as well as the current set.
    #[test]
    fn settings_defaults_are_their_types_defaults() {
        let settings = Settings::default();
        assert!(is_default(&settings.shadow_distance));
        assert!(is_default(&settings.shadow_resolution));
        assert!(is_default(&settings.shadow_cascades));
        assert!(is_default(&settings.msaa));
        assert!(is_default(&settings.bloom));
        assert!(is_default(&settings.vsync));
        assert!(is_default(&settings.render_scale));
        assert!(is_default(&settings.window_mode));
        assert!(is_default(&settings.display));
        assert!(is_default(&settings.lod_pixel_budget));
        assert!(is_default(&settings.frame_cap));
        assert!(is_default(&settings.ui_scale));
        assert!(is_default(&settings.debug_panel));
        assert!(is_default(&settings.pen_cam_corner));
        assert!(is_default(&settings.kill_cam));
        assert!(settings.kill_cam.on(), "the kill cam plays by default");
        assert!(is_default(&settings.reduce_motion));
        assert!(
            settings.debug_panel.shown(),
            "the debug card is on by default during the beta"
        );
        assert!(
            settings.legacy_vsync.is_none(),
            "the legacy shadow field must default absent — it exists only inside a parse"
        );
        assert_ne!(
            settings.version, 0,
            "the version stamp is the one field exempt from the sparse-write rule"
        );
    }
}
