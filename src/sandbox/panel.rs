//! The armour-sandbox control panel — one clickable egui `SidePanel` that is the sandbox's whole
//! non-camera control surface, mirroring the treatment the `track_sandbox` panel gave the driving
//! rig ([`crate::track_sandbox::panel`], the pattern this file follows).
//!
//! Compiled ONLY under the `dev_ui` feature (`cargo armor` = `cargo run --bin armor_sandbox
//! --features dev_ui`), so `bevy_egui` never lands in the shipping client — the whole reason the
//! panel is its own `#[cfg(feature = "dev_ui")]` submodule of [`crate::sandbox`] rather than folded
//! into it.
//!
//! # Change-tick discipline
//!
//! The panel NEVER hands egui `&mut resource.field`: it copies each mutable resource into a local,
//! draws every widget against the local, and writes the resource back only when the local actually
//! differs (`!=`-guarded). Every resource it edits — [`ArmourOverlay`], [`ShotParams`], the look's
//! [`XRay`] flag — is `Copy + PartialEq` for exactly that. The armour sandbox has NO `run_if(resource_changed::<...>)`
//! reactor on any resource this panel touches (unlike the track sandbox's `sync_collider_gizmos`),
//! so a spurious change-tick could not retrigger heavy work even if we leaked one; the write-on-change
//! discipline is kept regardless, both to match the precedent and to keep the intent seams
//! ([`ClearRequested`] / [`ResetRequested`]) firing exactly once.
//!
//! The panel runs in `EguiPrimaryContextPass` (mandatory for the multi-pass primary context in
//! bevy_egui 0.41) and publishes egui's focus state into [`PanelWantsInput`] so [`crate::sandbox`]
//! gates the firing / fly-camera / slow-mo input off while a widget is focused — the click-the-panel-
//! must-not-fire-a-shell guard.

use bevy::ecs::system::SystemParam;
use bevy::prelude::*;
use bevy::time::Virtual;
use bevy_egui::{EguiContexts, EguiPlugin, EguiPrimaryContextPass, egui};

use crate::ballistics::MarchMode;
use crate::damage_cam::pen_cam::game::XRay;
use crate::tank::Weapon;

use super::{
    ArmourOverlay, ClearRequested, PANEL_WIDTH, PanelWantsInput, RangeGun, ResetRequested, SPEEDS,
    ShotParams, SpeedIndex,
};

pub(super) fn plugin(app: &mut App) {
    app.add_plugins(EguiPlugin::default())
        // The primary context runs multi-pass in 0.41, so UI MUST be drawn in this schedule.
        .add_systems(EguiPrimaryContextPass, panel);
}

/// The resources the panel WRITES, as one bundle. Each is `Copy + PartialEq` or a tiny flag, so the
/// panel edits a local and writes back on change (see the module doc).
#[derive(SystemParam)]
struct Write<'w> {
    xray: ResMut<'w, XRay>,
    overlay: ResMut<'w, ArmourOverlay>,
    shot: ResMut<'w, ShotParams>,
    march: ResMut<'w, MarchMode>,
    speed: ResMut<'w, SpeedIndex>,
    time: ResMut<'w, Time<Virtual>>,
    clear: ResMut<'w, ClearRequested>,
    reset: ResMut<'w, ResetRequested>,
    want: ResMut<'w, PanelWantsInput>,
}

fn panel(mut contexts: EguiContexts, mut w: Write, gun: Query<&Weapon, With<RangeGun>>) {
    let Ok(ctx) = contexts.ctx_mut() else {
        return;
    };

    // Publish egui's focus so `crate::sandbox` suppresses firing/fly/slow-mo input while a widget is
    // active. Write-on-change so `PanelWantsInput` is not marked changed every frame.
    let (kb, ptr) = (
        ctx.egui_wants_keyboard_input(),
        ctx.egui_wants_pointer_input(),
    );
    if w.want.keyboard != kb {
        w.want.keyboard = kb;
    }
    if w.want.pointer != ptr {
        w.want.pointer = ptr;
    }

    // egui 0.41 shows panels INTO a `Ui`, not directly onto the context, so build the full-viewport
    // background `Ui` the panel lays out inside (the bevy_egui `side_panel` example pattern).
    let mut viewport = egui::Ui::new(
        ctx.clone(),
        "armor_sandbox_viewport".into(),
        egui::UiBuilder::new()
            .layer_id(egui::LayerId::background())
            .max_rect(ctx.viewport_rect()),
    );

    // Local working copies — every widget edits THESE, never the resources (change-tick discipline).
    let mut look = w.xray.on;
    let mut overlay = *w.overlay;
    let mut shot = *w.shot;
    let mut march_real = *w.march == MarchMode::Real;
    let mut speed_idx = w.speed.0;
    let mut paused = w.time.is_paused();
    let time_scale = w.time.relative_speed();
    let mut do_clear = false;
    let mut do_reset = false;
    let mut reset_shot = false;

    egui::Panel::left("armor_sandbox_panel")
        .resizable(false)
        .default_size(PANEL_WIDTH)
        .show(&mut viewport, |ui| {
            egui::ScrollArea::vertical().show(ui, |ui| {
                ui.heading("Armor sandbox");

                egui::CollapsingHeader::new("View")
                    .default_open(true)
                    .show(ui, |ui| view_section(ui, &mut look, &mut overlay));

                egui::CollapsingHeader::new("Shot")
                    .default_open(true)
                    .show(ui, |ui| {
                        shot_section(ui, gun.single().ok(), &mut shot, &mut reset_shot)
                    });

                egui::CollapsingHeader::new("Time")
                    .default_open(false)
                    .show(ui, |ui| {
                        time_section(ui, &mut paused, &mut speed_idx, &mut march_real)
                    });

                egui::CollapsingHeader::new("Telemetry")
                    .default_open(true)
                    .show(ui, |ui| {
                        telemetry_section(ui, time_scale, paused, march_real)
                    });

                egui::CollapsingHeader::new("Scene")
                    .default_open(false)
                    .show(ui, |ui| {
                        if ui.button("Clear").clicked() {
                            do_clear = true;
                        }
                        if ui.button("Rebuild").clicked() {
                            do_reset = true;
                        }
                    });
            });
        });

    // Reset restores the first round at its own speed; it wins over any edit made the same frame.
    if reset_shot {
        shot = ShotParams::default();
    }

    // Write-backs — resources are touched ONLY on a real change.
    if look != w.xray.on {
        w.xray.on = look;
    }
    if overlay != *w.overlay {
        *w.overlay = overlay;
    }
    if shot != *w.shot {
        *w.shot = shot;
    }
    let want_march = if march_real {
        MarchMode::Real
    } else {
        MarchMode::Demo
    };
    if want_march != *w.march {
        *w.march = want_march;
    }
    // The slow-mo ladder: changing the index resumes and re-scales virtual time (mirrors the `Up`/
    // `Down` `time_controls` path). Only touch the clock on a real change.
    if speed_idx != w.speed.0 {
        w.speed.0 = speed_idx;
        w.time.set_relative_speed(SPEEDS[speed_idx]);
        w.time.unpause();
        paused = false;
    }
    if paused != w.time.is_paused() {
        if paused {
            w.time.pause();
        } else {
            w.time.unpause();
        }
    }
    if do_clear {
        w.clear.0 = true;
    }
    if do_reset {
        w.reset.0 = true;
    }
}

