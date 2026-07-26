//! The marker-driven track model: reads the sharp sources of truth out of the glb (the `Pin_*`
//! marker empties, the `Link_Box` shoe volume, the `Sprocket_*` / `Idler_*` rig meshes, and the
//! road-wheel mesh radii) and derives the geometry its consumers render and simulate
//! ([`super::rig_geom`] assembles it into the sandbox's rig; [`crate::track_sandbox::suspension_viz`] draws it).
//! This is the "new model" — nothing here reads the RON's geometry fields (pitch, thickness,
//! plane_x, width, sprocket/idler centre, wheel_radius); those all come from the model now. Only
//! `mass` still comes from the spec, and it is passed IN as a scalar so this module cannot reach
//! the RON at all; `link_count` and `teeth` are caller knobs (see [`super::rig_geom::RigGeom`]).
//!
//! **No fallback.** [`DerivedModel::build`] either measures the glb or aborts — see [`refuse`].
//! A substitute path (rebuilding RON geometry when a lookup fails) means a re-export that deletes
//! marker nodes leaves the tool quietly measuring a DIFFERENT tank, with the only trace a substring
//! in one log line. A missing marker is fatal by policy: declared correctly, or fail loud.
//!
//! It reads the glb DIRECTLY via the `gltf` crate, composing the FULL `T·R·S` world transform down
//! the node tree. The bake's `root_position` is not usable here: it omits scale, and the markers sit
//! under scaled ancestors (`Link` carries 0.808, the wheels 4.34), so a position taken without the
//! scale chain is simply the wrong place. What that composed transform yields is both datums this
//! module needs — a node's authored ORIGIN (the axle of every body of revolution: sprocket, idler,
//! road wheel) and its vertices in world space (which answer sizes: radii, the shoe's faces). When
//! the model graduates into the game this measurement moves into the bake.
//!
//! **The width rule.** The geometric model is purely 2-D in the SIDE PLANE `(z, y)` — circles,
//! routes, droop, perimeter, the link window all ignore the lateral axis. Width enters in exactly
//! two places: the grip columns ([`super::rig_geom::RigGeom::grip_columns`]) and rendering. That is
//! why the lateral datums below are carried as raw `x` values rather than folded into the side-plane
//! geometry, and why [`DerivedModel::pitch`] is measured with the lateral component projected out.
//!
//! **Robustness.** Every SIZE here is computed on a vertex cloud an artist owns, so it is written to
//! survive loose greebles: the rim radius is a quantile-anchored maximum rather than a raw `max`,
//! which one stray vertex destroys. Every CENTRE, by contrast, is not computed at all — it is the
//! node's authored origin, a fact no stray vertex can move. See [`measure_mesh`].

use std::collections::HashMap;
use std::fmt;
use std::path::Path;

use bevy::math::{Mat4, Quat, Vec2, Vec3};
use bevy::prelude::{Resource, warn};

/// The pin markers: the two ends of ONE link, the loop's immutable pitch.
const PIN_START: &str = "Pin_Start";
const PIN_END: &str = "Pin_End";
/// The link's collision box — the shoe's own volume, and since the 2026-07 re-export the source of
/// every lateral/radial datum (it replaced the deleted `Inner_Surface`/`Outer_Surface` empties).
/// Authored so its COMPOSED world scale is exactly 1: its extents read directly in metres.
const LINK_BOX: &str = "Link_Box";
/// The hull-fixed running gear, right side (the sides are mirror images in the side plane).
const SPROCKET: &str = "Sprocket_R";
const IDLER: &str = "Idler_R";
/// The road-wheel station the tread radius is read from. Since the re-export the wheels are single
/// unsuffixed nodes (the `_Ballistic`/`_Visual` split is gone) carrying a non-unit local scale and a
/// 180° Y rotation, and each node is a full TWO-DISC station — all of which [`GlbNodes::read`]'s
/// full `T·R·S` composition and [`measure_mesh`]'s origin-centred radius handle. Station 0 is an
/// EVEN station (0/2/4/6 measure 0.3870, the odd ones 0.3885 — the interleaved pack alternates).
const WHEEL: &str = "Wheel_R_0";

/// THE CONTRACT with the export, in the two flavours [`GlbNodes::read`] resolves. Kept as lists
/// (rather than spelled out at the call) so the read and the failure message can never drift: what
/// [`DerivedModel::from_markers`] asks for is literally what [`refuse`] tells the artist to restore.
const REQUIRED_EMPTIES: [&str; 2] = [PIN_START, PIN_END];
const REQUIRED_MESHES: [&str; 4] = [LINK_BOX, SPROCKET, IDLER, WHEEL];

/// Plausibility band for a road-wheel/idler contact radius (m). Not a tuning knob — a tripwire for
/// the failure mode this file is most exposed to: a re-export that changes a node's scale or its
/// mesh content still resolves every lookup, so it cannot be caught by a missing-node check.
const PLAUSIBLE_RADIUS: (f32, f32) = (0.10, 1.00);
/// Plausibility band for the link pitch (m) — a tank shoe, not a bicycle chain or a bridge span.
const PLAUSIBLE_PITCH: (f32, f32) = (0.03, 0.60);
/// Plausibility band for the shoe width (m).
const PLAUSIBLE_WIDTH: (f32, f32) = (0.10, 1.50);

