//! Player-facing graphics settings: the persisted model, its on-disk home, and the ONE place that
//! applies it to the rendering world.
//!
//! **Not `dev_tools`-gated** — this ships, and it is the ONLY thing that configures the renderer.
//!
//! # Three writers: two player intent, one reality
//!
//! | Writer | Writes [`Settings`] | Writes the file |
//! |---|---|---|
//! | The settings page (`settings::ui`) | yes | **yes** — every change saves |
//! | [`observe_window_mode`] (OS fullscreen toggle) | yes — window mode only | **yes** — the green button IS a deliberate choice |
//! | [`normalize_vsync`] (a CONCLUSIVE capability probe answer) | yes — vsync only | **yes** — see [`Settings::effective_vsync`] |
//!
//! The third row is the odd one: it writes a value the player did NOT choose. It exists because a
//! `video.ron` is portable between machines while the vsync ladder is not — a rung this surface
//! cannot present is a stored value that can only ever be a lie, and the file is where the lie
//! would otherwise persist. See [`Settings::effective_vsync`] for the whole argument. Because it
//! spends a player's stored choice, it acts ONLY on a probe that positively reported the surface's
//! capability list — never on a probe that could not ask ([`PresentCaps`] is a tri-state for
//! exactly this reason).
//!
//! The table is exhaustive: nothing else writes this resource, so a value can only be in it because
//! the player put it there.
//!
//! The saving is deliberately NOT "save whatever `Settings` holds when the app exits": only an
//! explicit [`SaveSettings`] writes the file, so a value that arrived any other way can never be
//! laundered into the player's config.
//!
//! # Forward compatibility, and the one rule for changing this file
//!
//! Every field is `#[serde(default)]` and serde ignores unknown fields, so **adding a setting needs
//! no migration in either direction**: an old file missing the new key loads it at its default, and
//! an old build reading a newer file skips the key it doesn't know. `render_scale` was the first
//! exercise of that; the V2 slice (window mode, frame cap, UI scale) landed exactly that way — a
//! `Default`, a `skip_serializing_if`, and a row in `ui::Row::ORDER`; nothing else moved. The one
//! V2 entry that could NOT ride the free path was vsync's third rung — a `bool` field cannot grow
//! one — and its key rename (`vsync` → `vsync_mode`, with the old key still read) is documented on
//! [`VsyncMode`].
//!
//! **The policy, stated once so it is not re-litigated:**
//!
//! * ADD or REMOVE a field — free. Do nothing.
//! * RENAME a field — add `#[serde(alias = "old_name")]`. That is the ENTIRE rename story (it is
//!   what rustup does), and it **never bumps [`SETTINGS_VERSION`]**.
//! * Change a field's TYPE while keeping its on-disk shape — free, via `#[serde(from/into)]`. That
//!   is how `vsync` first became a two-rung [`VsyncMode`] ladder while still reading and writing
//!   the `bool` v1 shipped. When the shape CANNOT hold the new type (the ladder's third rung), the
//!   move is a new key plus a read-only legacy shadow of the old one — see [`VsyncMode`]'s
//!   migration note; still no version bump.
//! * Change a field's MEANING under the same name — the only thing serde cannot express, and the
//!   only thing that bumps the version.
//!
//! Files from the future are refused rather than guessed at, and moved aside so a downgrade cannot
//! silently overwrite them. See `store` for the mechanism and
//! `.agents/docs/design/display/persistence-brief-2026-07.md` for the evidence behind all of it.

use bevy::light::{CascadeShadowConfig, CascadeShadowConfigBuilder, DirectionalLightShadowMap};
use bevy::prelude::*;
use bevy::render::view::Msaa;
use bevy::ui::UiScale;
use bevy::window::{MonitorSelection, PresentMode, PrimaryWindow, WindowMode};
use serde::{Deserialize, Serialize};

/// The end-of-frame frame-rate limiter — armed only by [`VsyncMode::Off`] plus a non-off
/// [`FrameCap`]. Pure std timing; see its doc for the coarse-sleep-then-spin shape.
mod limiter;
/// The one-shot present-mode capability probe: creates a throwaway wgpu surface for the primary
/// window IN THE RENDER WORLD, reads `Surface::get_capabilities().present_modes`, and shuttles the
/// result into the main-world [`PresentCaps`]. Probe, don't guess — no `cfg(target_os)` anywhere.
mod probe;
/// The persistence SEAM — location, filename, format, write safety, tolerance. Swapping any of that
/// is a rewrite of one file. Its doc records why the hand-roll beats every crate AND bevy's own
/// first-party `bevy_settings` (settled: brief §1), and why the file is named `video.ron`.
mod store;
/// The page itself. Private to this module: composition roots reach it only through [`plugin`],
/// which is what makes mounting the page without its entry declarer unrepresentable.
mod ui;

/// The schema generation. See the module doc's policy: renames use `#[serde(alias)]` and do NOT
/// bump this; only a field whose MEANING changed under the same name does.
const SETTINGS_VERSION: u32 = 1;

/// The DEFAULT cascade count — [`ShadowCascades::default`]'s value, and the ONE number the whole
/// shadow ladder is priced against.
///
/// **It was 4 (bevy's own `CascadeShadowConfigBuilder::num_cascades`) until 2026-07-28, and it is 3
/// now.** MEASURED on an M4 (10 GPU cores, two idling Tigers, release, vsync off, 20 s settled
/// windows): a cascade costs **~1.16 ms of frame time each**, while the whole 1024 → 4096 map-size
/// axis spans 0.27 ms and the whole 150 → 1000 m distance axis costs ~0.71 ms (re-measured
/// 2026-07-28 at the shipped 3 cascades / 4096 with track shadow proxies on; the first reading of
/// this axis was ~1.17 ms at 4 cascades / 2048, which is the same axis priced against a costlier
/// ladder). The cascade COUNT is the expensive axis and nothing else on this page is close, so
/// dropping one cascade buys back more than the crispness and the envelope together cost — and the
/// shipped envelope only spends 0.30 ms of that 0.71 ms axis anyway
/// ([`ShadowDistance`]'s table). What keeps the picture honest at 3 is
/// [`SHADOW_FIRST_CASCADE_FAR_BOUND_M`] — see its doc for the split arithmetic, which is the reason
/// this drop is not simply a quality cut.
///
/// **This used to be a hard constant, and the story below is why.** The count is a live row now
/// ([`ShadowCascades`]), and changing it at runtime is safe ONLY because
/// `vendor/bevy_light-0.19.0-cascade-count/` backports upstream PR #24807 (merged, milestone
/// 0.19.1 — unreleased as of 2026-07-27); see
/// `.agents/docs/upstream/bevy-cascade-count-stale-local-parallel.md` for the upstream record and
/// the validation evidence. **The vendored patch must outlive this row**: dropping the
/// `[patch.crates-io]` entry before bevy 0.19.1 ships reintroduces the crash below on the first
/// grow step. Everything from MEASURED down is kept as the historical record of the mechanism.
///
/// MEASURED 2026-07-26 (field crash): stepping a shadow knob from a 2-cascade setting back to a
/// 4-cascade one panicked in `bevy_light-0.19.0/src/lib.rs:477`,
/// `check_dir_light_mesh_visibility` — "index out of bounds: the len is 2 but the index is 2".
///
/// The mechanism, read out of bevy_light/bevy_utils rather than inferred. That system accumulates
/// per-cascade visible entities into `mut view_visible_entities_queue: Local<Parallel<Vec<Vec<Entity>>>>`.
/// The per-cascade length is established in `par_iter().for_each_init`'s INIT closure
/// (`entities.resize(view_frusta.len(), …)`), which only runs on threads that actually receive a
/// chunk that frame — but the collect loop afterwards walks `Parallel::iter_mut()`, which is
/// `self.locals.iter_mut()`: EVERY thread-local ever borrowed, including ones that sat out this
/// frame. Because the `Local` outlives the frame, a queue still sized to the previous cascade count
/// survives there and gets indexed with the new one. Growing the count (2 → 4) indexes past the end
/// and panics; shrinking (4 → 2) does not panic and silently drops cascades instead, which is worse
/// to debug. It is scheduling-dependent — whether any pooled thread misses a frame — so it is
/// neither reliably reproducible nor reliably absent.
///
/// The rule this left ON STOCK 0.19.0: **`maximum_distance` and the shadow-map SIZE are safe to
/// change at runtime (no per-cascade array length moves); the cascade COUNT is not.** The vendored
/// backport (above) is what retired that rule — it resizes every thread slot before each view's
/// par_iter, exactly as merged upstream — and is the sole reason [`ShadowCascades`] may exist.
///
/// # The general hazard, and the audit that bounds it
///
/// That crash is one instance of a class: bevy caches per-frame work in `Local`s that OUTLIVE the
/// frame, and a `Local` sized from a config value goes stale the moment that value changes. Every
/// other setting [`apply_settings`] writes was audited against bevy 0.19 for the same shape
/// (`Local<Parallel<…>>` — pooled thread-locals, exactly what blew up):
///
/// | Site | Element | Positionally indexed by a config length? |
/// |---|---|---|
/// | `bevy_light` `check_dir_light_mesh_visibility` | `Vec<Vec<Entity>>` | **YES — cascade count. The bug.** |
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
/// * **Render scale** — `MainPassResolutionOverride` is reconciled onto the render-world view every
///   frame from an extracted resource, and it feeds only per-frame values: the pass scissor rect and
///   `View::main_pass_viewport`. Nothing is sized from it — view targets, depth and prepass textures
///   are all still allocated at the full `physical_target_size` (see [`crate::render_scale`]'s
///   honest-limits section) — so there is no length for a stale `Local` to index with.
pub(crate) const SHADOW_CASCADES: usize = 3;

