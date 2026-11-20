//! Break relief, view only (ADR-0014): an element whose visual has an entry in
//! `assets/materials/fracture.ron` draws each chunk face a departed neighbour opened, and the
//! matter within a wavelength of it, displaced by one bounded field, so a break reads rough while
//! the collider stays the law hull. A face no break reaches draws its bare hull face.
//!
//! - **Breaks.** A chunk's face on none of its element's hull planes is a cut; it is open while a
//!   chunk across it ([`across`]) is not drawn with it (gone, or on another piece). The edges of
//!   the open faces one mesh draws of an element are its breaks ([`Breaks`]).
//! - **One field.** A point moves by [`Relief::shift`] times its reach, one less its distance to
//!   the nearest break over `wavelength_m`, at least zero: a function of its asset-frame position,
//!   its element's hull and the mesh's breaks alone, so faces sharing an edge draw one curve and a
//!   piece's open face, all of whose points lie on breaks, fits the hole it left. Within
//!   `wavelength_m` of a hull plane the field fades its component along that plane's normal, so a
//!   point on the hull stays on it and the element's silhouette never moves. No point moves farther
//!   than `amplitude_m`. An edge a break reaches is cut every `edge_pitch_m` from its lexically
//!   first end, so each face drawing it draws the same points.
//! - **Triangles.** A face with a moved point ear-clips its drawn outline seen along its normal,
//!   which never folds ([`MAX_WARP_SLOPE`], [`Relief::first_fold`]). An open face takes smooth
//!   normals; another keeps its plane's.
//! - **Optics.** A cut face is core: its substance's exposed colour, with its visual's [`Units`]
//!   drawn where the construction's joint planes cross it (`construction_surface.wesl` owns the
//!   vertex contract), read at its asset-frame position so they move with a fallen piece.

use std::collections::HashSet;
use std::sync::LazyLock;

use super::*;
use crate::provenance::Sourced;

const FILE: &str = "assets/materials/fracture.ron";
const SHIPPED: &str = include_str!("../../../assets/materials/fracture.ron");

/// A visual's break look (module doc), its entry in [`FILE`].
#[derive(Debug, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Relief {
    amplitude_m: Sourced<f32>,
    wavelength_m: Sourced<f32>,
    edge_pitch_m: Sourced<f32>,
    /// The units a break face shows, `None` for matter laid in none.
    units: Option<Units>,
}

/// The units a construction is laid in, as its break faces show them: a joint of `mortar_linear`
/// colour and `joint_m` wide on each bed plane every `course_m` (one through `course_phase_m`
/// up), each head plane every `head_m` along an element's run and each leaf plane every `leaf_m`
/// across its depth from its face, heads and leaves shifting `stagger` of a unit each course.
#[derive(Debug, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct Units {
    mortar_linear: Sourced<[f32; 3]>,
    joint_m: Sourced<f32>,
    course_m: Sourced<f32>,
    course_phase_m: Sourced<f32>,
    head_m: Sourced<f32>,
    leaf_m: Sourced<f32>,
    stagger: Sourced<f32>,
}

/// The largest `amplitude_m / wavelength_m` that keeps a drawn outline simple. With `r` that ratio,
/// the displacement's Jacobian has Frobenius norm at most `(2 √3 + 2) r`: the noise's `2 √3 r`
/// (each component changes by at most `2 amplitude / (√3 wavelength)` per metre along each axis),
/// the hull fades' `r` (orthonormal directions, each weight changing by at most `1 / wavelength`
/// per metre against at most `amplitude` of displacement, however many fade) and the reach's `r`.
/// Below `1 / (2 + 2 √3)` the displacement is a contraction, so its image of any plane is
/// one-to-one seen along that plane's normal, and a convex outline stays simple (DERIVED).
const MAX_WARP_SLOPE: f32 = 0.183;

/// The relief of `visual`, `None` for a visual drawn as its bare hull.
pub(super) fn of(visual: &str) -> Option<&'static Relief> {
    static RELIEFS: LazyLock<BTreeMap<String, Relief>> =
        LazyLock::new(|| parse(SHIPPED).unwrap_or_else(|err| panic!("{err}")));
    RELIEFS.get(visual)
}

/// The construction material's break records for the visual a palette lane draws
/// ([`crate::construction_surface::SurfaceParams`], laid out as `construction_surface.wesl`'s
/// header states); zero for a visual without units, whose break faces show no joint.
pub(crate) fn break_params(visual: &str) -> [Vec4; 3] {
    of(visual)
        .and_then(|relief| relief.units.as_ref())
        .map_or([Vec4::ZERO; 3], |units| {
            [
                Vec3::from_array(units.mortar_linear.0).extend(units.joint_m.0),
                Vec4::new(
                    1.0 / units.course_m.0,
                    units.course_phase_m.0,
                    1.0 / units.head_m.0,
                    1.0 / units.leaf_m.0,
                ),
                Vec4::new(units.stagger.0, 0.0, 0.0, 0.0),
            ]
        })
}

