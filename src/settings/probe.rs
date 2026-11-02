//! One-shot surface present-mode probe; capability is a surface fact, not an OS guess. Metal
//! reports `[Fifo, Immediate]` and panics if configured with `Mailbox`; Wayland commonly reports
//! `[Fifo, Mailbox]` and refuses `Immediate`; Vulkan/X11 and Windows usually offer all three. A
//! platform table would drift with driver/compositor updates.
//!
//! Bevy's render-world `WindowSurfaces` keeps its fields private. Create a short-lived surface
//! from the primary window's raw handles under `create_surfaces`' unsafe/main-thread contract.
//! The query configures nothing; the surface drops before return.
//!
//! [`ProbeChannel`] shares one `Arc<Mutex<…>>` between worlds: render writes once, main polls
//! until [`PresentCaps::answered`]. No extract-schedule coupling or main-world polling after arrival.
//! Without a render app, caps stays Unprobed: offer everything, promise nothing.
//!
//! Creation failure and an empty capability list mean Unavailable, never a conclusive negative.
//! wgpu 29 returns defaults on a failed query or adapter-incompatible surface; any presentable
//! surface reports at least `Fifo`. Only a nonempty list such as `[Fifo]` can prove both uncapped
//! modes absent. See [`PresentCaps`] for why a false negative would spend a stored player choice.

use std::sync::{Arc, Mutex};

use bevy::ecs::system::NonSendMarker;
use bevy::prelude::*;
use bevy::render::renderer::{RenderAdapter, RenderInstance};
use bevy::render::view::window::ExtractedWindows;
use bevy::render::{Render, RenderApp};

use super::PresentCaps;

/// One-shot render→main hand-off; both worlds hold the same `Arc`, independent of extraction.
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

/// Distil the two ladder gates. Empty means query failure/incompatibility, never evidence
/// for normalization; see the module doc.
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
            // Terminal failure, not evidence for normalization; see [`PresentCaps`].
            warn!("present-mode probe: could not create a probe surface ({err})");
            *channel.0.lock().unwrap() = Some(PresentCaps::Unavailable);
            *done = true;
            return;
        }
    };
    let caps = caps_from_present_modes(&surface.get_capabilities(&adapter).present_modes);
    if caps == PresentCaps::Unavailable {
        // wgpu returns defaults rather than an error; report that capability gating is unavailable.
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
        // What each distilled `PresentCaps` OFFERS is pinned exhaustively by
        // `settings::the_offered_rungs_follow_the_probe`; this test owns only the
        // present-mode-list → `PresentCaps` distillation.

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

        // A typical Vulkan/X11 or Windows surface: everything.
        let vulkan = caps_from_present_modes(&[
            wgpu::PresentMode::Fifo,
            wgpu::PresentMode::FifoRelaxed,
            wgpu::PresentMode::Immediate,
            wgpu::PresentMode::Mailbox,
        ]);
        assert_eq!(
            vulkan,
            PresentCaps::Reported {
                immediate: true,
                mailbox: true,
            },
        );

        // A nonempty list without uncapped modes is conclusive: ON only.
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
        }

        // Unavailable stops polling but claims neither capability. The parent normalization test
        // owns the save-side contract.
        assert!(PresentCaps::Unavailable.answered());
        assert!(!PresentCaps::Unprobed.answered());
        assert!(!PresentCaps::Unavailable.immediate() && !PresentCaps::Unavailable.mailbox());
    }

    /// An empty wgpu list must remain Unavailable through normalization: no rung removed and no
    /// save requested. A real minimal `[Fifo]` report is the contrasting case. See the module doc.
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

        // Exercise the writer from wgpu-shaped input, so the failure cannot become Reported en route.
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
