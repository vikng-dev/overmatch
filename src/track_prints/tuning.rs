//! The ground-type tuning panel: the owner sets each ground type's track-print response (cosmetic
//! terrain marks, view only) by eye in the offline game, and writes the chosen values back to the
//! table's file (`assets/terrain/ground_types.ron`).
//!
//! Compiled only under `dev_ui`, so `bevy_egui` never lands in the shipping client, and mounted only
//! by the offline root. Shown while the game is paused (Esc), where the cursor is free: every value
//! the prints draw is read from the [`GroundTypes`] resource through the material's uniform
//! (`track_prints::sync_material`), so a slider moves every print already laid, and the drawn belt's
//! sink with it.
//!
//! The panel edits a copy and writes the resource back only when the copy differs, so the
//! material is re-uploaded on a real change and on nothing else.

use bevy::prelude::*;
use bevy_egui::{
    EguiContexts, EguiGlobalSettings, EguiPlugin, EguiPrimaryContextPass, PrimaryEguiContext, egui,
};

use crate::ground_types::GroundTypes;
use crate::state::AppState;
use crate::terrain_blend::{BASE_GROUND, BLEND_LAYERS};
use crate::track::rig_geom::RigGeom;
use crate::view::PlayerView;

/// The panel's width, logical pixels.
const PANEL_WIDTH: f32 = 320.0;

/// What the last save or reload said, for the panel's status line.
#[derive(Resource, Default)]
struct Status(String);

pub(crate) fn offline_plugin(app: &mut App) {
    app.add_plugins(EguiPlugin::default());
    let mut settings = app.world_mut().resource_mut::<EguiGlobalSettings>();
    // The context goes on the player's camera, not on whichever camera spawns first (a picture
    // camera renders to an image), and the game's own cursor stays the game's.
    settings.auto_create_primary_context = false;
    settings.enable_cursor_icon_updates = false;
    app.init_resource::<Status>()
        .add_systems(Update, attach_context)
        .add_systems(
            EguiPrimaryContextPass,
            panel
                .run_if(in_state(AppState::Paused))
                .run_if(resource_exists::<GroundTypes>),
        );
}

/// Put the primary egui context on the player's camera, once it exists.
fn attach_context(
    mut commands: Commands,
    camera: Query<Entity, With<PlayerView>>,
    existing: Query<(), With<PrimaryEguiContext>>,
) {
    if !existing.is_empty() {
        return;
    }
    if let Some(camera) = camera.iter().next() {
        commands.entity(camera).insert(PrimaryEguiContext);
    }
}

fn panel(
    mut contexts: EguiContexts,
    mut ground: ResMut<GroundTypes>,
    mut status: ResMut<Status>,
    geom: Option<Res<RigGeom>>,
) {
    let Ok(ctx) = contexts.ctx_mut() else {
        return;
    };
    let kpa = geom.map(|geom| super::tank_ground_pressure_kpa(&geom));
    let mut table = ground.clone();
    let mut save = false;
    let mut reload = false;
    // egui shows panels INTO a `Ui`: the full-viewport background `Ui` the panel lays out inside
    // (the armour sandbox's pattern, `sandbox::panel`).
    let mut viewport = egui::Ui::new(
        ctx.clone(),
        "ground_types_viewport".into(),
        egui::UiBuilder::new()
            .layer_id(egui::LayerId::background())
            .max_rect(ctx.viewport_rect()),
    );
    egui::Panel::left("ground_types_panel")
        .resizable(false)
        .default_size(PANEL_WIDTH)
        .show(&mut viewport, |ui| {
            egui::ScrollArea::vertical().show(ui, |ui| {
                ui.heading("Ground types");
                ui.label(match kpa {
                    Some(kpa) => format!("This tank bears {kpa:.1} kPa on the ground."),
                    None => "No running gear yet.".to_string(),
                });
                let mut packs = vec![format!("base: {BASE_GROUND}")];
                packs.extend(
                    BLEND_LAYERS
                        .iter()
                        .map(|layer| format!("{}: {}", layer.pack, layer.ground)),
                );
                ui.label(packs.join("\n"));
                ui.separator();
                for (name, ground) in &mut table.types {
                    let print = &mut ground.print;
                    egui::CollapsingHeader::new(name.as_str())
                        .default_open(true)
                        .show(ui, |ui| {
                            ui.add(
                                egui::Slider::new(&mut print.softness_mm_per_kpa, 0.0..=3.0)
                                    .text("softness, mm/kPa"),
                            );
                            ui.add(
                                egui::Slider::new(&mut print.depth_cap_mm, 0.0..=300.0)
                                    .text("depth cap, mm"),
                            );
                            ui.add(
                                egui::Slider::new(&mut print.darkening, 0.0..=1.0)
                                    .text("darkening"),
                            );
                            ui.add(
                                egui::Slider::new(&mut print.roughness_gain, 0.2..=1.5)
                                    .text("roughness gain"),
                            );
                            if let Some(kpa) = kpa {
                                ui.label(format!(
                                    "This tank prints {:.0} mm deep here.",
                                    print.depth_m(kpa) * 1000.0
                                ));
                            }
                        });
                }
                ui.separator();
                ui.horizontal(|ui| {
                    save = ui.button("Save to the table").clicked();
                    reload = ui.button("Reload from the table").clicked();
                });
                ui.label(&status.0);
            });
        });
    if table != *ground {
        *ground = table;
    }
    let path = GroundTypes::shipped_path();
    if save {
        status.0 = match ground.save(&path) {
            Ok(()) => format!("Saved {}", path.display()),
            Err(err) => format!("Not saved: {err}"),
        };
        info!("ground types: {}", status.0);
    }
    if reload {
        match GroundTypes::load(&path) {
            Ok(loaded) => {
                *ground = loaded;
                status.0 = format!("Reloaded {}", path.display());
            }
            Err(err) => status.0 = format!("Not reloaded: {err}"),
        }
    }
}
