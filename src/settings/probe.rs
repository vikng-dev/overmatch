//! The present-mode capability probe: what the window surface ACTUALLY supports, asked of wgpu
//! once, instead of guessed from the OS name.
//!
//! # Why a probe at all
//!
//! The vsync ladder's uncapped rungs map to `Mailbox` and `Immediate`, and both are per-surface
//! facts, not per-OS ones: wgpu-hal's Metal backend reports `[Fifo, Immediate]` and hits an
//! `unreachable!()` if a surface is configured with `Mailbox`; Wayland compositors report
//! `[Fifo, Mailbox]` and refuse `Immediate`; Vulkan on X11/Windows usually reports all of them.
//! A `cfg(target_os)` table would be a copy of somebody else's driver matrix that goes stale the
//! day a compositor updates — so the rule here is **probe, don't guess**: the settings page offers
//! only rungs the surface reported, and `Settings::present_mode` emits a concrete uncapped mode
//! only once this probe has confirmed it.
//!
//! # How the answer travels
//!
//! `wgpu::Surface::get_capabilities` needs the surface and the adapter, and both live in the
//! RENDER world — bevy's own surface (`bevy_render`'s `WindowSurfaces`) holds them in private
//! fields, so this module creates its own short-lived probe surface from the primary window's raw
//! handle, exactly as `bevy_render::view::window::create_surfaces` does (same `unsafe` contract,
//! same main-thread pinning via `NonSendMarker`). The capability query itself configures nothing —
//! it is read-only against the adapter — and the throwaway surface is dropped before the system
//! returns.
//!
//! The result then crosses to the main world through a shared `Arc<Mutex<…>>` cell
//! ([`ProbeChannel`]) inserted into BOTH worlds at plugin build: the render-world system writes it
//! once, and a main-world system polls until it lands in [`PresentCaps`] (and stops running — its
//! run condition is [`PresentCaps::answered`]). One cell, written once, read until seen: no
//! extract-schedule coupling, no per-frame cost after arrival.
//!
//! Headless roots mount none of this (no render app → the render half is never added), and the
//! main-world [`PresentCaps`] then simply stays [`PresentCaps::Unprobed`], which every consumer
//! treats as "offer everything, promise nothing".
//!
//! # A probe that could not ask answers UNAVAILABLE, not "empty"
//!
//! Surface creation can fail. This module used to report that as a probed-but-empty capability
//! list, which reads to every consumer as the surface positively refusing both uncapped modes —
//! and once `settings::normalize_vsync` existed (a writer that SAVES), that fabricated negative
//! would have eaten a player's stored FAST/OFF for good. The failure path therefore answers
//! [`PresentCaps::Unavailable`], which is the same "nothing is known" state as pre-probe: safe to
//! present from, never evidence of anything. See [`PresentCaps`]'s doc for the full argument.
//!
//! **The capability QUERY can fail the same way, and it fails silently.**
//! `Surface::get_capabilities` returns no `Result` — wgpu 29 turns a failed
//! `surface_get_capabilities` into `SurfaceCapabilities::default()` (an empty `present_modes`
//! vector), and documents the same empty list for a surface the adapter cannot present to. Since
//! any presentable surface reports at least `Fifo`, an empty list is exactly the same
//! "we could not learn" as a failed surface creation, and [`caps_from_present_modes`] maps it to
//! [`PresentCaps::Unavailable`] for exactly that reason. What is left over — a NON-empty list
//! without `Immediate` or `Mailbox`, e.g. `[Fifo]` — is the genuine conclusive negative.

use std::sync::{Arc, Mutex};

use bevy::ecs::system::NonSendMarker;
use bevy::prelude::*;
use bevy::render::renderer::{RenderAdapter, RenderInstance};
use bevy::render::view::window::ExtractedWindows;
use bevy::render::{Render, RenderApp};

use super::PresentCaps;

/// The one-shot render→main hand-off cell. A resource in BOTH worlds (same `Arc`), which is what
/// lets the two halves communicate without touching the extract schedule.
#[derive(Resource, Clone, Default)]
struct ProbeChannel(Arc<Mutex<Option<PresentCaps>>>);

/// Mounted by `settings::plugin` (only — never the headless server, which has no render app and
/// takes the early return).
pub(super) fn plugin(app: &mut App) {
    let channel = ProbeChannel::default();
    app.init_resource::<PresentCaps>()
        .insert_resource(channel.clone())
        .add_systems(
            Update,
            receive_probe.run_if(|caps: Res<PresentCaps>| !caps.answered()),
        );
    let Some(render_app) = app.get_sub_app_mut(RenderApp) else {
        return;
    };
    render_app
        .insert_resource(channel)
        .add_systems(Render, probe_present_modes);
}

