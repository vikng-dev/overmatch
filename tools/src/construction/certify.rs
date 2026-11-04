//! What every bake certifies of its cut before writing it, whatever the construction (a house, a
//! trunk): the runtime builds colliders, law blocks and the joint graph from these chunks and
//! never re-derives them, so each is proved here from the geometry, never from a name or a count.
//!
//! - **Addresses.** Every piece's name is its own: an element's name is its address.
//! - **Closure.** Every chunk is a closed convex cell ([`Polyhedron::certify`]).
//! - **Partition.** Each piece's chunks lie inside it, their interiors are disjoint, and their
//!   volumes sum to its own: no gap, no overlap, no chunk twice.
//! - **Mass.** Every chunk's mass and inertia tensor, at its material's density, are finite in the
//!   f32 the game reads and positive definite.
//! - **Interfaces.** Inside a piece every face a cut made is matched by joints to the chunks across
//!   it, over its whole area.
//! - **Reach.** The ground holds an anchor, and every chunk reaches one through joints.

use std::collections::BTreeSet;

use glam::{DMat3, DVec3};
use overmatch::construction::schema::ConstructionAsset;

use super::PLANE_M;
use super::contact::MIN_AREA_M2;
use super::geom::Polyhedron;
use super::spec::Construction;

/// How far a sum of chunk volumes may sit from its piece's, relative to the piece's (Resolution:
/// many times the clip's welded-corner rounding, far below a chunk's share of any piece).
const VOLUME_REL: f64 = 1.0e-6;

/// Addresses, closure, partition and mass of `cut` (one run of chunks per piece of
/// `construction`, in order), each chunk weighed at `densities` (kg/m³, per piece). Names the
/// piece and chunk of the first breach.
pub fn cut(
    construction: &Construction,
    cut: &[Vec<Polyhedron>],
    densities: &[f64],
) -> Result<(), String> {
    let mut names = BTreeSet::new();
    for (piece, (chunks, &rho)) in construction.pieces.iter().zip(cut.iter().zip(densities)) {
        let name = &piece.name;
        if !names.insert(name.as_str()) {
            return Err(format!(
                "two pieces are named {name}: an element's name is its address"
            ));
        }
        let mut sum = 0.0;
        for (k, chunk) in chunks.iter().enumerate() {
            chunk
                .certify(PLANE_M)
                .map_err(|err| format!("chunk {k} of {name} is no closed convex cell ({err})"))?;
            for &p in &chunk.corners {
                if piece
                    .solid
                    .faces
                    .iter()
                    .any(|face| face.normal.dot(p) - face.offset > PLANE_M)
                {
                    return Err(format!("chunk {k} of {name} leaves its piece"));
                }
            }
            inertia(chunk, rho).map_err(|err| format!("chunk {k} of {name}: {err}"))?;
            sum += chunk.volume();
        }
        let whole = piece.solid.volume();
        if (sum - whole).abs() > VOLUME_REL * whole {
            return Err(format!(
                "{name}'s chunks hold {sum} m³ of its {whole}: the cut leaves a gap or an overlap"
            ));
        }
        let bounds: Vec<(DVec3, DVec3)> = chunks.iter().map(Polyhedron::bounds).collect();
        for (a, chunk) in chunks.iter().enumerate() {
            for b in a + 1..chunks.len() {
                let (lo, hi) = (bounds[a].0.max(bounds[b].0), bounds[a].1.min(bounds[b].1));
                if lo.cmpgt(hi).any() {
                    continue;
                }
                let shared = chunk
                    .faces
                    .iter()
                    .try_fold(chunks[b].clone(), |part, face| {
                        part.clip(face.normal, face.offset)
                    });
                if shared.is_some() {
                    return Err(format!("chunks {a} and {b} of {name} overlap"));
                }
            }
        }
    }
    Ok(())
}