/// The x-ray (`X`) and the armour overlay (`F2`) over the panel's LOCAL copies.
fn view_section(ui: &mut egui::Ui, look: &mut bool, overlay: &mut ArmourOverlay) {
    ui.checkbox(look, "x-ray");
    ui.horizontal(|ui| {
        ui.label("armour");
        for option in ArmourOverlay::ALL {
            if ui
                .selectable_label(*overlay == option, option.label())
                .clicked()
            {
                *overlay = option;
            }
        }
    });
}

/// The range gun's rounds as a selectable list, each display name with its registry key, and a
/// muzzle-speed override that stands in for range, over the panel's LOCAL [`ShotParams`]: off
/// fires the round at its own speed, on starts at it. Selecting another round drops the override.
fn shot_section(
    ui: &mut egui::Ui,
    gun: Option<&Weapon>,
    shot: &mut ShotParams,
    reset_shot: &mut bool,
) {
    let Some(gun) = gun else {
        return;
    };
    for (index, loaded) in gun.rounds.iter().enumerate() {
        let Ok(index) = u8::try_from(index) else {
            break;
        };
        let label = format!("{}  [{}]", loaded.name, loaded.key);
        if ui.selectable_label(shot.round == index, label).clicked() && shot.round != index {
            *shot = ShotParams {
                round: index,
                muzzle_speed: None,
            };
        }
    }
    let Some(own) = gun
        .rounds
        .get(usize::from(shot.round))
        .map(|loaded| loaded.round.muzzle_speed)
    else {
        return;
    };
    ui.separator();
    ui.monospace(format!("own speed  {own:.0} m/s"));
    let mut overriding = shot.muzzle_speed.is_some();
    if ui.checkbox(&mut overriding, "override").changed() {
        shot.muzzle_speed = overriding.then_some(own);
    }
    if let Some(speed) = shot.muzzle_speed.as_mut() {
        ui.add(egui::Slider::new(speed, 100.0..=1200.0).text("m/s"));
    }
    if ui.button("Reset shot").clicked() {
        *reset_shot = true;
    }
}

/// The sim-clock controls: freeze, the slow-mo ladder, and the march-cadence A/B — the panel homes
/// for `Space`, `Up`/`Down`, and `T`.
fn time_section(
    ui: &mut egui::Ui,
    paused: &mut bool,
    speed_idx: &mut usize,
    march_real: &mut bool,
) {
    ui.checkbox(paused, "freeze");
    ui.separator();
    ui.label("slow-mo");
    ui.horizontal_wrapped(|ui| {
        for (i, scale) in SPEEDS.iter().enumerate() {
            let label = if *scale >= 1.0 {
                "1x".to_string()
            } else {
                format!("{scale:.3}x")
            };
            if ui.selectable_label(*speed_idx == i, label).clicked() {
                *speed_idx = i;
            }
        }
    });
    ui.separator();
    ui.label("march cadence");
    ui.radio_value(march_real, false, MARCH_DEMO);
    ui.radio_value(march_real, true, MARCH_REAL);
}

/// The march cadences' names: [`MarchMode::Demo`] marches each frame, [`MarchMode::Real`] each
/// fixed tick, as the server does.
const MARCH_DEMO: &str = "per frame";
const MARCH_REAL: &str = "per tick";

/// What the picture cannot show: the clock's rate and the march cadence. Monospace so the
/// columns line up.
fn telemetry_section(ui: &mut egui::Ui, time_scale: f32, paused: bool, march_real: bool) {
    let rate = if paused {
        "paused".to_string()
    } else {
        format!("{time_scale:.3}x")
    };
    let march = if march_real { MARCH_REAL } else { MARCH_DEMO };
    ui.monospace(format!("time     {rate}"));
    ui.monospace(format!("march    {march}"));
}
