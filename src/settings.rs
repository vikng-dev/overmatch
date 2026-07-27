//! Player-facing graphics settings: the persisted model, its on-disk home, and the ONE place that
//! applies it to the rendering world.
//!
//! **Not `dev_tools`-gated** — this ships, and it is the ONLY thing that configures the renderer.
//!
//! # One writer, on purpose
//!
//! | Writer | Writes [`Settings`] | Writes the file |
//! |---|---|---|
//! | The settings page (`settings::ui`) | yes | **yes** — every change saves |
//!
//! That table used to have three rows: a `dev_tools` perf panel carried function-key knobs that
//! wrote this same resource without persisting, plus a dev-only "shadows fully off" resource the
//! player-facing ladder could not express. It was DELETED 2026-07-27 (Yan) as superseded — the
//! settings page reaches everything it reached, `cargo tracy` (`.cargo/config.toml`) is the frame-cost
//! instrument, and shadows off is now a player-facing rung ([`ShadowDistance::Off`]). What the
//! deletion buys is that a value can only be in this resource because the player put it there.
//!
//! The saving is still deliberately NOT "save whatever `Settings` holds when the app exits": only an
//! explicit [`SaveSettings`] writes the file, so a value that arrived any other way can never be
//! laundered into the player's config.
//!
//! # Forward compatibility, and the one rule for changing this file
//!
//! Every field is `#[serde(default)]` and serde ignores unknown fields, so **adding a setting needs
//! no migration in either direction**: an old file missing the new key loads it at its default, and
//! an old build reading a newer file skips the key it doesn't know. `render_scale` was the first
//! exercise of that and cost exactly one field; the entries still queued behind the display research
//! (window mode, UI scale, frame cap) slot in the same way — a `Default`, a `skip_serializing_if`,
//! and a row in `ui::Row::ORDER`; nothing else moves.
//!
//! **The policy, stated once so it is not re-litigated:**
//!
//! * ADD or REMOVE a field — free. Do nothing.
//! * RENAME a field — add `#[serde(alias = "old_name")]`. That is the ENTIRE rename story (it is
//!   what rustup does), and it **never bumps [`SETTINGS_VERSION`]**.
//! * Change a field's TYPE while keeping its on-disk shape — free, via `#[serde(from/into)]`. That
//!   is how `vsync` became the two-rung [`VsyncMode`] ladder while still reading and writing the
//!   `bool` v1 shipped.
//! * Change a field's MEANING under the same name — the only thing serde cannot express, and the
//!   only thing that bumps the version.
//!
//! Files from the future are refused rather than guessed at, and moved aside so a downgrade cannot
//! silently overwrite them. See `store` for the mechanism and
//! `.agents/docs/design/display/persistence-brief-2026-07.md` for the evidence behind all of it.

use bevy::light::{CascadeShadowConfig, CascadeShadowConfigBuilder, DirectionalLightShadowMap};
use bevy::prelude::*;
use bevy::render::view::Msaa;
use bevy::window::{PresentMode, PrimaryWindow};
use serde::{Deserialize, Serialize};

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

/// The cascade count every shadow setting uses — **constant, and it must stay that way.**
/// This is bevy's own default (`CascadeShadowConfigBuilder::num_cascades`), so the shipped look at
/// [`ShadowDistance::M150`] is bit-identical to what the game rendered before this module existed.
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
/// The rule this leaves: **`maximum_distance` and the shadow-map SIZE are safe to change at runtime
/// (no per-cascade array length moves); the cascade COUNT is not.** Cascade count is therefore a
/// compile-time constant, not a setting and not a row.
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
pub(crate) const SHADOW_CASCADES: usize = 4;

/// How far directional shadows are drawn before they stop — the far bound of the last cascade, given
/// the cascade COUNT is pinned by [`SHADOW_CASCADES`].
///
/// **Split from [`ShadowResolution`] on purpose** (Yan, 2026-07-27). Distance and map resolution are
/// independent costs: distance widens the area each cascade must cover (cheap to raise on a machine
/// with fill rate to spare, and what decides whether a distant treeline shades anything at all),
/// resolution buys crispness at a fixed area. Pairing them into coupled Low/Medium/High presets — as
/// this type used to — meant a player who wanted the far envelope had to pay for 4K maps too, and it
/// hid which of the two a frame-rate complaint was actually about.
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
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default, Serialize, Deserialize)]
pub(crate) enum ShadowDistance {
    /// Nothing casts: [`apply_settings`] clears `DirectionalLight::shadow_maps_enabled`. This is the
    /// whole shadow budget in one A/B, and the rung whose future is undecided (see the type doc).
    Off,
    M100,
    /// **The shipped default** — bevy's own `CascadeShadowConfigBuilder::maximum_distance`, i.e. the
    /// envelope the game rendered before this module existed.
    #[default]
    M150,
    M300,
}