/// Everything the derived geometry needs, read out of the model (all lengths in metres, side-plane
/// centres as `(z, y)` in the glTF frame the blueprint's wheel positions also use).
///
/// Lateral (`x`) fields are stored for the RIGHT side, i.e. positive; the left side is the exact
/// mirror (`−x`). Consumers that need a side apply [`crate::track::side::Side::sign`] — see
/// [`super::rig_geom::RigGeom::grip_columns`].
#[derive(Resource, Clone, Copy)]
pub struct DerivedModel {
    /// Link pitch = the pin-to-pin span measured IN THE SIDE PLANE (see [`Self::plane_x`] for why
    /// the lateral component is dropped).
    pub pitch: f32,
    /// Measured pin-centre → inner-face offset (stands the track off the wheels/sprocket), from
    /// `Link_Box`'s `+y` radial face.
    pub pin_to_inner: f32,
    /// Measured pin-centre → outer-face offset (the ground-contact side), from `Link_Box`'s `−y`
    /// radial face. `−y` is OUTER in the model's frame: the shoe hangs below the pin line.
    pub pin_to_outer: f32,
    /// The track's lateral median plane = |x| of the PIN markers. This is the plane the 2-D route
    /// lives in, and it is NOT where the shoe is centred — see [`Self::link_center_x`].
    pub plane_x: f32,
    /// Shoe width (m), lateral = `Link_Box`'s lateral extent. Measured, not authored: the RON's
    /// 0.79 was wrong by 64 mm, and nothing reads `spec.track.width` any more.
    pub width: f32,
    /// Inboard lateral face of the shoe (m, right side) — `Link_Box`'s minimum `x`.
    pub lateral_min: f32,
    /// Outboard lateral face of the shoe (m, right side) — `Link_Box`'s maximum `x`.
    pub lateral_max: f32,
    /// The shoe's OWN lateral centre (m, right side) — the datum to RENDER the link at.
    ///
    /// Distinct from [`Self::plane_x`] on purpose: the Tiger's shoe is authored ~16.8 mm OUTBOARD of
    /// the pin plane (flush with the wheel pack inboard, overhanging outboard), so it is not
    /// laterally symmetric about the pins. `plane_x ± width/2` is therefore the WRONG construction
    /// for anything lateral — use [`Self::lateral_min`]/[`Self::lateral_max`], which carry the real
    /// asymmetry, and this for the centre.
    pub link_center_x: f32,
    /// Sprocket centre, side-plane `(z, y)`, from the `Sprocket_R` mesh.
    pub sprocket_center: Vec2,
    /// Idler centre, side-plane `(z, y)`, from the `Idler_R` mesh.
    pub idler_center: Vec2,
    /// Idler rim (track-contact) radius, from the `Idler_R` mesh.
    pub idler_radius: f32,
    /// Road-wheel tread radius, from the [`WHEEL`] station.
    pub wheel_tread: f32,
    /// Hull mass (RON) — for the spring-rate readout.
    pub mass: f32,
}

impl DerivedModel {
    /// Derive from the model markers/nodes, or ABORT. `mass` (the one non-geometry input) is passed
    /// in rather than read here, so this module never touches the RON.
    ///
    /// There is deliberately no `Result` and no substitute geometry: see [`refuse`] for why a dev
    /// tool answers a broken export with a panic.
    pub fn build(mass: f32, glb_path: &Path) -> Self {
        let model = Self::from_markers(mass, glb_path).unwrap_or_else(|gap| refuse(&gap, glb_path));
        model.warn_if_implausible();
        model
    }

    /// The marker read proper — fallible, so the tests can drive it and inspect the gap without
    /// unwinding, and so [`Self::build`] has exactly one place that decides what a gap MEANS.
    fn from_markers(mass: f32, glb_path: &Path) -> Result<Self, ModelGap> {
        let g = GlbNodes::read(glb_path, &REQUIRED_EMPTIES, &REQUIRED_MESHES)?;

        // Every lookup is collected rather than `?`-ed, so ONE warning names the WHOLE gap. With
        // `?` a re-export that dropped four nodes reported one of them per rebuild cycle.
        let mut miss = MissingNodes::default();
        let p0 = miss.empty(&g, PIN_START);
        let p1 = miss.empty(&g, PIN_END);
        let link_box = miss.mesh(&g, LINK_BOX);
        let sprocket = miss.mesh(&g, SPROCKET);
        let idler = miss.mesh(&g, IDLER);
        let wheel = miss.mesh(&g, WHEEL);
        miss.into_result()?;

        // PITCH, in the side plane only. A pin is a CYLINDER spanning the shoe's width: every point
        // along its axis is equally "the pin", so the marker's lateral x is abstract — the artist
        // drops it anywhere along the bore. Projecting x out makes the number depend only on what a
        // pitch IS: the pin-to-pin spacing measured around the loop, which lives in the side plane.
        // (On today's Tiger the two markers happen to agree to 0.24 mm in x, so the 3-D length is
        // the same to six decimals. That is luck, and a derivation must not rest on it.)
        let pitch = super::derive::pitch_from_pins(p0.with_x(0.0), p1.with_x(0.0));
        let pin_mid = (p0 + p1) * 0.5;

        // The two RADIAL faces of the shoe, from the link's collision box (which replaced the
        // deleted `Inner_Surface`/`Outer_Surface` empties — it reproduces them to ~1 mm). `−y` is
        // the OUTER, ground-contact side; `+y` is INNER, toward the wheels. Read independently, so
        // there is no mid-plate assumption: the Tiger's shoe really is asymmetric (24.7 vs 25.6 mm).
        let pin_to_outer = pin_mid.y - link_box.lo.y;
        let pin_to_inner = link_box.hi.y - pin_mid.y;

        // The running gear's centres are AUTHORED, not inferred — but the assumption that makes
        // that legal (the origin sits on the axle) is the artist's, so it is checked, per node,
        // every read. Not for `Link_Box`: a shoe is not a body of revolution and its origin is
        // free, which is why its lateral centre alone still comes off the bounds.
        for (name, m) in [(SPROCKET, &sprocket), (IDLER, &idler), (WHEEL, &wheel)] {
            m.warn_if_origin_off_axle(name);
        }

        Ok(Self {
            pitch,
            pin_to_inner,
            pin_to_outer,
            plane_x: pin_mid.x.abs(),
            // LATERAL datums, all from the same box, all kept as the true faces: the shoe sits
            // outboard of the pin plane, so no ± half-width shortcut is available (see
            // `link_center_x`).
            width: link_box.hi.x - link_box.lo.x,
            lateral_min: link_box.lo.x,
            lateral_max: link_box.hi.x,
            link_center_x: link_box.center.x,
            // side-plane (z, y): glTF x is lateral, y is height, z is longitudinal. Centres are the
            // AUTHORED node origins — see [`MeshMeasure::origin`].
            sprocket_center: Vec2::new(sprocket.origin.z, sprocket.origin.y),
            idler_center: Vec2::new(idler.origin.z, idler.origin.y),
            idler_radius: idler.disc_radius,
            wheel_tread: wheel.disc_radius,
            mass,
        })
    }

