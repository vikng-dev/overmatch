//! Ship-facing view-layer combat VFX (ADR-0014): render-only subscribers to the sim's seams —
//! `ballistics::Impact` (dust puffs) and `ballistics::FireShell` (the 88's muzzle dressing + smoke
//! trail) — plus the shared machinery they are built from. Mounted ONLY by the two windowed client
//! compositions (SP `ClientPlugin` and `NetClientPlugin`); the headless server and the scripted
//! harness never mount any of it. Nothing here writes sim state, and all randomness is view-side
//! ([`ViewRng`]) — the deterministic sim never sees it.
//!
//! Layout:
//!   * [`billboard`] — the shared sprite machinery: camera-facing flipbook quads with an
//!     erosion + gradient-map material (`VfxBillboardMaterial`), aging/ring-cap systems, and the
//!     procedural gradient-LUT builder. Both the 88 dressing here and the MG dressing (slice B)
//!     consume it.
//!   * [`muzzle`] — the 88's firing signature: 1–2-frame flash cluster, transient shadowless
//!     muzzle light, lingering eroded smoke puff.
//!   * [`trail`] — the 88 shell's smoke trail: one camera-facing ribbon per shell, built from the
//!     sim's `ShellPath` recording, noise-eroded at the tail (`VfxTrailMaterial`).
//!   * [`impact`] — the impact dust puffs (every landed round reads at the target).
//!   * [`prewarm`] — startup warm-up spawns that force the shell scene and every VFX
//!     mesh/material pipeline permutation to compile before the first shot, killing the measured
//!     first-fire hitch.

use bevy::prelude::*;

pub(crate) mod billboard;
mod impact;
mod muzzle;
mod prewarm;
mod trail;

pub fn plugin(app: &mut App) {
    app.init_resource::<ViewRng>();
    app.add_plugins((
        billboard::plugin,
        impact::plugin,
        muzzle::plugin,
        trail::plugin,
    ));
    // The prewarm rig wants the OTHER modules' preloaded assets (it warms the exact mesh/material
    // combinations they will draw), so its spawn orders after their Startup setups.
    app.add_systems(
        Startup,
        prewarm::spawn_prewarm_rig
            .after(muzzle::setup_muzzle_assets)
            .after(trail::setup_trail_assets)
            .after(impact::setup_puff_assets),
    );
    app.add_systems(
        Update,
        (prewarm::tag_prewarm_meshes, prewarm::expire_prewarm),
    );
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
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^= z >> 31;
        (z >> 40) as f32 / (1u64 << 24) as f32
    }

    /// Next uniform sample in `[lo, hi)`.
    pub(crate) fn range(&mut self, lo: f32, hi: f32) -> f32 {
        lo + (hi - lo) * self.next_f32()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The view RNG stays in-range and actually varies — all it owes anyone (it is cosmetic,
    /// view-side, and deliberately outside the sim's determinism domain).
    #[test]
    fn view_rng_is_uniformish_and_in_range() {
        let mut rng = ViewRng::seeded(7);
        let mut min = f32::MAX;
        let mut max = f32::MIN;
        for _ in 0..1000 {
            let x = rng.next_f32();
            assert!((0.0..1.0).contains(&x), "sample out of [0,1): {x}");
            min = min.min(x);
            max = max.max(x);
        }
        assert!(
            min < 0.1 && max > 0.9,
            "1000 samples should span [0,1): min {min}, max {max}"
        );
        let r = rng.range(3.0, 5.0);
        assert!((3.0..5.0).contains(&r));
    }
}