impl ShadowDistance {
    /// The far bound of the last cascade, metres — `None` for [`ShadowDistance::Off`], which is not
    /// a distance at all. Returning an `Option` rather than `0.0` is what keeps a caller from
    /// building a degenerate zero-metre cascade config and calling it "off".
    pub(crate) const fn distance_m(self) -> Option<f32> {
        match self {
            ShadowDistance::Off => None,
            ShadowDistance::M100 => Some(100.0),
            ShadowDistance::M150 => Some(150.0),
            ShadowDistance::M300 => Some(300.0),
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
            ShadowDistance::M100 => "100 M",
            ShadowDistance::M150 => "150 M",
            ShadowDistance::M300 => "300 M",
        }
    }

    /// Ascending in cost, so the settings page's right arrow always moves toward more shadow.
    pub(crate) const ORDER: [ShadowDistance; 4] = [
        ShadowDistance::Off,
        ShadowDistance::M100,
        ShadowDistance::M150,
        ShadowDistance::M300,
    ];
}

/// Edge length of each cascade's shadow map — how CRISP a shadow is, independent of how far it is
/// drawn (see [`ShadowDistance`] for why the two are separate rows).
///
/// Deliberately has no `Off`: switching shadows off is [`ShadowDistance::Off`]'s job, and two ways to
/// express the same state is how a UI ends up with rows that contradict each other.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default, Serialize, Deserialize)]
pub(crate) enum ShadowResolution {
    X1024,
    /// **The shipped default** — bevy's own `DirectionalLightShadowMap::default().size`.
    #[default]
    X2048,
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

/// Whether frames are locked to the display refresh. A two-rung LADDER rather than a `bool`, so it
/// is shaped like every other row on the page: it has a [`VsyncMode::ORDER`] the arrows walk with
/// the shared `step_in`, a [`VsyncMode::label`] the row renders, and a `Default` the sparse-write
/// test reads — none of which a `bool` can carry, and each of which was a special case in the page
/// before this type existed.
///
/// # It is stored on disk as a BOOL, deliberately
///
/// The field shipped as `vsync: bool` at [`SETTINGS_VERSION`] 1, and `video.ron` files carrying
/// `vsync: false` are on players' disks now. A type mismatch is not a tolerated unknown key — RON
/// fails the whole parse, so the file would be reported CORRUPT and every OTHER setting in it would
/// reset too. `#[serde(from/into)]` keeps the on-disk shape byte-identical in both directions (an
/// older build can still read a file this one wrote), which is strictly cheaper than a migration and
/// is why [`SETTINGS_VERSION`] does not move.
///
/// **The consequence, stated so it is not discovered later:** a THIRD rung cannot be added under
/// this field name — `bool` has exactly two values. That is the module doc's "change a field's
/// MEANING under the same name" case, so it would take a new field (free) or a version bump.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default, Serialize, Deserialize)]
#[serde(from = "bool", into = "bool")]
pub(crate) enum VsyncMode {
    /// [`PresentMode::AutoNoVsync`] — can tear, lower input lag.
    Off,
    /// **The shipped default** — [`PresentMode::Fifo`]. See [`Settings::present_mode`] for why those
    /// two modes exactly.
    #[default]
    On,
}

impl From<bool> for VsyncMode {
    fn from(on: bool) -> Self {
        if on { VsyncMode::On } else { VsyncMode::Off }
    }
}

impl From<VsyncMode> for bool {
    fn from(mode: VsyncMode) -> Self {
        matches!(mode, VsyncMode::On)
    }
}

impl VsyncMode {
    /// ASCII only — it reaches `Text`.
    pub(crate) const fn label(self) -> &'static str {
        match self {
            VsyncMode::Off => "OFF",
            VsyncMode::On => "ON",
        }
    }

    /// Ascending in cost, like every other ladder here — the right arrow moves toward more work per
    /// frame.
    pub(crate) const ORDER: [VsyncMode; 2] = [VsyncMode::Off, VsyncMode::On];
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
    #[serde(skip_serializing_if = "is_default")]
    pub(crate) msaa: MsaaLevel,
    /// Stored on disk as the `bool` it shipped as — see [`VsyncMode`].
    #[serde(skip_serializing_if = "is_default")]
    pub(crate) vsync: VsyncMode,
    /// The fraction of the window the 3D pass renders at — applied by writing
    /// [`crate::render_scale::RenderScale`], which the render app extracts.
    #[serde(skip_serializing_if = "is_default")]
    pub(crate) render_scale: RenderScaleLevel,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            version: SETTINGS_VERSION,
            shadow_distance: ShadowDistance::default(),
            shadow_resolution: ShadowResolution::default(),
            msaa: MsaaLevel::default(),
            vsync: VsyncMode::default(),
            render_scale: RenderScaleLevel::default(),
        }
    }
}