fn parse(text: &str) -> Result<BTreeMap<String, Relief>, String> {
    let reliefs =
        crate::provenance::from_shipped_str::<crate::keyed_map::KeyedMap<Relief>>(FILE, text)?.0;
    let visuals = crate::tank_surface::VisualLibrary::parse(include_str!(
        "../../../assets/materials/visuals.ron"
    ))?;
    for (key, relief) in &reliefs {
        if visuals.get(key).is_none() {
            return Err(format!(
                "{FILE}: {key}: name a visual of assets/materials/visuals.ron"
            ));
        }
        relief
            .check()
            .map_err(|(field, fix)| format!("{FILE}: {key}.{field}: {fix}"))?;
    }
    Ok(reliefs)
}

/// Refuses construction `kind` where a relieved chunk drawn alone, every cut face of it open, folds
/// one: an open face's every point lies on a break, so its drawn outline is the same beside any
/// break, and this sweep holds every open face it will draw. `visual` names each element's.
pub(crate) fn refuse_folds<'a>(
    kind: &ConstructionKind,
    visual: impl Fn(usize) -> Result<&'a str, String>,
) -> Result<(), String> {
    for e in 0..kind.layout.element_blocks.len() {
        let visual = visual(e)?;
        if let Some(c) = of(visual).and_then(|relief| relief.first_fold(kind, e)) {
            return Err(format!(
                "chunk {c}: {visual}'s relief folds a break's outline there; lower {FILE}: \
                 {visual}.amplitude_m"
            ));
        }
    }
    Ok(())
}

// Integer hash constants define the noise, not authored magnitudes.
fn hash(p: IVec3, channel: u32) -> f32 {
    let mut h = (p.x as u32).wrapping_mul(0x8da6_b343)
        ^ (p.y as u32).wrapping_mul(0xd816_3841)
        ^ (p.z as u32).wrapping_mul(0xcb1a_b31f)
        ^ channel;
    h ^= h >> 16;
    h = h.wrapping_mul(0x7feb_352d);
    h ^= h >> 15;
    h = h.wrapping_mul(0x846c_a68b);
    h ^= h >> 16;
    (h >> 8) as f32 / ((1u32 << 24) - 1) as f32
}

/// Value noise in `[0, 1]`, affine within each tetrahedron of the Freudenthal split of the unit
/// lattice.
fn noise(p: Vec3, channel: u32) -> f32 {
    let base = p.floor();
    let f = p - base;
    let mut axes = [(f.x, IVec3::X), (f.y, IVec3::Y), (f.z, IVec3::Z)];
    axes.sort_by(|a, b| b.0.total_cmp(&a.0));
    let [(hi, a), (mid, b), (lo, _)] = axes;
    let origin = base.as_ivec3();
    hash(origin, channel) * (1.0 - hi)
        + hash(origin + a, channel) * (hi - mid)
        + hash(origin + a + b, channel) * (mid - lo)
        + hash(origin + IVec3::ONE, channel) * lo
}

/// Resolution: a turn this small (twice a triangle's area, m²) is a straight run, not a corner; far
/// below a millimetre-wide sliver of a centimetre edge.
const STRAIGHT_M2: f32 = 1e-9;

/// Resolution: a vertex this close (m) to an ear counts as on it, so a straight run whose f32
/// coordinates wander by an ulp (about 1e-7 m at a metre, DERIVED) still blocks an ear across it.
/// Distance to the triangle, never to its edges' lines: a thin ear's reach past its tip would
/// block every ear of a sliver.
const ON_EAR_M: f32 = 1e-6;

/// A corner's identity: the bits of its baked coordinates.
fn key(p: Vec3) -> [u32; 3] {
    p.to_array().map(f32::to_bits)
}

/// An edge's identity, whichever way a face walks it.
fn edge(a: Vec3, b: Vec3) -> [[u32; 3]; 2] {
    let (a, b) = (key(a), key(b));
    if a <= b { [a, b] } else { [b, a] }
}

/// A polygon's edges, each from its corner to the next.
fn edges(corners: &[Vec3]) -> impl Iterator<Item = (Vec3, Vec3)> + '_ {
    corners
        .iter()
        .zip(corners.iter().cycle().skip(1))
        .map(|(a, b)| (*a, *b))
}

/// The breaks one mesh draws of one element (module doc).
#[derive(Default)]
pub(super) struct Breaks {
    /// Each open face: its chunk and its place in [`Block::faces`] order.
    open: HashSet<(usize, usize)>,
    /// Their edges.
    edges: Vec<(Vec3, Vec3)>,
}

