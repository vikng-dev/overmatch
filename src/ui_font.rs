//! Bundled Barlow Condensed UI fonts, and the handful of style constants every client surface draws
//! with.
//!
//! Handles are inserted during plugin setup so every `Startup` UI spawner can read [`UiFonts`]. UI
//! strings must remain within the shipped font coverage enforced by `src/tests/ui_ascii.rs`.
//!
//! The colours below live here for the same reason [`OVERLAY_FONT_PX`] does: they are the family
//! standard, and they had already been hand-copied into three files. Layout is deliberately NOT
//! unified — each HUD card has its own anti-jitter column shape, and collapsing those would trade a
//! real property for a cosmetic one.

use bevy::prelude::*;

/// The two bundled Barlow Condensed weights, as ready-to-clone `Handle<Font>`s. Cheap to `clone`
/// (a handle is refcounted), so each `TextFont` site clones the weight it wants.
#[derive(Resource, Clone)]
pub(crate) struct UiFonts {
    /// SemiBold — HUD overlays, all-caps banners, big prompts, identity chips.
    pub hud: Handle<Font>,
    /// Regular — the smaller, denser numeric readouts (metric rows, reticle numbers).
    pub body: Handle<Font>,
}

/// The family font size for every full-screen overlay banner (menu, connect-status, death, pause),
/// in pixels. Unifies what had drifted — the pause overlay used to render at 80 px while the other
/// three used 48 — onto the family standard.
const OVERLAY_FONT_PX: f32 = 48.0;

/// The cold blue-white every client surface renders its live text in — the crew card, the net debug
/// panel and the settings page all drew this literal before it was named once here.
pub(crate) const TEXT: Color = Color::srgb(0.85, 0.95, 1.0);

/// The muted companion to [`TEXT`], for a line that explains rather than reports (the settings
/// page's footer hint and its control legend). Kept beside [`TEXT`] so the contrast between the two
/// is a decision rather than two literals that drifted apart.
pub(crate) const TEXT_DIM: Color = Color::srgb(0.55, 0.63, 0.70);

/// The translucent slab behind a HUD CARD — a panel that sits over live play and must not hide it
/// (the crew status card, the net ping/FPS panel).
///
/// **Known drift, deliberately left:** `hud.rs`'s tank-state card uses the same RGB at `0.55` alpha
/// rather than this `0.62`. Unifying it would change a shipped picture on a judgement call nobody
/// has made, so it stays a separate literal there; if the two are ever reconciled, this is the
/// constant that should win.
pub(crate) const PANEL_BG: Color = Color::srgba(0.04, 0.06, 0.08, 0.62);

/// The card behind a MODAL page (the settings page). The same family as [`PANEL_BG`] but far more
/// opaque, and on purpose: a modal has stopped play and owns the screen, so its text wants a solid
/// backing rather than a readable-through one.
pub(crate) const MODAL_BG: Color = Color::srgba(0.03, 0.05, 0.07, 0.92);

/// The death screen's red wash over the world (TUNED), drawn only by `overlay::death_tint`.
pub(crate) const DEATH_TINT: Color = Color::srgba(0.15, 0.0, 0.0, 0.6);

