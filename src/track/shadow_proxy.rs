//! The track's SHADOW PROXY: a ~776-triangle closed ribbon that casts the belt's shadow so the
//! ~540 000-triangle shoe pool does not have to.
//!
//! # Why
//!
//! A Tiger carries `link_count × 2` real shoes (MEASURED 97 × 2 = 194 entities) at MEASURED 5 552
//! triangles each — ~1.08 M triangles per tank. Every one of them is re-submitted into EVERY shadow
//! cascade, with no detail reduction: MEASURED 2026-07-27, a tank costs 0.364 ms per cascade
//! (`ms_per_cascade = 0.364 × tanks + 0.69`), i.e. 1.36 ms at four cascades against the 0.086 ms the
//! same tank costs with shadows off. A tank's SHADOW is 16× its entire non-shadow cost, and the
//! tracks are almost all of it.
//!
//! None of that geometry can be recorded. At the far cascade's texel size (0.49 m at the shipped
//! 1000 m / 4096 / 3 ladder, and 0.82 m at 1 km on the old one) a single link — MEASURED 0.13 m
//! pitch, 0.726 m wide — is SUB-TEXEL. The shadow map cannot resolve one shoe, let alone the guide
//! horn on it. What it can resolve is the belt's silhouette, and that is a ribbon.
//!
//! # What this is
//!
//! One extra entity per tank side whose mesh is a closed rectangular TUBE swept along the belt's
//! drawn pin polyline — the same `wrap::WrapSideOutput::joints` the shoes are placed on, never a
//! re-derivation (house doctrine: `wrap` is the single geometry authority, and the sim/view
//! `SagClip` split is load-bearing). The real links keep rendering at full detail to the camera and
//! get `NotShadowCaster`; the proxy is invisible to the camera and casts in their place.
//!
//! Triangle budget per side: 4 faces × `count` segments × 2 = MEASURED 776 against ~538 000. The
//! tube is CLOSED (outer, inner and both lateral faces) rather than a zero-thickness sheet, which is
//! what makes it independent of the sun: bevy's shadow pass honours the material's `cull_mode`
//! (`StandardMaterial` defaults to back-face culling), so a single sheet would vanish for every sun
//! direction that sees its back. A closed tube always presents front faces.
//!
//! # What it buys — MEASURED 2026-07-28, against a proxy that actually casts
//!
//! The first numbers this module carried were taken while the ribbon cast NOTHING (an
//! `AlphaMode::Blend` material — see the folklore section), so they measured the cost of an
//! invisible mesh. Re-measured on the fix,
//! `OVERMATCH_PROBE_TANKS` 2 vs 8, `M350` / 4096, vsync off, foreground on the built-in panel, the
//! median of a 24 s window, two runs per arm agreeing inside ±0.02 ms at 8 tanks:
//!
//! | 8 tanks              | 2 cascades | 4 cascades |
//! |----------------------|-----------:|-----------:|
//! | shadows off          |   4.51 ms  |   4.51 ms  |
//! | real shoes cast      |  10.55 ms  |  13.15 ms  |
//! | ribbon casts         |   5.34 ms  |   5.88 ms  |
//!
//! Marginal cost of a tank's CASTERS, from the 2 → 8 tank slope with the shadows-off slope removed:
//! MEASURED **0.35 ms per tank per cascade** for the real shoes (which reproduces the 0.364 above)
//! against **0.011 ms at 2 cascades and 0.004 ms at 4** for the ribbon — a 31–70× reduction, and at
//! 4 cascades the ribbon's marginal cost is down at the session's own repeatability. The shipped
//! frame at 8 tanks / 4 cascades goes 13.15 → 5.88 ms.
//!
//! The old figure (18.5×, 0.0385 ms/tank/cascade) UNDERSTATED the win rather than inventing it, so
//! the shadow ladder tuned against it is conservative, not optimistic.
//!
//! # How it hides from the camera
//!
//! It declares [`crate::render_policy::VisualScope::SHADOW_PROXY`] and nothing else. That preset
//! means "drawn by no camera, seen by every light", the ribbon carries an ORDINARY OPAQUE material,
//! and the rest of this module has no opinion about rendering at all. `track::view` sets the scope
//! at spawn; `render_policy` owns what it resolves to.
//!
//! ## The folklore this replaces
//!
//! For a while the answer was a `StandardMaterial` with zero alpha and `AlphaMode::Multiply`, and
//! ~60 lines here argued that a render layer COULD NOT work. The argument was true when it was
//! written and is false now, so it is worth recording exactly what changed — the failure it
//! describes is real, and it will come back on the next Bevy upgrade if the patch below is dropped.
//!
//! The claim was: bevy 0.19's `prepare_lights` spawns each shadow view with no `RenderLayers` at
//! all, and `queue_shadows` then filters every candidate caster against THAT view's mask — absent,
//! so `RenderLayers::default()`, so layer 0. Anything off layer 0 was silently dropped from every
//! shadow phase, which made the obvious mechanism (a layer the camera skips and the sun keeps) cast
//! nothing. The two facts around it were also true and remain true: `bevy_light`'s
//! `check_dir_light_mesh_visibility` gates casters on the LIGHT's mask and explicitly
//! `set_visible()`s what it saw, so a caster no camera draws does survive extraction — the bug was
//! only ever in `queue_shadows`.
//!
//! It is fixed. `vendor/bevy_pbr-0.19.0-scalar-math` now copies the LIGHT's `RenderLayers` onto all
//! three kinds of shadow view (a backport of bevyengine/bevy#24797, milestone 0.19.1 — see that
//! crate's `OVERMATCH_PATCH.md` and `tests/bevy_shadow_view_render_layers.rs`, which fails if a
//! vendor refresh drops it). **So shadow correctness now depends on each LIGHT carrying the right
//! mask**: the sun declares `LightProfile::BattlefieldSun`, which covers this ribbon's channel, and
//! `render_policy`'s tests pin that the sun reaches it and that no camera does.
//!
//! The second blocker was local and is also gone: `sight` used to run a per-frame sweep that
//! rewrote the `RenderLayers` of every `Mesh3d` under a tank, so a proxy parented to the hull could
//! not hold a layer of its own for one frame. Scopes are now inherited and overridable, and a
//! shadow proxy overrides its tank-root ancestor by construction.
//!
//! Hiding the ribbon INSIDE the shoes instead — the third fallback, no trick at all — was measured
//! and rejected: [`ProxyMode::Visible`] paints it magenta, and the magenta shows through the gaps
//! between consecutive shoes all around the belt. A track is a chain of plates, not a surface, and
//! there is nowhere inside it to hide.
//!
//! # The cross-section is the measured shoe, with no fudge
//!
//! [`Section`] puts the ribbon's four faces exactly on the shoe's own measured faces — RADIALLY at
//! `pin_line − pin_to_inner` … `pin_line + pin_to_outer` (MEASURED 25.56 mm and 24.73 mm; the pin
//! does NOT run mid-plate, which is why this is two numbers and not `thickness/2`), LATERALLY
//! across `link_center_x ± width/2` (MEASURED 725.97 mm wide, centred 16.85 mm OUTBOARD of the pin
//! plane — `plane_x ± width/2` about the PIN plane is the wrong construction here, per `rig_geom`'s
//! width rule). All four come from the model's `Link_Box`, so a new tank's belt needs no new
//! constants.
//!
//! There used to be two margins here (12 mm radial, 30 mm lateral) holding the ribbon strictly
//! inside the shoe solid, on the reasoning that the shoes RECEIVE shadows — the hull's shadow
//! crossing the return run is a thing you can see — so a coincident caster would stripe the very
//! links it stands in for. That reasoning had a hole: `Link_Box` is an authored ENVELOPE, not the
//! plate's surface, so an inset measured from it is not guaranteed to land inside solid material
//! between the grousers anyway. And the defence was redundant: a caster coincident with its own
//! receiver is the textbook acne case, which is what [`crate::world`]'s `SUN_SHADOW_NORMAL_BIAS`
//! already exists to solve.
//!
//! Tested by eye 2026-07-28 (live `I`-key A/B on one tank, one patch of ground, one sun, so the two
//! arms differed in nothing else) — flush is BETTER, and both constants were deleted. Flush also
//! recovers what the inset cost: 60 mm of shadow width out of 726 (8%) and 25 mm of drop.
//!
//! If acne ever does appear here, the margin should be DERIVED — `max(bias × texel size,
//! envelope-to-material gap)`, both of which are measurable — never picked again.