/// Far bound of the FIRST cascade, metres — the split knob, stated here instead of inherited from
/// `CascadeShadowConfigBuilder::default()`'s **10.0**.
///
/// Bevy splits the camera frustum geometrically: cascade `i`'s far bound is
/// `first · (maximum/first)^(i/(n−1))`. So `first` is not "how far the crisp cascade reaches", it is
/// the RATIO between neighbouring cascades — and at a fixed count, raising it flattens the whole
/// ladder. Bevy's 10.0 is a character-scale default (a 10 m first cascade is a room); on a 1 km map
/// viewed from a tank it spends the entire cascade budget inside the first 10 m of the picture and
/// leaves the far field to one enormous slab.
///
/// The arithmetic, at 16:9 / 45° vertical / 1964 px tall, in **screen pixels per shadow texel at the
/// cascade's near bound** (the worst point in a cascade, and the number a player actually sees):
///
/// | config | worst px per texel |
/// |---|---|
/// | 4 casc, first 10, 150 m, 2048 (the old default) | 6.08 |
/// | 3 casc, first 10, 1000 m, 4096 | 12.23 |
/// | 3 casc, first 40, 1000 m, 4096 | 6.12 |
/// | 3 casc, first 10, 350 m, 4096 | 7.24 |
/// | **3 casc, first 40, 350 m, 4096 (the default)** | **3.64** |
///
/// That is the whole case for the constant: at bevy's 10.0 the cascade drop would halve far-field
/// crispness, and 40.0 buys all of it back — at the 1000 m envelope it lands within 0.7 % of the old
/// default while reaching 6.7× further, and at the 350 m envelope that actually ships it lands 1.7×
/// CRISPER than the old default while still reaching 2.3× further. The quality is free because it
/// comes out of near-field texel density nobody was looking at — the first cascade's texel grows
/// 0.83 cm → 1.66 cm, which is still under two centimetres on a 3 m tank.
///
/// Note the two knobs are not interchangeable, which is why both are pinned: at the shipped 350 m
/// envelope, winding this constant back to bevy's 10.0 would cost 7.24 against 3.64 — the ENVELOPE
/// being short does not rescue a badly split ladder.
///
/// **Not a free parameter to nudge.** It is coupled to [`SHADOW_CASCADES`] through that exponent: at
/// a different count, 40 m means a different ratio and the table above has to be recomputed.
const SHADOW_FIRST_CASCADE_FAR_BOUND_M: f32 = 40.0;

/// How much each cascade overlaps the previous one, as a fraction of the previous far bound — bevy's
/// own default value, stated explicitly for the same reason as
/// [`SHADOW_FIRST_CASCADE_FAR_BOUND_M`]: the builder's `..default()` is a silent third author of the
/// picture, and one of its two silent values turned out to be wrong for this world.
///
/// Deliberately UNCHANGED at 0.2. It buys the blend band that hides the seam where a shadow crosses
/// a cascade boundary, and it is paid for out of the near bound — the `(1 − overlap)` in the
/// px-per-texel table above. Nothing measured argues with it, so it stays where it was, now visibly.
const SHADOW_CASCADE_OVERLAP: f32 = 0.2;

/// How many cascades the directional shadow map is split into — how gracefully shadow crispness
/// falls off with distance, at a fixed [`ShadowResolution`] and [`ShadowDistance`]. Fewer cascades
/// cost less (one shadow-map render pass each) and alias more near the camera.
///
/// **A live row only by grace of the vendored bevy_light patch** — see [`SHADOW_CASCADES`]'s doc
/// for the crash that makes stock 0.19.0 unable to grow this at runtime, and for the vendor entry
/// that must outlive this type.
///
/// **This is the expensive row** — the only one on this page worth a whole millisecond. MEASURED on
/// an M4: ~1.16 ms per cascade, against 0.27 ms for the entire resolution ladder. A frame-rate
/// complaint is almost always this row.
///
/// No `Off` and no `1`: switching shadows off is [`ShadowDistance::Off`]'s job, and a single
/// cascade stretched to the full distance is a quality floor nobody asked for — 2 is already the
/// honest budget rung.
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
    /// The `CascadeShadowConfigBuilder::num_cascades` this asks for. The default rung IS
    /// [`SHADOW_CASCADES`] — named here so the const and the ladder cannot drift apart.
    pub(crate) const fn count(self) -> usize {
        match self {
            ShadowCascades::Two => 2,
            ShadowCascades::Three => SHADOW_CASCADES,
            ShadowCascades::Four => 4,
        }
    }

    /// ASCII only — it reaches `Text`. The bare count, for the same honest-numbers reason
    /// [`ShadowDistance::label`] quotes metres.
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
/// **Split from [`ShadowResolution`] on purpose** (Yan, 2026-07-27), and the split is about COST,
/// not about quality. As costs they really are independent, which is why they are two rows: distance
/// widens the area each cascade must cover, resolution buys texels at a fixed area, and pairing them
/// into coupled Low/Medium/High presets — as this type used to — meant a player who wanted the far
/// envelope had to pay for 4K maps too, and hid which of the two a frame-rate complaint was about.
///
/// **As QUALITY they are coupled, and the doc here used to say otherwise** (corrected 2026-07-28).
/// A cascade's texel is `frustum_diameter / map_size`, so distance grows it and resolution shrinks
/// it, and the artefact that texel size drives is not blur — it is **peter-panning**, the shadow
/// sliding off the object that casts it. `bevy_pbr` offsets the shadow lookup by
/// `shadow_normal_bias · √2 · cascade_texel` along the surface normal (see [`crate::world`]'s
/// `SUN_SHADOW_NORMAL_BIAS`), i.e. detachment is **3.68 × the cascade's texel**, in metres of world.
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
/// # Why the default is the FLOOR rung, not the far one (measured 2026-07-28)
///
/// The envelope was priced on an M4 at the shipped 3 cascades / 4096 / 40 m first bound, two tanks
/// idling with track shadow proxies on, against shadows-off controls that agreed to 0.157 ms across
/// the session:
///
/// | envelope | frame p50 | px per texel (lower = crisper) |
/// |---|---|---|
/// | 150 m (retired ceiling, reachable only via [`shadow_distance_override`]) | 14.56 ms | 2.44 |
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
/// **`Off` is reachable, and that RESCINDS a stated product rule.** The rule was: shadows are
/// GAMEPLAY INFORMATION, not decoration — a hull-down tank's shadow, the shadow a barrel throws
/// across a slope, the dark under a treeline are all things a player reads to find and range a
/// target, so a client that could switch them off would see targets its opponent could not, which is
/// a competitive-integrity break rather than a graphics option (the same reason competitive shooters
/// pin foliage and shadow draw). That reasoning is NOT withdrawn — it is SUSPENDED pending perf data
/// (Yan, 2026-07-27): whether the shadow budget is where 15v15 frame headroom has to come from is a
/// measurement nobody has yet, and the whole-budget number needs the off state to exist. **Removing
/// `Off` again once that measurement lands is a deliberately open question**, so nothing downstream
/// may assume either way.
///
/// # The ladder moved on 2026-07-28, and old files migrate
///
/// The rungs were `100 / 150 / 300`; they are `350 / 700 / 1000` now. The old ceiling was never a
/// choice anyone made — 150 m is bevy's stock `maximum_distance`, tuned for character-scale scenes —
/// and it is below this world's PHYSICAL floor: at [`crate::world`]'s 17° sun a shadow runs 3.27×
/// the caster's height, so 100 m of terrain relief self-shadows out to ~327 m. Under the old ladder
/// a ridge simply stopped shading the valley behind it partway down, at every rung.
///
/// **`M100`/`M150`/`M300` are gone as VARIANTS and survive as `#[serde(alias)]`s** on the nearest
/// surviving rung, so an existing `video.ron` loads without error and lands on a real ladder
/// position. Migrating beats keeping them representable-but-hidden: `ui::step_in` treats a current
/// value it cannot find in `ORDER` as index 0, so an off-ladder rung would send the next press of
/// the right arrow to `Off` — a stored value the page cannot walk out of is worse than a stored
/// value the page silently rounds. All three land on `M350` (|300 − 350| = 50 beats |300 − 700| =
/// 400), which does mean a player who chose 100 m for frame rate is moved UP; that is accepted
/// because the same release drops a cascade, and the 150 → 350 m they are moved across costs
/// ~0.30 ms (table below) against the ~1.16 ms that one fewer cascade gives back — see
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
    /// The map is 1 km across (`terrain_grid::WORLD_SIZE`), so this is the rung at which the whole
    /// world casts and the draw volume stops growing — MEASURED: the cost curve saturates around
    /// 700 m for exactly that reason (700 and 1000 m are within the session's own repeatability).
    /// It was the shipped default for one day, 2026-07-28; the type doc's envelope table is why it
    /// is a top rung instead. Only affordable together with [`ShadowResolution::X4096`] — at 2048 it
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

    /// The row value the settings page renders. ASCII only — it reaches `Text`. Honest metres rather
    /// than LOW/MEDIUM/HIGH: the number is the thing a player can actually compare against a sight
    /// picture, and it is what makes "my shadows stop halfway to that ridge" a self-answering
    /// complaint.
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