    /// Second tripwire, for the failure a missing-node check CANNOT catch: a re-export that keeps
    /// every name but changes what a node MEANS (a rescaled parent, a mesh that swallowed a loose
    /// part, a marker moved onto the wrong feature). Every lookup resolves, the numbers are simply
    /// wrong — so the derived values are range-checked against what a tank can physically be.
    ///
    /// **Warn, never abort — and that is not inconsistent with [`refuse`].** The two guards are
    /// different KINDS of statement. A node name is a CONTRACT: either the export honours it or the
    /// measurement is of something else entirely, and there is no such thing as a partially-correct
    /// answer — fatal. A plausibility band is a HEURISTIC: it encodes what a tank has looked like so
    /// far, and the day someone rigs a genuinely unusual vehicle (or measures a test asset) the
    /// right outcome is a tool that still starts and shows the number, not one that refuses. So
    /// missing contract = fatal, implausible value = warning.
    fn warn_if_implausible(&self) {
        let checks = [
            ("pitch", self.pitch, PLAUSIBLE_PITCH),
            ("width", self.width, PLAUSIBLE_WIDTH),
            ("wheel_tread", self.wheel_tread, PLAUSIBLE_RADIUS),
            ("idler_radius", self.idler_radius, PLAUSIBLE_RADIUS),
        ];
        let bad: Vec<String> = checks
            .iter()
            .filter(|(_, v, (lo, hi))| !(*lo..=*hi).contains(v))
            .map(|(name, v, (lo, hi))| format!("{name} = {v:.4} (expected {lo}..={hi})"))
            .collect();
        if !bad.is_empty() {
            warn!(
                "track model: marker read succeeded but returned implausible geometry — {}. The \
                 usual cause is a node whose scale or mesh content changed in the re-export (the \
                 wheels carry a 4.34 local scale and a 180° Y rotation; the link sits under a \
                 0.808 scale).",
                bad.join(", "),
            );
        }
    }
}

/// Abort the sandbox because the glb cannot answer the marker read.
///
/// **Why a panic, in a file that otherwise only warns.** This is `bin/track_sandbox`, a dev tool
/// whose entire job is to MEASURE the Tiger — not the shipped client, where refusing to start would
/// cost a player a session. The alternative (run on substituted geometry) is what shipped before,
/// and it failed in the worst possible way: the tool kept working, drew a rig, printed numbers, and
/// they were a different tank's. A tool that refuses to start costs a re-export; a tool that lies
/// costs a day of chasing a suspension model that was never wrong. So the failure is loud, it is at
/// rig-build time — before a single frame is drawn — and it is unconditional. It also matches how
/// [`crate::bake`] already answers an unreadable glb.
///
/// **One message, everything in it.** The panic string is the whole diagnosis: every missing node at
/// once (never the first one per rebuild cycle — that is what [`MissingNodes`] accumulates for), the
/// exact file that was read, the full contract so the artist can see what else must survive, and the
/// two actions that fix it. It is a `panic!` payload rather than an `error!` so no log filter can
/// swallow it, and the default hook prints it BEFORE any backtrace — it is the first thing on the
/// terminal, not a line buried in frames.
fn refuse(gap: &ModelGap, glb_path: &Path) -> ! {
    panic!(
        "track model: {} could not answer the marker read — {gap}.\n\
         \n\
         The suspension sandbox measures the track geometry (pitch, shoe faces, lateral plane, \
         sprocket/idler centres, wheel tread) ONLY from these glb nodes: empties [{}], meshes [{}]. \
         There is NO fallback to the RON's authored geometry: substituting it once let this tool \
         measure a different tank while it looked like it was working, so a gap is fatal by policy.\n\
         \n\
         Fix it one of two ways: re-export {} with the node(s) named above restored under exactly \
         those names, or — if a name changed on purpose — update the node-name constants at the top \
         of src/track/marker_model.rs to match the new export.",
        glb_path.display(),
        REQUIRED_EMPTIES.join(", "),
        REQUIRED_MESHES.join(", "),
        glb_path.display(),
    )
}