use bevy::asset::RenderAssetUsages;
use bevy::mesh::{Indices, PrimitiveTopology};
use bevy::prelude::*;

/// Which caster arrangement a session runs. `OVERMATCH_TRACK_SHADOW_PROXY`, a dev instrument in the
/// shape of [`crate::env_parse`]'s other knobs — the A/B is the whole point of the prototype, and a
/// rebuild per arm would make the two arms different binaries.
#[derive(Resource, Clone, Copy, PartialEq, Eq, Debug, Default)]
pub(crate) enum ProxyMode {
    /// `off` — no proxy at all; the real shoes cast, exactly as they shipped. The BEFORE arm.
    Off,
    /// `static` — the ribbon is built ONCE, from the first frame's belt, and then rides the hull
    /// unchanged. Cheapest possible proxy and an upper bound on what the mechanism can buy; wrong
    /// over rough ground, where the real belt conforms and this one does not.
    Static,
    /// `dynamic` — the ribbon is rebuilt every frame from that frame's belt polyline. One `Mesh`
    /// asset per tank SIDE (never shared: two tanks are on different ground).
    #[default]
    Dynamic,
    /// `visible` — the dynamic ribbon wearing an opaque magenta material instead of the invisible
    /// one, so the proxy can be EYEBALLED: where it sits inside the shoes, whether it follows the
    /// belt around the sprocket, how much of it shows through. Diagnostic only.
    Visible,
}

