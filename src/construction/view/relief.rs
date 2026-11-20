//! Break relief, view only (ADR-0014): an element whose visual has an entry in
//! `assets/materials/fracture.ron` draws each chunk face a departed neighbour opened, and the rims
//! beside it, displaced by one bounded field, so a break reads rough while the collider stays the
//! law hull. A face no break touches draws its flat hull face, exactly as without relief.
//!
//! - **Open faces.** A chunk's face on none of its element's hull planes is a cut; it is open while
//!   a chunk across it ([`across`]) is not drawn with it (gone, or on another piece). Every corner
//!   of an open face moves, and every edge of one is cut every `edge_pitch_m` from its lexically
//!   first end, so each face drawing the edge draws the same points.
//! - **One field.** A point moves by [`Relief::warp`], a function of its asset-frame position and
//!   its element's hull alone: faces sharing an edge draw one curve, and a fallen piece keeps its
//!   relief. Within `wavelength_m` of a hull plane the field fades its component along that plane's
//!   normal, so a point on the hull stays on it and the element's silhouette never moves. No point
//!   moves farther than `amplitude_m`.
//! - **Triangles.** A face with a moved point ear-clips its warped outline seen along its normal.
//!   An open face's never folds ([`MAX_WARP_SLOPE`]) and takes smooth normals. Another face moves
//!   only some points, so its outline folds where it is narrower than the relief beside a rim;
//!   there it clips its law outline instead, and it keeps its plane's normal.
//! - **Optics.** A hull face keeps its photograph framed in metres. A cut face is core: lane
//!   `2 + lane` in UV1.x, its vertex colour its substance's exposed colour, and its drawn
//!   asset-frame position along the element's run, up and across its depth in UV0 and the colour's
//!   alpha, so the shader draws the visual's units ([`break_params`]) where the construction's own
//!   joint planes cross the face, moving with a fallen piece.

use std::collections::HashSet;
use std::sync::LazyLock;

use super::*;
use crate::provenance::Sourced;

const FILE: &str = "assets/materials/fracture.ron";

/// A visual's break look (module doc), its entry in [`FILE`].
#[derive(Debug, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Relief {
    amplitude_m: Sourced<f32>,
    wavelength_m: Sourced<f32>,
    edge_pitch_m: Sourced<f32>,
    mortar_linear: Sourced<[f32; 3]>,
    joint_m: Sourced<f32>,
    course_m: Sourced<f32>,
    course_phase_m: Sourced<f32>,
    head_m: Sourced<f32>,
    leaf_m: Sourced<f32>,
}

/// The largest `amplitude_m / wavelength_m` that keeps a warped face's outline simple. Within one
/// lattice tetrahedron each displacement component changes by at most `2 amplitude / (√3
/// wavelength)` per metre along an axis, so the in-plane part of the field's Jacobian has Frobenius
/// norm at most `2.31 amplitude / wavelength`; each of at most two hull fades crossing a face adds
/// `amplitude / wavelength`. Below `1 / 4.31` the warp of a plane stays invertible (DERIVED), so a
/// convex outline stays a simple polygon.
const MAX_WARP_SLOPE: f32 = 0.23;

/// The relief of `visual`, `None` for a visual drawn as its bare hull.
pub(super) fn of(visual: &str) -> Option<&'static Relief> {
    static RELIEFS: LazyLock<BTreeMap<String, Relief>> = LazyLock::new(|| {
        parse(include_str!("../../../assets/materials/fracture.ron"))
            .unwrap_or_else(|err| panic!("{err}"))
    });
    RELIEFS.get(visual)
}