impl Breaks {
    /// Element `e`'s breaks where the chunks `drawn` names are drawn together.
    pub(super) fn of(kind: &ConstructionKind, e: usize, drawn: &dyn Fn(usize) -> bool) -> Self {
        let mut breaks = Self::default();
        let mut seen = HashSet::new();
        for c in kind.layout.chunks_of(e).filter(|&c| drawn(c)) {
            let open: Vec<usize> = (kind.dressing.across[c].iter().enumerate())
                .filter(|(_, across)| across.iter().flatten().any(|&o| !drawn(usize::from(o))))
                .map(|(k, _)| k)
                .collect();
            if open.is_empty() {
                continue; // Every face shut, or on the hull.
            }
            let faces = kind.layout.blocks[c].faces();
            for k in open {
                breaks.open.insert((c, k));
                let new = edges(&faces[k].1).filter(|&(a, b)| seen.insert(edge(a, b)));
                breaks.edges.extend(new);
            }
        }
        breaks
    }

    /// Those that can reach the box `min`–`max`: within `wavelength` of it.
    fn near(&self, (min, max): (Vec3, Vec3), wavelength: f32) -> Vec<(Vec3, Vec3)> {
        let (lo, hi) = (min - wavelength, max + wavelength);
        self.edges
            .iter()
            .filter(|(a, b)| a.min(*b).cmple(hi).all() && a.max(*b).cmpge(lo).all())
            .copied()
            .collect()
    }
}

/// One less `p`'s distance to the nearest of `breaks` over `wavelength`, at least zero.
fn reach(breaks: &[(Vec3, Vec3)], p: Vec3, wavelength: f32) -> f32 {
    let nearest = breaks
        .iter()
        .map(|&(a, b)| {
            let t = ((p - a).dot(b - a) / a.distance_squared(b)).clamp(0.0, 1.0);
            p.distance(a.lerp(b, t))
        })
        .fold(f32::INFINITY, f32::min);
    (1.0 - nearest / wavelength).max(0.0)
}

impl Relief {
    /// The field it breaks, and the fix.
    fn check(&self) -> Result<(), (&'static str, String)> {
        for (field, value) in [
            ("amplitude_m", self.amplitude_m.0),
            ("wavelength_m", self.wavelength_m.0),
            ("edge_pitch_m", self.edge_pitch_m.0),
        ] {
            if !(value.is_finite() && value > 0.0) {
                return Err((field, "use a positive finite length".into()));
            }
        }
        if self.amplitude_m.0 > MAX_WARP_SLOPE * self.wavelength_m.0 {
            return Err((
                "amplitude_m",
                format!(
                    "keep it within {MAX_WARP_SLOPE} × wavelength_m, or a break outline can fold"
                ),
            ));
        }
        self.units.as_ref().map_or(Ok(()), Units::check)
    }

    /// The field's displacement at `p` in the element bounded by `hull` (its planes), before its
    /// reach (module doc).
    fn shift(&self, p: Vec3, hull: &[(Vec3, f32)]) -> Vec3 {
        let wavelength = self.wavelength_m.0;
        let at = p / wavelength;
        let mut d = Vec3::new(noise(at, 0), noise(at, 1), noise(at, 2)) * 2.0 - Vec3::ONE;
        d *= self.amplitude_m.0 / 3.0_f32.sqrt();
        let mut near: Vec<(Vec3, f32)> = hull
            .iter()
            .map(|&(n, offset)| (n, 1.0 - (n.dot(p) - offset).abs() / wavelength))
            .filter(|(_, weight)| *weight > 0.0)
            .collect();
        // Nearest first, each against the directions already taken out, so a point on several hull
        // planes keeps none of their normals whichever order the hull lists them in, and a plane
        // parallel to a nearer one adds nothing.
        near.sort_by(|a, b| b.1.total_cmp(&a.1));
        let mut taken: Vec<Vec3> = Vec::with_capacity(3);
        for (n, weight) in near {
            let rest = taken.iter().fold(n, |m, u| m - *u * u.dot(m));
            if rest.length_squared() < 1.0 - ON_PLANE_MIN_COS * ON_PLANE_MIN_COS {
                continue;
            }
            let u = rest.normalize();
            // A fade, not a cut: a hard projection would tear a face at the fade's edge.
            d -= u * (u.dot(d) * weight);
            taken.push(u);
        }
        d
    }

    /// `p` as drawn in the element bounded by `hull` beside the breaks `near` it.
    fn drawn(&self, p: Vec3, hull: &[(Vec3, f32)], near: &[(Vec3, Vec3)]) -> Vec3 {
        let reach = reach(near, p, self.wavelength_m.0);
        if reach > 0.0 {
            p + self.shift(p, hull) * reach
        } else {
            p
        }
    }

    /// Edge `a`–`b`'s points from `a`, its far end left to the next edge: `a`, then, where a break
    /// `near` it reaches the edge, a point every `edge_pitch_m` (module doc).
    fn points(&self, a: Vec3, b: Vec3, near: &[(Vec3, Vec3)]) -> Vec<Vec3> {
        if near.is_empty() {
            return vec![a];
        }
        let (lo, hi) = if key(a) <= key(b) { (a, b) } else { (b, a) };
        let steps = (lo.distance(hi) / self.edge_pitch_m.0).ceil().max(1.0) as usize;
        let mut inner: Vec<Vec3> = (1..steps)
            .map(|k| lo.lerp(hi, k as f32 / steps as f32))
            .collect();
        if lo != a {
            inner.reverse();
        }
        let reached = [a, b]
            .iter()
            .chain(&inner)
            .any(|&p| reach(near, p, self.wavelength_m.0) > 0.0);
        let mut points = vec![a];
        if reached {
            points.extend(inner);
        }
        points
    }