/// Edge length of each cascade's shadow map — how CRISP a shadow is at a given
/// [`ShadowDistance`]. Separate ROWS because their costs are separable, NOT independent in quality:
/// see [`ShadowDistance`] for the peter-panning that couples them, and for why this row's default
/// moved in the same change as that one's.
///
/// **The cheap row.** MEASURED on an M4: the whole 1024 → 4096 span is 0.27 ms, against ~1.16 ms for
/// a single cascade ([`ShadowCascades`]). Texel count is not what a tiled GPU is spending its shadow
/// budget on; render-pass count is.
///
/// Deliberately has no `Off`: switching shadows off is [`ShadowDistance::Off`]'s job, and two ways to
/// express the same state is how a UI ends up with rows that contradict each other.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default, Serialize, Deserialize)]
pub(crate) enum ShadowResolution {
    X1024,
    /// Bevy's own `DirectionalLightShadowMap::default().size`, and the shipped default until
    /// 2026-07-28 — it detaches a far shadow by ~1.07 m at the shipped 350 m envelope and by ~3 m at
    /// [`ShadowDistance::M1000`], so it is a budget rung now rather than the reference one.
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

    /// ASCII only — it reaches `Text`. The texel count itself, for the same reason
    /// [`ShadowDistance::label`] quotes metres.
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

/// What the window surface actually supports — the probe's answer (see `probe`), as a TRI-STATE.
///
/// **"We could not learn" is not "the surface lacks it", and the difference is a player's saved
/// setting.** This was a two-field struct with a `probed: bool`, and the probe reported a FAILED
/// surface creation as `probed: true` with an empty capability list — an inability to ask, dressed
/// up as a conclusive negative answer. Read-only consumers survived that (an empty list gates the
/// ladder down to the universally-supported ON, which is safe), but [`normalize_vsync`] is a
/// WRITER: it would have taken the fabricated negative as authority, rewritten a perfectly valid
/// stored FAST/OFF to ON, and SAVED it — a transient probe failure permanently eating the player's
/// choice. So the two states are now distinct variants, and only [`PresentCaps::Reported`] is ever
/// treated as evidence of a rung's absence.
///
/// **There are TWO ways to fail to learn, and wgpu makes the second one silent.** Creating the
/// probe surface can error, and `Surface::get_capabilities` cannot error at all — wgpu 29 answers a
/// failed query (and an adapter-incompatible surface) with an empty present-mode list. Since any
/// presentable surface reports at least `Fifo`, `probe` maps BOTH to
/// [`PresentCaps::Unavailable`], and a `Reported` value can only ever come from a list that
/// actually had something in it.
///
/// Both unknown states behave identically at runtime, and identically to the pre-probe boot state
/// this type has always had: the page offers every rung, [`Settings::effective_vsync`] is the
/// identity (nothing is normalised on a guess), and [`Settings::present_mode`] answers with the
/// self-negotiating [`PresentMode::AutoNoVsync`] for the uncapped rungs — bevy filters that against
/// the real capabilities at surface-configure time, so an unprobed frame can never panic a backend.
/// A concrete `Mailbox`/`Immediate` is still emitted ONLY from a positive [`PresentCaps::Reported`],
/// which is the Metal-panic guard and is unchanged by any of this.
#[derive(Resource, Clone, Copy, PartialEq, Eq, Debug, Default)]
pub(crate) enum PresentCaps {
    /// The boot state: the probe has not answered yet. The two windowed roots describe their window
    /// with this deliberately — it is what "we have no surface to ask about yet" looks like.
    #[default]
    Unprobed,
    /// The probe ran and could NOT learn — it could not create a probe surface, or the capability
    /// query came back empty (wgpu reports a failed query, and an adapter-incompatible surface, as
    /// an empty present-mode list rather than an error; see `probe`). Terminal — the probe does not
    /// retry — and deliberately NOT a statement about what the surface supports.
    Unavailable,
    /// The surface's own capability list, distilled to the two rungs the ladder gates on. The only
    /// variant that is evidence a rung is missing, and it is only ever built from a NON-EMPTY list.
    ///
    /// Both flags `false` therefore means something specific and real: the surface answered with a
    /// list that carries neither uncapped mode — `[Fifo]`, or `[Fifo, FifoRelaxed]`. That is the
    /// genuine conclusive negative (it gates the ladder to ON alone and DOES normalise a stored
    /// FAST/OFF), and it is exactly what an empty list is NOT.
    Reported { immediate: bool, mailbox: bool },
}

impl PresentCaps {
    /// Whether the probe has answered at all, either way. The `receive_probe` run condition — an
    /// [`PresentCaps::Unavailable`] answer must stop the polling exactly as a reported one does.
    pub(crate) const fn answered(self) -> bool {
        !matches!(self, PresentCaps::Unprobed)
    }

    /// `PresentMode::Immediate` was POSITIVELY reported in the surface's capability list. Both
    /// unknown states answer `false` — this is the question `present_mode` must ask before emitting
    /// a concrete mode, and "we don't know" is not a yes.
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

    /// Whether the settings page should OFFER this rung. Both unknown states offer everything
    /// (there is nothing to gate on, and the mapping below stays safe regardless); a reported list
    /// offers only what it contains. FAST deliberately requires `Mailbox` specifically — falling
    /// back to `Immediate` would make FAST and OFF the same rung twice on a Metal surface.
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

