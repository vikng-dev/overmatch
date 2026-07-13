//! Driving systems and data. The fixed-step schedule lives here; implementation lives in focused
//! submodules.

mod contact;
mod susp_trace;
mod suspension;
mod traction;

pub use contact::{SPHERE_CAST_TOI_SLACK, sphere_cast_ground_contact};
pub use suspension::{Suspension, SuspensionParams, SuspensionProbe};
pub use traction::{DriveState, Drivetrain};

use bevy::prelude::*;

use crate::state::GameplaySet;
use suspension::{apply_suspension, log_suspension_probe};
use traction::{apply_drive, ramp_drive};

pub fn plugin(app: &mut App) {
    // The body's centre of mass needs no system here: complete tank construction inserts
    // `CenterOfMass` from the authored `Center_Of_Mass` empty's extracted position at spawn
    // (the model owns the COM; `NoAutoCenterOfMass` keeps the collision proxies' centroid from
    // diluting it — ADR-0011).
    app.insert_resource(SuspensionProbe::from_env())
        .add_systems(Startup, log_suspension_probe)
        // Order matters within the fixed step: ramp the command into the drive signal, settle
        // springs (sets per-wheel load), then drive (reads that load for the friction circle).
        // All gated by the gameplay set.
        .add_systems(
            FixedUpdate,
            (ramp_drive, apply_suspension, apply_drive)
                .chain()
                .in_set(GameplaySet),
        );
}
