//! Bundled Barlow Condensed handles and required UI styling from `assets/ui/style.ron`.
//! Handles exist before Startup; both weights join presentation admission before play.
//! Rendered strings stay within the bundled coverage guarded by `src/tests/ui_ascii.rs`.

use bevy::prelude::*;

/// The bundled weights shared by every client text surface.
#[derive(Resource, Clone)]
pub(crate) struct UiFonts {
    /// SemiBold — HUD overlays, all-caps banners, big prompts, identity chips.
    pub hud: Handle<Font>,
    /// Regular — the smaller, denser numeric readouts (metric rows, reticle numbers).
    pub body: Handle<Font>,
}

include!(concat!(env!("OUT_DIR"), "/ui_style_authored.rs"));

/// A centered overlay banner. `node_markers` own the backdrop's lifetime and layer; `text_markers`
/// identify a label that changes. `backdrop: None` leaves the scene undimmed. Every banner uses
/// the required overlay font size and colour.
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
            TextColor(OVERLAY_TEXT),
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
    app.insert_resource(fonts)
        .init_resource::<crate::assets::PresentationLoads>()
        .add_systems(PreStartup, require_fonts);
}

/// Bevy 0.19's FontLoader wraps bytes without decoding them. Admit format and glyph coverage
/// synchronously, then let the shared presentation gate wait for both asynchronous handles.
fn require_fonts(fonts: Res<UiFonts>, mut required: ResMut<crate::assets::PresentationLoads>) {
    let root = crate::assets::asset_root();
    for (path, handle) in [
        ("fonts/BarlowCondensed-SemiBold.ttf", &fonts.hud),
        ("fonts/BarlowCondensed-Regular.ttf", &fonts.body),
    ] {
        let bytes = crate::assets::require_file(&root, path, "UiFonts");
        admit_font(path, &bytes);
        required.0.push((path.into(), handle.clone().untyped()));
    }
}

fn admit_font(path: &str, bytes: &[u8]) {
    let face = ttf_parser::Face::parse(bytes, 0).unwrap_or_else(|error| {
        panic!("{path}: UiFonts: invalid font: {error:?}; restore the bundled font export")
    });
    for glyph in (' '..='~').chain(TYPOGRAPHIC) {
        let index = face.glyph_index(glyph).unwrap_or_else(|| {
            panic!("{path}: UiFonts: missing required glyph {glyph:?}; restore a font with the bundled UI coverage")
        });
        assert!(
            glyph == ' ' || face.glyph_bounding_box(index).is_some(),
            "{path}: UiFonts: unreadable outline for {glyph:?}; restore the bundled font export"
        );
    }
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

/// Assign `value` to `text` only when it differs. Every write to a [`Text`] marks it changed, and
/// a changed text is re-measured and re-laid-out by `bevy_ui`; a HUD that rewrites an unchanged
/// string every frame pays that cost for nothing. Per-frame HUD writers go through here.
pub(crate) fn set_text(mut text: Mut<Text>, value: &str) {
    if text.0 != value {
        text.0.clear();
        text.0.push_str(value);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Client admission must wait for both weights, including a client with no text entities yet.
    /// Mount the shipped font installer: omitting either registration cannot pass this witness.
    #[test]
    fn both_weights_are_required_presentation_dependencies() {
        let mut app = App::new();
        app.add_plugins((MinimalPlugins, AssetPlugin::default()))
            .init_asset::<Font>()
            .init_asset_loader::<bevy::text::FontLoader>()
            .add_plugins(plugin);
        app.update();
        let fonts = app.world().resource::<UiFonts>();
        let required = &app.world().resource::<crate::assets::PresentationLoads>().0;
        for handle in [&fonts.hud, &fonts.body] {
            assert!(
                required
                    .iter()
                    .any(|(_, entry)| entry.id() == handle.clone().untyped().id())
            );
        }
        assert_ne!(fonts.hud.id(), fonts.body.id());
    }

    /// A loader's Loaded state alone cannot prove drawable text in Bevy 0.19. Corrupt or
    /// truncated bytes must refuse at the font gate, naming the asset and its repair.
    #[test]
    fn font_admission_refuses_corrupt_bytes_and_checks_the_bundled_coverage() {
        for path in [
            "fonts/BarlowCondensed-SemiBold.ttf",
            "fonts/BarlowCondensed-Regular.ttf",
        ] {
            let bytes =
                crate::assets::require_file(&crate::assets::asset_root(), path, "font fixture");
            admit_font(path, &bytes);
            for bad in [&b"not a font"[..], &bytes[..bytes.len() / 2]] {
                let error = std::panic::catch_unwind(|| admit_font(path, bad)).unwrap_err();
                let text = error
                    .downcast_ref::<String>()
                    .expect("font admission formats its cause");
                assert!(text.contains(path) && text.contains("restore"), "{text}");
            }
        }
    }

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