/// `chunk`'s inertia tensor about its centroid at density `rho`, kg·m², refused unless finite in
/// f32 and positive definite: the signed tetrahedra from the centroid to each face's fan, each
/// `det / 120 · (Σ vᵢvᵢᵀ + (Σ vᵢ)(Σ vᵢ)ᵀ)` of second moment.
fn inertia(chunk: &Polyhedron, rho: f64) -> Result<DMat3, String> {
    let c = chunk.centroid();
    let mut second = DMat3::ZERO;
    for face in chunk.faces() {
        let first = face.corners[0] - c;
        for pair in face.corners.windows(2).skip(1) {
            let (p, q) = (pair[0] - c, pair[1] - c);
            let det = first.dot(p.cross(q));
            let sum = first + p + q;
            let outer = |v: DVec3| DMat3::from_cols(v * v.x, v * v.y, v * v.z);
            second += (outer(first) + outer(p) + outer(q) + outer(sum)) * (det / 120.0);
        }
    }
    let trace = second.x_axis.x + second.y_axis.y + second.z_axis.z;
    let tensor = (DMat3::from_diagonal(DVec3::splat(trace)) - second) * rho;
    let minors = [
        tensor.x_axis.x,
        tensor.x_axis.x * tensor.y_axis.y - tensor.x_axis.y * tensor.y_axis.x,
        tensor.determinant(),
    ];
    let representable = tensor
        .to_cols_array()
        .iter()
        .all(|&v| (v as f32).is_finite());
    if !(representable && minors.iter().all(|&m| m > 0.0)) {
        return Err(format!(
            "inertia {tensor:?} kg·m² is not finite and positive definite"
        ));
    }
    Ok(tensor)
}

/// Interfaces and reach of `asset`, baked from `cut` (one run per piece, the asset's chunk order).
pub fn graph(
    construction: &Construction,
    cut: &[Vec<Polyhedron>],
    asset: &ConstructionAsset,
) -> Result<(), String> {
    let mut joined = vec![0.0; asset.chunks.len()];
    for joint in &asset.joints {
        let (a, b) = (joint.a as usize, joint.b as usize);
        if asset.chunks[a].element == asset.chunks[b].element {
            joined[a] += f64::from(joint.area_m2);
            joined[b] += f64::from(joint.area_m2);
        }
    }
    let mut k = 0;
    for (piece, chunks) in construction.pieces.iter().zip(cut) {
        for (n, chunk) in chunks.iter().enumerate() {
            let interior: Vec<f64> = chunk
                .faces()
                .iter()
                .filter(|face| {
                    !piece.solid.faces.iter().any(|outer| {
                        outer.normal.dot(face.normal) > 1.0 - PLANE_M
                            && (outer.offset - face.offset).abs() <= PLANE_M
                    })
                })
                .map(|face| face.area())
                .collect();
            let area: f64 = interior.iter().sum();
            // The bake's canonical areas are f32 at a micrometre; a face under the contact
            // search's least overlap is no joint.
            let slack = interior.len() as f64 * MIN_AREA_M2 + 1.0e-6 * area;
            if (joined[k] - area).abs() > slack {
                return Err(format!(
                    "chunk {n} of {}: its cut faces span {area} m² and its joints inside the piece \
                     {} m²: an interface is unmatched",
                    piece.name, joined[k]
                ));
            }
            k += 1;
        }
    }
    reach(asset)
}

