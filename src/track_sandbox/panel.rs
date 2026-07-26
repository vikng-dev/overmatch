//! The `track_sandbox` control panel — one clickable egui `SidePanel` carrying every non-driving
//! control the sandbox has.
//!
//! Compiled ONLY under the `dev_ui` feature (`cargo run --bin track_sandbox --features dev_ui`), so
//! `bevy_egui` never lands in the shipping client — the whole reason the panel is its own module
//! behind a `#[cfg]` in `mod.rs` rather than folded into `suspension_viz`.
//!
//! # Change-tick discipline
//!
//! `refresh_envelope` / `refresh_hard_stops` recalibrate on a `RigSuspension` change tick, and
//! `apply_rig_counts` rebuilds the rig on a `RigCounts` change tick — recalibrating every frame was a
//! bug we already fixed once. So this panel NEVER hands egui `&mut resource.field`: it copies each
//! mutable resource into a local, draws every widget against the local, and writes the resource back
//! only when the local actually differs (the write-backs are guarded by `!=`). Every value this panel
//! writes — [`RigSuspension`], [`RigCounts`], [`TransSwitch`], [`VizLayers`], [`SuspensionViz`] — is
//! `Copy + PartialEq` for exactly that pattern. The heavy commit path stays OUT of the panel: a count
//! edit writes the [`RigCounts`] intent (committed by `apply_rig_counts`), so the panel holds no
//! `ResMut<RigGeom>` / `ResMut<PinBelt>` / belt state and cannot itself trigger a per-frame rebuild.
//! A transmission flip writes [`TransSwitch`]; the adapter reset is `reset_trans_on_change`, exactly
//! as the `T` key drives it.
//!
//! The panel runs in `EguiPrimaryContextPass` (mandatory for the multi-pass primary context in
//! bevy_egui 0.41) and publishes egui's focus state into [`PanelWantsInput`] so `mod.rs` gates the
//! driving/camera input off while a widget is focused.

use avian3d::prelude::LinearVelocity;
use bevy::ecs::system::SystemParam;
use bevy::prelude::*;
use bevy_egui::{EguiContexts, EguiPlugin, EguiPrimaryContextPass, egui};

use super::belt::{EnvelopeLaw, ViewPerf};
use super::derive;
use super::rig_geom::{DroopLimiter, RigGeom};
use super::suspension_viz::SuspensionViz;
use super::{
    BeltContacts, BeltSpeed, Hull, MeshState, PanelWantsInput, ResetRequested, RigCounts, RigSpec,
    RigSuspension, TransSwitch, VizLayers, VolumeState, clamp_link_count,
};
use crate::bake::TankBlueprint;
use crate::track::side::Side;
use crate::track::transmission::TransmissionMode;

pub(super) fn plugin(app: &mut App) {
    app.add_plugins(EguiPlugin::default())
        // The primary context runs multi-pass in 0.41, so UI MUST be drawn in this schedule.
        .add_systems(EguiPrimaryContextPass, panel);
}

/// The resources the panel WRITES, as one bundle. Every one derives `Copy + PartialEq`, or is a
/// tiny flag, so the panel edits a local and writes back on change (see the module doc).
#[derive(SystemParam)]
struct Write<'w> {
    knobs: ResMut<'w, RigSuspension>,
    counts: ResMut<'w, RigCounts>,
    trans: ResMut<'w, TransSwitch>,
    layers: ResMut<'w, VizLayers>,
    viz: ResMut<'w, SuspensionViz>,
    reset: ResMut<'w, ResetRequested>,
    want: ResMut<'w, PanelWantsInput>,
}

/// The resources the panel READS. The rig-derived ones are `Option` because they land a flush after
/// the deferred `build_rig`, so the panel renders a "deriving..." state on the pre-rig frames.
#[derive(SystemParam)]
struct Read<'w> {
    geom: Option<Res<'w, RigGeom>>,
    law: Option<Res<'w, EnvelopeLaw>>,
    rig: Option<Res<'w, RigSpec>>,
    blueprint: Option<Res<'w, TankBlueprint>>,
    contacts: Res<'w, BeltContacts>,
    belt: Res<'w, BeltSpeed>,
    view_perf: Res<'w, ViewPerf>,
}

