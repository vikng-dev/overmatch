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
//! differs (`!=`-guarded). Every resource it edits — [`LayerView`], [`ShotParams`] — derives
//! `Copy + PartialEq` for exactly that. The armour sandbox has NO `run_if(resource_changed::<...>)`
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

use crate::ballistics::{ComponentHealth, MarchMode, ShellReadout};
use crate::damage::{CookedOff, TankKnockedOut};

use super::{
    ClearRequested, LayerView, MeshState, PanelWantsInput, ResetRequested, SPEEDS, ShotParams,
    SpeedIndex, VolumeState,
};

pub(super) fn plugin(app: &mut App) {
    app.add_plugins(EguiPlugin::default())
        // The primary context runs multi-pass in 0.41, so UI MUST be drawn in this schedule.
        .add_systems(EguiPrimaryContextPass, panel);
}

const OK: egui::Color32 = egui::Color32::from_rgb(120, 210, 140);
const WARN: egui::Color32 = egui::Color32::from_rgb(235, 90, 70);

/// The resources the panel WRITES, as one bundle. Each is `Copy + PartialEq` or a tiny flag, so the
/// panel edits a local and writes back on change (see the module doc).
#[derive(SystemParam)]
struct Write<'w> {
    layers: ResMut<'w, LayerView>,
    shot: ResMut<'w, ShotParams>,
    march: ResMut<'w, MarchMode>,
    speed: ResMut<'w, SpeedIndex>,
    time: ResMut<'w, Time<Virtual>>,
    clear: ResMut<'w, ClearRequested>,
    reset: ResMut<'w, ResetRequested>,
    want: ResMut<'w, PanelWantsInput>,
}

/// The live read-outs the Telemetry section shows: the count of shells still on the board and the
/// target's damage state.
#[derive(SystemParam)]
struct Telemetry<'w, 's> {
    shells: Query<'w, 's, (), With<ShellReadout>>,
    knocked_out: Query<'w, 's, (), With<TankKnockedOut>>,
    cooked_off: Query<'w, 's, (), With<CookedOff>>,
    health: Query<'w, 's, &'static ComponentHealth>,
}

fn panel(mut contexts: EguiContexts, mut w: Write, telemetry: Telemetry) {
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
    let mut layers = *w.layers;
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
        .default_size(300.0)
        .show(&mut viewport, |ui| {
            egui::ScrollArea::vertical().show(ui, |ui| {
                ui.heading("Armor sandbox");

                egui::CollapsingHeader::new("Layers")
                    .default_open(true)
                    .show(ui, |ui| layers_section(ui, &mut layers));

                egui::CollapsingHeader::new("Shot")
                    .default_open(true)
                    .show(ui, |ui| shot_section(ui, &mut shot, &mut reset_shot));

                egui::CollapsingHeader::new("Time")
                    .default_open(false)
                    .show(ui, |ui| {
                        time_section(ui, &mut paused, &mut speed_idx, &mut march_real)
                    });

                egui::CollapsingHeader::new("Telemetry")
                    .default_open(true)
                    .show(ui, |ui| {
                        telemetry_section(ui, &telemetry, time_scale, paused, march_real)
                    });

                egui::CollapsingHeader::new("Scene")
                    .default_open(false)
                    .show(ui, |ui| {
                        if ui.button("Clear shots").clicked() {
                            do_clear = true;
                        }
                        if ui.button("Reset world (rebuild target)").clicked() {
                            do_reset = true;
                        }
                    });
            });
        });

    // Reset-to-88 restores the authored shot; it wins over any slider edit made the same frame.
    if reset_shot {
        shot = ShotParams::default();
    }

    // Write-backs — resources are touched ONLY on a real change.
    if layers != *w.layers {
        *w.layers = layers;
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

/// A segmented control row for the hull's [`MeshState`] — the solid / x-ray / off loop as a strip of
/// `SelectableLabel`s over the panel's LOCAL copy (write-on-change is enforced in [`panel`]).
fn mesh_state_row(ui: &mut egui::Ui, label: &str, state: &mut MeshState) {
    ui.horizontal(|ui| {
        ui.label(label);
        for option in MeshState::ALL {
            if ui
                .selectable_label(*state == option, option.label())
                .clicked()
            {
                *state = option;
            }
        }
    });
}

/// A segmented control row for a volume category's [`VolumeState`] — off / on-top / solid / x-ray.
fn volume_state_row(ui: &mut egui::Ui, label: &str, state: &mut VolumeState) {
    ui.horizontal(|ui| {
        ui.label(label);
        for option in VolumeState::ALL {
            if ui
                .selectable_label(*state == option, option.label())
                .clicked()
            {
                *state = option;
            }
        }
    });
}

/// The three inspection layers (the `F1`/`F2`/`F3` tap-loops) as segmented controls over the panel's
/// LOCAL [`LayerView`].
fn layers_section(ui: &mut egui::Ui, layers: &mut LayerView) {
    mesh_state_row(ui, "mesh     ", &mut layers.mesh);
    volume_state_row(ui, "armor    ", &mut layers.armor);
    volume_state_row(ui, "component", &mut layers.components);
}

/// The live shot parameters — muzzle speed, calibre, mass — as sliders over the panel's LOCAL
/// [`ShotParams`], plus a restore-the-88 button. Calibre is edited in mm (stored in m).
fn shot_section(ui: &mut egui::Ui, shot: &mut ShotParams, reset_shot: &mut bool) {
    ui.add(egui::Slider::new(&mut shot.muzzle_speed, 100.0..=1200.0).text("muzzle (m/s)"));
    let mut cal_mm = shot.caliber * 1e3;
    if ui
        .add(egui::Slider::new(&mut cal_mm, 20.0..=150.0).text("calibre (mm)"))
        .changed()
    {
        shot.caliber = cal_mm / 1e3;
    }
    ui.add(egui::Slider::new(&mut shot.mass, 1.0..=25.0).text("mass (kg)"));
    if ui.button("Reset to 88 mm").clicked() {
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
    ui.radio_value(march_real, false, "Demo (smooth per-frame)");
    ui.radio_value(march_real, true, "Real (fixed server cadence)");
}

/// Live read-out: the clock state, the shell count on the board, and the target's damage state —
/// what the retired top-left `StatusText` HUD used to carry, plus a shot-verdict line. Monospace so
/// the columns line up.
fn telemetry_section(
    ui: &mut egui::Ui,
    t: &Telemetry,
    time_scale: f32,
    paused: bool,
    march_real: bool,
) {
    let rate = if paused {
        "paused".to_string()
    } else {
        format!("{time_scale:.3}x")
    };
    let mode = if march_real { "real" } else { "demo" };
    ui.monospace(format!("time    {rate}  [{mode}]"));
    ui.monospace(format!("shells  {}", t.shells.iter().count()));

    ui.separator();
    let (mut damaged, mut total) = (0usize, 0usize);
    for hp in &t.health {
        total += 1;
        if hp.current < hp.max {
            damaged += 1;
        }
    }
    ui.monospace(format!("modules {damaged}/{total} damaged"));
    if t.cooked_off.iter().next().is_some() {
        ui.colored_label(WARN, "ammo COOKED OFF");
    } else if t.knocked_out.iter().next().is_some() {
        ui.colored_label(WARN, "target KNOCKED OUT");
    } else {
        ui.colored_label(OK, "target operational");
    }
}