    /// The rung a stored `mode` actually RESOLVES to here: itself while it is offered, otherwise
    /// [`VsyncMode::On`] — the universal `Fifo` rung, and the one every fallback chain in this
    /// module already terminates at. Both unknown states resolve to `mode` unchanged, because
    /// [`offers`] offers everything until there is something real to gate on — which is also what
    /// makes [`normalize_vsync`] a no-op unless the surface conclusively answered.
    ///
    /// **Why ON rather than the adjacent rung.** A Wayland surface refuses `Immediate` but has
    /// `Mailbox`, so a stored OFF *could* be walked to FAST instead — still uncapped, still the
    /// player's "don't wait for the display". It deliberately is not: FAST is a different product
    /// promise (no tearing, no frame cap) and picking it for the player is a guess about which half
    /// of OFF they wanted. ON is the rung the surface can honour without inventing intent, and it is
    /// the same one an unsupported rung's present mode already negotiates down to.
    ///
    /// [`offers`]: PresentCaps::offers
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
/// mode whose monitor cannot be resolved (the `--reset-display` escape hatch's origin story), and
/// borderless is what every modern display stack actually wants.
///
/// The monitor selection is always [`MonitorSelection::Current`], which cannot panic for
/// borderless: `bevy_winit::select_monitor` merely warns and passes `None` (= current) through.
///
/// **The row REFLECTS the OS, it does not own it** — see `observe_window_mode`: the player can
/// toggle fullscreen with the green button, and the stored value follows reality rather than
/// fighting it.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default, Serialize, Deserialize)]
pub(crate) enum WindowModeSetting {
    /// **The shipped default** — the window the game has always opened with.
    #[default]
    Windowed,
    Fullscreen,
}

impl WindowModeSetting {
    /// The `Window::mode` this asks for — the ONE mapping, used at boot window description and by
    /// `apply_settings` alike.
    pub(crate) const fn to_window_mode(self) -> WindowMode {
        match self {
            WindowModeSetting::Windowed => WindowMode::Windowed,
            WindowModeSetting::Fullscreen => {
                WindowMode::BorderlessFullscreen(MonitorSelection::Current)
            }
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

/// Whether a field still holds its default — the test behind every `skip_serializing_if` below.
///
/// ONE generic function rather than a per-field one, which is only possible because every skipped
/// field's [`Settings::default`] value now equals its own type's `Default` (the last exception was
/// `vsync: bool`, whose "on" default was the opposite of `bool::default()` — [`VsyncMode`] retired
/// it). `settings_defaults_are_their_types_defaults` pins that structurally, so this cannot silently
/// start writing a field it should skip.
fn is_default<T: Default + PartialEq>(value: &T) -> bool {
    *value == T::default()
}

/// The persisted player settings — and, as a `Resource`, the live truth the renderer is reconciled
/// against by [`apply_settings`]. Nothing else may write `Msaa`, `CascadeShadowConfig`,
/// `DirectionalLightShadowMap` or `Window::present_mode`.
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
    /// The cascade count — live-changeable only because of the vendored bevy_light backport; see
    /// [`ShadowCascades`] and [`SHADOW_CASCADES`].
    #[serde(skip_serializing_if = "is_default")]
    pub(crate) shadow_cascades: ShadowCascades,
    #[serde(skip_serializing_if = "is_default")]
    pub(crate) msaa: MsaaLevel,
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
    /// The frame-rate cap, active only under [`VsyncMode::Off`] — see [`FrameCap`].
    #[serde(skip_serializing_if = "is_default")]
    pub(crate) frame_cap: FrameCap,
    /// UI scale percent — a multiplier on [`UiScale`]; see [`UiScalePercent`].
    #[serde(skip_serializing_if = "is_default")]
    pub(crate) ui_scale: UiScalePercent,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            version: SETTINGS_VERSION,
            shadow_distance: ShadowDistance::default(),
            shadow_resolution: ShadowResolution::default(),
            shadow_cascades: ShadowCascades::default(),
            msaa: MsaaLevel::default(),
            vsync: VsyncMode::default(),
            legacy_vsync: None,
            render_scale: RenderScaleLevel::default(),
            window_mode: WindowModeSetting::default(),
            frame_cap: FrameCap::default(),
            ui_scale: UiScalePercent::default(),
        }
    }
}

impl Settings {
    /// The vsync rung this configuration is EFFECTIVELY running at on a surface with `caps` — the
    /// one fact the present mode, the frame-cap gate and the page's greying all read, so that a
    /// stored rung the surface cannot present is impossible to disagree about.
    ///
    /// **The bug this exists to make unrepresentable.** A `video.ron` written on a Windows/Vulkan
    /// machine can carry `vsync_mode: Fast`; copied to a Mac, `Mailbox` does not exist, the present
    /// mode negotiates down (to `Immediate` in practice), and the page went on displaying FAST over
    /// a surface that was tearing exactly like OFF. The mirror case is a stored OFF on Wayland,
    /// where `Immediate` is refused: the surface ends up compositor-paced while the frame-cap row
    /// stayed lit and armed, so the player had two limiters fighting over a rung they were not
    /// actually on. Both are the same fault — three consumers each deciding for themselves what an
    /// unsupported stored value means.
    ///
    /// **Only a REPORTED capability list resolves anything.** Both unknown states ([`PresentCaps`]'s
    /// pre-probe and could-not-ask variants) are the identity here, deliberately: with nothing
    /// learned, the player's stored rung is the best available statement of what they are on, so the
    /// page keeps showing it and the frame-cap row keeps following it. `present_mode` is where
    /// safety is enforced in that state, and it needs no help from this — it emits a concrete
    /// uncapped mode only from a positive report and negotiates otherwise.
    ///
    /// [`normalize_vsync`] additionally writes this value back into [`Settings::vsync`] (and to
    /// disk) once the probe lands, so the stored rung converges on the honest one instead of
    /// sitting there as phantom state. The COST of that, stated so it is a known trade rather than
    /// a surprise: a config roamed between a Metal laptop and a Vulkan desktop forgets FAST on the
    /// laptop's first launch. Accepted, because the alternative is a file whose values only mean
    /// something once you know which machine last read them.
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