/// Spawn a full-screen, centered overlay: an optional dim translucent backdrop with one line (or
/// block) of centered SemiBold [`UiFonts::hud`] text. This is the single shape behind the menu,
/// connect-status, death, and pause overlays, which had drifted into four near-identical copies that
/// each commented that they "mirror" the others. Callers supply only what genuinely differs:
///
/// - `node_markers` — component(s) placed on the backdrop node. This is each site's identity and its
///   despawn handle: the shared `overlay::OverlayNode(_)` (which drives z + one-scrim visibility) plus
///   any site-specific marker such as the death-screen state enum, or `DespawnOnExit(Paused)` for the
///   single-player pause overlay. Everything each site queries or despawns hangs off this.
/// - `text` — the message (may contain `\n`).
/// - `text_markers` — component(s) on the `Text` child. Only the connect overlay needs one
///   (`ConnectStatusText`, so its label can be rewritten later); the other three pass `()`.
/// - `backdrop` — the dim fill `Color`, or `None` for no fill (the pause overlay carries none).
///
/// Font size is [`OVERLAY_FONT_PX`] for every site. Returns the spawned node entity.
pub(crate) fn spawn_overlay(
    commands: &mut Commands,
    font: &Handle<Font>,
    node_markers: impl Bundle,
    text: impl Into<String>,
    text_markers: impl Bundle,
    backdrop: Option<Color>,
) -> Entity {
    let mut node = commands.spawn((
        node_markers,
        Node {
            width: Val::Percent(100.0),
            height: Val::Percent(100.0),
            justify_content: JustifyContent::Center,
            align_items: AlignItems::Center,
            ..default()
        },
    ));
    if let Some(color) = backdrop {
        node.insert(BackgroundColor(color));
    }
    node.with_children(|parent| {
        parent.spawn((
            text_markers,
            Text::new(text),
            TextFont {
                // SemiBold: a big all-caps overlay banner.
                font: font.clone().into(),
                font_size: FontSize::Px(OVERLAY_FONT_PX),
                ..default()
            },
            TextColor(Color::WHITE),
        ));
    });
    node.id()
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

/// The glyphs past printable ASCII that both shipped weights carry, each confirmed in their cmaps;
/// `src/tests/ui_ascii.rs` guards source literals against the same set.
const TYPOGRAPHIC: [char; 7] = [
    '\u{2026}', '\u{2014}', '\u{2013}', '\u{00B0}', '\u{00D7}', '\u{00B1}', '\u{2264}',
];

/// `text` reduced to what the shipped font draws: printable ASCII and [`TYPOGRAPHIC`], every run of
/// anything else collapsed to one space, so a dropped glyph never welds two words. `None` where no
/// letter or digit survives: the caller says what is missing instead. For strings that come from
/// data (a player's name, a tank file's label), which `src/tests/ui_ascii.rs` cannot see.
pub(crate) fn drawable(text: &str) -> Option<String> {
    let mut out = String::new();
    for character in text.chars() {
        let kept = if (' '..='~').contains(&character) || TYPOGRAPHIC.contains(&character) {
            character
        } else {
            ' '
        };
        if kept != ' ' || !(out.is_empty() || out.ends_with(' ')) {
            out.push(kept);
        }
    }
    let out = out.trim_end().to_owned();
    out.chars()
        .any(|character| character.is_ascii_alphanumeric())
        .then_some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// What the font draws passes unchanged; anything else collapses to a space, runs of spaces
    /// collapse to one and the ends are trimmed, and a string with nothing legible left is refused.
    /// The display row's localized monitor names are the inputs that reach it least legible.
    /// Escaped, not literal: the rendered-coverage guard (`src/tests/ui_ascii.rs`) scans `Text`
    /// sources, and these are inputs that must NOT survive.
    #[test]
    fn drawable_keeps_only_what_the_font_draws() {
        assert_eq!(drawable("Tiger I").as_deref(), Some("Tiger I"));
        assert_eq!(
            drawable("88mm_AP 773 m/s \u{00B1}1\u{00B0}").as_deref(),
            Some("88mm_AP 773 m/s \u{00B1}1\u{00B0}")
        );
        assert_eq!(drawable("Lt. \u{674E} Wei").as_deref(), Some("Lt. Wei"));
        assert_eq!(drawable("\u{00DC}nter").as_deref(), Some("nter"));
        assert_eq!(drawable("\u{864E}\u{738B}"), None);
        assert_eq!(drawable("\u{2014}"), None);
        assert_eq!(
            drawable("DELL\u{a0}U2723QE").as_deref(),
            Some("DELL U2723QE")
        );
        assert_eq!(
            drawable("  \u{c9}cran  externe  ").as_deref(),
            Some("cran externe")
        );
        assert_eq!(drawable("\u{30c7}\u{30a3}\u{30b9}"), None);
        assert_eq!(drawable("--"), None);
        assert_eq!(drawable(""), None);
    }
}