/// Distil a wgpu capability list into the two facts the ladder gates on. Pure, so the mapping is
/// testable against fake capability lists.
///
/// **An EMPTY list is a failure, not an answer.** `Surface::get_capabilities` does not return a
/// `Result`: wgpu 29 flattens a failed `surface_get_capabilities` into `SurfaceCapabilities::
/// default()`, whose `present_modes` is an empty `Vec`, and its own docs specify the same empty
/// list for a surface the adapter cannot present to. A surface that CAN be presented to always
/// reports at least `Fifo` (wgpu guarantees it), so "no present modes at all" can only mean the
/// query failed or the surface is incompatible — it is not evidence about `Immediate` or `Mailbox`,
/// and letting it reach [`PresentCaps::Reported`] would hand `settings::normalize_vsync` a
/// fabricated negative to spend a player's stored FAST/OFF on. Same class as the surface-creation
/// failure below, one layer deeper, and answered the same way.
fn caps_from_present_modes(present_modes: &[wgpu::PresentMode]) -> PresentCaps {
    if present_modes.is_empty() {
        return PresentCaps::Unavailable;
    }
    PresentCaps::Reported {
        immediate: present_modes.contains(&wgpu::PresentMode::Immediate),
        mailbox: present_modes.contains(&wgpu::PresentMode::Mailbox),
    }
}

/// Render-world half: create a probe surface for the primary window, read its present modes, write
/// the channel, and never run the body again. Runs (and early-returns) until a primary window has
/// been extracted — frame 1 on every windowed root.
///
/// The `NonSendMarker` is load-bearing twice over: wgpu surface creation must happen on the main
/// thread on some platforms (bevy's own `create_surfaces` pins itself the same way), and it must
/// hold even though this system lives in the render schedule.
fn probe_present_modes(
    _non_send_marker: NonSendMarker,
    mut done: Local<bool>,
    windows: Res<ExtractedWindows>,
    instance: Res<RenderInstance>,
    adapter: Res<RenderAdapter>,
    channel: Res<ProbeChannel>,
) {
    if *done {
        return;
    }
    let Some(window) = windows
        .primary
        .and_then(|entity| windows.windows.get(&entity))
    else {
        return;
    };
    let target = wgpu::SurfaceTargetUnsafe::RawHandle {
        raw_display_handle: Some(window.handle.get_display_handle()),
        raw_window_handle: window.handle.get_window_handle(),
    };
    // SAFETY: the same contract as bevy's `create_surfaces`, on the same handles — the window
    // handles in `ExtractedWindows` are valid objects to create surfaces on, and the marker above
    // keeps this on the main thread where some platforms require it.
    let surface = match unsafe { instance.create_surface_unsafe(target) } {
        Ok(surface) => surface,
        Err(err) => {
            // Answer UNAVAILABLE rather than retrying forever — and specifically NOT an empty
            // capability list, which would be this code inventing a negative answer it does not
            // have. `PresentCaps::Unavailable` keeps every consumer on the same graceful
            // nothing-is-known path as the pre-probe state, and in particular keeps
            // `settings::normalize_vsync` from rewriting (and SAVING) a stored rung on the strength
            // of a failure. See [`PresentCaps`]'s doc for the bug that distinction exists to stop.
            warn!("present-mode probe: could not create a probe surface ({err})");
            *channel.0.lock().unwrap() = Some(PresentCaps::Unavailable);
            *done = true;
            return;
        }
    };
    let caps = caps_from_present_modes(&surface.get_capabilities(&adapter).present_modes);
    if caps == PresentCaps::Unavailable {
        // The only way to hear about a failed capability query: wgpu returns defaults rather than
        // an error. Warned rather than silently swallowed, because it means the ladder will never
        // gate on anything on this machine.
        warn!(
            "present-mode probe: the surface reported no present modes at all — treating that as a \
             failed query rather than as a surface that refuses every mode"
        );
    }
    debug!("present-mode probe: {caps:?}");
    *channel.0.lock().unwrap() = Some(caps);
    *done = true;
    // The probe surface drops here — it was never configured, so there is nothing to unconfigure.
}

/// Main-world half: poll the channel until an answer lands — reported OR unavailable — then stop
/// (the run condition sees [`PresentCaps::answered`] and never schedules this again).
fn receive_probe(channel: Res<ProbeChannel>, mut caps: ResMut<PresentCaps>) {
    let Some(probed) = *channel.0.lock().unwrap() else {
        return;
    };
    *caps = probed;
    info!(
        "settings: present-mode probe — vsync rungs offered: {}",
        super::VsyncMode::ORDER
            .into_iter()
            .filter(|mode| probed.offers(*mode))
            .map(super::VsyncMode::label)
            .collect::<Vec<_>>()
            .join("/"),
    );
}

#[cfg(test)]
mod tests {
    use super::super::{SaveSettings, Settings, VsyncMode, normalize_vsync};
    use super::*;