/// The ground holds an anchor and every chunk reaches one through joints.
fn reach(asset: &ConstructionAsset) -> Result<(), String> {
    if asset.anchors.is_empty() {
        return Err("no chunk face lies on the foundation plane: nothing holds it up".into());
    }
    let mut parent: Vec<usize> = (0..asset.chunks.len()).collect();
    fn root(parent: &mut [usize], mut k: usize) -> usize {
        while parent[k] != k {
            parent[k] = parent[parent[k]];
            k = parent[k];
        }
        k
    }
    for joint in &asset.joints {
        let (a, b) = (
            root(&mut parent, joint.a as usize),
            root(&mut parent, joint.b as usize),
        );
        parent[a] = b;
    }
    let ground: BTreeSet<usize> = asset
        .anchors
        .iter()
        .map(|anchor| root(&mut parent, anchor.chunk as usize))
        .collect();
    match (0..asset.chunks.len()).find(|&k| !ground.contains(&root(&mut parent, k))) {
        Some(k) => Err(format!(
            "chunk {k} of {} reaches no anchor through joints",
            asset.elements[usize::from(asset.chunks[k].element)].name
        )),
        None => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::super::{CutCount, Spec, assemble, fracture, schema, specs};
    use super::*;

    /// Every shipped construction and its cut, whatever it builds.
    fn shipped() -> Vec<(Spec, Vec<Vec<Polyhedron>>)> {
        specs(&crate::repo_root())
            .expect("the shipped specs parse")
            .into_iter()
            .map(|spec| {
                let cut = spec
                    .construction
                    .pieces
                    .iter()
                    .map(|piece| {
                        fracture::chunks(piece, &spec.construction.id, &mut CutCount::default())
                    })
                    .collect();
                (spec, cut)
            })
            .collect()
    }

    fn refusal(construction: &Construction, cut: Vec<Vec<Polyhedron>>) -> String {
        assemble(
            construction,
            cut,
            schema::substance_density,
            CutCount::default(),
        )
        .err()
        .unwrap_or_else(|| panic!("{}: the mutant bakes", construction.id))
    }

    /// The piece with the most chunks, and its chunk of the largest volume.
    fn largest(cut: &[Vec<Polyhedron>]) -> (usize, usize) {
        let piece = (0..cut.len()).max_by_key(|&p| cut[p].len()).unwrap();
        let chunk = (0..cut[piece].len())
            .max_by(|&a, &b| cut[piece][a].volume().total_cmp(&cut[piece][b].volume()))
            .unwrap();
        (piece, chunk)
    }

    /// THE SHIPPED CUTS ARE CERTIFIED, so each mutant below is refused for what it breaks and not
    /// for a construction the certificate never admits.
    #[test]
    fn every_shipped_cut_is_certified() {
        for (spec, cut) in shipped() {
            assemble(
                &spec.construction,
                cut,
                schema::substance_density,
                CutCount::default(),
            )
            .unwrap_or_else(|err| panic!("{}: {err}", spec.path));
        }
    }

    /// A GAP IS REFUSED: one cut face of the largest chunk drawn inward by a hundredth of the
    /// chunk's own depth along it leaves matter no chunk holds. The support and wear laws read the
    /// chunks as the whole piece; a gap would lose mass and joints unseen.
    #[test]
    fn a_gap_is_refused() {
        for (spec, mut cut) in shipped() {
            let (p, k) = largest(&cut);
            let piece = &spec.construction.pieces[p];
            let chunk = &cut[p][k];
            let face = chunk
                .faces
                .iter()
                .find(|face| {
                    !piece.solid.faces.iter().any(|outer| {
                        outer.normal.dot(face.normal) > 1.0 - PLANE_M
                            && (outer.offset - face.offset).abs() <= PLANE_M
                    })
                })
                .expect("a piece of several chunks has a cut face");
            let (lo, hi) = chunk.extent(face.normal);
            cut[p][k] = chunk
                .clip(face.normal, face.offset - 0.01 * (hi - lo))
                .expect("a hundredth of the depth leaves the chunk solid");
            let err = refusal(&spec.construction, cut);
            assert!(err.contains("gap"), "{}: {err}", spec.path);
        }
    }

    /// A CHUNK TWICE IS REFUSED: the same matter at two addresses doubles its mass and joints.
    #[test]
    fn a_duplicate_chunk_is_refused() {
        for (spec, mut cut) in shipped() {
            let (p, k) = largest(&cut);
            let twin = cut[p][k].clone();
            cut[p].push(twin);
            let err = refusal(&spec.construction, cut);
            assert!(err.contains("gap or an overlap"), "{}: {err}", spec.path);
        }
    }

    /// TWO ELEMENTS AT ONE NAME ARE REFUSED: a record entry names an element by its address.
    #[test]
    fn a_duplicate_name_is_refused() {
        for (spec, mut cut) in shipped() {
            let mut construction = spec.construction.clone();
            let mut piece = construction.pieces[0].clone();
            piece.solid = cut[0][0].clone();
            construction.pieces.push(piece);
            cut.push(vec![cut[0][0].clone()]);
            let err = refusal(&construction, cut);
            assert!(err.contains("two pieces are named"), "{}: {err}", spec.path);
        }
    }

    /// A DEGENERATE HULL IS REFUSED: the largest chunk pressed flat onto one of its own faces
    /// bounds no solid, so no collider or law block can be built from it.
    #[test]
    fn a_degenerate_hull_is_refused() {
        for (spec, mut cut) in shipped() {
            let (p, k) = largest(&cut);
            let chunk = &mut cut[p][k];
            let (normal, offset) = (chunk.faces[0].normal, chunk.faces[0].offset);
            for corner in &mut chunk.corners {
                *corner -= normal * (normal.dot(*corner) - offset);
            }
            let err = refusal(&spec.construction, cut);
            assert!(
                err.contains("no closed convex cell"),
                "{}: {err}",
                spec.path
            );
        }
    }

    /// A MISSING ANCHOR IS REFUSED: the foundation plane lowered by a hundredth of the lowest
    /// chunk's height touches no chunk face, so nothing holds the construction up.
    #[test]
    fn a_missing_anchor_is_refused() {
        for (spec, cut) in shipped() {
            let mut construction = spec.construction.clone();
            let height = cut
                .iter()
                .flatten()
                .map(|chunk| chunk.extent(DVec3::Y))
                .min_by(|a, b| a.0.total_cmp(&b.0))
                .map(|(lo, hi)| hi - lo)
                .expect("a construction has chunks");
            construction.foundation_y -= 0.01 * height;
            let err = refusal(&construction, cut);
            assert!(err.contains("nothing holds it up"), "{}: {err}", spec.path);
        }
    }

    /// A CHUNK CUT OFF IS REFUSED: with its joints gone its interface is unmatched, and with
    /// every joint gone a chunk off the ground reaches no anchor.
    #[test]
    fn a_chunk_cut_off_is_refused() {
        for (spec, cut) in shipped() {
            let baked = assemble(
                &spec.construction,
                cut.clone(),
                schema::substance_density,
                CutCount::default(),
            )
            .unwrap();
            let (p, k) = largest(&cut);
            let index = cut[..p].iter().map(Vec::len).sum::<usize>() + k;
            let mut asset = baked.asset.clone();
            asset
                .joints
                .retain(|j| j.a as usize != index && j.b as usize != index);
            let err = graph(&spec.construction, &cut, &asset).unwrap_err();
            assert!(err.contains("unmatched"), "{}: {err}", spec.path);
            let mut asset = baked.asset;
            asset.joints.clear();
            let ground: BTreeSet<u32> = asset.anchors.iter().map(|a| a.chunk).collect();
            if ground.len() < asset.chunks.len() {
                let err = reach(&asset).unwrap_err();
                assert!(err.contains("reaches no anchor"), "{}: {err}", spec.path);
            }
        }
    }

    /// AN INERTIA THE GAME CANNOT HOLD IS REFUSED: at a density whose tensor overflows f32 the
    /// solver could not integrate the chunk.
    #[test]
    fn an_unrepresentable_inertia_is_refused() {
        for (spec, cut) in shipped() {
            let err = assemble(
                &spec.construction,
                cut,
                |_: &str| Ok(f64::from(f32::MAX) * 1.0e6),
                CutCount::default(),
            )
            .err()
            .expect("an overflowing density is refused");
            assert!(err.contains("inertia"), "{}: {err}", spec.path);
        }
    }
}