const WARN: egui::Color32 = egui::Color32::from_rgb(235, 90, 70);
const CHAIN: egui::Color32 = egui::Color32::from_rgb(230, 180, 60);
const OK: egui::Color32 = egui::Color32::from_rgb(120, 210, 140);

fn panel(
    mut contexts: EguiContexts,
    mut w: Write,
    r: Read,
    hull: Query<(&Transform, &LinearVelocity), With<Hull>>,
) {
    let Ok(ctx) = contexts.ctx_mut() else {
        return;
    };

    // Publish egui's focus so `mod.rs` suppresses driving/camera input while a widget is active.
    // Write-on-change to keep `PanelWantsInput` from being marked changed every frame. (egui 0.35
    // renamed these `egui_wants_*_input`.)
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

    // egui 0.35 shows panels INTO a `Ui`, not directly onto the context, so build the
    // full-viewport background `Ui` the panel lays out inside (the bevy_egui `side_panel` example
    // pattern).
    let mut viewport = egui::Ui::new(
        ctx.clone(),
        "track_sandbox_viewport".into(),
        egui::UiBuilder::new()
            .layer_id(egui::LayerId::background())
            .max_rect(ctx.viewport_rect()),
    );

    // Local working copies — every widget edits THESE, never the resources (change-tick discipline).
    let mut knobs = w.knobs.0;
    let mut counts = *w.counts;
    let mut trans = w.trans.0;
    let mut layers = *w.layers;
    let mut viz = *w.viz;
    let mut do_reset = false;
    let mut reset_to_ron = false;

    egui::Panel::left("track_sandbox_panel")
        .resizable(false)
        .default_size(310.0)
        .show(&mut viewport, |ui| {
            egui::ScrollArea::vertical().show(ui, |ui| {
                ui.heading("Track sandbox");

                egui::CollapsingHeader::new("Tune")
                    .default_open(true)
                    .show(ui, |ui| {
                        tune_section(ui, &r, &mut knobs, &mut counts, &mut reset_to_ron)
                    });

                egui::CollapsingHeader::new("Model")
                    .default_open(true)
                    .show(ui, |ui| {
                        // The drivetrain adapter — the live selector the `T` key also cycles. The
                        // support law (calibrated contact envelope) and grip law (per-element shear)
                        // are the settled model, not switchable. `TransSwitch` holds an Option:
                        // `None` only on the pre-rig frames (no radio lit), then `build_rig` seeds
                        // it from the spec's declared architecture — a click is always an EXPLICIT
                        // override of that seed.
                        ui.label("transmission");
                        ui.radio_value(&mut trans, Some(TransmissionMode::Governor), "Governor");
                        ui.radio_value(
                            &mut trans,
                            Some(TransmissionMode::Hybrid),
                            "Hybrid (continuous regenerative)",
                        );
                        ui.radio_value(
                            &mut trans,
                            Some(TransmissionMode::FixedRadii),
                            "L600 (geared steering)",
                        );
                    });

                egui::CollapsingHeader::new("Layers")
                    .default_open(false)
                    .show(ui, |ui| layers_section(ui, &mut layers, &mut viz));

                egui::CollapsingHeader::new("Telemetry")
                    .default_open(false)
                    .show(ui, |ui| telemetry_section(ui, &r, &hull));

                egui::CollapsingHeader::new("Scene")
                    .default_open(false)
                    .show(ui, |ui| {
                        if ui.button("Reset tank (cycles spots)").clicked() {
                            do_reset = true;
                        }
                    });
            });
        });

    // Reset-to-RON restores the authored knobs; it wins over any slider edit made the same frame.
    if reset_to_ron && let Some(bp) = r.blueprint.as_deref() {
        knobs = bp.spec.track.suspension.params();
    }

    // Write-backs — resources are touched ONLY on a real change.
    if knobs != w.knobs.0 {
        w.knobs.0 = knobs;
    }
    if counts != *w.counts {
        *w.counts = counts;
    }
    if trans != w.trans.0 {
        w.trans.0 = trans;
    }
    if layers != *w.layers {
        *w.layers = layers;
    }
    if viz != *w.viz {
        *w.viz = viz;
    }
    if do_reset {
        w.reset.0 = true;
    }
}