    /// The frame period `limiter` must enforce, or `None` for "do not limit". This is the ONE place
    /// the "cap only under VSync OFF" conjunction is decided — the page greys the row from the same
    /// fact ([`ui`] calls the row disabled exactly when this side of it is `None`-by-vsync), so the
    /// picture and the limiter cannot disagree.
    ///
    /// It gates on the EFFECTIVE rung ([`Settings::effective_vsync`]), not the stored one, which is
    /// what makes that agreement structural rather than a consequence of [`normalize_vsync`] having
    /// already run: in the frames between a probe landing and the normalisation write, an
    /// unsupported stored OFF still arms nothing.
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

/// The CLI escape hatch: `overmatch --reset-display` deletes the saved settings and launches on
/// defaults.
///
/// It exists because a display setting can make the game unlaunchable on the machine that saved it —
/// bevy panics on an EXCLUSIVE `Fullscreen` whose monitor cannot be resolved, so a config written
/// with a second display attached could brick boot once it is unplugged. A player in that state
/// cannot reach the settings page to undo it, so the escape must live outside the app's own UI.
/// (The window-mode row that landed deliberately keeps that state unrepresentable —
/// [`WindowModeSetting`] is borderless-on-current-monitor only, which winit merely warns about —
/// so nothing in [`Settings`] can cause it today either; the hatch stays because config files are
/// hand-editable and future rows may not be so careful.)
const RESET_FLAG: &str = "--reset-display";

/// Read the player's settings INTO the app being built, and hand the values back.
///
/// Called by each windowed composition root before `DefaultPlugins`, because
/// [`Settings::present_mode`] has to be known when the primary `Window` is described — a present
/// mode applied after surface configuration costs a visible reconfigure on the first frame, and the
/// window-mode/size fields queued behind the display research cannot be applied after creation at
/// all. That is also why the values are RETURNED as well as inserted: the caller needs them for the
/// `Window` it is about to describe.
///
/// Both resources are inserted here rather than by each root, because they must travel together and
/// were being hand-copied: the values are what [`plugin`]'s `init_resource` then finds already
/// present, and the report is carried as DATA because this read happens before `LogPlugin` installs
/// a subscriber ([`report_store_load`] turns it into log lines at `Startup`).
///
/// Honours [`RESET_FLAG`] first: with it, the file is deleted and defaults are used. Never fails: a
/// missing or corrupt file yields defaults.
pub(crate) fn load_at_boot(app: &mut App) -> Settings {
    let (settings, note) = if std::env::args().any(|arg| arg == RESET_FLAG) {
        store::reset();
        // Deliberately reports as a first launch rather than inventing a note: after the reset that
        // is exactly what the next read would see, and it is what the player asked for.
        (Settings::default(), store::LoadNote::FirstLaunch)
    } else {
        let store::Load { settings, note } = store::load();
        (settings, note)
    };
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
                    observe_window_mode.before(ui::DeclareSettingsPage),
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

/// Dev-only far-shadow override, metres: `OVERMATCH_SHADOW_DISTANCE_M` replaces whatever
/// [`ShadowDistance`] the settings row asks for, so the cascade envelope can be set to values the
/// player-facing enum does not offer.
///
/// **Still earning its keep after the 2026-07-28 ladder change**, which is worth stating because the
/// question it was built to answer — "does the whole map cast?" — is now a rung
/// ([`ShadowDistance::M1000`]). What it is for now is the SPACE BETWEEN, BELOW and BEYOND the rungs:
/// the ladder is four values and a sweep needs a continuum, so measuring where the cost curve
/// actually saturates (it is flat by ~700 m on this 1 km map — see [`ShadowDistance`]'s envelope
/// table) or what 2 km would do still needs this. It is also the only way to reach a RETIRED
/// envelope: the 2026-07-28 decision sweep priced the old 150 m ceiling through
/// `OVERMATCH_SHADOW_DISTANCE_M=150`, because no rung expresses it any more. A ladder rung is a
/// product decision; this is the instrument that produces the evidence for the next one.
///
/// Read ONCE and cached: [`apply_settings`] runs every frame and must not touch the process
/// environment there. Unset — every normal run, and every test — yields `None` and the enum decides,
/// so this is inert rather than a second authority. It does NOT enable casting: with
/// [`ShadowDistance::Off`] selected the light still stops casting, the override only sizes the
/// envelope.
///
/// One sharp edge, since the value is unvalidated beyond `> 0`: a distance below
/// [`SHADOW_FIRST_CASCADE_FAR_BOUND_M`] inverts the split (the cascade bounds run backwards) rather
/// than being clamped. That threshold used to be bevy's 10 m and is 40 m now, so the useless range
/// is wider than it was. Nothing asserts, because nothing should — the ladder's own floor is 350 m
/// and this is an instrument, not a rung.
fn shadow_distance_override() -> Option<f32> {
    static OVERRIDE: std::sync::OnceLock<Option<f32>> = std::sync::OnceLock::new();
    *OVERRIDE.get_or_init(|| {
        crate::env_parse::<f32>("OVERMATCH_SHADOW_DISTANCE_M").filter(|metres| *metres > 0.0)
    })
}

/// Reconcile the rendering world to [`Settings`]. The ONE writer of every knob below, which is what
/// makes "the page and the picture cannot disagree" true by construction rather than by discipline.
fn apply_settings(
    settings: Res<Settings>,
    mut shadow_map: ResMut<DirectionalLightShadowMap>,
    mut cameras: Query<&mut Msaa, With<Camera3d>>,
    mut lights: Query<(&mut DirectionalLight, &mut CascadeShadowConfig)>,
    // Optional because a headless/simulate root has no primary window at all — the present mode is
    // then simply nothing to apply, not a missing dependency.
    window: Option<Single<&mut Window, With<PrimaryWindow>>>,
    // NOT optional: `plugin` mounts `render_scale::plugin` itself, so every path that can run this
    // system has the resource. That is the property the fold bought.
    mut render_scale: ResMut<crate::render_scale::RenderScale>,
    // What the surface reported (or an unknown state, before/instead of that) — the gate on the
    // uncapped modes.
    caps: Res<PresentCaps>,
    // bevy_ui's global multiplier. Present on every windowed root (it comes with `UiPlugin`); a
    // bare-`App` test must init it, same as the shadow map above.
    mut ui_scale: ResMut<UiScale>,
) {
    let msaa = settings.msaa.to_msaa();
    for mut camera_msaa in &mut cameras {
        camera_msaa.set_if_neq(msaa);
    }

    // The three shadow rows are reconciled independently, because they are independent settings:
    // the resolution applies whatever the distance says, and the distance's `Off` touches only the
    // cast switch. (The cascade-count row rides the same rebuild as the distance, below — the two
    // share the one `CascadeShadowConfig`.)
    let size = settings.shadow_resolution.shadow_map_size();
    // Compare before writing: `DirectionalLightShadowMap` is extracted to the render world every
    // frame, and touching it through `ResMut` unconditionally would mark it changed forever. (Hand
    // written rather than `set_if_neq` — the bevy type derives no `PartialEq`.)
    if shadow_map.size != size {
        shadow_map.size = size;
    }
    let distance = settings.shadow_distance;
    for (mut light, mut config) in &mut lights {
        // `Off` deliberately leaves the cascade config at its LAST value rather than rebuilding it
        // to something degenerate: nothing samples it while casting is disabled, and re-enabling
        // must not depend on a zero-metre envelope having been repaired first. A cascade-count
        // change made while off therefore stays PENDING and lands on this same rebuild the moment a
        // distance exists again — the row is inert while off, not lost.
        if let Some(maximum_distance) = shadow_distance_override().or_else(|| distance.distance_m())
        {
            // The cascade COUNT may vary at runtime ONLY because the vendored bevy_light backport
            // resizes the stale per-thread queues — see `SHADOW_CASCADES` for the crash this line
            // used to be constant to avoid.
            //
            // The split parameters are STATED, not inherited. `..default()` used to cover them, and
            // that silently pinned `first_cascade_far_bound = 10.0` — a character-scale value that
            // halves far-field crispness on a 1 km map (see `SHADOW_FIRST_CASCADE_FAR_BOUND_M`).
            // The overlap is written out at the same value bevy defaults to, so that a builder field
            // NOT listed here is a deliberate "bevy decides", not an oversight. Exactly one such
            // field is left: `minimum_distance` (0.1 m), the near clip, which no row of this page
            // has any business moving.
            let rebuilt = CascadeShadowConfigBuilder {
                num_cascades: settings.shadow_cascades.count(),
                maximum_distance,
                first_cascade_far_bound: SHADOW_FIRST_CASCADE_FAR_BOUND_M,
                overlap_proportion: SHADOW_CASCADE_OVERLAP,
                ..default()
            }
            .build();
            if config.bounds != rebuilt.bounds {
                *config = rebuilt;
            }
        }
        let enabled = distance.casts();
        if light.shadow_maps_enabled != enabled {
            light.shadow_maps_enabled = enabled;
        }
    }

    if let Some(window) = window {
        let mut window = window.into_inner();
        window.present_mode = settings.present_mode(*caps);
        // Guarded: `Window::mode` is also written by `observe_window_mode` (the reflect direction),
        // and both writers keep it inside the two values `WindowModeSetting::to_window_mode` can
        // produce — so equality here means "already agreed" and an unconditional write would only
        // churn change ticks.
        let mode = settings.window_mode.to_window_mode();
        if window.mode != mode {
            window.mode = mode;
        }
    }

    // Guarded by hand — bevy's `UiScale` derives no `PartialEq`.
    let factor = settings.ui_scale.factor();
    if ui_scale.0 != factor {
        ui_scale.0 = factor;
    }

    // `set_if_neq` rather than a plain write: this resource is extracted to the render world every
    // frame, and marking it changed unconditionally would defeat nothing today but is the same
    // discipline the shadow map above needs for a real reason.
    render_scale.set_if_neq(crate::render_scale::RenderScale(
        settings.render_scale.fraction(),
    ));
}

/// Bring a stored vsync rung this surface cannot present down to the one it resolves to — the
/// "observe reality" half of the vsync row, and the third writer in the module doc's table.
///
/// Runs only when [`PresentCaps`] changes, which on a real root is exactly twice: the frame the
/// resource is inserted (still [`PresentCaps::Unprobed`]) and the frame the probe answers. A player
/// editing the row afterwards can only reach offered rungs (`ui::Row::step` walks inside them), so
/// there is nothing left to correct.
///
/// **Only a [`PresentCaps::Reported`] answer can move anything, and that is by construction rather
/// than by a guard here**: `PresentCaps::resolve` is the identity on both unknown states, so the
/// equality below short-circuits on a pre-probe frame AND on a probe that FAILED. That distinction
/// is the whole reason the caps type is a tri-state — this system saves the value it writes, and a
/// transient surface-creation failure must not be allowed to spend a player's stored FAST/OFF on a
/// negative answer nobody actually gave (see [`PresentCaps`]).
///
/// It PERSISTS the correction, which is the deliberate part: everything downstream already reads
/// the effective rung, so leaving the stored one alone would buy nothing but a file that disagrees
/// with every consumer of it, and a page whose value flickers back to the unsupported rung the next
/// time anything re-reads the file. See [`Settings::effective_vsync`] for the trade-off that costs.
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

/// Reflect the OS's ACTUAL fullscreen state back into [`Settings`] and `Window::mode` — the
/// "observe external changes" half of the window-mode row. On macOS the green traffic-light button
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
fn observe_window_mode(
    _non_send_marker: bevy::ecs::system::NonSendMarker,
    window: Option<Single<(Entity, &mut Window), With<PrimaryWindow>>>,
    mut settings: ResMut<Settings>,
    mut save: MessageWriter<SaveSettings>,
    mut last_seen: Local<Option<bool>>,
) {
    let Some(window) = window else {
        return;
    };
    let (entity, mut window) = window.into_inner();
    let Some(fullscreen) = bevy::winit::WINIT_WINDOWS.with_borrow(|winit_windows| {
        winit_windows
            .get_window(entity)
            .map(|winit_window| winit_window.fullscreen().is_some())
    }) else {
        return;
    };
    let previous = last_seen.replace(fullscreen);
    // The first observation is a baseline, not an edge — and a same-state frame is nothing at all.
    if previous.is_none_or(|previous| previous == fullscreen) {
        return;
    }
    let observed = WindowModeSetting::from_fullscreen(fullscreen);
    // Keep `Window::mode` truthful FIRST, whatever the settings say: bevy's change detection can
    // only express "leave fullscreen" later if the component actually says it is fullscreen now.
    let mode = observed.to_window_mode();
    if window.mode != mode {
        window.mode = mode;
    }
    if settings.window_mode != observed {
        settings.window_mode = observed;
        save.write(SaveSettings);
        info!(
            "settings: window mode -> {} (changed outside the settings page)",
            observed.label()
        );
    }
}

/// Persist on request. Reads the settings AFTER [`ApplySettings`] so what is written is what the
/// player is now looking at.
fn save_on_request(mut requests: MessageReader<SaveSettings>, settings: Res<Settings>) {
    // Collapse a frame's requests: several rows changed in one frame is still one file write.
    if requests.read().count() == 0 {
        return;
    }
    store::save(&settings);
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
fn report_store_load(report: Option<Res<StoreReport>>) {
    let Some(report) = report else {
        return;
    };
    // Named in every branch, because "where do I edit this?" is the first question a complaint
    // raises. `None` only when there is no platform config directory at all.
    let path = store::location().map_or_else(
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

    /// The defaults, and WHOSE they are.
    ///
    /// **This test used to assert the shadow rows equalled bevy's own defaults** — the reasoning
    /// being that merely adding a settings module must not change the product. That reasoning
    /// expired on 2026-07-28, when the shadow defaults were retuned off measurement (see
    /// [`SHADOW_CASCADES`], [`SHADOW_FIRST_CASCADE_FAR_BOUND_M`] and [`ShadowDistance`]'s ladder
    /// note); the picture was DELIBERATELY changed, and pinning it to bevy would now assert the
    /// opposite of the intent. So the three shadow rows are pinned to their own measured values, and
    /// each one is stated as a divergence FROM bevy so that a silent drift back is still a failure.
    ///
    /// Everything else still answers to bevy, and still for the original reason.
    #[test]
    fn defaults_reproduce_the_shipped_render() {
        let settings = Settings::default();
        assert_eq!(settings.msaa.to_msaa(), Msaa::default());

        let bevy_cascades = CascadeShadowConfigBuilder::default();
        // Moved 1000.0 -> 350.0 on 2026-07-28: the envelope was measured across the ladder and
        // everything past ~350 m reaches into sky this 17°-sun world cannot shadow, so the far rungs
        // cost frame time and texel density for reach nobody sees (see `ShadowDistance`'s table).
        assert_eq!(
            settings.shadow_distance.distance_m(),
            Some(350.0),
            "the default envelope is this world's shadow horizon — a 17° sun over 100 m of relief \
             self-shadows to ~327 m — not bevy's {} m character-scale default",
            bevy_cascades.maximum_distance,
        );
        assert_eq!(
            settings.shadow_cascades.count(),
            SHADOW_CASCADES,
            "the default cascade row must be the pinned count"
        );
        assert_eq!(SHADOW_CASCADES, 3);
        assert_eq!(
            bevy_cascades.num_cascades, 4,
            "we drop ONE cascade off bevy's default at ~1.16 ms each — if bevy's default moves, \
             this trade has to be re-measured, not silently inherited",
        );
        assert_eq!(
            settings.shadow_resolution.shadow_map_size(),
            4096,
            "4× bevy's {} because 2048 detaches a far shadow by ~1 m at the shipped 350 m envelope, \
             and by a whole tank at the M1000 rung",
            DirectionalLightShadowMap::default().size,
        );
        assert_eq!(
            SHADOW_FIRST_CASCADE_FAR_BOUND_M, 40.0,
            "the split knob that pays for the dropped cascade",
        );
        assert!(
            SHADOW_FIRST_CASCADE_FAR_BOUND_M > bevy_cascades.first_cascade_far_bound,
            "bevy's {} m first bound is a character-scale value; on a 1 km map it spends the whole \
             cascade budget inside the first ten metres",
            bevy_cascades.first_cascade_far_bound,
        );
        assert_eq!(
            SHADOW_CASCADE_OVERLAP, bevy_cascades.overlap_proportion,
            "the overlap is stated, not changed — if bevy moves it, we keep ours",
        );
        assert_eq!(
            settings.vsync,
            VsyncMode::On,
            "the shipped window presents with vsync on"
        );
        assert_eq!(
            settings.render_scale.fraction(),
            1.0,
            "the shipped default renders the 3D pass at native — see `render_scale`'s zero-cost path"
        );
    }

    /// The render-scale row reaches the render app through exactly one door: this reconciler writing
    /// [`crate::render_scale::RenderScale`]. Pinned here because it is the seam a future
    /// "let the UI write the resource directly" shortcut would quietly break — after which the saved
    /// file and the live picture would be configuring two different things.
    #[test]
    fn the_render_scale_row_is_applied_through_the_one_resource() {
        use crate::render_scale::RenderScale;

        let mut app = App::new();
        app.init_resource::<Settings>()
            .init_resource::<DirectionalLightShadowMap>()
            .init_resource::<RenderScale>()
            .init_resource::<PresentCaps>()
            .init_resource::<UiScale>()
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

    /// The ladder is ascending and bottoms out where the brief says it does. `Percent100` must be
    /// the top rung: there is no supersampling row, and `render_scale` treats anything >= 1.0 as the
    /// native no-override path.
    #[test]
    fn the_render_scale_ladder_is_ascending_and_tops_out_at_native() {
        let fractions: Vec<f32> = RenderScaleLevel::ORDER
            .iter()
            .map(|level| level.fraction())
            .collect();
        assert!(
            fractions.windows(2).all(|w| w[0] < w[1]),
            "{fractions:?} must ascend, or the settings page's arrows point the wrong way"
        );
        assert_eq!(fractions.first(), Some(&0.5), "50% is the floor rung");
        assert_eq!(fractions.last(), Some(&1.0), "native is the top rung");
        assert_eq!(
            RenderScaleLevel::ORDER.last(),
            Some(&RenderScaleLevel::default()),
            "the default must be the top rung — a player who never opens the page gets native"
        );
    }

    /// `Off` is exactly one rung, it is the FLOOR of the distance ladder, and it is the only thing
    /// that stops casting. Pins the shape the suspended competitive-integrity rule (see
    /// [`ShadowDistance`]) would have to be re-argued against: an off state hidden anywhere else —
    /// a zero-metre distance, a zero-texel resolution — would be the same product change made
    /// silently.
    #[test]
    fn off_is_the_only_rung_that_stops_casting() {
        let off: Vec<ShadowDistance> = ShadowDistance::ORDER
            .into_iter()
            .filter(|distance| !distance.casts())
            .collect();
        assert_eq!(
            off,
            vec![ShadowDistance::Off],
            "exactly one rung may disable shadows, and it is the named one"
        );
        assert_eq!(
            ShadowDistance::ORDER.first(),
            Some(&ShadowDistance::Off),
            "off is the floor — the right arrow must always mean more shadow"
        );
        for distance in ShadowDistance::ORDER {
            assert_eq!(
                distance.casts(),
                distance.distance_m().is_some(),
                "{distance:?}: casting and having a distance are the same fact",
            );
        }
        for resolution in ShadowResolution::ORDER {
            assert!(
                resolution.shadow_map_size() > 0,
                "{resolution:?}: the resolution row has no off rung — that is the distance row's job",
            );
        }
    }

    /// **The 2026-07-28 ladder migration**, at the enum level (the same thing through a whole
    /// `video.ron` is `store`'s `the_retired_shadow_distance_rungs_load_and_migrate`).
    ///
    /// Two claims. First: the three retired tokens still DESERIALIZE — `M100`/`M150`/`M300` are
    /// `#[serde(alias)]`es on [`ShadowDistance::M350`], so no player's stored file becomes a parse
    /// error. Second, and the reason migrating beat hiding them: whatever they land on must be ON
    /// the ladder, because `ui::step_in` resolves a current value missing from `ORDER` to index 0 —
    /// which is `Off`. An off-ladder rung would therefore turn the next press of the RIGHT arrow
    /// into "shadows off", the one transition the page must never make by accident.
    ///
    /// The named list is the strongest guard available without `variant_count`: a fifth variant
    /// added later and left out of `ORDER` would not fail here, so `ORDER`'s own doc carries the
    /// rule in words as well.
    #[test]
    fn the_retired_distance_rungs_migrate_onto_the_ladder() {
        assert_eq!(
            ShadowDistance::ORDER,
            [
                ShadowDistance::Off,
                ShadowDistance::M350,
                ShadowDistance::M700,
                ShadowDistance::M1000,
            ],
            "every representable rung must be on the ladder — see the test doc",
        );
        for retired in ["M100", "M150", "M300"] {
            let parsed: ShadowDistance = ron::de::from_str(retired)
                .unwrap_or_else(|error| panic!("{retired} must still deserialize: {error}"));
            assert_eq!(
                parsed,
                ShadowDistance::M350,
                "{retired} must migrate to the nearest surviving rung",
            );
            assert!(
                ShadowDistance::ORDER.contains(&parsed),
                "{retired} migrated to an off-ladder value, which `step_in` would resolve to Off",
            );
        }
        // The migration is one-way by construction: the aliases are read-only names, so nothing can
        // write a retired token back out.
        assert_eq!(
            ron::ser::to_string(&ShadowDistance::M350).expect("serializes"),
            "M350",
        );
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
        // `Off` carries no distance, so the ascent is checked over the rungs that have one — with
        // `Off` already pinned as the floor by `off_is_the_only_rung_that_stops_casting`.
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

    /// **The field-crash regression** (Yan, 2026-07-26: an `index out of bounds: the len is 2 but
    /// the index is 2` panic inside `bevy_light`'s `check_dir_light_mesh_visibility`, one shadow-knob
    /// press after the ladder wrapped from a 2-cascade setting to the 4-cascade default).
    ///
    /// The cascade count is a LIVE ROW now (safe only under the vendored bevy_light backport — see
    /// [`SHADOW_CASCADES`]), so the invariant this test pins moved: after every step of the WHOLE
    /// distance × resolution × cascades grid (twice, so every cell is entered from another one,
    /// off/on edges included), the `CascadeShadowConfig`'s length must equal the LAST APPLIED
    /// cascade row — moved by that row alone, never by the distance or resolution rows. That length
    /// sizes every per-cascade array downstream (the frusta, the per-view visible-entity vectors,
    /// and the pooled thread-local queues that blew up in the field), so "only the row moves it,
    /// deliberately, through `apply_settings`" is the property that keeps the change auditable.
    ///
    /// A count changed WHILE OFF stays pending (the off branch touches nothing) and lands on the
    /// next casting frame — the expected-length tracking below encodes exactly that.
    ///
    /// It also pins the builder parameters the count rebuild must NOT move. Those used to be
    /// "whatever `..default()` gave us"; two of the three are STATED constants now
    /// ([`SHADOW_FIRST_CASCADE_FAR_BOUND_M`], [`SHADOW_CASCADE_OVERLAP`]), so the assertion below
    /// checks them against OUR values rather than bevy's — the first bound in particular is
    /// deliberately not bevy's, and reading it out of the built config in every cell is what proves
    /// the builder is actually carrying it. `minimum_distance` is the one field still left to bevy
    /// and is still checked against bevy's reference.
    ///
    /// What is deliberately NOT tested: the (now vendored-away) bevy panic itself was
    /// scheduling-dependent (it needs a pooled thread-local that missed a frame's init), so a test
    /// that tried to reproduce it could pass with the bug fully live — worse than no test. The
    /// backport's own validation lives with the vendor entry
    /// (`.agents/docs/upstream/bevy-cascade-count-stale-local-parallel.md`).
    #[test]
    fn the_cascade_count_follows_its_row_and_only_its_row() {
        let mut app = App::new();
        app.init_resource::<Settings>()
            .init_resource::<DirectionalLightShadowMap>()
            // Not optional in the reconciler any more — `plugin` mounts `render_scale::plugin`, so
            // the only path without it is a bare-`App` test like this one.
            .init_resource::<crate::render_scale::RenderScale>()
            .init_resource::<PresentCaps>()
            .init_resource::<UiScale>()
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
    }

    /// **A persisted rung this surface cannot present.**
    ///
    /// A `video.ron` carrying `vsync_mode: Fast` opened on a Metal surface used to leave the page
    /// showing FAST while the present mode negotiated away to something that tears exactly like
    /// OFF. Now the rung RESOLVES (to ON), every consumer reads the resolved value, and
    /// [`normalize_vsync`] writes it back — the page, the present mode and the frame-cap gate are
    /// one fact rather than three readings of a dead one.
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
    /// supported rung — and the whole pre-probe state — is left alone.
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

    /// **A probe FAILURE is not a negative answer.**
    ///
    /// `probe` used to report a surface it could not even create as "probed, capability list
    /// empty". Normalisation is a writer that SAVES, so that fabricated negative would have spent
    /// a player's perfectly valid stored FAST/OFF — permanently, on a machine whose surface
    /// supports it — the first time a probe surface failed to be created. [`PresentCaps`] is a
    /// tri-state so that cannot be expressed: this pins that the could-not-ask state moves nothing
    /// and writes nothing, while leaving the safe presentation fallback exactly as it was.
    #[test]
    fn a_failed_probe_never_rewrites_the_stored_rung() {
        for vsync in VsyncMode::ORDER {
            let mut app = App::new();
            app.add_message::<SaveSettings>()
                .insert_resource(Settings {
                    vsync,
                    frame_cap: FrameCap(120),
                    ..default()
                })
                .insert_resource(PresentCaps::Unavailable)
                .add_systems(Update, normalize_vsync);
            app.update();
            let settings = *app.world().resource::<Settings>();
            assert_eq!(
                settings.vsync, vsync,
                "a probe that could not ask must not rewrite {vsync:?}",
            );
            assert_eq!(
                app.world()
                    .resource::<bevy::ecs::message::Messages<SaveSettings>>()
                    .len(),
                0,
                "a probe that could not ask must not write the file ({vsync:?})",
            );
            // The runtime behaviour that failure state is allowed to have: presentation still
            // negotiates rather than gambling, and the stored rung still gates the frame cap.
            assert_eq!(settings.effective_vsync(PresentCaps::Unavailable), vsync);
            assert_ne!(
                settings.present_mode(PresentCaps::Unavailable),
                PresentMode::Mailbox,
            );
            assert_ne!(
                settings.present_mode(PresentCaps::Unavailable),
                PresentMode::Immediate,
            );
            assert_eq!(
                settings
                    .frame_limit_period(PresentCaps::Unavailable)
                    .is_some(),
                vsync == VsyncMode::Off,
                "an unknown surface leaves the cap on the player's own rung ({vsync:?})",
            );
        }
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

    /// The frame-cap ladder: OFF is the floor, the rungs ascend 30..=240 by 10, stepping saturates,
    /// the slider fraction round-trips every stop, and a hand-edited off-ladder value is honoured
    /// until touched (then snaps to its nearest stop).
    #[test]
    fn the_frame_cap_ladder_is_sane() {
        assert_eq!(FrameCap::OFF.fps(), None);
        assert_eq!(FrameCap::OFF.label(), "OFF");
        assert_eq!(FrameCap::OFF.step(-1), FrameCap::OFF, "the floor saturates");
        assert_eq!(FrameCap::OFF.step(1).fps(), Some(30), "OFF steps up to MIN");
        assert_eq!(
            FrameCap(30).step(-1),
            FrameCap::OFF,
            "MIN steps down to OFF"
        );
        assert_eq!(FrameCap(240).step(1).fps(), Some(240), "the top saturates");
        // Every stop survives the slider's fraction round-trip.
        let mut cap = FrameCap::OFF;
        loop {
            assert_eq!(FrameCap::from_fraction(cap.fraction()), cap, "{cap:?}");
            let next = cap.step(1);
            if next == cap {
                break;
            }
            cap = next;
        }
        assert_eq!(cap.fps(), Some(240), "the walk ends at the ceiling");
        // Hand-edited values: honoured as written, clamped only at the absurd ends, snapped to the
        // ladder on first touch.
        assert_eq!(FrameCap(144).fps(), Some(144));
        assert_eq!(FrameCap(144).label(), "144 FPS");
        assert_eq!(FrameCap(144).step(1).fps(), Some(150));
        assert_eq!(FrameCap(5).fps(), Some(30));
        assert_eq!(FrameCap(9999).fps(), Some(240));
    }

    /// The UI-scale ladder: 75..=150 by 5, default 100 (a no-op multiplier), saturating steps,
    /// slider round-trip, and the factor clamp that keeps a hand-edited file from making the page
    /// itself unreachable.
    #[test]
    fn the_ui_scale_ladder_is_sane() {
        assert_eq!(UiScalePercent::default().factor(), 1.0);
        assert_eq!(UiScalePercent::default().label(), "100%");
        assert_eq!(UiScalePercent(75).step(-1), UiScalePercent(75));
        assert_eq!(UiScalePercent(150).step(1), UiScalePercent(150));
        assert_eq!(UiScalePercent(100).step(1), UiScalePercent(105));
        let mut scale = UiScalePercent(UiScalePercent::MIN);
        loop {
            assert_eq!(
                UiScalePercent::from_fraction(scale.fraction()),
                scale,
                "{scale:?}"
            );
            let next = scale.step(1);
            if next == scale {
                break;
            }
            scale = next;
        }
        assert_eq!(scale, UiScalePercent(UiScalePercent::MAX));
        assert_eq!(UiScalePercent(500).factor(), 1.5, "absurd values clamp");
        assert_eq!(UiScalePercent(10).factor(), 0.75);
    }

    /// The window-mode mapping: borderless-only (exclusive fullscreen is unrepresentable — it is
    /// the mode that can panic on an unresolvable monitor), always the CURRENT monitor, and the
    /// observe direction inverts it.
    #[test]
    fn window_mode_is_borderless_current_only() {
        assert_eq!(
            WindowModeSetting::Windowed.to_window_mode(),
            WindowMode::Windowed,
        );
        assert_eq!(
            WindowModeSetting::Fullscreen.to_window_mode(),
            WindowMode::BorderlessFullscreen(MonitorSelection::Current),
        );
        for setting in WindowModeSetting::ORDER {
            let fullscreen = setting == WindowModeSetting::Fullscreen;
            assert_eq!(WindowModeSetting::from_fullscreen(fullscreen), setting);
        }
        assert_eq!(
            WindowModeSetting::default(),
            WindowModeSetting::Windowed,
            "a player who never opens the page gets the window the game always opened with"
        );
    }

    /// **What lets `is_default` be ONE generic function.** Every skipped field's `Settings::default`
    /// value must equal its own type's `Default`, or `skip_serializing_if = "is_default"` would
    /// either freeze a default into every player's file or omit a deliberate choice from it.
    ///
    /// This used to be false — `vsync: bool` defaulted to `true` while `bool::default()` is `false`,
    /// which is exactly why there were five hand-written predicates instead of one. [`VsyncMode`]
    /// retired the exception; this test is what keeps the next field from re-introducing it.
    /// `version` is deliberately NOT covered: it is the one field with no `skip_serializing_if`, and
    /// its default is [`SETTINGS_VERSION`] rather than `u32::default()` on purpose.
    #[test]
    fn settings_defaults_are_their_types_defaults() {
        let settings = Settings::default();
        assert!(is_default(&settings.shadow_distance));
        assert!(is_default(&settings.shadow_resolution));
        assert!(is_default(&settings.shadow_cascades));
        assert!(is_default(&settings.msaa));
        assert!(is_default(&settings.vsync));
        assert!(is_default(&settings.render_scale));
        assert!(is_default(&settings.window_mode));
        assert!(is_default(&settings.frame_cap));
        assert!(is_default(&settings.ui_scale));
        assert!(
            settings.legacy_vsync.is_none(),
            "the legacy shadow field must default absent — it exists only inside a parse"
        );
        assert_ne!(
            settings.version, 0,
            "the version stamp is the one field exempt from the sparse-write rule"
        );
    }

    /// **The vsync field migration.** The ladder grew a third rung, which a `bool` cannot carry, so
    /// the persisted key MOVED from `vsync` (bool) to `vsync_mode` (enum). This pins all three
    /// sides of that story:
    ///
    /// * the new build never writes the old key (an older build reading a new file skips the
    ///   unknown `vsync_mode` and lands on the safe default ON — the documented dropped-key cost);
    /// * a v1 `vsync: false` on disk is still honoured through [`Settings::absorb_legacy_vsync`];
    /// * a hand-edited file carrying BOTH keys resolves by "the more specific statement wins": a
    ///   non-default `vsync_mode` beats the legacy bool.
    #[test]
    fn vsync_moved_to_a_new_key_and_the_old_bool_is_absorbed() {
        let written = ron::ser::to_string(&Settings {
            vsync: VsyncMode::Off,
            ..default()
        })
        .expect("settings serialize");
        assert!(
            written.contains("vsync_mode:Off"),
            "the off rung must write the NEW key: {written}",
        );
        assert!(
            !written.contains("vsync:"),
            "the retired bool key must never be written again: {written}",
        );

        let absorb = |settings: Settings| settings.absorb_legacy_vsync();
        assert_eq!(
            absorb(Settings {
                legacy_vsync: Some(false),
                ..default()
            })
            .vsync,
            VsyncMode::Off,
            "a v1 `vsync: false` still means OFF",
        );
        assert_eq!(
            absorb(Settings {
                legacy_vsync: Some(true),
                ..default()
            })
            .vsync,
            VsyncMode::On,
        );
        assert_eq!(
            absorb(Settings {
                legacy_vsync: Some(false),
                vsync: VsyncMode::Fast,
                ..default()
            })
            .vsync,
            VsyncMode::Fast,
            "an explicit non-default vsync_mode beats the legacy bool",
        );
        let absorbed = absorb(Settings {
            legacy_vsync: Some(false),
            ..default()
        });
        assert!(
            absorbed.legacy_vsync.is_none(),
            "absorption must consume the legacy value — it never escapes the parse seam",
        );
        assert_eq!(
            VsyncMode::ORDER,
            [VsyncMode::Off, VsyncMode::Fast, VsyncMode::On],
            "the right arrow must move toward more waiting per frame, like every other ladder",
        );
    }
}