    /// Chunk `c`'s faces as drawn beside `breaks`, in [`Block::faces`] order (module doc).
    fn outlines(&self, kind: &ConstructionKind, c: usize, breaks: &Breaks) -> Vec<Outline> {
        let hull = &kind.layout.element_blocks[usize::from(kind.layout.element_of[c])].planes;
        let block = &kind.layout.blocks[c];
        let near = breaks.near((block.min, block.max), self.wavelength_m.0);
        block
            .faces()
            .into_iter()
            .zip(&kind.dressing.across[c])
            .enumerate()
            .map(|(k, ((n, corners), across))| {
                let drawn = edges(&corners)
                    .flat_map(|(a, b)| self.points(a, b, &near))
                    .map(|p| self.drawn(p, hull, &near))
                    .collect();
                Outline {
                    n,
                    open: breaks.open.contains(&(c, k)),
                    cut: across.is_some(),
                    drawn,
                    corners,
                }
            })
            .collect()
    }

    /// Edge `a`–`b` of element `e` as drawn beside its `breaks`, both ends included (module doc).
    pub(super) fn line(
        &self,
        kind: &ConstructionKind,
        e: usize,
        breaks: &Breaks,
        (a, b): (Vec3, Vec3),
    ) -> Vec<Vec3> {
        let hull = &kind.layout.element_blocks[e].planes;
        let near = breaks.near((a.min(b), a.max(b)), self.wavelength_m.0);
        let mut points = self.points(a, b, &near);
        points.push(b);
        (points.into_iter())
            .map(|p| self.drawn(p, hull, &near))
            .collect()
    }

    /// Element `e`'s first chunk drawn alone whose open face's drawn outline folds, if any.
    fn first_fold(&self, kind: &ConstructionKind, e: usize) -> Option<usize> {
        kind.layout.chunks_of(e).find(|&c| {
            let alone = Breaks::of(kind, e, &|o| o == c);
            (self.outlines(kind, c, &alone).iter())
                .any(|face| face.open && ears(&face.flat(&face.drawn)).is_none())
        })
    }

    /// Chunk `c` of element `look`'s faces, drawn `offset` back and tagged `tag`, beside `breaks`
    /// (module doc).
    pub(super) fn chunk(
        &self,
        faces: &mut Faces,
        kind: &ConstructionKind,
        c: usize,
        look: &ElementLook,
        (offset, tag): (Vec3, f32),
        breaks: &Breaks,
    ) {
        let e = usize::from(kind.layout.element_of[c]);
        for face in self.outlines(kind, c, breaks) {
            let tint = skin_tint(kind, e, face.n, &face.corners);
            if face.drawn == face.corners {
                faces.face(face.n, &face.corners, look, offset, (tag, tint));
                continue;
            }
            let triangles = face.triangles();
            let mut normals = vec![if face.open { Vec3::ZERO } else { face.n }; face.drawn.len()];
            if face.open {
                for &[i, j, k] in &triangles {
                    let [a, b, c] = [i, j, k].map(|v| face.drawn[v]);
                    for v in [i, j, k] {
                        normals[v] += (b - a).cross(c - a);
                    }
                }
            }
            let skin = if face.cut {
                Skin::Core
            } else {
                Skin::Hull(tint)
            };
            let vertices: Vec<(Vec3, Vec3)> = (face.drawn.iter().zip(normals))
                .map(|(&p, normal)| {
                    let normal = normal.try_normalize();
                    (
                        p,
                        normal.expect(
                            "every outline vertex is a corner of a triangle of positive area",
                        ),
                    )
                })
                .collect();
            faces.polygon(face.n, &vertices, &triangles, look, offset, (tag, skin));
        }
    }
}

impl Units {
    /// The field it breaks, and the fix.
    fn check(&self) -> Result<(), (&'static str, String)> {
        for (field, value) in [
            ("units.joint_m", self.joint_m.0),
            ("units.course_m", self.course_m.0),
            ("units.head_m", self.head_m.0),
            ("units.leaf_m", self.leaf_m.0),
        ] {
            if !(value.is_finite() && value > 0.0) {
                return Err((field, "use a positive finite length".into()));
            }
        }
        for (field, value) in [
            ("units.course_phase_m", self.course_phase_m.0),
            ("units.stagger", self.stagger.0),
        ] {
            if !value.is_finite() {
                return Err((field, "use a finite value".into()));
            }
        }
        if self.joint_m.0 >= self.course_m.0.min(self.head_m.0).min(self.leaf_m.0) {
            return Err((
                "units.joint_m",
                "keep the joint narrower than course_m, head_m and leaf_m, or the break shows no unit"
                    .into(),
            ));
        }
        if (self.mortar_linear.0.iter()).any(|v| !(0.0..=1.0).contains(v)) {
            return Err(("units.mortar_linear", "use linear RGB in [0, 1]".into()));
        }
        Ok(())
    }
}