/// Ride-frequency / damping / bump-stop sliders, the link-count stepper with its feasible window and
/// verdict, the read-only derived values, and the Reset-to-RON button.
fn tune_section(
    ui: &mut egui::Ui,
    r: &Read,
    knobs: &mut derive::SuspensionParams,
    counts: &mut RigCounts,
    reset_to_ron: &mut bool,
) {
    ui.add(egui::Slider::new(&mut knobs.ride_frequency, 0.6..=2.5).text("ride freq (Hz)"));
    ui.add(egui::Slider::new(&mut knobs.damping_ratio, 0.1..=1.0).text("damping ratio"));
    ui.add(egui::Slider::new(&mut knobs.bump_stop, 0.05..=0.35).text("bump stop (m)"));

    let Some(geom) = r.geom.as_deref() else {
        ui.label("deriving rig from glb markers...");
        return;
    };

    // Link count: a clamped stepper (the `;`/`'` keys write the SAME intent), with the feasible
    // window beside it and a coloured verdict below. Clamp against the LIVE knobs so softening the
    // springs widens the band this frame.
    let window = geom.link_window(knobs);
    ui.horizontal(|ui| {
        ui.label("link count");
        if ui.button("-").clicked() {
            counts.link_count = clamp_link_count(geom, knobs, counts.link_count as i32 - 1);
        }
        ui.monospace(format!("{:>3}", counts.link_count));
        if ui.button("+").clicked() {
            counts.link_count = clamp_link_count(geom, knobs, counts.link_count as i32 + 1);
        }
        ui.label(format!("feasible {}..{}", window.n_min, window.n_droop));
    });
    let (color, verdict) = match window.limiter {
        DroopLimiter::Impossible => (
            WARN,
            format!("INFEASIBLE - loop too short ({:+.2} m)", window.slack_rest),
        ),
        DroopLimiter::Chain => (
            CHAIN,
            format!("chain-limited (slack {:+.2} m)", window.slack_rest),
        ),
        DroopLimiter::Spring => (
            OK,
            format!("spring-limited (slack {:+.2} m)", window.slack_rest),
        ),
    };
    ui.colored_label(color, verdict);

    ui.separator();
    // Read-only derived values (mirror the old Suspension page).
    let droop = geom.droop_travel(knobs);
    let tag = if droop.chain_limited() {
        "CHAIN-limited"
    } else {
        "spring-limited"
    };
    ui.monospace(format!(
        "free travel  {:>5.0} mm  ({tag})",
        droop.effective * 1e3
    ));
    ui.monospace(format!(
        "static defl  {:>5.0} mm",
        derive::static_deflection(knobs.ride_frequency) * 1e3
    ));
    match r.law.as_deref() {
        Some(law) => {
            ui.monospace(format!(
                "stiffness    {:>5.0} kN/m per m",
                law.stiffness_per_m / 1e3
            ));
            ui.monospace(format!(
                "damping      {:>5.2} kN.s/m per m",
                law.damping_per_m / 1e3
            ));
        }
        None => {
            ui.monospace("stiffness    -- (deriving)");
        }
    }

    if r.blueprint.is_some() && ui.button("Reset to RON").clicked() {
        *reset_to_ron = true;
    }
}

/// A segmented control row for a [`MeshState`] category — the solid/x-ray/hidden loop as a strip of
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

/// A segmented control row for a [`VolumeState`] category — off / on-top / solid / x-ray.
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