impl Settings {
    /// The present mode this configuration asks for. Split out so the window can be BUILT with it
    /// (before the surface is ever configured) and so [`apply_settings`] reuses the one mapping.
    ///
    /// **Exactly two modes are reachable, and that is a safety property, not a simplification.**
    /// On Metal, wgpu-hal's `Mailbox` and `FifoRelaxed` reach an `unreachable!()` — they **PANIC**,
    /// with no fallback (wgpu-hal metal `adapter.rs`, `surface.rs`). `AutoVsync` is also avoided
    /// even though it looks safe: bevy documents it as *FifoRelaxed → Fifo*, i.e. it names a
    /// panicking mode as its first choice and only avoids it by capability negotiation. `Fifo` is
    /// unconditional, universally supported, and traditionally exactly what "VSync On" means.
    ///
    /// Keeping this a two-rung [`VsyncMode`] rather than a `PresentMode` field is what makes the
    /// dangerous modes **unrepresentable** — neither a config file nor a UI row can ask for one, and
    /// the exhaustive match below is where that property is now legible.
    pub(crate) const fn present_mode(self) -> PresentMode {
        match self.vsync {
            VsyncMode::On => PresentMode::Fifo,
            VsyncMode::Off => PresentMode::AutoNoVsync,
        }
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
/// bevy panics on a `Fullscreen` whose monitor cannot be resolved, so a config written with a second
/// display attached can brick boot once it is unplugged. A player in that state cannot reach the
/// settings page to undo it, so the escape must live outside the app's own UI. (Nothing in
/// [`Settings`] can cause that TODAY; the flag lands with the foundation because the window-mode row
/// that can is the next slice, and an escape hatch added after the trap is an escape hatch that
/// arrives too late.)
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
        app.add_plugins((crate::render_scale::plugin, ui::plugin))
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
                    apply_settings
                        .in_set(ApplySettings)
                        .run_if(resource_changed::<Settings>),
                    save_on_request.after(ApplySettings),
                ),
            );
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
) {
    let msaa = settings.msaa.to_msaa();
    for mut camera_msaa in &mut cameras {
        camera_msaa.set_if_neq(msaa);
    }

    // The two shadow rows are reconciled independently, because they are independent settings: the
    // resolution applies whatever the distance says, and the distance's `Off` touches only the cast
    // switch.
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
        // must not depend on a zero-metre envelope having been repaired first. It also keeps the
        // cascade array length — the one thing that must never move mid-run (see `SHADOW_CASCADES`)
        // — untouched across the off/on edge.
        if let Some(maximum_distance) = distance.distance_m() {
            // The cascade COUNT never varies — see `SHADOW_CASCADES`.
            let rebuilt = CascadeShadowConfigBuilder {
                num_cascades: SHADOW_CASCADES,
                maximum_distance,
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
        window.into_inner().present_mode = settings.present_mode();
    }

    // `set_if_neq` rather than a plain write: this resource is extracted to the render world every
    // frame, and marking it changed unconditionally would defeat nothing today but is the same
    // discipline the shadow map above needs for a real reason.
    render_scale.set_if_neq(crate::render_scale::RenderScale(
        settings.render_scale.fraction(),
    ));
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

    /// The defaults must reproduce the picture the game shipped BEFORE settings existed, or merely
    /// adding this module changed the product. Bevy's own defaults are the reference.
    #[test]
    fn defaults_reproduce_the_shipped_render() {
        let settings = Settings::default();
        assert_eq!(settings.msaa.to_msaa(), Msaa::default());
        let bevy_cascades = CascadeShadowConfigBuilder::default();
        assert_eq!(
            settings.shadow_distance.distance_m(),
            Some(bevy_cascades.maximum_distance),
        );
        assert_eq!(SHADOW_CASCADES, bevy_cascades.num_cascades);
        assert_eq!(
            settings.shadow_resolution.shadow_map_size(),
            DirectionalLightShadowMap::default().size,
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
    /// [`apply_settings`] is driven across the WHOLE distance × resolution grid twice — so every
    /// combination is entered from another one, INCLUDING the off/on edges the split ladders made
    /// newly reachable — and after each step the `CascadeShadowConfig` must still carry exactly
    /// [`SHADOW_CASCADES`] bounds. That length sizes every per-cascade array downstream (the frusta,
    /// the per-view visible-entity vectors, and the pooled thread-local queues that actually blew
    /// up), so holding it invariant is what makes the crash unreachable.
    ///
    /// It also pins the two properties the split introduced: the rows are INDEPENDENT (the map is
    /// resized whatever the distance says, including while off), and re-enabling after `Off`
    /// restores casting — the failure mode where "off" is a one-way door.
    ///
    /// What is deliberately NOT tested: the bevy panic itself is scheduling-dependent (it needs a
    /// pooled thread-local that missed a frame's init), so a test that tried to reproduce it could
    /// pass with the bug fully live — worse than no test. This pins the deterministic input
    /// condition the bug requires instead.
    #[test]
    fn applying_any_shadow_combination_never_changes_the_cascade_count() {
        let mut app = App::new();
        app.init_resource::<Settings>()
            .init_resource::<DirectionalLightShadowMap>()
            // Not optional in the reconciler any more — `plugin` mounts `render_scale::plugin`, so
            // the only path without it is a bare-`App` test like this one.
            .init_resource::<crate::render_scale::RenderScale>()
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
                    .map(move |resolution| (distance, resolution))
            })
            .collect::<Vec<_>>();
        // Two full laps, so every cell is also reached from the last cell of the previous lap.
        for (distance, resolution) in grid.iter().copied().chain(grid.iter().copied()) {
            {
                let mut settings = app.world_mut().resource_mut::<Settings>();
                settings.shadow_distance = distance;
                settings.shadow_resolution = resolution;
            }
            app.update();
            let config = app
                .world()
                .get::<CascadeShadowConfig>(sun)
                .expect("sun carries a cascade config");
            assert_eq!(
                config.bounds.len(),
                SHADOW_CASCADES,
                "{distance:?}/{resolution:?} changed the cascade count — that is the bevy_light crash",
            );
            if let Some(want) = distance.distance_m() {
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

    /// **The Metal panic guard.** `Mailbox` and `FifoRelaxed` hit an `unreachable!()` in wgpu-hal's
    /// Metal backend — they PANIC, no fallback — and `AutoVsync` names `FifoRelaxed` as its first
    /// choice. Only `Fifo` and `AutoNoVsync` may ever be produced here.
    ///
    /// Now driven off [`VsyncMode::ORDER`] rather than `[true, false]`, so a rung added to the
    /// ladder is covered by this guard the moment it exists.
    #[test]
    fn only_the_metal_safe_present_modes_are_reachable() {
        for vsync in VsyncMode::ORDER {
            let mode = Settings { vsync, ..default() }.present_mode();
            assert!(
                matches!(mode, PresentMode::Fifo | PresentMode::AutoNoVsync),
                "vsync={vsync:?} produced {mode:?}, which can panic on Metal",
            );
        }
        assert_eq!(
            Settings {
                vsync: VsyncMode::On,
                ..default()
            }
            .present_mode(),
            PresentMode::Fifo,
            "VSync ON is Fifo — NOT AutoVsync, whose first choice is the panicking FifoRelaxed",
        );
        assert_eq!(
            Settings {
                vsync: VsyncMode::Off,
                ..default()
            }
            .present_mode(),
            PresentMode::AutoNoVsync,
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
        assert!(is_default(&settings.msaa));
        assert!(is_default(&settings.vsync));
        assert!(is_default(&settings.render_scale));
        assert_ne!(
            settings.version, 0,
            "the version stamp is the one field exempt from the sparse-write rule"
        );
    }

    /// **The v1 on-disk compatibility of the VSync ladder.** [`VsyncMode`] is a two-rung enum in
    /// memory and a plain `bool` on disk, because that is what shipped — and a RON type mismatch
    /// fails the WHOLE parse, so getting this wrong would reset every other setting in the file too.
    /// Both directions are pinned: this build reads what v1 wrote, and writes what a v1 build could
    /// still read.
    #[test]
    fn vsync_is_a_ladder_in_memory_and_a_bool_on_disk() {
        assert_eq!(VsyncMode::from(false), VsyncMode::Off);
        assert_eq!(VsyncMode::from(true), VsyncMode::On);
        assert!(!bool::from(VsyncMode::Off));
        assert!(bool::from(VsyncMode::On));
        let written = ron::ser::to_string(&Settings {
            vsync: VsyncMode::Off,
            ..default()
        })
        .expect("settings serialize");
        assert!(
            written.contains("vsync:false") || written.contains("vsync: false"),
            "the off rung must still write the v1 bool an older build can read: {written}",
        );
        assert_eq!(
            VsyncMode::ORDER,
            [VsyncMode::Off, VsyncMode::On],
            "the right arrow must move toward more work per frame, like every other ladder",
        );
    }
}
