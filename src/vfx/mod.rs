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
mod scatter_hit;
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
        // The scatter-crossing read: rides `impact`'s terrain effects, so it mounts after it.
        scatter_hit::plugin,
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
        "vfx/blast_core_a.ktx2",
        "vfx/blast_core_b.ktx2",
        "vfx/blast_core_c.ktx2",
        "vfx/blast_core_d.ktx2",
        "vfx/blast_core_e.ktx2",
        "vfx/blast_core_f.ktx2",
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

#[cfg(test)]
mod gpu_layout_tests {
    use std::mem::size_of;

    use bevy::math::Vec4;
    use bevy::render::render_resource::ShaderType;

    use super::{billboard::VfxParams, trail::TrailParams};

    /// A `Vec4` lane, both sides of the wire.
    const LANE_BYTES: usize = 16;
    const LANE_WGSL: &str = "vec4<f32>";

    /// The field names of a derived-`Debug` struct, in DECLARATION order. Neither uniform type is
    /// `Reflect`, and `Debug` is generated from the same field list the uniform is packed from, so
    /// it is the field order — reordering, adding or removing one moves this list.
    fn rust_lanes<T: std::fmt::Debug>(value: &T) -> Vec<String> {
        let text = format!("{value:?}");
        let body = text
            .split_once('{')
            .and_then(|(_, rest)| rest.rsplit_once('}'))
            .expect("a derived struct Debug")
            .0;
        let mut lanes = Vec::new();
        let mut token = String::new();
        let mut depth = 0usize;
        for ch in body.chars() {
            match ch {
                '(' | '[' | '{' => {
                    depth += 1;
                    token.clear();
                }
                ')' | ']' | '}' => {
                    depth = depth.saturating_sub(1);
                    token.clear();
                }
                ':' if depth == 0 => lanes.push(std::mem::take(&mut token).trim().to_owned()),
                ',' if depth == 0 => token.clear(),
                _ => token.push(ch),
            }
        }
        lanes
    }

    /// The `(name, type)` of every field of `struct <name>` in a WGSL source, in declaration order.
    fn wgsl_lanes(source: &str, name: &str) -> Vec<(String, String)> {
        let block = source
            .split_once(&format!("struct {name} {{"))
            .and_then(|(_, rest)| rest.split_once('}'))
            .unwrap_or_else(|| panic!("no `struct {name}` block in the shader"))
            .0;
        block
            .lines()
            .map(|line| line.split("//").next().unwrap_or_default().trim())
            .filter(|line| !line.is_empty())
            .map(|line| {
                let (field, ty) = line
                    .trim_end_matches(',')
                    .split_once(':')
                    .unwrap_or_else(|| panic!("`{line}` is not a `field: type` declaration"));
                (field.trim().to_owned(), ty.trim().to_owned())
            })
            .collect()
    }

    fn shader(file: &str) -> String {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("assets/shaders")
            .join(file);
        std::fs::read_to_string(&path).unwrap_or_else(|err| panic!("{}: {err}", path.display()))
    }

    /// The uniform-layout LAW: each vfx uniform's WGSL block and its Rust struct declare the same
    /// lanes, in the same order, all of them `vec4`, and the Rust size is that lane count. The two
    /// halves are uploaded through Encase against a WGSL block nothing else validates, so a field
    /// added, dropped or reordered on one side alone silently reinterprets every lane after it.
    #[test]
    fn project_vfx_uniforms_agree_with_their_shaders() {
        // Written as LITERALS, so a lane added to either struct is a compile error here before it
        // is a mismatch below.
        let billboard = VfxParams {
            frame: Vec4::ZERO,
            fade: Vec4::ZERO,
            glow: Vec4::ZERO,
        };
        let trail = TrailParams {
            shape: Vec4::ZERO,
            glow: Vec4::ZERO,
        };
        for (rust, wgsl, file, size) in [
            (
                rust_lanes(&billboard),
                wgsl_lanes(&shader("vfx_billboard.wgsl"), "VfxParams"),
                "vfx_billboard.wgsl",
                (size_of::<VfxParams>(), VfxParams::min_size().get() as usize),
            ),
            (
                rust_lanes(&trail),
                wgsl_lanes(&shader("vfx_trail.wgsl"), "TrailParams"),
                "vfx_trail.wgsl",
                (
                    size_of::<TrailParams>(),
                    TrailParams::min_size().get() as usize,
                ),
            ),
        ] {
            let names: Vec<String> = wgsl.iter().map(|(name, _)| name.clone()).collect();
            assert_eq!(rust, names, "{file}: lane names/order must match Rust");
            for (name, ty) in &wgsl {
                assert_eq!(ty, LANE_WGSL, "{file}: lane `{name}` must stay a vec4");
            }
            assert_eq!(
                size,
                (rust.len() * LANE_BYTES, rust.len() * LANE_BYTES),
                "{file}: the Rust struct must be exactly its {} lanes",
                rust.len()
            );
        }
    }
}
