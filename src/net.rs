//! Shared networking protocol registration (lightyear spike, `net` feature). Mounted by both
//! `spike_server` and `spike_client` — lightyear requires identical protocol registration on
//! both sides of the wire, added after `ServerPlugins`/`ClientPlugins` and before the
//! `Server`/`Client` connection entity is spawned (see the spike map §3 ordering note).

use bevy::prelude::*;
use lightyear::prelude::*;
use serde::{Deserialize, Serialize};

use crate::TankCommand;

/// A trivial replicated marker — step 3 of the spike: proves `.replicate()` registration and
/// ordering before any real game state rides the wire.
#[derive(Component, Clone, Copy, Default, Serialize, Deserialize)]
pub struct SpikeBeacon;

/// Registers everything both sides of the wire must agree on: replicated components and the
/// `TankCommand` input protocol. Grows as later increments add more (§5/§7 of the spike map).
pub fn plugin(app: &mut App) {
    app.component::<SpikeBeacon>().replicate();
    app.add_plugins(input::native::InputPlugin::<TankCommand>::default());
}