/// The construction material's break records for the visual a palette lane draws
/// ([`crate::construction_surface::SurfaceParams`]): `(mortar linear RGB, joint width m)` and
/// `(courses per metre, a bed joint's height m, heads per metre, leaves per metre)`; zero for a
/// visual without relief, which draws no core face.
pub(crate) fn break_params(visual: &str) -> [Vec4; 2] {
    of(visual).map_or([Vec4::ZERO; 2], |r| {
        [
            Vec3::from_array(r.mortar_linear.0).extend(r.joint_m.0),
            Vec4::new(
                1.0 / r.course_m.0,
                r.course_phase_m.0,
                1.0 / r.head_m.0,
                1.0 / r.leaf_m.0,
            ),
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

impl Relief {
    /// The field it breaks, and the fix.
    fn check(&self) -> Result<(), (&'static str, String)> {
        for (field, value) in [
            ("amplitude_m", self.amplitude_m.0),
            ("wavelength_m", self.wavelength_m.0),
            ("edge_pitch_m", self.edge_pitch_m.0),
            ("joint_m", self.joint_m.0),
            ("course_m", self.course_m.0),
            ("head_m", self.head_m.0),
            ("leaf_m", self.leaf_m.0),
        ] {
            if !(value.is_finite() && value > 0.0) {
                return Err((field, "use a positive finite length".into()));
            }
        }
        if !self.course_phase_m.0.is_finite() {
            return Err(("course_phase_m", "use a finite height".into()));
        }
        if self.joint_m.0 >= self.course_m.0.min(self.head_m.0).min(self.leaf_m.0) {
            return Err((
                "joint_m",
                "keep the joint narrower than course_m, head_m and leaf_m, or the break shows no unit"
                    .into(),
            ));
        }
        if self.amplitude_m.0 > MAX_WARP_SLOPE * self.wavelength_m.0 {
            return Err((
                "amplitude_m",
                format!(
                    "keep it within {MAX_WARP_SLOPE} × wavelength_m, or a break outline can fold"
                ),
            ));
        }
        if self
            .mortar_linear
            .0
            .iter()
            .any(|v| !(0.0..=1.0).contains(v))
        {
            return Err(("mortar_linear", "use linear RGB in [0, 1]".into()));
        }
        Ok(())
    }

    /// `p` displaced by the field (module doc) in the element bounded by `hull` (its planes).
    fn warp(&self, p: Vec3, hull: &[(Vec3, f32)]) -> Vec3 {
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
        p + d
    }

    /// The points edge `a`–`b` draws between its ends, from `a` (module doc).
    fn samples(&self, a: Vec3, b: Vec3) -> Vec<Vec3> {
        let (lo, hi) = if key(a) <= key(b) { (a, b) } else { (b, a) };
        let steps = (lo.distance(hi) / self.edge_pitch_m.0).ceil().max(1.0) as usize;
        let mut points: Vec<Vec3> = (1..steps)
            .map(|k| lo.lerp(hi, k as f32 / steps as f32))
            .collect();
        if lo != a {
            points.reverse();
        }
        points
    }

    /// Chunk `c`'s faces as drawn beside the chunks `together` names, in [`Block::faces`] order
    /// (module doc).
    fn outlines(
        &self,
        kind: &ConstructionKind,
        c: usize,
        together: &dyn Fn(usize) -> bool,
    ) -> Vec<Outline> {
        let hull = &kind.layout.element_blocks[usize::from(kind.layout.element_of[c])].planes;
        let mut faces: Vec<Outline> = kind.layout.blocks[c]
            .faces()
            .into_iter()
            .zip(&kind.dressing.across[c])
            .map(|((n, corners), across)| Outline {
                n,
                open: across
                    .iter()
                    .flatten()
                    .any(|&other| !together(usize::from(other))),
                cut: across.is_some(),
                corners,
                points: Vec::new(),
                drawn: Vec::new(),
            })
            .collect();
        let open = faces.iter().filter(|face| face.open);
        let moved: HashSet<[u32; 3]> = open
            .clone()
            .flat_map(|face| face.corners.iter().map(|p| key(*p)))
            .collect();
        let rough: HashSet<[[u32; 3]; 2]> = open
            .flat_map(|face| edges(&face.corners).map(|(a, b)| edge(a, b)))
            .collect();
        for face in &mut faces {
            for (p, q) in edges(&face.corners) {
                let mut add = |point: Vec3, moves: bool| {
                    face.points.push(point);
                    face.drawn
                        .push(if moves { self.warp(point, hull) } else { point });
                };
                add(p, moved.contains(&key(p)));
                if rough.contains(&edge(p, q)) {
                    for point in self.samples(p, q) {
                        add(point, true);
                    }
                }
            }
        }
        faces
    }

    /// Chunk `c` of element `look`'s faces, drawn `offset` back and tagged `tag`, beside the chunks
    /// `together` names (module doc).
    pub(super) fn chunk(
        &self,
        faces: &mut Faces,
        kind: &ConstructionKind,
        c: usize,
        look: &ElementLook,
        (offset, tag): (Vec3, f32),
        together: &dyn Fn(usize) -> bool,
    ) {
        let e = usize::from(kind.layout.element_of[c]);
        let lane = if look.secondary { 1.0 } else { 0.0 };
        let depth = look.run.cross(Vec3::Y);
        for face in self.outlines(kind, c, together) {
            let tint = skin_tint(kind, e, face.n, &face.corners);
            if face.points == face.drawn {
                // No point moves and none was added: the face exactly as without relief.
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
            let t = tangent(face.n, look.frame, look.run);
            let b = face.n.cross(t);
            let first =
                u32::try_from(faces.positions.len()).expect("a construction's mesh fits u32");
            for (q, normal) in face.drawn.iter().zip(normals) {
                faces.positions.push((*q - offset).to_array());
                faces.normals.push(
                    normal
                        .try_normalize()
                        .expect("every outline vertex is a corner of a triangle of positive area")
                        .to_array(),
                );
                faces.tangents.push(t.extend(1.0).to_array());
                if face.cut {
                    faces.uvs.push([q.dot(look.run), q.y]);
                    faces.packs.push([2.0 + lane, tag]);
                    faces
                        .colors
                        .push(Vec3::from_array(look.core).extend(q.dot(depth)).to_array());
                } else {
                    faces.uvs.push([q.dot(t), -q.dot(b)]);
                    faces.packs.push([lane, tag]);
                    faces.colors.push(tint);
                }
            }
            faces.indices.extend(
                triangles
                    .iter()
                    .flat_map(|triangle| triangle.map(|v| first + v as u32)),
            );
        }
    }
}

/// One face of a chunk as drawn ([`Relief::outlines`]).
struct Outline {
    /// Outward unit normal of its law plane.
    n: Vec3,
    /// Its law corners, counter-clockwise seen from outside.
    corners: Vec<Vec3>,
    /// Its corners with the points its rough edges add, in order.
    points: Vec<Vec3>,
    /// Each of `points` as drawn.
    drawn: Vec<Vec3>,
    /// Whether it lies on none of its element's hull planes.
    cut: bool,
    /// Whether a chunk across it is not drawn with its own.
    open: bool,
}

impl Outline {
    /// Its triangles over `drawn`, each facing out where its drawn outline, seen along its
    /// normal, does not fold (module doc).
    fn triangles(&self) -> Vec<[usize; 3]> {
        match ears(&self.flat(&self.drawn)) {
            Some(triangles) => triangles,
            None if self.open => {
                panic!("MAX_WARP_SLOPE keeps an open face's drawn outline from folding")
            }
            None => ears(&self.flat(&self.points)).expect("a convex outline keeps an ear"),
        }
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
/// hull planes, else the chunks of its element whose joints lie on that face.
pub(super) fn across(layout: &Layout) -> Vec<Vec<Option<Vec<u16>>>> {
    let on = |(n, o): (Vec3, f32), p: Vec3| (n.dot(p) - o).abs() < CRACK_FACE_TOLERANCE_M;
    let mut across: Vec<Vec<Option<Vec<u16>>>> = (0..layout.blocks.len())
        .map(|c| {
            let hull = &layout.element_blocks[usize::from(layout.element_of[c])].planes;
            layout.blocks[c]
                .faces()
                .into_iter()
                .map(|(n, corners)| {
                    let on_hull = hull.iter().any(|&(out, offset)| {
                        out.dot(n) > ON_PLANE_MIN_COS
                            && corners.iter().all(|&p| on((out, offset), p))
                    });
                    (!on_hull).then(Vec::new)
                })
                .collect()
        })
        .collect();
    for joint in &layout.joints {
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
            let face = (0..planes.len())
                .filter(|&k| aligned(k) > ON_PLANE_MIN_COS && on(planes[k], joint.centroid))
                .max_by(|&i, &j| aligned(i).total_cmp(&aligned(j)))
                .expect("a joint lies on a face of each of its chunks");
            across[c][face]
                .as_mut()
                .expect("a joint inside an element lies on a cut")
                .push(u16::try_from(other).expect("law chunks fit the wire's u16"));
        }
    }
    across
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
    use super::*;
    use crate::construction::tests::shipped_house;

    /// The relief chunk `c`'s element draws, if any.
    fn relief_of(kind: &ConstructionKind, c: usize) -> Option<&'static Relief> {
        ElementLook::of(&kind.asset, usize::from(kind.layout.element_of[c])).relief
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

    /// One chunk's drawn faces close (every edge one face draws, another draws back), and a face
    /// turns a triangle away only where its drawn outline, seen along its normal, folds.
    fn assert_closed_and_unfolded(faces: &[Outline], c: usize) {
        let edges: HashSet<_> = faces
            .iter()
            .flat_map(|face| edges(&face.drawn).map(|(a, b)| (key(a), key(b))))
            .collect();
        for &(a, b) in &edges {
            assert!(
                edges.contains(&(b, a)),
                "chunk {c}: an edge drawn one way only"
            );
        }
        for face in faces {
            let turned = face.triangles().into_iter().any(|[i, j, k]| {
                let [a, b, d] = [i, j, k].map(|v| face.drawn[v]);
                (b - a).cross(d - a).dot(face.n) <= 0.0
            });
            let folds = || ears(&face.flat(&face.drawn)).is_none();
            assert!(!turned || folds(), "chunk {c}: a triangle turned away");
        }
    }

    /// A BREAK DRAWS CLOSED SURFACES THAT AGREE ACROSS IT. Each relieved chunk of the shipped
    /// house in turn is gone (its neighbours drawn without it) and alone (a piece of one): every
    /// drawn chunk closes, turns no triangle away unless its outline itself folds (an open one
    /// never does, `MAX_WARP_SLOPE`), a neighbour's face toward the gone chunk matches the piece's
    /// face toward it, and two neighbours draw the face they share alike. The view relies on it for
    /// a rim without slits or dark facets and a piece that fits its hole. A moved set per face, an
    /// edge sampled from its own start, a warp read in a piece's frame, or a face beside a break
    /// clipped on its law outline where its drawn one clips fails it.
    #[test]
    fn a_break_draws_closed_surfaces_that_agree_across_it() {
        let kind = shipped_house();
        let mut breaks = 0;
        for gone in 0..kind.layout.blocks.len() {
            let Some(relief) = relief_of(&kind, gone) else {
                continue; // A bare visual draws no relief.
            };
            let alone = relief.outlines(&kind, gone, &|o| o == gone);
            assert_closed_and_unfolded(&alone, gone);
            let near: Vec<(usize, Vec<Outline>)> = kind.dressing.across[gone]
                .iter()
                .flatten()
                .flatten()
                .map(|&c| {
                    let c = usize::from(c);
                    (c, relief.outlines(&kind, c, &|o| o != gone))
                })
                .collect();
            for (c, faces) in &near {
                assert_closed_and_unfolded(faces, *c);
                let (rim, piece) = (
                    toward(&kind, faces, *c, gone),
                    toward(&kind, &alone, gone, *c),
                );
                assert!(
                    rim.open && alike(rim, piece),
                    "chunk {c} and its piece {gone}"
                );
                for (d, other) in near.iter().filter(|(d, _)| d != c) {
                    if kind.dressing.across[*c]
                        .iter()
                        .flatten()
                        .flatten()
                        .any(|&o| usize::from(o) == *d)
                    {
                        let shared = toward(&kind, faces, *c, *d);
                        let back = toward(&kind, other, *d, *c);
                        assert!(alike(shared, back), "chunks {c} and {d} beside {gone}");
                    }
                }
            }
            breaks += 1;
        }
        assert!(breaks > 0, "the shipped house draws relief");
    }

    /// AN INTACT FACE IS TODAY'S FACE. The shipped house with every element swapped: with nothing
    /// fallen every chunk, and with one relieved chunk fallen every chunk not across it, draws
    /// exactly the vertices and triangles its bare law faces draw ([`Faces::face`]). The view
    /// relies on it for a wall that reads unchanged until a chunk goes. Relief drawn on every cut
    /// face, or a face clipped where nothing moved, fails it.
    #[test]
    fn a_face_no_break_reaches_draws_as_today() {
        let kind = shipped_house();
        let layout = &kind.layout;
        let gone = (0..layout.blocks.len())
            .find(|&c| relief_of(&kind, c).is_some())
            .expect("the shipped house draws relief");
        for fallen in [None, Some(gone)] {
            let together = |c: usize| Some(c) != fallen;
            for c in (0..layout.blocks.len()).filter(|&c| together(c)) {
                if fallen.is_some_and(|g| {
                    kind.dressing.across[g]
                        .iter()
                        .flatten()
                        .flatten()
                        .any(|&o| usize::from(o) == c)
                }) {
                    continue; // A neighbour of the break.
                }
                let e = usize::from(layout.element_of[c]);
                let look = ElementLook::of(&kind.asset, e);
                let (mut drawn, mut bare) = (Faces::default(), Faces::default());
                drawn.chunk(&kind, c, &look, (Vec3::ZERO, 1.0), &together);
                for (n, corners) in layout.blocks[c].faces() {
                    let tint = skin_tint(&kind, e, n, &corners);
                    bare.face(n, &corners, &look, Vec3::ZERO, (1.0, tint));
                }
                bare.cards(&kind, &look, |owner| owner == c, Vec3::ZERO);
                assert!(drawn == bare, "chunk {c} beside {fallen:?}");
            }
        }
    }

    /// A REDRAW REBUILDS ONLY WHAT MOVED. The shipped house with every element swapped is drawn,
    /// each kept section marked, then one chunk falls: only its element's section is rebuilt, and
    /// it equals a fresh draw. The view relies on it for a redraw whose cost follows the break,
    /// not the house. A cache rebuilt every redraw, or one never rebuilt, fails it.
    #[test]
    fn a_redraw_rebuilds_only_the_element_that_changed() {
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
    /// break's outline can fold, which the first test's outward triangles rely on not happening.
    #[test]
    fn a_relief_steeper_than_its_fold_bound_is_refused() {
        let shipped = parse(include_str!("../../../assets/materials/fracture.ron"))
            .expect("the shipped reliefs");
        let (_, mut steep) = shipped.into_iter().next().expect("a shipped relief");
        assert_eq!(steep.check(), Ok(()));
        steep.amplitude_m.0 = 1.01 * MAX_WARP_SLOPE * steep.wavelength_m.0;
        assert_eq!(
            steep.check().map_err(|(field, _)| field),
            Err("amplitude_m")
        );
    }
}