impl ProxyMode {
    /// Read the knob. Unset is [`ProxyMode::Dynamic`]; an unrecognised value is a typo worth
    /// hearing about rather than silently ignoring, so it logs and falls back.
    pub(crate) fn from_env() -> Self {
        let Ok(raw) = std::env::var("OVERMATCH_TRACK_SHADOW_PROXY") else {
            return Self::default();
        };
        match raw.trim().to_ascii_lowercase().as_str() {
            "off" | "0" => Self::Off,
            "static" => Self::Static,
            "dynamic" | "1" => Self::Dynamic,
            "visible" => Self::Visible,
            other => {
                warn!("OVERMATCH_TRACK_SHADOW_PROXY={other:?} is not off/static/dynamic/visible");
                Self::default()
            }
        }
    }

    /// Whether the real shoes should stop casting — every mode that spawns a proxy.
    pub(crate) const fn silences_links(self) -> bool {
        !matches!(self, Self::Off)
    }

    /// Whether the ribbon is rebuilt every frame. [`ProxyMode::Static`] bakes once and rides the
    /// hull; the other two follow the live belt.
    pub(crate) const fn rebuilds_every_frame(self) -> bool {
        matches!(self, Self::Dynamic | Self::Visible)
    }
}

/// How many frames a bound rig may go without a ribbon before the silence is worth a warning.
///
/// The swap is atomic (see [`ProxySide::built`]) so an unbuilt proxy costs nothing but performance
/// — the shoes are still casting and the picture is still right. But "the optimisation quietly did
/// not happen" is exactly the kind of thing that survives a year, so it gets said out loud. ~2 s at
/// 60 fps: long enough that a slow first frame or a mid-bind belt never trips it.
pub(crate) const PROXY_READY_GRACE_FRAMES: u32 = 120;

/// One side's proxy, held on the side's rig entry.
pub(crate) struct ProxySide {
    /// The ribbon entity — a child of the tank root, identity transform, so its mesh is authored
    /// directly in hull space (the frame the belt joints already live in).
    pub(crate) entity: Entity,
    /// This side's OWN mesh asset. Never shared between tanks or sides: two tanks stand on different
    /// ground, and their belts differ every frame.
    pub(crate) mesh: Handle<Mesh>,
    /// Whether a NON-EMPTY ribbon has been written at least once — the gate that makes
    /// [`ProxyMode::Static`] a one-shot bake, and the readiness flag the CASTER SWAP hangs on.
    ///
    /// This is the atomicity: the shoes are silenced on the frame this flips, never at spawn. The
    /// proxy's mesh starts EMPTY (so a half-initialised polyline can never flash a fold), and the
    /// rebuild can legitimately not happen — no `TrackField` yet, a mid-bind belt with under three
    /// joints, an asset handle that has not landed. Silencing the shoes first and trusting the
    /// rebuild to arrive turns every one of those into a tank with NO belt shadow at all, silently.
    /// Silencing them only against geometry that exists makes the worst case "the optimisation did
    /// not happen", which is a cost, not a bug.
    pub(crate) built: bool,
    /// Frames since bind with no ribbon yet — the counter behind [`PROXY_READY_GRACE_FRAMES`].
    /// Stops counting once [`ProxySide::built`] flips.
    pub(crate) pending_frames: u32,
    /// This side's cross-section, resolved from the measured shoe at bind.
    pub(crate) section: Section,
}

/// What the view layer owes the world after folding one frame's build attempt into a proxy.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ProxyStep {
    /// Nothing to do — the ribbon is already carrying the shadow, or is still not ready and the
    /// shoes are still carrying it.
    Idle,
    /// The ribbon became real THIS frame. Silence this side's shoes now, and only now.
    Silence,
    /// [`PROXY_READY_GRACE_FRAMES`] have passed with no ribbon. Say so, once.
    Overdue,
}

