//! The common shoe template's pin and face datums, extracted synchronously from the sim artifact.
//!
//! `Pin_Start` and `Pin_End` define pitch; `Link_Box` defines the shoe's radial and lateral faces.
//! Running-gear membership and contact geometry come from the tank sheet and its declared axle
//! nodes, through `rig_geom`. No loaded view geometry supplies simulation construction.

use std::collections::HashMap;
use std::fmt;
use std::path::Path;

use bevy::math::{Mat4, Quat, Vec3};
use bevy::prelude::Resource;

/// The pin markers: the two ends of ONE link, the loop's immutable pitch.
const PIN_START: &str = "Pin_Start";
const PIN_END: &str = "Pin_End";
/// The link's collision box — the shoe's own volume, and since the 2026-07 re-export the source of
/// every lateral/radial datum (it replaced the deleted `Inner_Surface`/`Outer_Surface` empties).
/// Authored so its COMPOSED world scale is exactly 1: its extents read directly in metres.
const LINK_BOX: &str = "Link_Box";
/// The common shoe template's marker contract.
const REQUIRED_EMPTIES: [&str; 2] = [PIN_START, PIN_END];
const REQUIRED_MESHES: [&str; 1] = [LINK_BOX];

/// Everything the derived geometry needs, read out of the model (all lengths in metres, side-plane
/// centres as `(z, y)` in the glTF frame the blueprint's wheel positions also use).
///
/// Lateral (`x`) fields describe the common right-facing shoe template. `RigGeom` places that
/// template on the sheet's independent pin planes and mirrors it on the left.
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
    /// Hull mass (RON), supplying nominal ground pressure and spring-rate readouts.
    pub mass: f32,
}

impl DerivedModel {
    /// Derive from the model markers/nodes, or ABORT. `mass` (the one non-geometry input) is passed
    /// in rather than read here, so this module never touches the RON.
    ///
    /// There is deliberately no `Result` and no substitute geometry: see [`refuse`] for why a dev
    /// tool answers a broken export with a panic.
    pub fn build(mass: f32, glb_path: &Path) -> Self {
        Self::from_markers(mass, glb_path).unwrap_or_else(|gap| refuse(&gap, glb_path))
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

        let model = Self {
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
            mass,
        };
        for (field, value) in [
            ("pin pitch", model.pitch),
            ("Link_Box inner-face offset", model.pin_to_inner),
            ("Link_Box outer-face offset", model.pin_to_outer),
            ("Link_Box width", model.width),
        ] {
            if !value.is_finite() || value <= 0.0 {
                return Err(ModelGap::Invalid(format!(
                    "{field} must be finite and positive (got {value}); restore the pin and shoe-face datums"
                )));
            }
        }
        if ![
            model.plane_x,
            model.lateral_min,
            model.lateral_max,
            model.link_center_x,
        ]
        .into_iter()
        .all(f32::is_finite)
        {
            return Err(ModelGap::Invalid("pin plane and Link_Box lateral faces must be finite; restore the template transform".into()));
        }
        Ok(model)
    }
}

/// Refuse malformed shipped shoe geometry before any body or running gear is published.
/// The diagnosis names the sim artifact, every missing datum and the authoring fix.
fn refuse(gap: &ModelGap, glb_path: &Path) -> ! {
    panic!(
        "track model: {} could not answer the marker read — {gap}.\n\
         \n\
         The shoe template's geometry (pitch, radial and lateral faces) is measured ONLY from these glb nodes: empties [{}], meshes [{}]. \
         There is NO fallback to the RON's authored geometry: substituting it once let the sandbox \
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
    Invalid(String),
    /// The glb read fine, but these named nodes were absent (or carried no usable mesh).
    Missing(Vec<&'static str>),
}

impl fmt::Display for ModelGap {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unreadable(why) => write!(f, "glb unreadable: {why}"),
            Self::Invalid(why) => write!(f, "invalid shoe geometry: {why}"),
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
/// Bounds of the declared shoe template, composed into the model frame.
#[derive(Clone, Copy, Default)]
struct MeshMeasure {
    lo: Vec3,
    hi: Vec3,
    center: Vec3,
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
                    if let Some(m) = measure_mesh(&verts) {
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
pub(crate) fn node_matrix(node: &gltf::Node) -> Mat4 {
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

fn measure_mesh(verts: &[Vec3]) -> Option<MeshMeasure> {
    if verts.len() < 3 || verts.iter().any(|vertex| !vertex.is_finite()) {
        return None;
    }
    let (lo, hi) = bounds(verts);
    Some(MeshMeasure {
        lo,
        hi,
        center: (lo + hi) * 0.5,
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
#[cfg(test)]
mod tests {
    use super::*;

    /// A missing node must be REPORTED, by name — the whole point of killing the silent fallback.
    #[test]
    fn a_missing_lookup_names_itself() {
        let g = GlbNodes {
            empties: HashMap::from([(PIN_START, Vec3::ZERO)]),
            meshes: HashMap::new(),
        };
        let mut miss = MissingNodes::default();
        let _ = miss.empty(&g, PIN_START);
        let _ = miss.empty(&g, "Outer_Surface");
        let gap = miss
            .into_result()
            .expect_err("the fixture deliberately omits the requested marker");
        let text = gap.to_string();
        assert!(text.contains("Outer_Surface"), "{text}");
        assert!(!text.contains(PIN_START), "{text}");
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
        let gap = ModelGap::Missing(vec![PIN_END, LINK_BOX]);
        let text = panic_text(|| refuse(&gap, Path::new("assets/tiger_1/tiger_1.glb")));

        for name in [PIN_END, LINK_BOX] {
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
}
