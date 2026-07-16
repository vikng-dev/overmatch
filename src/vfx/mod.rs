//! View-layer combat VFX.
//!
//! Invariant (ADR-0014): these systems subscribe to simulation seams but write no simulation state;
//! randomness is view-local. Windowed client compositions mount this module, not the server.

use bevy::prelude::*;

pub(crate) mod billboard;
mod ember;
mod impact;
mod muzzle;
mod prewarm;
mod tracer;
mod trail;

#[cfg(test)]
pub(crate) use trail::{TrailHarnessSet, TrailStationMeshEvidence};

#[cfg(test)]
pub(crate) fn mount_trail_loss_harness(app: &mut App) {
    trail::mount_loss_harness(app);
}

pub fn plugin(app: &mut App) {
    app.init_resource::<ViewRng>();
    app.add_plugins((
        billboard::plugin,
        impact::plugin,
        muzzle::plugin,
        trail::plugin,
        ember::plugin,
        tracer::plugin,
    ));
    // The prewarm rig wants the OTHER modules' preloaded assets (it warms the exact mesh/material
    // combinations they will draw), so its spawn orders after their Startup setups.
    app.add_systems(
        Startup,
        prewarm::spawn_prewarm_rig
            .after(muzzle::setup_muzzle_assets)
            .after(trail::setup_trail_assets)
            .after(impact::setup_impact_assets)
            .after(ember::setup_ember_assets),
    );
    app.add_systems(
        Update,
        (prewarm::tag_prewarm_meshes, prewarm::expire_prewarm),
    );
    // Dev-only guard (§5a): confirm every vfx texture path actually resolves, so a renamed/missing
    // atlas surfaces as a loud error instead of silently regressing to blank/again-square sprites.
    #[cfg(debug_assertions)]
    app.add_systems(Update, verify_vfx_assets);
}

/// Dev-time asset-load guard: each frame until every vfx texture has settled, check its load state
/// and `error!` any that FAILED (a bad path, a missing file, a renamed atlas). Cheap — the paths are
/// already loaded by the effect setups, so `asset_server.load` here just returns the existing handle
/// (the server dedupes), and the whole system idles off once everything is settled. `debug_assertions`
/// only, so shipped clients never pay for it.
#[cfg(debug_assertions)]
fn verify_vfx_assets(asset_server: Res<AssetServer>, mut done: Local<bool>) {
    use bevy::asset::LoadState;

    if *done {
        return;
    }
    /// Every texture the vfx layer draws (the LUTs/noise are procedural, so not here).
    const VFX_TEXTURES: &[&str] = &[
        "vfx/flash_core_atlas.png",
        "vfx/mg_core.png",
        "vfx/flash_flames_atlas.png",
        "vfx/smoke_atlas.png",
        "vfx/impact_dust.png",
        "vfx/spark_atlas.png",
    ];
    let mut all_settled = true;
    for path in VFX_TEXTURES {
        let handle: Handle<Image> = asset_server.load(*path);
        match asset_server.load_state(&handle) {
            LoadState::Failed(err) => {
                error!("vfx asset failed to load: {path}: {err}");
            }
            LoadState::Loaded => {}
            // NotLoaded / Loading: come back next frame.
            _ => all_settled = false,
        }
    }
    if all_settled {
        *done = true;
    }
}

/// The splitmix64 finalizer's two multiply-xorshift rounds — the bit-mixing core shared by
/// [`ViewRng::next_f32`] and the trail's value-noise lattice ([`trail::noise_texture`]). It is
/// deliberately *just* the two rounds: `ViewRng` follows it with a final `z ^= z >> 31`, the noise
/// lattice does not, so folding that step in here would change one output or the other. Cosmetic,
/// view-side only (never the sim's determinism domain).
pub(super) fn mix64(z: u64) -> u64 {
    let z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB)
}

/// View-layer RNG for cosmetic variation (sprite roll, scale jitter, flipbook start frames). A tiny
/// splitmix64 — deliberately NOT the sim's determinism domain: it is seeded from wall-clock entropy,
/// lives only in the windowed clients, and nothing the sim hashes or replicates ever reads it.
/// Tests construct it with a fixed seed for reproducibility.
#[derive(Resource)]
pub(crate) struct ViewRng(u64);

impl Default for ViewRng {
    fn default() -> Self {
        // Wall-clock nanos are plenty for cosmetic variation; view-side only (see the type doc).
        let seed = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos() as u64)
            .unwrap_or(0x9E37_79B9_7F4A_7C15);
        Self::seeded(seed)
    }
}

impl ViewRng {
    pub(crate) fn seeded(seed: u64) -> Self {
        Self(seed)
    }

    /// Next uniform sample in `[0, 1)` (splitmix64, top 24 bits).
    pub(crate) fn next_f32(&mut self) -> f32 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = mix64(self.0);
        z ^= z >> 31;
        (z >> 40) as f32 / (1u64 << 24) as f32
    }

    /// Next uniform sample in `[lo, hi)`.
    pub(crate) fn range(&mut self, lo: f32, hi: f32) -> f32 {
        lo + (hi - lo) * self.next_f32()
    }
}