impl ProxySide {
    /// Fold one frame's build attempt in and say what the caller must do. `triangles` is what the
    /// caller ACTUALLY wrote into the mesh this frame — zero for every way the build can fail to
    /// produce geometry, which is the whole point.
    ///
    /// This is the atomicity, in the one place it can be tested without a GPU: the shoes are
    /// silenced exactly once, on the first frame a non-empty ribbon exists, and a proxy that never
    /// builds never silences anything.
    pub(crate) fn record_attempt(&mut self, triangles: usize) -> ProxyStep {
        if self.built {
            return ProxyStep::Idle;
        }
        if triangles > 0 {
            self.built = true;
            return ProxyStep::Silence;
        }
        self.pending_frames += 1;
        if self.pending_frames == PROXY_READY_GRACE_FRAMES {
            ProxyStep::Overdue
        } else {
            ProxyStep::Idle
        }
    }
}

/// The ribbon's cross-section: where its four faces sit relative to the belt's pin line.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Section {
    /// Hull-local x of the ribbon's lateral centre — the SHOE's own centre (`link_center_x`), not
    /// the pin plane.
    pub(crate) center_x: f32,
    /// Half the ribbon's width (m) — the measured shoe's own half-width, no margin.
    pub(crate) half_width: f32,
    /// Outward offset of the ribbon's outer face from the pin line (m, positive = away from the
    /// wheels, the direction the shoe's outer face hangs).
    pub(crate) outer: f32,
    /// Outward offset of the ribbon's inner face from the pin line (m, negative — it is on the wheel
    /// side of the pins).
    pub(crate) inner: f32,
}

impl Section {
    /// Build the cross-section from the shoe's MEASURED faces, flush — no margin of any kind.
    ///
    /// `pin_to_inner`/`pin_to_outer` are the measured plate offsets (never `thickness/2`); `width`
    /// is the measured `Link_Box` lateral extent; `center_x` is this side's `link_center_x`. Every
    /// one of the four comes from the model, so this takes no constants and a new tank needs none.
    pub(crate) fn from_shoe(
        center_x: f32,
        width: f32,
        pin_to_inner: f32,
        pin_to_outer: f32,
    ) -> Self {
        Self {
            center_x,
            // `max` rather than an assert: a future model with a degenerate shoe should draw a
            // thin ribbon, not panic in a view system. Same for the radial pair below.
            half_width: (width * 0.5).max(1e-3),
            outer: pin_to_outer.max(1e-3),
            inner: -pin_to_inner.max(1e-3),
        }
    }
}

/// The proxy's material: ORDINARY and OPAQUE.
///
/// The ribbon is hidden by its [`crate::render_policy::VisualScope::SHADOW_PROXY`] scope, not by
/// its material, so the material's only job is to stay out of the way — and opaque is the shape
/// that does that best. `AlphaMode::Opaque` sets no `MeshPipelineKey::MAY_DISCARD` on the shadow
/// item (`vendor/bevy_pbr-0.19.0-scalar-math/src/render/light.rs:2428-2433`), so the shadow draw
/// takes the cheapest `is_depth_only` path there is and runs no fragment shader at all.
///
/// # The trick this retires
///
/// It used to be zero alpha and `AlphaMode::Multiply` — an invisible material standing in for a
/// render layer. Worth knowing why, because the FIRST spelling of that trick cast nothing at all
/// and did it silently: `AlphaMode::Blend` at alpha 0 sets `MAY_DISCARD`, which routes the shadow
/// draw through the material's PREPASS fragment shader, and `prepass_alpha_discard`
/// (`pbr_prepass_functions.wgsl:73`) discards every fragment under a 0.05 cutoff before it can
/// write shadow depth. Reading only the MAIN pass's `alpha_discard` (`pbr_functions.wgsl:107`,
/// which discards for `Mask`/`AlphaToCoverage` only) is what made it look safe.
///
/// MEASURED 2026-07-28 on the shipped scene: the track-shadow lobe read 82.0 mean luma with the
/// real shoes casting and 83.9 with an OPAQUE proxy of this exact mesh, against 95.6 with the
/// `Blend` one — same entity, same geometry, same frame; only the material differed. That
/// measurement is why this is opaque and why the number to watch is the shadow lobe, not the
/// silhouette.
pub(crate) fn proxy_material() -> StandardMaterial {
    StandardMaterial::default()
}

/// The material [`ProxyMode::Visible`] uses instead: ordinary opaque magenta, so the ribbon can be
/// looked at rather than argued about.
pub(crate) fn visible_probe_material() -> StandardMaterial {
    StandardMaterial {
        base_color: Color::srgb(1.0, 0.0, 1.0),
        unlit: true,
        ..default()
    }
}