/// One face of a chunk as drawn ([`Relief::outlines`]).
struct Outline {
    /// Outward unit normal of its law plane.
    n: Vec3,
    /// Its law corners, counter-clockwise seen from outside.
    corners: Vec<Vec3>,
    /// Its corners with the points its reached edges add, in order, as drawn.
    drawn: Vec<Vec3>,
    /// Whether it lies on none of its element's hull planes.
    cut: bool,
    /// Whether a chunk across it is not drawn with its own.
    open: bool,
}

impl Outline {
    /// Its triangles over `drawn`, each facing out.
    fn triangles(&self) -> Vec<[usize; 3]> {
        ears(&self.flat(&self.drawn)).expect(
            "a drawn outline never folds: MAX_WARP_SLOPE bounds the field, the load gate's \
             refuse_folds swept every open face",
        )
    }

    /// `outline` seen along its normal.
    fn flat(&self, outline: &[Vec3]) -> Vec<Vec2> {
        let t = (self.corners[1] - self.corners[0]).normalize();
        let b = self.n.cross(t);
        outline
            .iter()
            .map(|p| Vec2::new(p.dot(t), p.dot(b)))
            .collect()
    }
}

/// Per law chunk, per face of its block in [`Block::faces`] order: `None` on one of its element's
/// hull planes, else the chunks of its element whose joints lie on that face. Refuses a joint on no
/// face of its chunk, and one inside an element on a hull face (a cell too thin to tell apart).
pub(super) fn across(layout: &Layout) -> Result<Vec<Vec<Option<Vec<u16>>>>, String> {
    let mut across: Vec<Vec<Option<Vec<u16>>>> = (0..layout.blocks.len())
        .map(|c| {
            let hull = &layout.element_blocks[usize::from(layout.element_of[c])].planes;
            layout.blocks[c]
                .faces()
                .into_iter()
                .map(|(n, corners)| {
                    let on_hull = hull.iter().any(|&plane| on_plane(plane, n, &corners));
                    (!on_hull).then(Vec::new)
                })
                .collect()
        })
        .collect();
    for (j, joint) in layout.joints.iter().enumerate() {
        let Side::Chunk(b) = joint.b else {
            continue; // An anchor lies on the hull.
        };
        if layout.element_of[joint.a] != layout.element_of[b] {
            continue; // Another element's joint lies on the hull.
        }
        for (c, other) in [(joint.a, b), (b, joint.a)] {
            // The plane its normal matches best: two faces a few degrees apart both hold a
            // centroid near their shared edge.
            let planes = &layout.blocks[c].planes;
            let aligned = |k: usize| planes[k].0.dot(joint.normal).abs();
            let on =
                |(n, o): (Vec3, f32)| (n.dot(joint.centroid) - o).abs() < CRACK_FACE_TOLERANCE_M;
            let face = (0..planes.len())
                .filter(|&k| aligned(k) > ON_PLANE_MIN_COS && on(planes[k]))
                .max_by(|&i, &j| aligned(i).total_cmp(&aligned(j)))
                .ok_or_else(|| {
                    format!("joint {j} lies on no face of its chunk {c}; re-bake the construction")
                })?;
            across[c][face]
                .as_mut()
                .ok_or_else(|| {
                    format!(
                        "chunk {c}'s face holding joint {j} lies on its element's hull: a cell \
                         there is thinner than {CRACK_FACE_TOLERANCE_M} m; re-bake it without"
                    )
                })?
                .push(u16::try_from(other).expect("law chunks fit the wire's u16"));
        }
    }
    Ok(across)
}