/// The layer groups — Render meshes (multi-state hull, the moving-sim bools), the Volumes (the
/// `*_Collider` / `*_Ballistic` inspection layers), and Debug overlays — over the panel's LOCAL
/// [`VizLayers`] / [`SuspensionViz`] copies. Multi-state categories are segmented controls; the
/// moving-simulation views (running gear, wheels, links, belt line) and the debug overlays stay
/// checkboxes.
fn layers_section(ui: &mut egui::Ui, layers: &mut VizLayers, viz: &mut SuspensionViz) {
    ui.label("Render");
    mesh_state_row(ui, "hull model", &mut layers.hull);
    mesh_state_row(ui, "world", &mut layers.world);
    ui.checkbox(&mut layers.running_gear, "running gear (driven)");
    ui.checkbox(&mut layers.wheels, "wheel meshes");
    ui.checkbox(&mut layers.links, "track links");
    ui.checkbox(&mut layers.belt_line, "belt line");

    ui.separator();
    ui.label("Volumes");
    volume_state_row(ui, "collider proxies", &mut layers.collider_volumes);
    volume_state_row(ui, "ballistic volumes", &mut layers.ballistic_volumes);

    ui.separator();
    ui.label("Debug overlays");
    ui.checkbox(&mut viz.rest_route, "rest route (orange)");
    ui.checkbox(&mut viz.droop_route, "droop route (green)");
    ui.checkbox(&mut viz.compression_route, "compression route (red)");
    ui.checkbox(&mut viz.wheels, "wheel circles");
    ui.checkbox(&mut viz.sprocket, "sprocket tooth ring");
    ui.horizontal(|ui| {
        ui.label("grip sampler");
        if ui.button(viz.grip.label()).clicked() {
            viz.grip = viz.grip.next();
        }
    });
    ui.checkbox(&mut layers.outer, "outer belt line");
    ui.checkbox(&mut layers.hubs, "hub markers");
    ui.checkbox(&mut layers.dots, "contact dots");
    ui.checkbox(&mut layers.normals, "contact normals");
    ui.checkbox(&mut layers.forces, "force arrows");
    ui.checkbox(&mut layers.casts, "cast stations");
    ui.checkbox(&mut layers.reference, "reference loop");
    ui.checkbox(&mut layers.colliders, "physics colliders");
}

/// Live read-out: hull speed, per-track belt speed, and per-side contact station count / load /
/// %-weight / mean slip. Monospace so the columns line up.
fn telemetry_section(
    ui: &mut egui::Ui,
    r: &Read,
    hull: &Query<(&Transform, &LinearVelocity), With<Hull>>,
) {
    match hull.iter().next() {
        Some((tf, lv)) => {
            let fwd = lv.0.dot(tf.forward().into());
            ui.monospace(format!("speed  {fwd:>6.2} m/s  {:>6.1} km/h", fwd * 3.6));
        }
        None => {
            ui.monospace("speed  --");
        }
    }
    ui.monospace(format!("belt L {:>6.2} m/s", r.belt.get(Side::Left)));
    ui.monospace(format!("belt R {:>6.2} m/s", r.belt.get(Side::Right)));

    let weight = r.rig.as_deref().map(|s| s.weight_n);
    ui.separator();
    ui.monospace("side  sta   load   %w  slip");
    for side in Side::ALL {
        let contacts = r.contacts.0.get(side);
        let n = contacts.len();
        let load: f32 = contacts.iter().map(|c| c.load).sum();
        let slip = if n > 0 {
            contacts.iter().map(|c| c.slip.abs()).sum::<f32>() / n as f32
        } else {
            0.0
        };
        let pct = weight.map_or(0.0, |wt| 100.0 * load / wt);
        ui.monospace(format!(
            "{:<5} {n:>2}  {:>5.0}kN {pct:>3.0} {slip:>5.2}",
            format!("{side:?}"),
            load / 1e3,
        ));
    }

    // Track-view cost (µs/frame, cumulative avg for ONE tank — the game pays it per rendered tank).
    ui.separator();
    ui.monospace(format!(
        "view cost (1 tank)  {:>6.0} µs/frame",
        r.view_perf.wrap_us
    ));
}