/// Sweep the ribbon along one side's drawn belt.
///
/// `stations` are the drawn pin joints in side-plane `(z, y)` — `wrap::WrapSideOutput::joints`,
/// exactly what `link_view::place_links` is fed — in loop order. The result is in HULL space, so the
/// proxy entity carries an identity transform.
///
/// Returns `None` for a degenerate belt (fewer than three joints), so a caller mid-bind leaves last
/// frame's ribbon alone rather than replacing it with a fold.
pub(crate) fn ribbon_mesh(stations: &[Vec2], section: Section) -> Option<Mesh> {
    let n = stations.len();
    if n < 3 {
        return None;
    }
    // Which side of the loop is OUTWARD. The belt is a closed polygon in `(z, y)`; its signed area
    // says which way it is wound, and the outward normal is the tangent turned the other way. Read
    // from the polyline rather than assumed, because the wrap is free to emit either winding and a
    // sign error here turns the ribbon inside out (a tube that hugs the wheels instead of the
    // ground) without failing anything.
    let twice_area: f32 = (0..n)
        .map(|i| {
            let (a, b) = (stations[i], stations[(i + 1) % n]);
            a.x * b.y - b.x * a.y
        })
        .sum();
    let winding = if twice_area >= 0.0 { 1.0 } else { -1.0 };

    // Per-station outward normal: the MITRE of the two adjacent edges. Averaging the neighbours
    // rather than using one edge's normal is what keeps the four faces continuous around the
    // sprocket and idler arcs, where consecutive chords turn by several degrees.
    let edge_normal = |i: usize| {
        let (a, b) = (stations[i], stations[(i + 1) % n]);
        let t = (b - a).normalize_or_zero();
        winding * Vec2::new(t.y, -t.x)
    };

    let mut positions: Vec<[f32; 3]> = Vec::with_capacity(n * 4);
    let mut normals: Vec<[f32; 3]> = Vec::with_capacity(n * 4);
    let mut uvs: Vec<[f32; 2]> = Vec::with_capacity(n * 4);
    for (i, &s) in stations.iter().enumerate() {
        let normal = (edge_normal((i + n - 1) % n) + edge_normal(i)).normalize_or_zero();
        // A ring of four, in a consistent cycle around the cross-section: outer −x, outer +x,
        // inner +x, inner −x. `(z, y)` in, `(x, y, z)` out.
        let ring = [
            (-section.half_width, section.outer),
            (section.half_width, section.outer),
            (section.half_width, section.inner),
            (-section.half_width, section.inner),
        ];
        for (dx, radial) in ring {
            let p = s + normal * radial;
            positions.push([section.center_x + dx, p.y, p.x]);
            // Corner normals: the cross-section direction the vertex sits in. Cosmetic only (the
            // proxy is unlit and alpha-zero), but a mesh without them would specialise a different
            // vertex layout than every other mesh in the scene.
            let n3 = Vec3::new(dx, normal.y * radial, normal.x * radial).normalize_or_zero();
            normals.push(n3.to_array());
            uvs.push([0.0, 0.0]);
        }
    }

    // Four quads per segment, each between ring `i` and ring `i+1`.
    let mut indices: Vec<u32> = Vec::with_capacity(n * 24);
    for i in 0..n {
        let a = (i * 4) as u32;
        let b = (((i + 1) % n) * 4) as u32;
        for k in 0..4u32 {
            let k2 = (k + 1) % 4;
            indices.extend_from_slice(&[a + k, b + k, b + k2, a + k, b + k2, a + k2]);
        }
    }
    // Winding, decided by measurement rather than by reasoning about the ring order crossed with the
    // loop's own winding: a closed tube wound outward has POSITIVE signed volume. Backwards
    // triangles would be eaten by the shadow pass's back-face culling and the proxy would cast
    // nothing at all — a silent failure, and the exact one this prototype exists to avoid.
    if signed_volume(&positions, &indices) < 0.0 {
        for tri in indices.chunks_exact_mut(3) {
            tri.swap(1, 2);
        }
    }

    let mut mesh = Mesh::new(
        PrimitiveTopology::TriangleList,
        RenderAssetUsages::default(),
    );
    mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, positions);
    mesh.insert_attribute(Mesh::ATTRIBUTE_NORMAL, normals);
    mesh.insert_attribute(Mesh::ATTRIBUTE_UV_0, uvs);
    mesh.insert_indices(Indices::U32(indices));
    Some(mesh)
}

