//! An unattended run: one nobody sits at, on a machine its owner may be working at. It is silent
//! for its whole length, and a focus loss never pauses it (`state::FocusPauseSuppressed`), so a
//! scripted leg never stops halfway. `OVERMATCH_UNATTENDED=1` asks for one on a window somebody can
//! see (a visible frame-cost measurement); a capture plan's frames imply one (`dev_capture`).
//!
//! The silence is written in `PreStartup`, after every plugin has built (`AudioPlugin` inserts its
//! own `GlobalVolume` at build) and before any audio sink exists (bevy reads the global volume when
//! it creates a sink), so no mount order lets an unattended run make a sound. There is no player
//! volume setting for it to override.

use bevy::audio::{GlobalVolume, Volume};
use bevy::prelude::*;

/// The variable that asks for an unattended run.
const VARIABLE: &str = "OVERMATCH_UNATTENDED";

/// Mounted by every windowed root (`crate::windowed_runtime`); inert unless [`VARIABLE`] is on.
pub(crate) fn plugin(app: &mut App) {
    if crate::env_flag(VARIABLE, false) {
        info!("unattended: {VARIABLE} — silent, a focus loss never pauses");
        mount(app);
    }
}

/// Make this run unattended (module doc).
pub(crate) fn mount(app: &mut App) {
    app.add_systems(PreStartup, |mut commands: Commands| {
        commands.insert_resource(GlobalVolume::new(Volume::Linear(0.0)));
    });
    app.insert_resource(crate::state::FocusPauseSuppressed);
}

#[cfg(test)]
mod tests {
    use super::*;

    /// AN UNATTENDED RUN IS SILENT WHATEVER MOUNTS AFTER IT, AND A FOCUS LOSS NEVER PAUSES IT: the
    /// perf scripts and capture runs rely on it on a machine its owner is working at. `AudioPlugin`
    /// inserts its own `GlobalVolume` when it builds, so a silence written at build would be undone
    /// by an audio plugin mounted later.
    #[test]
    fn an_unattended_run_is_silent_whatever_mounts_after_it_and_never_paused() {
        let mut app = App::new();
        mount(&mut app);
        // What a later `AudioPlugin` build writes.
        app.insert_resource(GlobalVolume::new(Volume::Linear(1.0)));
        app.update();
        assert_eq!(
            app.world().resource::<GlobalVolume>().volume.to_linear(),
            0.0
        );
        assert!(
            app.world()
                .contains_resource::<crate::state::FocusPauseSuppressed>()
        );
    }
}