/// Why the marker read could not answer — carried out of the read so [`refuse`] can NAME it.
enum ModelGap {
    /// The glb itself could not be opened, parsed, or walked.
    Unreadable(&'static str),
    /// The glb read fine, but these named nodes were absent (or carried no usable mesh).
    Missing(Vec<&'static str>),
}

impl fmt::Display for ModelGap {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unreadable(why) => write!(f, "glb unreadable: {why}"),
            Self::Missing(names) => write!(f, "missing nodes: {}", names.join(", ")),
        }
    }
}

/// Accumulator that turns "N `?`-ed lookups" into one report. A missed lookup yields a zeroed value
/// so the caller can keep going and collect the rest; [`Self::into_result`] then refuses to hand
/// back a model built on any of them.
#[derive(Default)]
struct MissingNodes(Vec<&'static str>);

impl MissingNodes {
    fn empty(&mut self, g: &GlbNodes, name: &'static str) -> Vec3 {
        g.empties.get(name).copied().unwrap_or_else(|| {
            self.0.push(name);
            Vec3::ZERO
        })
    }

    fn mesh(&mut self, g: &GlbNodes, name: &'static str) -> MeshMeasure {
        g.meshes.get(name).copied().unwrap_or_else(|| {
            self.0.push(name);
            MeshMeasure::default()
        })
    }

    fn into_result(self) -> Result<(), ModelGap> {
        if self.0.is_empty() {
            Ok(())
        } else {
            Err(ModelGap::Missing(self.0))
        }
    }
}

/// One mesh node's world-space measurements (see [`measure_mesh`]).
#[derive(Clone, Copy, Default)]
struct MeshMeasure {
    /// The node's own composed world ORIGIN — the artist's authored datum, and for every body of
    /// revolution in the running gear THE axle. This is the centre the circles are built on.
    ///
    /// It is a stated fact, not an estimate, and that is the whole reason to prefer it: a rotation
    /// centre is something the rig DECLARES (Blender spins the wheel about this point), whereas any
    /// vertex statistic only ever approximates it and does so worst exactly where the mesh is least
    /// regular. The `Sprocket_*` / `Idler_*` nodes carry a real axle origin
    /// (`bake::tests::rotating_nodes_carry_their_own_axle_origin` pins that), so no vertex-statistic
    /// estimator is needed. Only the lateral `x` is not on the axle mid-plane (the origin sits on the outer disc
    /// row) — irrelevant, because every circle in this module is a side-plane `(z, y)` construction.
    origin: Vec3,
    /// Axis-aligned bounds of the mesh's vertices.
    lo: Vec3,
    hi: Vec3,
    /// Centre of those bounds. Used for the shoe's lateral centre (`Link_Box` is a box, not a body
    /// of revolution, so its origin is free and its bounds ARE the statement), and as the
    /// cross-check on the authored origin ([`Self::warn_if_origin_off_axle`]). NOT the vertex
    /// centroid, which is a mesh DENSITY statistic — on the two-disc wheel stations the centroid
    /// sits ~7 mm off the axle and inflated the tread radius by the same amount.
    center: Vec3,
    /// Rim radius about [`Self::origin`], measured in the side plane `(z, y)`.
    disc_radius: f32,
}

/// How far the authored origin may sit from the mesh's own side-plane bounds centre (m) before the
/// read says so.
///
/// Sized off the 2026-07-23 audit of the shipped Tiger, which is also the evidence that the origin
/// IS the axle: `Wheel_R_0` agrees to 0.00000 m (exactly — the origin's y/z and its bounds centre
/// print identically; only lateral x differs, by 164 mm, because the origin sits on the outer disc
/// row), `Sprocket_R` to 0.00106 m (its rim is TEETH, so its bounds are not a circle and never will
/// be), `Idler_R` to 0.00395 m. The band clears all three by ~8× while still catching the failures
/// worth catching — an origin left at the model root (metres out) or dropped onto a hub face. It is
/// deliberately NOT tightened to fire on today's idler: a warning that prints on every launch of a
/// known-open issue is a warning nobody reads.
const AXLE_ORIGIN_TOLERANCE: f32 = 0.03;

impl MeshMeasure {
    /// Guard on the ONE assumption the authored-origin datum rests on: that the origin is on the
    /// axle. A future export that moves it (or a new node whose origin was never placed) would
    /// otherwise shift a whole circle silently.
    ///
    /// Warn, don't abort — same rule as [`DerivedModel::warn_if_implausible`]: the node contract is
    /// binary and therefore fatal, but "how close is close enough" is a tolerance someone chose, and
    /// a tolerance must never be the thing that stops a dev tool from starting.
    fn warn_if_origin_off_axle(&self, name: &str) {
        let off = Vec2::new(self.center.z - self.origin.z, self.center.y - self.origin.y).length();
        if off > AXLE_ORIGIN_TOLERANCE {
            warn!(
                "track model: {name}'s authored origin sits {off:.4} m off its own side-plane \
                 bounds centre (origin z,y = {:.4},{:.4}; bounds centre = {:.4},{:.4}). The circle \
                 is built on the ORIGIN, so if that origin is no longer on the axle every route \
                 around this wheel is displaced by that much. Re-place the object origin on the \
                 axle in Blender and re-export.",
                self.origin.z, self.origin.y, self.center.z, self.center.y,
            );
        }
    }
}

/// World positions of named empties + [`MeshMeasure`] of named meshes, read from the glb with the
/// full node transform chain.
struct GlbNodes {
    empties: HashMap<&'static str, Vec3>,
    meshes: HashMap<&'static str, MeshMeasure>,
}

impl GlbNodes {
    fn read(
        glb_path: &Path,
        empty_names: &[&'static str],
        mesh_names: &[&'static str],
    ) -> Result<Self, ModelGap> {
        let gltf::Gltf { document, mut blob } =
            gltf::Gltf::open(glb_path).map_err(|_| ModelGap::Unreadable("open/parse failed"))?;
        let mut buffers: Vec<Vec<u8>> = Vec::new();
        for buffer in document.buffers() {
            match buffer.source() {
                gltf::buffer::Source::Bin => {
                    buffers.push(blob.take().ok_or(ModelGap::Unreadable("no binary chunk"))?)
                }
                gltf::buffer::Source::Uri(_) => {
                    return Err(ModelGap::Unreadable(
                        "external buffer (not a self-contained glb)",
                    ));
                }
            }
        }

        let mut empties = HashMap::new();
        let mut meshes = HashMap::new();
        let scene = document
            .scenes()
            .next()
            .ok_or(ModelGap::Unreadable("no scene"))?;
        // DFS composing the world matrix (parent · local) down the tree.
        let mut stack: Vec<(gltf::Node, Mat4)> =
            scene.nodes().map(|n| (n, Mat4::IDENTITY)).collect();
        while let Some((node, parent)) = stack.pop() {
            let world = parent * node_matrix(&node);
            if let Some(name) = node.name() {
                if let Some(&name) = empty_names.iter().find(|n| **n == name) {
                    empties.insert(name, world.transform_point3(Vec3::ZERO));
                }
                if let Some(&name) = mesh_names.iter().find(|n| **n == name)
                    && let Some(mesh) = node.mesh()
                {
                    let mut verts: Vec<Vec3> = Vec::new();
                    for primitive in mesh.primitives() {
                        let reader =
                            primitive.reader(|b| buffers.get(b.index()).map(Vec::as_slice));
                        if let Some(positions) = reader.read_positions() {
                            verts.extend(positions.map(|p| world.transform_point3(Vec3::from(p))));
                        }
                    }
                    // The node's own origin, composed the same way the vertices were — the authored
                    // axle for every body of revolution here (see [`MeshMeasure::origin`]).
                    if let Some(m) = measure_mesh(&verts, world.transform_point3(Vec3::ZERO)) {
                        meshes.insert(name, m);
                    }
                }
            }
            for child in node.children() {
                stack.push((child, world));
            }
        }
        Ok(Self { empties, meshes })
    }
}

/// glTF node local transform → matrix (Matrix form, or composed from the decomposed TRS).
fn node_matrix(node: &gltf::Node) -> Mat4 {
    match node.transform() {
        gltf::scene::Transform::Matrix { matrix } => Mat4::from_cols_array_2d(&matrix),
        gltf::scene::Transform::Decomposed {
            translation,
            rotation,
            scale,
        } => Mat4::from_scale_rotation_translation(
            Vec3::from(scale),
            Quat::from_array(rotation),
            Vec3::from(translation),
        ),
    }
}

// -------------------------------------------------------------------------------------------
// Mesh statistics
// -------------------------------------------------------------------------------------------
//
// These meshes are ARTIST DATA. Yan's models carry loose greebles — a bolt head, an orphaned visual
// scrap, a stray vertex left by a boolean, and right now an `Idler_R` that is not watertight — so a
// statistic taken here is only worth having if a single such vertex cannot move it.
//
// The strategy is to DERIVE AS LITTLE AS POSSIBLE. There are only two questions a circle asks, and
// they have different answers:
//   * the CENTRE is not derived at all: it is the node's authored origin, a fact the rig states
//     (see `MeshMeasure::origin`). Nothing a stray vertex does can move a stated fact.
//   * the RADIUS must still be derived, and it is a MAXIMUM, the statistic one stray destroys. So it
//     is ANCHORED to a quantile: take the largest in-plane radius still within a hair of the 95th
//     percentile. A rim is a RING — hundreds of vertices at one radius — so it always clears the
//     quantile; a handful of strays never do. On a clean disc every radius is equal, so the answer
//     is bit-identical to a naive `max`.
//
// The plane is not inferred either. Every circle in this module is a SIDE-PLANE `(z, y)` circle and
// every axle in a tank's running gear is lateral, so the radius is measured in `(z, y)` by
// definition rather than by picking the mesh's thinnest axis — one less thing a re-export can flip.

/// Quantile of the in-plane radii that anchors "this is the rim".
const RIM_QUANTILE: f32 = 0.95;
/// How far past the rim quantile a vertex may still sit and count as rim (relative). Wide enough to
/// swallow the faceting of a coarse ring, far too narrow for a detached part.
const RIM_BAND: f32 = 0.01;

/// Bounds + bounds centre + side-plane rim radius about the node's authored `origin`.
fn measure_mesh(verts: &[Vec3], origin: Vec3) -> Option<MeshMeasure> {
    if verts.len() < 3 {
        return None;
    }
    let (lo, hi) = bounds(verts);

    // Radii in the side plane, about the AUTHORED axle — not about anything measured, so a lopsided
    // or leaky mesh cannot bias the whole ring outward the way an inferred centre did.
    let mut radii: Vec<f32> = verts
        .iter()
        .map(|v| Vec2::new(v.z - origin.z, v.y - origin.y).length())
        .collect();
    radii.sort_by(f32::total_cmp);
    let rim = quantile(&radii, RIM_QUANTILE) * (1.0 + RIM_BAND);
    // Sorted, so the last radius inside the band is the largest one — the rim, without whatever
    // sits beyond it.
    let disc_radius = radii
        .iter()
        .rev()
        .find(|r| **r <= rim)
        .copied()
        .unwrap_or_default();

    Some(MeshMeasure {
        origin,
        lo,
        hi,
        center: (lo + hi) * 0.5,
        disc_radius,
    })
}

/// Per-axis min/max of a point cloud.
fn bounds(verts: &[Vec3]) -> (Vec3, Vec3) {
    verts.iter().fold(
        (Vec3::splat(f32::INFINITY), Vec3::splat(f32::NEG_INFINITY)),
        |(lo, hi), &v| (lo.min(v), hi.max(v)),
    )
}

/// Linearly-interpolated quantile of an ASCENDING slice (numpy's default convention). `q` is
/// clamped to `0..=1`; an empty slice is 0.
fn quantile(sorted: &[f32], q: f32) -> f32 {
    if sorted.is_empty() {
        return 0.0;
    }
    let pos = q.clamp(0.0, 1.0) * (sorted.len() - 1) as f32;
    let lo = pos.floor() as usize;
    let hi = (lo + 1).min(sorted.len() - 1);
    sorted[lo] + (sorted[hi] - sorted[lo]) * (pos - lo as f32)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A clean ring of `n` vertices at `r` about `center`, in the plane perpendicular to x (the
    /// axle) — the idealisation of a road wheel's tread.
    fn ring(center: Vec3, r: f32, n: usize, half_width: f32) -> Vec<Vec3> {
        (0..n)
            .map(|i| {
                let a = std::f32::consts::TAU * i as f32 / n as f32;
                // Alternate sides so the ring has thickness along the axle, like a real station.
                let x = if i % 2 == 0 { half_width } else { -half_width };
                center + Vec3::new(x, r * a.sin(), r * a.cos())
            })
            .collect()
    }

    /// The user's stated hazard: a mesh that carries loose parts. The rim radius must be the SAME
    /// number with the strays present as without — a naive `max` reads the stray instead of the
    /// tread. The CENTRE needs no such defence (it is the authored origin), and this test pins WHY:
    /// a derived bounds centre is visibly wrecked by the same strays.
    #[test]
    fn rim_radius_ignores_loose_greebles() {
        let axle = Vec3::new(1.5, 0.5, -1.9);
        let clean = ring(axle, 0.387, 256, 0.1);
        let m = measure_mesh(&clean, axle).expect("a ring measures");
        assert!(
            (m.disc_radius - 0.387).abs() < 1e-5,
            "got {}",
            m.disc_radius
        );

        // Same wheel, plus a handful of detached scraps flung well clear of it.
        let mut greebled = clean.clone();
        greebled.extend([
            axle + Vec3::new(0.0, 1.4, 0.0),
            axle + Vec3::new(0.0, -1.1, 0.9),
            axle + Vec3::new(0.3, 0.0, -1.6),
            axle + Vec3::new(-0.2, 2.2, 2.2),
        ]);
        let g = measure_mesh(&greebled, axle).expect("a greebled ring still measures");
        assert!(
            (g.disc_radius - m.disc_radius).abs() < 1e-5,
            "greebles moved the rim: {} vs {}",
            g.disc_radius,
            m.disc_radius
        );
        assert_eq!(g.origin, axle, "the authored centre cannot be moved at all");
        // And the two naive statistics really would have been destroyed — not a vacuous test.
        let naive = greebled
            .iter()
            .map(|v| Vec2::new(v.y - axle.y, v.z - axle.z).length())
            .fold(0.0_f32, f32::max);
        assert!(naive > 1.0, "the strays must be far enough out to matter");
        assert!(
            (g.center - m.center).length() > 0.1,
            "the DERIVED centre is what the strays wreck — that is why it is not the datum"
        );
    }

    /// Two discs on one axle (what the re-exported wheel nodes actually are) with lopsided vertex
    /// density: the CENTROID is pulled off the axle and inflates the radius, the authored origin is
    /// not. This is the ~7 mm error that measuring from the origin removes.
    #[test]
    fn authored_origin_beats_the_centroid_on_a_two_disc_station() {
        let axle = Vec3::new(1.5, 0.5, -1.9);
        let mut verts = ring(axle, 0.387, 200, 0.1);
        // A dense hub blob off to one side — legitimate geometry, just unevenly tessellated.
        verts.extend((0..400).map(|i| {
            let a = std::f32::consts::TAU * i as f32 / 400.0;
            axle + Vec3::new(0.09, 0.05 * a.sin() + 0.06, 0.05 * a.cos() + 0.04)
        }));
        let m = measure_mesh(&verts, axle).expect("measures");
        assert!(
            (m.disc_radius - 0.387).abs() < 1e-4,
            "got {}",
            m.disc_radius
        );

        let centroid = verts.iter().fold(Vec3::ZERO, |a, &b| a + b) / verts.len() as f32;
        let by_centroid = verts
            .iter()
            .map(|v| Vec2::new(v.y - centroid.y, v.z - centroid.z).length())
            .fold(0.0_f32, f32::max);
        assert!(
            by_centroid > m.disc_radius + 2e-3,
            "the centroid read should be the inflated one: {by_centroid} vs {}",
            m.disc_radius
        );
    }

    /// The origin guard: it must stay quiet on an axle-centred origin and speak up when the origin
    /// is off the axle (the export regression it exists for). Only the side-plane offset counts —
    /// the real wheel origins sit on the OUTER disc row, laterally off-centre by design.
    #[test]
    fn the_axle_origin_guard_looks_only_at_the_side_plane() {
        let axle = Vec3::new(1.5, 0.5, -1.9);
        let verts = ring(axle, 0.387, 128, 0.1);
        let on_axle = measure_mesh(&verts, axle).expect("measures");
        let off_laterally =
            measure_mesh(&verts, axle + Vec3::new(0.4, 0.0, 0.0)).expect("measures");
        let off_in_plane =
            measure_mesh(&verts, axle + Vec3::new(0.0, 0.10, 0.0)).expect("measures");

        let offset =
            |m: &MeshMeasure| Vec2::new(m.center.z - m.origin.z, m.center.y - m.origin.y).length();
        assert!(offset(&on_axle) < AXLE_ORIGIN_TOLERANCE);
        assert!(
            offset(&off_laterally) < AXLE_ORIGIN_TOLERANCE,
            "a lateral offset is legitimate and must not trip the guard"
        );
        assert!(
            offset(&off_in_plane) > AXLE_ORIGIN_TOLERANCE,
            "a side-plane offset must trip it"
        );
    }

    /// Pitch must be a SIDE-PLANE measurement: the pins' lateral placement is abstract (a pin is a
    /// cylinder; any point on its axis is the pin), so sliding one marker along the bore must not
    /// change the pitch by a micron.
    #[test]
    fn pitch_ignores_the_pins_abstract_lateral_placement() {
        let a = Vec3::new(1.531, 0.0924, -2.5671);
        let b = Vec3::new(1.531, 0.0924, -2.6975);
        let pitch =
            |p: Vec3, q: Vec3| super::super::derive::pitch_from_pins(p.with_x(0.0), q.with_x(0.0));
        let straight = pitch(a, b);
        assert!((straight - 0.1304).abs() < 1e-4, "got {straight}");
        // Same two pins, one marker slid 30 cm along its own axis.
        let slid = pitch(a, b + Vec3::new(0.3, 0.0, 0.0));
        assert!((slid - straight).abs() < 1e-6, "{slid} vs {straight}");
        // The 3-D length, by contrast, is wrecked by exactly that slide — the reason for the
        // projection: a 0.3 m nudge along a pin's own axis would have more than DOUBLED the pitch,
        // and with it the whole material loop length.
        assert!((b + Vec3::new(0.3, 0.0, 0.0) - a).length() > straight * 2.0);
    }

    /// The end-to-end read against the SHIPPED glb: the numbers this whole module exists to
    /// produce, pinned so the next re-export that moves one of them fails here rather than in a
    /// playtest. Values are the 2026-07-23 audit of `tiger_1.glb`.
    #[test]
    fn tiger_glb_drives_the_model() {
        let m = DerivedModel::from_markers(57_000.0, Path::new(TIGER_GLB))
            .unwrap_or_else(|gap| panic!("the shipped glb must answer the marker read: {gap}"));

        assert!((m.pitch - 0.130_43).abs() < 1e-4, "pitch {}", m.pitch);
        assert!((m.plane_x - 1.531_2).abs() < 1e-3, "plane_x {}", m.plane_x);
        // Width from `Link_Box`, NOT the RON's 0.79 — the authored number was 64 mm too wide.
        assert!((m.width - 0.725_97).abs() < 1e-3, "width {}", m.width);
        assert!(
            (m.width - 0.79).abs() > 0.05,
            "width must be the measured one, not the RON's"
        );
        // The shoe is authored OUTBOARD of the pin plane: not symmetric about it.
        assert!(
            m.link_center_x - m.plane_x > 0.010,
            "expected the ~16.8 mm outboard offset, got {}",
            m.link_center_x - m.plane_x
        );
        assert!((m.lateral_max - m.lateral_min - m.width).abs() < 1e-5);
        assert!(((m.lateral_min + m.lateral_max) * 0.5 - m.link_center_x).abs() < 1e-5);
        // Radial faces off the box, `−y` outer: thinner outer, thicker inner.
        assert!(
            (m.pin_to_outer - 0.024_73).abs() < 5e-4,
            "{}",
            m.pin_to_outer
        );
        assert!(
            (m.pin_to_inner - 0.025_56).abs() < 5e-4,
            "{}",
            m.pin_to_inner
        );
        // The tread: the wheel node's 4.34 local scale and 180° Y rotation are composed away, and
        // the two-disc station reads its true rim about the authored axle.
        assert!(
            (m.wheel_tread - 0.386_97).abs() < 2e-3,
            "tread {}",
            m.wheel_tread
        );
        // Idler rim, measured from the AUTHORED origin — and now the two agree. This assertion used
        // to band 0.34583 because the authored origin sat 3.9 mm off the rim's true axis, so
        // origin-derived and circle-fitted radii DISAGREED by exactly that offset. The origin was
        // corrected in the model (it now lands within 0.6 um of the rim's least-squares centre), so
        // the honest number is the rim itself, 0.3419.
        //
        // The band stays tighter than the other pins for the original reason: a quiet return to a
        // vertex-DERIVED centre would put the centre ~7 mm off (the idler's vertex median is that
        // far from its axle), inflating this reading well past ±3 mm. So this still catches the
        // regression the authored-origin switch exists to prevent.
        assert!(
            (m.idler_radius - 0.341_9).abs() < 3e-3,
            "idler rim {} — is this measured from the node origin?",
            m.idler_radius
        );
        assert!(
            m.sprocket_center.x < 0.0 && m.idler_center.x > 0.0,
            "front/rear"
        );
    }

    /// The claim the authored-origin datum rests on, checked against the shipped asset rather than
    /// assumed: a road-wheel station's node origin IS its axle. In the side plane it agrees with the
    /// mesh's own bounds centre to floating-point noise; laterally it does NOT, because the origin
    /// sits on the outer disc row — which is precisely why the radius is a side-plane measurement.
    #[test]
    fn the_wheel_station_origin_is_its_axle() {
        let g = GlbNodes::read(Path::new(TIGER_GLB), &[], &[WHEEL])
            .unwrap_or_else(|gap| panic!("the shipped glb must read: {gap}"));
        let w = g.meshes.get(WHEEL).expect("the wheel station is present");
        assert!(
            (w.origin.y - w.center.y).abs() < 1e-4 && (w.origin.z - w.center.z).abs() < 1e-4,
            "origin {:?} vs bounds centre {:?}",
            w.origin,
            w.center
        );
        assert!(
            (w.origin.x - w.center.x).abs() > 0.10,
            "the lateral offset is expected and load-bearing for the side-plane rule: {} vs {}",
            w.origin.x,
            w.center.x
        );
    }

    /// A missing node must be REPORTED, by name — the whole point of killing the silent fallback.
    #[test]
    fn a_missing_lookup_names_itself() {
        let g = GlbNodes::read(
            Path::new(TIGER_GLB),
            &[PIN_START, "Outer_Surface"],
            &[LINK_BOX],
        )
        .unwrap_or_else(|gap| panic!("the shipped glb must read: {gap}"));
        let mut miss = MissingNodes::default();
        let _ = miss.empty(&g, PIN_START);
        let _ = miss.empty(&g, "Outer_Surface");
        let gap = miss.into_result().expect_err("the deleted marker is gone");
        let text = gap.to_string();
        assert!(text.contains("Outer_Surface"), "{text}");
        assert!(!text.contains(PIN_START), "{text}");
    }

    /// An unreadable path must fail LOUDLY rather than resolve to an empty node set (which would
    /// have looked like "every marker is missing" and buried the real cause).
    #[test]
    fn an_unreadable_glb_says_so() {
        let gap = DerivedModel::from_markers(1.0, Path::new("no/such/tank.glb"))
            .err()
            .expect("a missing file cannot answer");
        assert!(gap.to_string().contains("unreadable"), "{gap}");
    }

    /// Run `f`, which must panic, and hand back its message. `catch_unwind` (not `should_panic`) so
    /// the WHOLE diagnosis can be asserted: `should_panic` only proves one substring is present,
    /// and the contract here is that a single message answers every question at once.
    fn panic_text(f: impl FnOnce() + std::panic::UnwindSafe) -> String {
        let payload = std::panic::catch_unwind(f).expect_err("must abort");
        payload
            .downcast_ref::<String>()
            .cloned()
            .unwrap_or_else(|| "<non-string panic payload>".into())
    }

    /// THE POLICY TEST: a dropped marker aborts, and the abort message names EVERY missing node in
    /// one go (this is the incident that killed the fallback — three nodes vanished in one
    /// re-export), plus the file and the fix. Driven through [`refuse`] directly because the shipped
    /// glb is complete: there is no on-disk asset with a marker missing, and inventing one would
    /// test the fixture rather than the policy.
    #[test]
    fn a_missing_marker_aborts_and_names_the_whole_gap() {
        let gap = ModelGap::Missing(vec![PIN_END, LINK_BOX, WHEEL]);
        let text = panic_text(|| refuse(&gap, Path::new("assets/tiger_1/tiger_1.glb")));

        for name in [PIN_END, LINK_BOX, WHEEL] {
            assert!(text.contains(name), "gap must name {name}: {text}");
        }
        // The whole contract, so the artist can see what else has to survive the next export.
        for name in REQUIRED_EMPTIES.iter().chain(REQUIRED_MESHES.iter()) {
            assert!(text.contains(name), "contract must list {name}: {text}");
        }
        assert!(text.contains("tiger_1.glb"), "must name the file: {text}");
        assert!(text.contains("re-export"), "must say what to do: {text}");
        // And it must never offer the substitute geometry that caused the incident.
        assert!(
            !text.contains("falling back") && !text.contains("tiger_1.tank.ron"),
            "no fallback may be implied: {text}"
        );
    }

    /// The same refusal through the REAL entry point, on the other gap flavour: `build` returns a
    /// model or nothing at all — there is no `Result`, so an unreadable glb can only abort.
    #[test]
    fn an_unreadable_glb_aborts_the_build() {
        let text = panic_text(|| {
            DerivedModel::build(57_000.0, Path::new("no/such/tank.glb"));
        });
        assert!(text.contains("glb unreadable"), "{text}");
        assert!(text.contains("no/such/tank.glb"), "{text}");
    }

    /// Path is relative to the crate root, matching the other asset-reading tests (`spec.rs`).
    const TIGER_GLB: &str = "assets/tiger_1/tiger_1.glb";
}