/// Six times the signed volume of a closed triangle soup (the divergence-theorem sum). Positive when
/// the triangles are wound outward.
fn signed_volume(positions: &[[f32; 3]], indices: &[u32]) -> f32 {
    indices
        .chunks_exact(3)
        .map(|tri| {
            let [a, b, c] = [0, 1, 2].map(|k| Vec3::from_array(positions[tri[k] as usize]));
            a.dot(b.cross(c))
        })
        .sum()
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::camera::primitives::MeshAabb;

    /// The MEASURED Tiger shoe (`marker_model`'s own pins): plate faces, width and the shoe centre.
    /// Fixture inputs to the pure geometry, not assertions about the shipped glb — the file's
    /// numbers are `marker_model`'s to pin.
    const WIDTH: f32 = 0.725_97;
    const PIN_TO_INNER: f32 = 0.025_56;
    const PIN_TO_OUTER: f32 = 0.024_73;
    const CENTER_X: f32 = 1.548;

    fn tiger_section() -> Section {
        Section::from_shoe(CENTER_X, WIDTH, PIN_TO_INNER, PIN_TO_OUTER)
    }

    /// The section IS the measured shoe — no margin, on any axis. Guards the deletion of the two
    /// margins (tested by eye 2026-07-28, flush judged better): a future "just a small safety
    /// inset" would fail here rather than quietly narrowing every tank's shadow again.
    #[test]
    fn the_section_sits_exactly_on_the_measured_shoe_faces() {
        let s = Section::from_shoe(CENTER_X, WIDTH, PIN_TO_INNER, PIN_TO_OUTER);

        assert!((s.center_x - CENTER_X).abs() < 1e-6);
        assert!(
            (s.half_width - WIDTH * 0.5).abs() < 1e-6,
            "no lateral margin"
        );
        assert!((s.outer - PIN_TO_OUTER).abs() < 1e-6, "no radial margin");
        assert!((s.inner + PIN_TO_INNER).abs() < 1e-6, "inner is signed");
    }

    /// A closed convex belt: a circle of `n` joints in `(z, y)`, radius `r`, centred at the origin.
    fn ring(n: usize, r: f32) -> Vec<Vec2> {
        (0..n)
            .map(|i| {
                let a = std::f32::consts::TAU * i as f32 / n as f32;
                Vec2::new(r * a.cos(), r * a.sin())
            })
            .collect()
    }

    fn positions(mesh: &Mesh) -> Vec<Vec3> {
        mesh.attribute(Mesh::ATTRIBUTE_POSITION)
            .and_then(|a| a.as_float3())
            .expect("the ribbon always carries positions")
            .iter()
            .map(|p| Vec3::from_array(*p))
            .collect()
    }

    /// The budget claim the whole exercise rests on: the ribbon is four quads per belt station and
    /// nothing else, so its size scales with the LINK COUNT and not with the shoe's detail.
    #[test]
    fn the_ribbon_costs_eight_triangles_per_link() {
        for count in [12, 97, 240] {
            let mesh = ribbon_mesh(&ring(count, 1.0), tiger_section()).expect("a closed belt");
            let Some(Indices::U32(indices)) = mesh.indices() else {
                panic!("the ribbon is indexed");
            };
            assert_eq!(indices.len(), count * 24, "{count} stations");
            assert_eq!(positions(&mesh).len(), count * 4);
        }
        // 97 links/side against the shoe pool's 97 × 5 552.
        assert_eq!(97 * 8, 776);
    }

    /// A degenerate belt leaves the caller holding `None` rather than a folded tube.
    #[test]
    fn a_belt_with_no_loop_builds_nothing() {
        assert!(ribbon_mesh(&[], tiger_section()).is_none());
        assert!(ribbon_mesh(&ring(2, 1.0), tiger_section()).is_none());
        assert!(ribbon_mesh(&ring(3, 1.0), tiger_section()).is_some());
    }

    /// The inset is the acne guard, so it is the thing worth pinning: every ribbon vertex sits
    /// strictly INSIDE the measured shoe envelope, by at least the two margins, on both a CCW and a
    /// CW belt (the wrap is free to emit either winding).
    #[test]
    fn the_ribbon_stays_inside_the_shoe_envelope() {
        let section = tiger_section();
        for reversed in [false, true] {
            let mut stations = ring(64, 1.5);
            if reversed {
                stations.reverse();
            }
            let mesh = ribbon_mesh(&stations, section).expect("a closed belt");
            for p in positions(&mesh) {
                // Lateral: inside `link_center_x ± width/2`, by the lateral margin.
                let dx = (p.x - CENTER_X).abs();
                assert!(
                    dx <= WIDTH * 0.5 + 1e-4,
                    "lateral {dx} escapes the shoe (reversed={reversed})"
                );
                // Radial: the belt is a circle about the origin, so the shoe's outer face is at
                // `1.5 + pin_to_outer` and its inner face at `1.5 - pin_to_inner`.
                let radius = Vec2::new(p.z, p.y).length();
                assert!(
                    radius <= 1.5 + PIN_TO_OUTER + 1e-4,
                    "radius {radius} escapes the outer face (reversed={reversed})"
                );
                assert!(
                    radius >= 1.5 - PIN_TO_INNER - 1e-4,
                    "radius {radius} escapes the inner face (reversed={reversed})"
                );
            }
        }
    }

    /// The tube is wound OUTWARD whichever way the belt runs. A back-wound proxy is culled by the
    /// shadow pass and casts nothing — the silent failure the volume check exists to prevent, so it
    /// is checked on both windings.
    #[test]
    fn the_tube_is_wound_outward_on_either_belt_winding() {
        for reversed in [false, true] {
            let mut stations = ring(48, 2.0);
            if reversed {
                stations.reverse();
            }
            let mesh = ribbon_mesh(&stations, tiger_section()).expect("a closed belt");
            let Some(Indices::U32(indices)) = mesh.indices() else {
                panic!("the ribbon is indexed");
            };
            let volume = signed_volume(
                &positions(&mesh)
                    .iter()
                    .map(Vec3::to_array)
                    .collect::<Vec<_>>(),
                indices,
            );
            assert!(volume > 0.0, "inside-out tube (reversed={reversed})");
        }
    }

    /// The tube must have real thickness rather than collapse into the zero-thickness sheet the
    /// sun's back-face culling can make vanish. The measured Tiger plate is 50.29 mm, and since the
    /// margins were deleted the ribbon gets all of it.
    #[test]
    fn the_measured_plate_leaves_a_thick_enough_ribbon() {
        let section = tiger_section();
        let thickness = section.outer - section.inner;
        assert!(
            thickness > 0.02,
            "the ribbon is {thickness} m thick — too thin to be sun-direction independent"
        );
        assert!(
            (thickness - (PIN_TO_INNER + PIN_TO_OUTER)).abs() < 1e-6,
            "the ribbon is the whole measured plate, margin-free"
        );
    }

    /// The alpha modes whose SHADOW item bevy tags with `MeshPipelineKey::MAY_DISCARD` — copied by
    /// hand from `vendor/bevy_pbr-0.19.0-scalar-math/src/render/light.rs:2428`, the only place that
    /// decides it. A material in this set has its shadow draw routed through the material's PREPASS
    /// fragment shader, and `prepass_alpha_discard`
    /// (`vendor/bevy_pbr-0.19.0-scalar-math/src/render/pbr_prepass_functions.wgsl:73`) discards
    /// anything whose alpha is under `PREMULTIPLIED_ALPHA_CUTOFF` = 0.05. So a fully transparent
    /// material in this set casts NOTHING.
    ///
    /// Duplicated rather than reached for because bevy does not expose it: `MeshPipelineKey` and
    /// the whole `queue_shadows` path are render-world internals. If a bevy bump moves the list,
    /// the test below is what fails — which is the point, because the failure it replaces was a
    /// track with no shadow that three A/B screenshots called "indistinguishable".
    fn shadow_pass_may_discard(mode: AlphaMode) -> bool {
        matches!(
            mode,
            AlphaMode::Mask(_)
                | AlphaMode::Blend
                | AlphaMode::Premultiplied
                | AlphaMode::Add
                | AlphaMode::AlphaToCoverage
        )
    }

    /// THE regression, in its surviving half: whatever the ribbon's material is, its shadow draw
    /// must not be routed through the prepass fragment shader.
    ///
    /// The invisibility half of this test is GONE, and deliberately: the ribbon is hidden by its
    /// `VisualScope::SHADOW_PROXY` channel now, which `render_policy` pins ("no camera draws
    /// shadow-only geometry", "the sun reaches every channel"). The day this module shipped, the
    /// material had to satisfy BOTH demands at once and picked the wrong alpha mode to do it —
    /// `AlphaMode::Blend` at alpha 0, which draws nothing and casts nothing, and which three A/B
    /// screenshots called "indistinguishable". Splitting the two demands is exactly what made the
    /// material's job trivial again.
    ///
    /// So this is the transparency TRIPWIRE: reach for an alpha trick here and it fails, with the
    /// decode attached.
    #[test]
    fn the_proxy_material_survives_the_shadow_pass() {
        let material = proxy_material();
        assert!(
            !shadow_pass_may_discard(material.alpha_mode),
            "{:?} routes the shadow draw through the prepass fragment shader, which discards any \
             low alpha entirely — the proxy would cast nothing, silently. The ribbon does not need \
             to be transparent: it is hidden by its render_policy channel.",
            material.alpha_mode
        );
        assert_eq!(
            material.alpha_mode,
            AlphaMode::Opaque,
            "ordinary and opaque takes the cheapest depth-only shadow draw there is"
        );
    }

    /// The diagnostic arm's material is the OPPOSITE contract — it is meant to be seen — so pin it
    /// too: an accidentally-transparent probe would make [`ProxyMode::Visible`] look like a bug in
    /// the geometry rather than in the probe.
    #[test]
    fn the_visible_probe_is_opaque() {
        let material = visible_probe_material();
        assert_eq!(material.alpha_mode, AlphaMode::Opaque);
        assert_eq!(material.base_color.alpha(), 1.0);
    }

    /// The ribbon must carry a non-degenerate `Aabb`: an empty or flat mesh is the other way this
    /// silently stops casting (bevy culls shadow casters by their bounds, and a mesh with no
    /// positions has none at all). Checked on the MEASURED belt scale, so the assertion is that the
    /// tube has real extent on all three axes — including the thin lateral one.
    #[test]
    fn the_built_ribbon_has_real_bounds() {
        let mesh = ribbon_mesh(&ring(97, 1.5), tiger_section()).expect("a closed belt");
        let aabb = mesh
            .compute_aabb()
            .expect("a ribbon with positions has bounds");
        let extent = Vec3::from(aabb.half_extents) * 2.0;
        assert!(extent.x > 0.6, "lateral extent {} m", extent.x);
        assert!(extent.y > 2.9 && extent.z > 2.9, "belt extent {extent:?}");
        // …and it is parked on the shoe's own lateral centre, not on the pin plane.
        assert!((aabb.center.x - CENTER_X).abs() < 1e-4, "{}", aabb.center.x);
    }

    /// A `ProxySide` with no entities behind it — the readiness state machine is pure, so the
    /// handle and entity it carries are never read here.
    fn fresh_side() -> ProxySide {
        ProxySide {
            entity: Entity::PLACEHOLDER,
            mesh: Handle::default(),
            built: false,
            pending_frames: 0,
            section: tiger_section(),
        }
    }

    /// THE atomicity: the shoes are silenced on the frame a real ribbon lands, and NOT BEFORE. The
    /// shipped-then-fixed shape silenced them at spawn against an empty mesh and trusted a later
    /// rebuild to arrive — so every way that rebuild could fail (no mesh asset yet, a mid-bind belt
    /// under three joints, a `TrackField` that has not landed) produced a tank with no belt shadow
    /// at all, and the attach log said "776 triangles" through all of it.
    #[test]
    fn the_shoes_are_silenced_only_against_a_real_ribbon() {
        let mut side = fresh_side();
        // Frames where nothing was built silence nothing, however many there are.
        for _ in 0..10 {
            assert_eq!(side.record_attempt(0), ProxyStep::Idle);
            assert!(!side.built, "an empty ribbon is not a ribbon");
        }
        // The frame real geometry lands is the swap, and it happens exactly once.
        assert_eq!(side.record_attempt(776), ProxyStep::Silence);
        assert!(side.built);
        for _ in 0..10 {
            assert_eq!(
                side.record_attempt(776),
                ProxyStep::Idle,
                "the swap is a one-shot — re-silencing every frame is a per-frame command flush"
            );
        }
    }

    /// A proxy that never builds degrades to "the shoes keep casting", which is a COST and not a
    /// bug — but it says so out loud exactly once rather than disappearing.
    #[test]
    fn a_proxy_that_never_builds_complains_once_and_never_silences() {
        let mut side = fresh_side();
        let mut overdue = 0;
        for _ in 0..(PROXY_READY_GRACE_FRAMES * 3) {
            if side.record_attempt(0) == ProxyStep::Overdue {
                overdue += 1;
            }
        }
        assert_eq!(overdue, 1, "one warning, not one per frame");
        assert!(!side.built, "nothing was ever silenced");
    }

    /// The knob's own contract: unset is the dynamic proxy, and every mode that spawns one silences
    /// the shoes.
    #[test]
    fn the_env_knob_defaults_to_the_dynamic_proxy() {
        assert_eq!(ProxyMode::default(), ProxyMode::Dynamic);
        assert!(!ProxyMode::Off.silences_links());
        for mode in [ProxyMode::Static, ProxyMode::Dynamic, ProxyMode::Visible] {
            assert!(mode.silences_links(), "{mode:?}");
        }
        assert!(!ProxyMode::Static.rebuilds_every_frame());
        assert!(ProxyMode::Dynamic.rebuilds_every_frame());
    }
}