/// Ear clipping of the polygon `points`, counter-clockwise, taking the fattest ear each time, or
/// `None` where it folds and no ear is left. A vertex on a straight run is never an ear's tip, so
/// every triangle has positive area, and blocks any ear across the run, so the run's vertices all
/// join the triangles: a run straight on this plane may still bend off it.
fn ears(points: &[Vec2]) -> Option<Vec<[usize; 3]>> {
    let n = points.len();
    let mut next: Vec<usize> = (0..n).map(|i| (i + 1) % n).collect();
    let mut prev: Vec<usize> = (0..n).map(|i| (i + n - 1) % n).collect();
    let mut alive = vec![true; n];
    let twice_area =
        |a: usize, b: usize, c: usize| (points[b] - points[a]).perp_dot(points[c] - points[a]);
    // An ear's fatness: its area over its summed squared sides, largest for an equilateral one.
    let fatness = |i: usize, prev: &[usize], next: &[usize], alive: &[bool]| -> Option<f32> {
        let (a, c) = (prev[i], next[i]);
        let area = twice_area(a, i, c);
        let off_edge = |p: usize, q: usize, v: usize| {
            let edge = points[q] - points[p];
            let t = ((points[v] - points[p]).dot(edge) / edge.length_squared()).clamp(0.0, 1.0);
            points[v].distance(points[p] + edge * t)
        };
        let lo = points[a].min(points[i]).min(points[c]) - Vec2::splat(ON_EAR_M);
        let hi = points[a].max(points[i]).max(points[c]) + Vec2::splat(ON_EAR_M);
        let on = |v: usize| {
            if points[v].cmplt(lo).any() || points[v].cmpgt(hi).any() {
                return false; // Outside the ear's box, so farther than ON_EAR_M from it.
            }
            let sides = [(a, i), (i, c), (c, a)];
            sides.iter().all(|&(p, q)| twice_area(p, q, v) >= 0.0)
                || sides.iter().any(|&(p, q)| off_edge(p, q, v) <= ON_EAR_M)
        };
        let blocked = (0..n).any(|v| alive[v] && ![a, i, c].contains(&v) && on(v));
        let spread = points[a].distance_squared(points[i])
            + points[i].distance_squared(points[c])
            + points[c].distance_squared(points[a]);
        (area > STRAIGHT_M2 && !blocked).then_some(area / spread)
    };
    let mut score: Vec<Option<f32>> = (0..n).map(|i| fatness(i, &prev, &next, &alive)).collect();
    let mut triangles = Vec::with_capacity(n.saturating_sub(2));
    for _ in 3..n {
        let (i, _) = (0..n)
            .filter(|&i| alive[i])
            .filter_map(|i| score[i].map(|s| (i, s)))
            .max_by(|a, b| a.1.total_cmp(&b.1))?;
        let (a, c) = (prev[i], next[i]);
        triangles.push([a, i, c]);
        alive[i] = false;
        next[a] = c;
        prev[c] = a;
        score[a] = fatness(a, &prev, &next, &alive);
        score[c] = fatness(c, &prev, &next, &alive);
    }
    let last = (0..n).find(|&i| alive[i]).expect("three vertices remain");
    let [a, i, c] = [prev[last], last, next[last]];
    triangles.push([a, i, c]);
    (twice_area(a, i, c) > STRAIGHT_M2).then_some(triangles)
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;
    use crate::construction::tests::shipped_house;

    /// The relief chunk `c`'s element draws, if any.
    fn relief_of(kind: &ConstructionKind, c: usize) -> Option<&'static Relief> {
        ElementLook::of(&kind.asset, usize::from(kind.layout.element_of[c])).relief
    }

    /// The chunks across `c`'s faces.
    fn neighbours(kind: &ConstructionKind, c: usize) -> impl Iterator<Item = usize> + '_ {
        kind.dressing.across[c]
            .iter()
            .flatten()
            .flatten()
            .map(|&o| usize::from(o))
    }

    /// Chunk `c`'s face toward `other`, among its drawn `faces`.
    fn toward<'a>(
        kind: &ConstructionKind,
        faces: &'a [Outline],
        c: usize,
        other: usize,
    ) -> &'a Outline {
        let k = kind.dressing.across[c]
            .iter()
            .position(|across| across.iter().flatten().any(|&o| usize::from(o) == other))
            .expect("a neighbour lies across one face");
        &faces[k]
    }

    /// Whether two faces draw the same points: two chunks' baked copies of one corner may differ
    /// in their last bits, so a point matches within 10 µm.
    fn alike(a: &Outline, b: &Outline) -> bool {
        a.drawn.len() == b.drawn.len()
            && a.drawn
                .iter()
                .all(|p| b.drawn.iter().any(|q| p.distance(*q) < 1e-5))
    }

    /// Whether no two edges of the closed polygon `points` that share no corner cross.
    fn simple(points: &[Vec2]) -> bool {
        let n = points.len();
        let turn = |a: Vec2, b: Vec2, c: Vec2| (b - a).perp_dot(c - a);
        let cross = |i: usize, j: usize| {
            let (a, b, c, d) = (
                points[i],
                points[(i + 1) % n],
                points[j],
                points[(j + 1) % n],
            );
            turn(a, b, c) * turn(a, b, d) < 0.0 && turn(c, d, a) * turn(c, d, b) < 0.0
        };
        (0..n).all(|i| (i + 2..n).all(|j| (j + 1) % n == i || !cross(i, j)))
    }

    /// Chunk `c` drawn beside `breaks` into a mesh of its own, tagged `tag`: each face draws its
    /// outline as drawn, simple seen along its normal, in `n − 2` triangles for `n` points, each
    /// facing out of its face and using each outline edge once, as the outline runs; every
    /// outline edge is drawn back by another face, so the surface closes; every vertex carries
    /// `tag`.
    fn assert_drawn(kind: &ConstructionKind, relief: &Relief, c: usize, breaks: &Breaks) {
        let look = ElementLook::of(&kind.asset, usize::from(kind.layout.element_of[c]));
        let tag = (c + 1) as f32;
        let mut faces = Faces::default();
        relief.chunk(&mut faces, kind, c, &look, (Vec3::ZERO, tag), breaks);
        let at = |v: u32| Vec3::from_array(faces.positions[v as usize]);
        let (mut first, mut triangles) = (0, faces.indices.chunks(3));
        let mut rims = HashSet::new();
        for face in relief.outlines(kind, c, breaks) {
            let n = face.drawn.len();
            assert!(
                simple(&face.flat(&face.drawn)),
                "chunk {c}: an outline folds"
            );
            let mut used: HashMap<(u32, u32), usize> = HashMap::new();
            for triangle in triangles.by_ref().take(n - 2) {
                let [a, b, d] = [0, 1, 2].map(|k| at(triangle[k]));
                let normal = (b - a).cross(d - a);
                assert!(
                    normal.dot(face.n) > 0.0,
                    "chunk {c}: a triangle turned away"
                );
                for k in 0..3 {
                    *used
                        .entry((triangle[k], triangle[(k + 1) % 3]))
                        .or_default() += 1;
                }
            }
            for i in 0..n {
                let (p, q) = (first + i as u32, first + ((i + 1) % n) as u32);
                assert_eq!(
                    at(p),
                    face.drawn[i],
                    "chunk {c}: the mesh draws the outline"
                );
                assert!(
                    used.get(&(p, q)) == Some(&1) && !used.contains_key(&(q, p)),
                    "chunk {c}: an outline edge not used once by its face's triangles"
                );
                rims.insert((key(at(p)), key(at(q))));
            }
            first += n as u32;
        }
        assert!(triangles.next().is_none() && first as usize == faces.positions.len());
        for &(p, q) in &rims {
            assert!(
                rims.contains(&(q, p)),
                "chunk {c}: an edge drawn one way only"
            );
        }
        assert!(faces.packs.iter().all(|pack| pack[1] == tag), "chunk {c}");
    }

    /// A BREAK DRAWS CLOSED SURFACES THAT AGREE ACROSS IT. Each relieved chunk of the shipped
    /// house in turn is gone (the rest of its element drawn without it) and alone (a piece of
    /// one): it and every chunk the break reaches draw a closed mesh of simple outlines and
    /// outward triangles ([`assert_drawn`]), a neighbour's face toward the gone chunk matches the
    /// piece's face toward it, and two neighbours draw the face they share alike. The view relies
    /// on it for a rim without slits or dark facets and a piece that fits its hole. An open set
    /// per chunk, an edge sampled from its own start, a warp read in a piece's frame, or a field
    /// steeper than its fold bound fails it.
    #[test]
    fn a_break_draws_closed_surfaces_that_agree_across_it() {
        let kind = shipped_house();
        let layout = &kind.layout;
        let mut breaks = 0;
        for gone in 0..layout.blocks.len() {
            let Some(relief) = relief_of(&kind, gone) else {
                continue; // A bare visual draws no relief.
            };
            let e = usize::from(layout.element_of[gone]);
            let alone = Breaks::of(&kind, e, &|o| o == gone);
            assert_drawn(&kind, relief, gone, &alone);
            let piece = relief.outlines(&kind, gone, &alone);
            let rim = Breaks::of(&kind, e, &|o| o != gone);
            let reached = layout.chunks_of(e).filter(|&c| {
                let block = &layout.blocks[c];
                c != gone
                    && !rim
                        .near((block.min, block.max), relief.wavelength_m.0)
                        .is_empty()
            });
            for c in reached {
                assert_drawn(&kind, relief, c, &rim);
            }
            let near: Vec<(usize, Vec<Outline>)> = neighbours(&kind, gone)
                .map(|c| (c, relief.outlines(&kind, c, &rim)))
                .collect();
            for (c, faces) in &near {
                let (face, back) = (
                    toward(&kind, faces, *c, gone),
                    toward(&kind, &piece, gone, *c),
                );
                assert!(
                    face.open && alike(face, back),
                    "chunk {c} and its piece {gone}"
                );
                for (d, other) in near
                    .iter()
                    .filter(|(d, _)| neighbours(&kind, *c).any(|o| o == *d))
                {
                    let (shared, back) =
                        (toward(&kind, faces, *c, *d), toward(&kind, other, *d, *c));
                    assert!(alike(shared, back), "chunks {c} and {d} beside {gone}");
                }
            }
            breaks += 1;
        }
        assert!(breaks > 0, "the shipped house draws relief");
    }

    /// A FACE NO BREAK REACHES DRAWS ITS BARE FACE. The shipped house with every element swapped:
    /// with nothing fallen every chunk, and with one relieved chunk fallen every chunk farther than
    /// `wavelength_m` from it, draws exactly the vertices and triangles its bare law faces draw
    /// ([`Faces::hull`]). The view relies on it for a wall that reads unchanged until a chunk goes.
    /// Relief drawn on every cut face, or a field reaching past its wavelength, fails it.
    #[test]
    fn a_face_no_break_reaches_draws_its_bare_face() {
        let kind = shipped_house();
        let layout = &kind.layout;
        let gone = (0..layout.blocks.len())
            .find(|&c| relief_of(&kind, c).is_some())
            .expect("the shipped house draws relief");
        let wavelength = relief_of(&kind, gone).expect("relieved").wavelength_m.0;
        let apart = |c: usize| {
            let (a, b) = (&layout.blocks[c], &layout.blocks[gone]);
            (a.min - b.max).max(b.min - a.max).max(Vec3::ZERO).length()
        };
        for fallen in [None, Some(gone)] {
            let drawn = |c: usize| Some(c) != fallen;
            for e in 0..layout.element_blocks.len() {
                let look = ElementLook::of(&kind.asset, e);
                let breaks = Breaks::of(&kind, e, &drawn);
                for c in layout.chunks_of(e) {
                    if fallen.is_some_and(|_| apart(c) <= wavelength) {
                        continue; // The break reaches it.
                    }
                    let (mut relieved, mut bare) = (Faces::default(), Faces::default());
                    relieved.chunk(&kind, c, &look, (Vec3::ZERO, 1.0), &breaks);
                    bare.hull(&kind, c, &look, (Vec3::ZERO, 1.0));
                    bare.cards(&kind, &look, |owner| owner == c, Vec3::ZERO);
                    assert!(relieved == bare, "chunk {c} beside {fallen:?}");
                }
            }
        }
    }

    /// A BROKEN MESH REBUILDS ONLY WHAT MOVED. The shipped house with every element swapped is
    /// drawn, each kept section marked, then one chunk falls: [`broken_mesh`] rebuilds only its
    /// element's section, and it equals a fresh draw. A redraw's cost following the break, not the
    /// house, relies on it. A cache rebuilt every draw, or one never rebuilt, fails it.
    #[test]
    fn a_broken_mesh_rebuilds_only_the_element_that_changed() {
        let kind = shipped_house();
        let mut shape = Shape::intact(&kind.layout);
        shape.swapped.fill(true);
        let mut sections = Sections::default();
        broken_mesh(&kind, &shape, &mut sections);
        let mark = [f32::MAX; 3];
        for section in &mut sections.0 {
            section.as_mut().expect("every element drawn").1.positions[0] = mark;
        }
        let c = kind
            .layout
            .chunks_of(0)
            .next()
            .expect("a swapped element holds chunks");
        shape.fate[c] = Fate::Fallen;
        broken_mesh(&kind, &shape, &mut sections);
        let mut fresh = Sections::default();
        broken_mesh(&kind, &shape, &mut fresh);
        for (e, (kept, fresh)) in sections.0.iter().zip(&fresh.0).enumerate() {
            let (kept, fresh) = (kept.as_ref().unwrap(), fresh.as_ref().unwrap());
            assert_eq!(kept.1.positions[0] == mark, e != 0, "element {e}");
            assert!(
                e != 0 || kept == fresh,
                "the changed element as a fresh draw"
            );
        }
    }

    /// A RELIEF STEEPER THAN ITS FOLD BOUND IS REFUSED, naming the field: past `MAX_WARP_SLOPE` a
    /// break's outline can fold, which every drawn face's triangles rely on not happening.
    #[test]
    fn a_relief_steeper_than_its_fold_bound_is_refused() {
        let (_, mut steep) = (parse(SHIPPED)
            .expect("the shipped reliefs")
            .into_iter()
            .next())
        .expect("a shipped relief");
        assert_eq!(steep.check(), Ok(()));
        steep.amplitude_m.0 = 1.01 * MAX_WARP_SLOPE * steep.wavelength_m.0;
        assert_eq!(
            steep.check().map_err(|(field, _)| field),
            Err("amplitude_m")
        );
    }

    /// A RELIEF THAT FOLDS A BREAK IS FOUND AT LOAD: the shipped house under its relief past the
    /// fold bound tenfold (as no parsed file can be) has a chunk whose open face folds, and under
    /// its shipped relief none ([`refuse_folds`], which names that chunk). The draw's never-fold
    /// panic relies on the load gate holding every open face. A sweep that skips a face, or reads
    /// the shipped relief whatever it is handed, fails here.
    #[test]
    fn a_relief_that_folds_a_break_is_found_at_load() {
        let kind = shipped_house();
        let e = (0..kind.layout.element_blocks.len())
            .find(|&e| ElementLook::of(&kind.asset, e).relief.is_some())
            .expect("the shipped house draws relief");
        let (_, mut steep) = (parse(SHIPPED)
            .expect("the shipped reliefs")
            .into_iter()
            .next())
        .expect("a shipped relief");
        assert_eq!(steep.first_fold(&kind, e), None);
        steep.amplitude_m.0 = 10.0 * MAX_WARP_SLOPE * steep.wavelength_m.0;
        assert!(steep.first_fold(&kind, e).is_some());
    }
}
