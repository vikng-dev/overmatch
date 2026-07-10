//! The bundled UI typeface — Barlow Condensed, loaded once and handed to every `Text` the client
//! spawns. Retires Bevy's ASCII-only default font (a FiraMono subset with no fallback, which draws
//! tofu for any non-ASCII glyph — the reason for the `tests/ui_ascii.rs` stopgap). Barlow Condensed
//! covers the full ASCII range plus the typographic set the UI actually uses (… — – ° × ± ≤), all
//! verified against the shipped `.ttf` cmaps; anything past that still needs a fresh cmap check
//! (see the rule in `.agents/AGENTS.md` and `tests/ui_ascii.rs`).
//!
//! Two weights ship: SemiBold for overlay/all-caps/identity text, Regular for the smaller, denser
//! numeric readouts. Both files live under `assets/fonts/` alongside their SIL OFL license
//! (`OFL.txt`), which the license requires to travel with the fonts — the release packaging copies
//! `assets/` wholesale, so it rides along automatically.
//!
//! Handles are resolved at **plugin-build time** (not in a `Startup` system): `AssetServer::load`
//! returns a handle synchronously, and inserting [`UiFonts`] during `build` guarantees it exists
//! before any `Startup` UI-spawn system reads it, so no spawn can race a not-yet-inserted resource.
//! Mounted by every composition root that spawns UI — [`crate::ClientPlugin`],
//! [`crate::NetClientPlugin`], and the armor sandbox — so the shared HUD plugins (`hud`, `crew_ui`,
//! …) always find the resource. The headless server and scripted harness mount no UI, so they never
//! mount this.

use bevy::prelude::*;

/// The two bundled Barlow Condensed weights, as ready-to-clone `Handle<Font>`s. Cheap to `clone`
/// (a handle is refcounted), so each `TextFont` site clones the weight it wants.
#[derive(Resource, Clone)]
pub(crate) struct UiFonts {
    /// SemiBold — HUD overlays, all-caps banners, big prompts, identity chips.
    pub hud: Handle<Font>,
    /// Regular — the smaller, denser numeric readouts (HP labels, metric rows, reticle numbers).
    pub body: Handle<Font>,
}

/// Resolve both font handles from the already-present `AssetServer` and insert [`UiFonts`] before
/// any `Startup` system runs. Requires `AssetPlugin` (part of `DefaultPlugins`) to have been added
/// first — every composition root that mounts this does so after `DefaultPlugins`.
pub(crate) fn plugin(app: &mut App) {
    let asset_server = app.world().resource::<AssetServer>();
    let fonts = UiFonts {
        hud: asset_server.load("fonts/BarlowCondensed-SemiBold.ttf"),
        body: asset_server.load("fonts/BarlowCondensed-Regular.ttf"),
    };
    app.insert_resource(fonts);
}