    /// The capability distillation against the real backend shapes (each list is what the named
    /// backend's `get_capabilities` actually returns), and the rungs each one yields.
    #[test]
    fn fake_capability_lists_gate_the_ladder_correctly() {
        let offered = |caps: PresentCaps| -> Vec<VsyncMode> {
            VsyncMode::ORDER
                .into_iter()
                .filter(|mode| caps.offers(*mode))
                .collect()
        };

        // Metal (macOS >= 10.13): Fifo + Immediate, no Mailbox — FAST must not be offered.
        let metal =
            caps_from_present_modes(&[wgpu::PresentMode::Fifo, wgpu::PresentMode::Immediate]);
        assert_eq!(
            metal,
            PresentCaps::Reported {
                immediate: true,
                mailbox: false,
            },
        );
        assert_eq!(offered(metal), vec![VsyncMode::Off, VsyncMode::On]);

        // Wayland: Fifo + Mailbox, Immediate refused — OFF must not be offered.
        let wayland =
            caps_from_present_modes(&[wgpu::PresentMode::Fifo, wgpu::PresentMode::Mailbox]);
        assert_eq!(
            wayland,
            PresentCaps::Reported {
                immediate: false,
                mailbox: true,
            },
        );
        assert_eq!(offered(wayland), vec![VsyncMode::Fast, VsyncMode::On]);

        // A typical Vulkan/X11 or Windows surface: everything.
        let vulkan = caps_from_present_modes(&[
            wgpu::PresentMode::Fifo,
            wgpu::PresentMode::FifoRelaxed,
            wgpu::PresentMode::Immediate,
            wgpu::PresentMode::Mailbox,
        ]);
        assert_eq!(offered(vulkan), VsyncMode::ORDER.to_vec());

        // A surface that reports a list with NEITHER uncapped mode in it — `[Fifo]`, or Fifo plus
        // FifoRelaxed. This is the genuine conclusive negative: the surface answered, and the
        // answer is "vsync ON only".
        for list in [
            vec![wgpu::PresentMode::Fifo],
            vec![wgpu::PresentMode::Fifo, wgpu::PresentMode::FifoRelaxed],
        ] {
            let neither = caps_from_present_modes(&list);
            assert_eq!(
                neither,
                PresentCaps::Reported {
                    immediate: false,
                    mailbox: false,
                },
                "{list:?} is a real answer, and it conclusively lacks both uncapped modes",
            );
            assert_eq!(offered(neither), vec![VsyncMode::On]);
        }

        // A FAILED probe is a different thing entirely, and this is the whole point of the
        // tri-state: it answers the poller so the probe stops, and it offers everything because it
        // knows nothing. `settings::a_failed_probe_never_rewrites_the_stored_rung` pins the writer
        // half.
        assert!(PresentCaps::Unavailable.answered());
        assert!(!PresentCaps::Unprobed.answered());
        assert_eq!(offered(PresentCaps::Unavailable), VsyncMode::ORDER.to_vec());
        assert!(!PresentCaps::Unavailable.immediate() && !PresentCaps::Unavailable.mailbox());
    }

    /// **The silent half of the failure story.**
    ///
    /// `Surface::get_capabilities` cannot report an error — wgpu 29 flattens a failed
    /// `surface_get_capabilities` into `SurfaceCapabilities::default()`, i.e. an EMPTY
    /// `present_modes`, and documents the same empty list for an adapter-incompatible surface. A
    /// presentable surface always reports at least `Fifo`, so an empty list is a failed query, not
    /// a surface refusing everything — and mapping it to `Reported` would hand
    /// `settings::normalize_vsync` the fabricated negative all over again, one layer below the
    /// surface-creation failure that was fixed first.
    #[test]
    fn an_empty_capability_list_is_a_failed_query_not_an_answer() {
        assert_eq!(
            caps_from_present_modes(&[]),
            PresentCaps::Unavailable,
            "an empty present-mode list is wgpu's failure shape, and must never be evidence",
        );
        // Which means nothing is gated away and nothing resolves: the identity, exactly as
        // pre-probe.
        for mode in VsyncMode::ORDER {
            assert!(caps_from_present_modes(&[]).offers(mode));
            assert_eq!(caps_from_present_modes(&[]).resolve(mode), mode);
        }
        // The one list that looks similar and is NOT this: a real, minimal answer.
        assert_ne!(
            caps_from_present_modes(&[wgpu::PresentMode::Fifo]),
            PresentCaps::Unavailable,
        );

        // End to end, from the wgpu-shaped input to the WRITER: an empty list must move no stored
        // rung and write no file. (`Settings`/`normalize_vsync` are the parent module's private
        // items, which this child module may reach — the point of testing it from here is that the
        // empty list never even becomes a `PresentCaps::Reported` on the way.)
        for vsync in VsyncMode::ORDER {
            let mut app = App::new();
            app.add_message::<SaveSettings>()
                .insert_resource(Settings { vsync, ..default() })
                .insert_resource(caps_from_present_modes(&[]))
                .add_systems(Update, normalize_vsync);
            app.update();
            assert_eq!(
                app.world().resource::<Settings>().vsync,
                vsync,
                "an empty capability list must not rewrite {vsync:?}",
            );
            assert_eq!(
                app.world()
                    .resource::<bevy::ecs::message::Messages<SaveSettings>>()
                    .len(),
                0,
                "an empty capability list must not write the file ({vsync:?})",
            );
        }
    }
}
