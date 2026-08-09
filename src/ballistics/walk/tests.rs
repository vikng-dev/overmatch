//! Pure-core fixtures for the §13 union field walk.
//!
//! Every test here feeds a synthetic hit list — no physics world, no Avian, no ECS. That is the
//! point: the §13.6 invariants are claims about the LAW, and a test that has to spawn a trimesh to
//! state one is testing the adapter instead. The trimesh/adapter half (winding recovery, hit-cap
//! exhaustion, real oriented meshes) belongs to slice 2.
//!
//! Distances are chosen from binary-exact values (multiples of 1/16, factors that are powers of
//! two) wherever a test asserts BYTE equality, so an assertion about seam invisibility is about the
//! law and not about the decimal literal it was written with.

use super::*;

// ---------------------------------------------------------------------------------------------
// Fixture helpers
// ---------------------------------------------------------------------------------------------

const AXIS: Vec3 = Vec3::Z;

fn volume(index: u32) -> Entity {
    Entity::from_raw_u32(index).expect("test entity index must be non-zero-invalid")
}

/// Synthetic collider identity. The bind gives every glb mesh primitive its own collider entity;
/// fixtures name them by a small index in the same space, offset so a primitive can never collide
/// with a volume id.
fn prim(index: u32) -> Entity {
    Entity::from_raw_u32(index + 1_000).expect("test entity index must be non-zero-invalid")
}

/// A plate crossing: `[enter, exit)` on one primitive, faces square to the ray.
fn plate(vol: u32, primitive: u32, enter: f32, exit: f32) -> Vec<FaceHit> {
    oblique_plate(vol, primitive, enter, exit, -AXIS, AXIS)
}

/// A slab whose faces are tilted `front` — the OUTWARD normal of the face the ray meets first, so
/// `axis · front < 0`. The back face is `-front`, the flat-plate case.
fn slab(vol: u32, primitive: u32, enter: f32, exit: f32, front: Vec3) -> Vec<FaceHit> {
    let front = front.normalize();
    assert!(
        AXIS.dot(front) < 0.0,
        "a front face's outward normal leans against the ray"
    );
    oblique_plate(vol, primitive, enter, exit, front, -front)
}

/// A plate crossing with authored face normals — the oblique/graze cases.
fn oblique_plate(
    vol: u32,
    primitive: u32,
    enter: f32,
    exit: f32,
    entry_normal: Vec3,
    exit_normal: Vec3,
) -> Vec<FaceHit> {
    vec![
        FaceHit {
            volume: volume(vol),
            primitive: prim(primitive),
            triangle: primitive * 100,
            t: enter,
            true_normal: entry_normal.normalize(),
        },
        FaceHit {
            volume: volume(vol),
            primitive: prim(primitive),
            triangle: primitive * 100 + 1,
            t: exit,
            true_normal: exit_normal.normalize(),
        },
    ]
}

fn corridor(length: f32, hits: Vec<FaceHit>) -> RayCorridor {
    RayCorridor {
        anchor: Vec3::ZERO,
        origin: Vec3::ZERO,
        axis: AXIS,
        length,
        initial_presence: Vec::new(),
        hits,
    }
}

fn table(entries: &[(u32, f32)]) -> VolumeTable {
    VolumeTable::new(entries.iter().map(|(v, f)| (volume(*v), *f))).expect("test factors are legal")
}

fn walk(corridor: &RayCorridor, volumes: &VolumeTable) -> RayWalk {
    walk_ray(0, corridor, volumes, &WalkLaws::default()).expect("fixture must resolve")
}

/// A `k = 1` disc: the fragment degeneracy (§13.5 — a fragment is a shell with r → 0).
fn point_disc(length: f32, sample: SampleCorridor) -> DiscCorridor {
    DiscCorridor {
        anchor: Vec3::ZERO,
        origin: Vec3::ZERO,
        axis: AXIS,
        length,
        radius: 0.0,
        frame: DiscFrame {
            u: Vec3::X,
            v: Vec3::Y,
        },
        samples: vec![sample],
    }
}

fn sample(offset: Vec3, hits: Vec<FaceHit>) -> SampleCorridor {
    SampleCorridor {
        offset,
        initial_presence: Vec::new(),
        hits,
    }
}

/// A planar slab defined GEOMETRICALLY rather than by `t` values.
///
/// The staged tests need fixtures a ray can be asked about from any origin along any axis — once
/// normalization bends the axis and the ring is transported, the transit corridor's sample rays
/// meet the same plate at different places, and a fixture built from hard-coded `t`s can only
/// describe the entrance.
#[derive(Clone, Copy)]
struct Slab {
    vol: u32,
    primitive: u32,
    /// A point on the FRONT face (the one facing the shooter).
    front: Vec3,
    /// Outward normal of the front face; `AXIS · normal < 0`.
    normal: Vec3,
    /// Measured along the normal, face to face.
    thickness: f32,
}

impl Slab {
    /// The slab whose front face crosses the reference axis at `t` and which presents `chord` metres
    /// of line-of-sight to a ray travelling along `AXIS`.
    fn at(vol: u32, primitive: u32, t: f32, normal: Vec3, chord: f32) -> Self {
        let normal = normal.normalize();
        assert!(AXIS.dot(normal) < 0.0, "a front face leans against the ray");
        Self {
            vol,
            primitive,
            front: AXIS * t,
            normal,
            thickness: chord * AXIS.dot(normal).abs(),
        }
    }

    fn key(&self) -> PrimitiveKey {
        PrimitiveKey {
            volume: volume(self.vol),
            primitive: prim(self.primitive),
        }
    }

    /// The face hits this slab presents to one ray.
    ///
    /// `seeded` is what a real adapter knows too: whether this ray was declared to START inside this
    /// primitive. If it was, a face behind the origin is the entry the seed already accounts for and
    /// is dropped; if it was not, a face a rounding-hair behind the origin is the face the ray is
    /// sitting ON and is clamped to `t = 0`, where the corridor processes it. The two rules are
    /// complements, so the entry is counted exactly once however the f32 falls.
    fn hits(&self, origin: Vec3, axis: Vec3, length: f32, seeded: bool) -> Vec<FaceHit> {
        let n = self.normal;
        let denominator = axis.dot(n);
        let front = (self.front - origin).dot(n) / denominator;
        let back = (self.front - n * self.thickness - origin).dot(n) / denominator;
        let (enter, exit) = if denominator < 0.0 {
            ((front, n), (back, -n))
        } else {
            ((back, -n), (front, n))
        };
        [(enter, 0u32), (exit, 1u32)]
            .into_iter()
            .filter_map(|((t, true_normal), face)| {
                let t = if t < 0.0 {
                    if seeded {
                        return None;
                    }
                    0.0
                } else {
                    t
                };
                (t < length).then_some(FaceHit {
                    volume: volume(self.vol),
                    primitive: prim(self.primitive),
                    triangle: self.primitive * 100 + face,
                    t,
                    true_normal,
                })
            })
            .collect()
    }
}

/// The ENTRANCE disc: k parallel rays from open air along `AXIS`.
fn entrance_disc(slabs: &[Slab], length: f32) -> DiscCorridor {
    let frame = DiscFrame::from_axis_and_reference(AXIS, Vec3::Y).unwrap();
    disc_along(Vec3::ZERO, AXIS, frame, 0.044, length, slabs, |_| true)
}

/// The TRANSIT disc the caller owes [`begin`] — each sample resuming where the request says it
/// resumes, along the bent axis, with the TRANSPORTED frame, seeded from the request. This is the
/// slice-2 driver contract written out; `finish` rejects a corridor that departs from it.
fn transit_disc(request: &TransitRequest, slabs: &[Slab], length: f32) -> DiscCorridor {
    let samples = request
        .seeds
        .iter()
        .map(|seed| SampleCorridor {
            offset: seed.offset,
            initial_presence: seed.inside.clone(),
            hits: slabs
                .iter()
                .flat_map(|slab| {
                    slab.hits(
                        request.origin + seed.offset,
                        request.axis,
                        length,
                        seed.inside.contains(&slab.key()),
                    )
                })
                .collect(),
        })
        .collect();
    DiscCorridor {
        anchor: request.anchor,
        origin: request.origin,
        axis: request.axis,
        length,
        radius: request.radius,
        frame: request.frame,
        samples,
    }
}

fn disc_along(
    origin: Vec3,
    axis: Vec3,
    frame: DiscFrame,
    radius: f32,
    length: f32,
    slabs: &[Slab],
    covered: impl Fn(usize) -> bool,
) -> DiscCorridor {
    let offsets = disc_offsets(&frame, radius, DEFAULT_RING);
    let samples = offsets
        .iter()
        .enumerate()
        .map(|(index, offset)| SampleCorridor {
            offset: *offset,
            initial_presence: Vec::new(),
            hits: if covered(index) {
                slabs
                    .iter()
                    .flat_map(|slab| slab.hits(origin + *offset, axis, length, false))
                    .collect()
            } else {
                Vec::new()
            },
        })
        .collect();
    DiscCorridor {
        anchor: Vec3::ZERO,
        origin,
        axis,
        length,
        radius,
        frame,
        samples,
    }
}

// ---------------------------------------------------------------------------------------------
// Topology reduction and pairing
// ---------------------------------------------------------------------------------------------

/// A ray landing on a face DIAGONAL reports two coplanar triangles at the same `t`. They are one
/// crossing; a naive per-hit toggle would enter and immediately exit.
#[test]
fn a_face_diagonal_hit_is_one_crossing() {
    let volumes = table(&[(1, 1000.0)]);
    let hits = vec![
        FaceHit {
            volume: volume(1),
            primitive: prim(0),
            triangle: 0,
            t: 1.0,
            true_normal: -AXIS,
        },
        FaceHit {
            volume: volume(1),
            primitive: prim(0),
            triangle: 1,
            t: 1.0,
            true_normal: -AXIS,
        },
        FaceHit {
            volume: volume(1),
            primitive: prim(0),
            triangle: 2,
            t: 1.125,
            true_normal: AXIS,
        },
    ];
    let result = walk(&corridor(4.0, hits), &volumes);
    assert_eq!(result.runs.len(), 1);
    assert_eq!(result.cost, 125.0);
}

/// Several faces incident on one shared VERTEX arrive at one `t` with differing normals. Still one
/// crossing — multiplicity of faces is not multiplicity of material.
#[test]
fn a_shared_vertex_cluster_is_one_crossing() {
    let volumes = table(&[(1, 1000.0)]);
    let mut hits = Vec::new();
    for (index, normal) in [
        Vec3::new(-0.2, 0.1, -1.0),
        Vec3::new(0.3, -0.2, -1.0),
        Vec3::new(0.0, 0.4, -1.0),
    ]
    .into_iter()
    .enumerate()
    {
        hits.push(FaceHit {
            volume: volume(1),
            primitive: prim(0),
            triangle: index as u32,
            t: 1.0,
            true_normal: normal.normalize(),
        });
    }
    hits.push(FaceHit {
        volume: volume(1),
        primitive: prim(0),
        triangle: 9,
        t: 1.5,
        true_normal: AXIS,
    });
    let result = walk(&corridor(4.0, hits), &volumes);
    assert_eq!(result.runs.len(), 1);
    assert_eq!(result.cost, 500.0);
}

/// An exact edge tangent puts entry AND exit faces of one primitive at one `t` with zero material
/// between them. It must toggle nothing — a fabricated zero-length interval would grow a spurious
/// entrance/exit event pair out of no steel at all.
#[test]
fn an_exact_edge_tangent_toggles_nothing() {
    let volumes = table(&[(1, 1000.0)]);
    let hits = vec![
        FaceHit {
            volume: volume(1),
            primitive: prim(0),
            triangle: 0,
            t: 1.0,
            true_normal: -AXIS,
        },
        FaceHit {
            volume: volume(1),
            primitive: prim(0),
            triangle: 1,
            t: 1.0,
            true_normal: AXIS,
        },
    ];
    let result = walk(&corridor(4.0, hits), &volumes);
    assert!(result.runs.is_empty());
    assert!(result.events.is_empty());
    assert_eq!(result.cost, 0.0);
}

/// The same edge, missed by a hair on either side: a real (if thin) chord. Catches a topology
/// tolerance grown large enough to erase genuine thin material.
#[test]
fn a_near_tangent_pair_keeps_its_real_thin_chord() {
    let volumes = table(&[(1, 1000.0)]);
    let hits = plate(1, 0, 1.0, 1.0 + 1.0e-4);
    let result = walk(&corridor(4.0, hits), &volumes);
    assert_eq!(result.runs.len(), 1);
    assert!(result.cost > 0.09 && result.cost < 0.11, "{}", result.cost);
}

/// A corridor whose origin lies inside a plate charges the REMAINING chord — declared, not inferred.
#[test]
fn starting_inside_with_declared_presence_charges_the_partial_chord() {
    let volumes = table(&[(1, 1000.0)]);
    let key = PrimitiveKey {
        volume: volume(1),
        primitive: prim(0),
    };
    let mut corridor = corridor(
        4.0,
        vec![FaceHit {
            volume: volume(1),
            primitive: prim(0),
            triangle: 0,
            t: 0.25,
            true_normal: AXIS,
        }],
    );
    corridor.initial_presence = vec![key];
    let result = walk(&corridor, &volumes);
    assert_eq!(result.cost, 250.0);
    assert_eq!(result.runs.len(), 1);
    assert_eq!(result.runs[0].start, 0.0);
}

/// The same hit list WITHOUT the declaration is an error, never inferred topology. "The first hit is
/// an exit, so we must have started inside" is exactly how a dropped entry face becomes free armour.
#[test]
fn starting_inside_without_declared_presence_is_a_structured_error() {
    let volumes = table(&[(1, 1000.0)]);
    let corridor = corridor(
        4.0,
        vec![FaceHit {
            volume: volume(1),
            primitive: prim(0),
            triangle: 7,
            t: 0.25,
            true_normal: AXIS,
        }],
    );
    match walk_ray(3, &corridor, &volumes, &WalkLaws::default()) {
        Err(WalkError::UnexpectedExit {
            sample,
            key,
            triangles,
            ..
        }) => {
            assert_eq!(sample, 3);
            assert_eq!(key.volume, volume(1));
            assert_eq!(triangles, vec![7]);
        }
        other => panic!("expected UnexpectedExit, got {other:?}"),
    }
}

/// A corridor that runs out with material still open must say so. It must NOT synthesize the exit,
/// which would charge a chord the geometry never claimed; extending the corridor completes it.
#[test]
fn a_corridor_ending_inside_errors_and_extending_it_completes() {
    let volumes = table(&[(1, 1000.0)]);
    let hits = plate(1, 0, 0.25, 1.5);
    match walk_ray(
        0,
        &corridor(1.0, hits.clone()),
        &volumes,
        &WalkLaws::default(),
    ) {
        Err(WalkError::IncompleteCorridor { open, length, .. }) => {
            assert_eq!(open.len(), 1);
            assert_eq!(length, 1.0);
        }
        other => panic!("expected IncompleteCorridor, got {other:?}"),
    }
    let completed = walk(&corridor(4.0, hits), &volumes);
    assert_eq!(completed.cost, 1250.0);
}

/// The corridor is half-open, `[0, length)`, so a boundary sitting exactly on the end belongs to the
/// NEXT corridor and is processed exactly once. Here that leaves the plate open — which is the
/// honest report, not a silent free exit.
#[test]
fn a_boundary_exactly_at_the_corridor_end_belongs_to_the_next_corridor() {
    let volumes = table(&[(1, 1000.0)]);
    let hits = plate(1, 0, 0.5, 2.0);
    assert!(matches!(
        walk_ray(
            0,
            &corridor(2.0, hits.clone()),
            &volumes,
            &WalkLaws::default()
        ),
        Err(WalkError::IncompleteCorridor { .. })
    ));
    let result = walk(&corridor(2.0 + 1.0e-3, hits), &volumes);
    assert_eq!(
        result
            .events
            .iter()
            .filter(|event| event.kind == BoundaryKind::Exit)
            .count(),
        1
    );
}

/// One entity, two DISJOINT islands: two presence intervals, two runs, both charged.
#[test]
fn one_entity_with_two_disjoint_primitives_yields_two_runs() {
    let volumes = table(&[(1, 1000.0)]);
    let mut hits = plate(1, 0, 0.25, 0.5);
    hits.extend(plate(1, 1, 1.0, 1.25));
    let result = walk(&corridor(4.0, hits), &volumes);
    assert_eq!(result.runs.len(), 2);
    assert_eq!(result.cost, 500.0);
    assert_eq!(result.presence.len(), 1);
    assert_eq!(result.presence[0].chord, 0.5);
}

/// One entity, two INTERPENETRATING islands (`E,E,X,X`). Entity-level parity would read the overlap
/// as absent; per-primitive presence unioned per entity charges it once and deposits once.
#[test]
fn one_entity_with_interpenetrating_primitives_unions_to_a_single_presence() {
    let volumes = table(&[(1, 1000.0)]);
    let mut hits = plate(1, 0, 0.25, 0.75);
    hits.extend(plate(1, 1, 0.5, 1.0));
    let result = walk(&corridor(4.0, hits), &volumes);
    assert_eq!(result.runs.len(), 1);
    assert_eq!(result.cost, 750.0);
    assert_eq!(result.presence.len(), 1);
    // The union, not the sum: 0.75 m, not 0.5 + 0.5.
    assert_eq!(result.presence[0].chord, 0.75);
}

/// Two DIFFERENT shells entering on one plane are not one crossing — their exits differ, and
/// over-deduplicating them would lose the deeper one entirely.
#[test]
fn two_shells_on_one_entry_plane_stay_distinct() {
    let volumes = table(&[(1, 1000.0), (2, 200.0)]);
    let mut hits = plate(1, 0, 0.5, 0.75);
    hits.extend(plate(2, 0, 0.5, 1.5));
    let result = walk(&corridor(4.0, hits), &volumes);
    assert_eq!(result.runs.len(), 1);
    // max(1000, 200) over [0.5, 0.75) then 200 over [0.75, 1.5).
    assert_eq!(result.cost, 0.25 * 1000.0 + 0.75 * 200.0);
    assert_eq!(result.presence.len(), 2);
}

/// UNBALANCED nesting is still a structured error — §13.7 legalized several shells per primitive,
/// not arbitrary face sequences.
///
/// Two entries and one exit is a shell that never closed, and the walk says so. What CHANGED is
/// which error it is: the second entry is no longer a contradiction in itself (see `Field`), so the
/// defect surfaces where it actually is — a primitive still open when the corridor ends. The mesh is
/// not the closed positively-oriented shell the bake gate promises either way, and nothing about it
/// resolves silently.
#[test]
fn unbalanced_nesting_in_one_primitive_is_a_structured_error() {
    let volumes = table(&[(1, 1000.0)]);
    let hits = vec![
        FaceHit {
            volume: volume(1),
            primitive: prim(0),
            triangle: 0,
            t: 0.25,
            true_normal: -AXIS,
        },
        FaceHit {
            volume: volume(1),
            primitive: prim(0),
            triangle: 1,
            t: 0.5,
            true_normal: -AXIS,
        },
        FaceHit {
            volume: volume(1),
            primitive: prim(0),
            triangle: 2,
            t: 1.0,
            true_normal: AXIS,
        },
    ];
    assert!(matches!(
        walk_ray(1, &corridor(4.0, hits), &volumes, &WalkLaws::default()),
        Err(WalkError::IncompleteCorridor { sample: 1, .. })
    ));
}

/// AND SO IS AN EXIT WITH NOTHING OPEN. The other half of the fail-loud posture, which depth
/// counting must not soften: a mesh whose entry face was dropped, or whose winding is inverted,
/// presents an exit the ray was never inside, and free armour is what silence there would buy.
#[test]
fn an_exit_below_depth_zero_is_still_a_structured_error() {
    let volumes = table(&[(1, 1000.0)]);
    // enter, exit, exit — the last one has no shell left to close.
    let mut hits = plate(1, 0, 0.25, 0.5);
    hits.push(FaceHit {
        volume: volume(1),
        primitive: prim(0),
        triangle: 9,
        t: 0.75,
        true_normal: AXIS,
    });
    assert!(matches!(
        walk_ray(2, &corridor(4.0, hits), &volumes, &WalkLaws::default()),
        Err(WalkError::UnexpectedExit { sample: 2, .. })
    ));
}

/// A factor that cannot participate in `max` or in a deterministic sum is rejected before any
/// resolution — a `NaN` would poison both the field maximum and the sort order.
#[test]
fn non_finite_and_negative_factors_are_rejected_at_construction() {
    for bad in [f32::NAN, f32::INFINITY, -1.0] {
        assert!(matches!(
            VolumeTable::new([(volume(1), bad)]),
            Err(WalkError::BadFactor { .. })
        ));
    }
    let table = VolumeTable::new([(volume(1), -0.0)]).unwrap();
    assert_eq!(table.factor(volume(1)).unwrap().to_bits(), 0.0f32.to_bits());
}

/// A hit naming a volume with no factor is a bind gap, and a bind gap must not resolve to a default.
#[test]
fn an_unbound_volume_is_an_error_not_a_default_factor() {
    let volumes = table(&[(1, 1000.0)]);
    let hits = plate(2, 0, 0.5, 1.0);
    assert!(matches!(
        walk_ray(0, &corridor(4.0, hits), &volumes, &WalkLaws::default()),
        Err(WalkError::UnknownVolume { .. })
    ));
}

// ---------------------------------------------------------------------------------------------
// §13.1 — the pathology table
// ---------------------------------------------------------------------------------------------

/// The headline defect: two overlapping plates. The serial resolver charges the second at ZERO;
/// double-counting (War Thunder's failure mode) is the other error. The union charges once.
#[test]
fn overlapping_plates_charge_the_union_not_zero_and_not_double() {
    let volumes = table(&[(1, 1000.0), (2, 1000.0)]);
    let mut hits = plate(1, 0, 0.0625, 0.1875);
    hits.extend(plate(2, 0, 0.125, 0.25));
    let result = walk(&corridor(1.0, hits), &volumes);
    // Union chord 0.1875 m, not 0.0 (the current bug) and not 0.25 (double-count).
    assert_eq!(result.cost, 187.5);
    assert_eq!(result.runs.len(), 1);
}

/// SEAM INVISIBILITY (§13.6), asserted on the bits: one slab, two abutting slabs and two
/// overlapping slabs of the same substance must be numerically indistinguishable. Cutting spans
/// only where `max(factor)` changes is what buys this — `(b−a)·F` is not bit-equal to
/// `(s−a)·F + (b−s)·F`, so a resolver that carried the seam into the arithmetic would fail here
/// even with a fixed iteration order.
#[test]
fn abutment_overlap_and_one_slab_are_byte_identical() {
    let volumes = table(&[(1, 1024.0), (2, 1024.0)]);

    let single = walk(&corridor(1.0, plate(1, 0, 0.0625, 0.1875)), &volumes);

    let mut abutting = plate(1, 0, 0.0625, 0.125);
    abutting.extend(plate(2, 0, 0.125, 0.1875));
    let abutting = walk(&corridor(1.0, abutting), &volumes);

    let mut overlapping = plate(1, 0, 0.0625, 0.15625);
    overlapping.extend(plate(2, 0, 0.125, 0.1875));
    let overlapping = walk(&corridor(1.0, overlapping), &volumes);

    assert_eq!(single.cost.to_bits(), abutting.cost.to_bits());
    assert_eq!(single.cost.to_bits(), overlapping.cost.to_bits());
    assert_eq!(single.spans, abutting.spans);
    assert_eq!(single.spans, overlapping.spans);
    assert_eq!(single.events.len(), abutting.events.len());
    assert_eq!(single.events, abutting.events);
    assert_eq!(single.events, overlapping.events);
}

/// A GENUINE gap (far past the weld tolerance) is free flight — no events inside it, no fabricated
/// terminal. That is what makes gaps detectable rather than event noise (§13.6), and it is how
/// spaced armour stays emergent rather than authored.
#[test]
fn a_genuine_gap_produces_no_events_inside_it() {
    let volumes = table(&[(1, 1000.0), (2, 1000.0)]);
    let mut hits = plate(1, 0, 0.25, 0.5);
    hits.extend(plate(2, 0, 1.5, 1.75));
    let result = walk(&corridor(4.0, hits), &volumes);
    assert_eq!(result.runs.len(), 2, "a real gap must not weld");
    assert!(
        !result
            .events
            .iter()
            .any(|event| event.t > 0.5 && event.t < 1.5)
    );
    assert_eq!(result.cost, 500.0);
}

/// The corridor origin sitting inside steel grants no free crossing — the remaining chord is
/// charged in full, at the union factor.
#[test]
fn a_corridor_starting_inside_a_volume_grants_no_free_crossing() {
    let volumes = table(&[(1, 1000.0)]);
    let mut corridor = corridor(
        1.0,
        vec![FaceHit {
            volume: volume(1),
            primitive: prim(0),
            triangle: 0,
            t: 0.125,
            true_normal: AXIS,
        }],
    );
    corridor.initial_presence = vec![PrimitiveKey {
        volume: volume(1),
        primitive: prim(0),
    }];
    let result = walk(&corridor, &volumes);
    assert_eq!(result.cost, 125.0);
    assert_eq!(result.presence[0].chord, 0.125);
}

// ---------------------------------------------------------------------------------------------
// §13.6 — the machine-checkable invariants
// ---------------------------------------------------------------------------------------------

/// IDEMPOTENCE: duplicating a volume changes no outcome — the same steel claimed twice is charged
/// once. This is the plate-junction fix stated as a law.
#[test]
fn duplicating_a_volume_changes_nothing() {
    let volumes = table(&[(1, 1000.0), (2, 1000.0)]);
    let one = walk(&corridor(1.0, plate(1, 0, 0.125, 0.375)), &volumes);
    let mut doubled = plate(1, 0, 0.125, 0.375);
    doubled.extend(plate(2, 0, 0.125, 0.375));
    let two = walk(&corridor(1.0, doubled), &volumes);
    assert_eq!(one.cost.to_bits(), two.cost.to_bits());
    assert_eq!(one.spans, two.spans);
    assert_eq!(one.events, two.events);
}

/// MONOTONICITY: adding any volume never lowers protection anywhere — a gunner's arm clipped into
/// the turret wall cannot dilute the steel.
#[test]
fn adding_a_volume_never_lowers_the_cost() {
    let volumes = table(&[(1, 1000.0), (2, 10.0), (3, 900.0)]);
    let base = walk(&corridor(2.0, plate(1, 0, 0.25, 0.5)), &volumes);
    for extra in [plate(2, 0, 0.3, 0.45), plate(3, 0, 0.4, 0.9)] {
        let mut hits = plate(1, 0, 0.25, 0.5);
        hits.extend(extra);
        let grown = walk(&corridor(2.0, hits), &volumes);
        assert!(grown.cost >= base.cost, "{} < {}", grown.cost, base.cost);
    }
}

/// ORDER INDEPENDENCE: authoring order cannot matter. Permuting the hit list must produce a
/// bit-identical result, not merely a close one.
#[test]
fn permuting_the_input_is_byte_identical() {
    let volumes = table(&[(1, 1000.0), (2, 900.0), (3, 10.0)]);
    let mut hits = plate(1, 0, 0.125, 0.375);
    hits.extend(plate(2, 0, 0.375, 0.5));
    hits.extend(plate(3, 0, 0.25, 0.625));
    hits.extend(plate(1, 1, 0.75, 0.875));
    let reference = walk(&corridor(2.0, hits.clone()), &volumes);

    // Every rotation of the list, plus its reverse — enough to break any incidental dependence on
    // arrival order without a random generator inside a deterministic test.
    for shift in 0..hits.len() {
        let mut permuted = hits.clone();
        permuted.rotate_left(shift);
        let result = walk(&corridor(2.0, permuted.clone()), &volumes);
        assert_eq!(result.cost.to_bits(), reference.cost.to_bits());
        assert_eq!(result, reference);
        permuted.reverse();
        let result = walk(&corridor(2.0, permuted), &volumes);
        assert_eq!(result, reference);
    }
}

/// SEGMENTATION REFINEMENT: burying a lower-factor volume inside steel adds boundaries the union
/// maximum never notices. The cost BITS must not move — otherwise a crewman's arm inside the hull
/// would change what the hull costs to cross.
#[test]
fn irrelevant_boundaries_inside_a_dominant_span_leave_the_cost_bits_alone() {
    let volumes = table(&[(1, 1024.0), (2, 10.0), (3, 200.0)]);
    let bare = walk(&corridor(1.0, plate(1, 0, 0.0625, 0.5)), &volumes);
    let mut refined = plate(1, 0, 0.0625, 0.5);
    refined.extend(plate(2, 0, 0.125, 0.25));
    refined.extend(plate(3, 0, 0.3, 0.4));
    let refined = walk(&corridor(1.0, refined), &volumes);
    assert_eq!(bare.cost.to_bits(), refined.cost.to_bits());
    assert_eq!(bare.spans, refined.spans);
}

/// One-ULP seam offsets around the topology threshold: an abutment jittered by a single ULP is still
/// one boundary, and the cost still matches the unsplit slab exactly.
#[test]
fn one_ulp_seam_offsets_stay_one_boundary() {
    let volumes = table(&[(1, 1024.0), (2, 1024.0)]);
    let seam = 0.125f32;
    let single = walk(&corridor(1.0, plate(1, 0, 0.0625, 0.1875)), &volumes);
    for jitter in [
        f32::from_bits(seam.to_bits() - 1),
        seam,
        f32::from_bits(seam.to_bits() + 1),
    ] {
        let mut hits = plate(1, 0, 0.0625, jitter);
        hits.extend(plate(2, 0, seam, 0.1875));
        let result = walk(&corridor(1.0, hits), &volumes);
        assert_eq!(result.runs.len(), 1);
        assert_eq!(
            result.cost.to_bits(),
            single.cost.to_bits(),
            "jitter {jitter:?} moved the cost"
        );
    }
}

/// A corridor anchored far out in the world keeps the millimetre-scale distinction the weld
/// tolerance is written in — the scale-aware topology tolerance is what buys this.
#[test]
fn large_world_coordinates_keep_the_millimetre_distinction() {
    let volumes = table(&[(1, 1000.0), (2, 1000.0)]);
    let far = 2000.0f32;
    // The corridor is ORIGIN-RELATIVE: slice 2 anchors it at first contact, so world position
    // reaches the walk through `origin` alone and `t` stays in metres. A 1.5 mm void must weld and a
    // 3 mm void must not, identically at the map edge and at the origin.
    let welding = |origin: Vec3, gap: f32| {
        let mut hits = plate(1, 0, 0.25, 0.35);
        hits.extend(plate(2, 0, 0.35 + gap, 0.45));
        walk(
            &RayCorridor {
                anchor: Vec3::ZERO,
                origin,
                axis: AXIS,
                length: 2.0,
                initial_presence: Vec::new(),
                hits,
            },
            &volumes,
        )
    };
    for gap in [1.5e-3, 3.0e-3] {
        let near = welding(Vec3::ZERO, gap);
        let far_out = welding(Vec3::new(-far, 1.5 * far, far), gap);
        assert_eq!(
            near.runs.len(),
            far_out.runs.len(),
            "gap {gap} reclassified"
        );
        assert_eq!(near.cost.to_bits(), far_out.cost.to_bits());
        assert_eq!(near.spans, far_out.spans);
    }
    assert_eq!(welding(Vec3::ZERO, 1.5e-3).runs.len(), 1);
    assert_eq!(welding(Vec3::ZERO, 3.0e-3).runs.len(), 2);
    // Even welded, the void survives as a zero-factor span: welding deletes faces, not steel.
    let welded = welding(Vec3::ZERO, 1.5e-3);
    assert_eq!(
        welded
            .spans
            .iter()
            .filter(|span| span.factor == 0.0 && span.start > 0.3 && span.end < 0.4)
            .count(),
        1
    );
}

// ---------------------------------------------------------------------------------------------
// §13.4 — ε-weld
// ---------------------------------------------------------------------------------------------

/// A micro-gap merges event topology — one entrance, one exit — and charges no gap.
#[test]
fn a_micro_gap_welds_event_topology_and_charges_nothing_for_the_void() {
    let volumes = table(&[(1, 1000.0), (2, 1000.0)]);
    let mut hits = plate(1, 0, 0.25, 0.35);
    hits.extend(plate(2, 0, 0.3505, 0.45));
    let result = walk(&corridor(2.0, hits), &volumes);
    assert_eq!(result.runs.len(), 1);
    assert_eq!(result.runs[0].joints, 1);
    assert_eq!(
        result
            .events
            .iter()
            .filter(|e| e.kind == BoundaryKind::Entrance)
            .count(),
        1
    );
    assert_eq!(
        result
            .events
            .iter()
            .filter(|e| e.kind == BoundaryKind::Exit)
            .count(),
        1
    );
    // Material only: 0.1 + 0.0995 metres, never the 0.5 mm void.
    let expected = (0.1f64 + 0.099_500_000_000_000_1) * 1000.0;
    assert!(
        (result.cost as f64 - expected).abs() < 0.02,
        "{}",
        result.cost
    );
}

/// The weld is measured PERPENDICULAR, not along the ray (§13.4). The same along-ray gap welds at
/// grazing exit and does not weld square-on — else grazing incidence un-welds exactly where a
/// micro-gap matters most.
#[test]
fn the_weld_is_measured_perpendicular_not_along_the_ray() {
    let volumes = table(&[(1, 1000.0), (2, 1000.0)]);
    let gap = 3.0e-3;

    // Square-on: gap_perp = gap_along = 3 mm > 2 mm.
    let mut square = plate(1, 0, 0.25, 0.35);
    square.extend(plate(2, 0, 0.35 + gap, 0.45));
    assert_eq!(walk(&corridor(2.0, square), &volumes).runs.len(), 2);

    // Grazing: faces tilted ~78° off the ray, so |axis·n| ≈ 0.2 and the same along-ray gap is
    // 0.6 mm perpendicular.
    let tilt = Vec3::new(0.0, 0.98, 0.2);
    let mut grazing = oblique_plate(1, 0, 0.25, 0.35, -tilt, tilt);
    grazing.extend(oblique_plate(2, 0, 0.35 + gap, 0.45, -tilt, tilt));
    let welded = walk(&corridor(2.0, grazing), &volumes);
    assert_eq!(welded.runs.len(), 1);
    assert_eq!(welded.runs[0].joints, 1);
}

/// A grazing exit must not weld to unrelated geometry downrange. `|axis · n| → 0` makes ANY gap
/// "perpendicular-small", so the face-compatibility test — the two faces must be opposing sides of
/// one void — is what stops a near-tangent side face from swallowing the next plate entirely.
#[test]
fn a_grazing_exit_does_not_weld_to_an_unrelated_orthogonal_plate() {
    let volumes = table(&[(1, 1000.0), (2, 1000.0)]);
    // Exit face almost parallel to the ray; the next plate is square-on and 40 mm away.
    let tilt = Vec3::new(0.0, 0.999, 0.045);
    let mut hits = oblique_plate(1, 0, 0.25, 0.35, -tilt, tilt);
    hits.extend(plate(2, 0, 0.39, 0.45));
    let result = walk(&corridor(2.0, hits), &volumes);
    assert_eq!(result.runs.len(), 2, "incompatible faces must not weld");
}

/// Chained welds are transitive but CAPPED: A~B~C only while the voids they delete still sum to a
/// micro-gap. Without the cap a picket fence collapses into one run with one terminal spall budget.
#[test]
fn chained_welds_are_capped_by_the_run_gap_budget() {
    let volumes = table(&[(1, 1000.0), (2, 1000.0), (3, 1000.0)]);

    // Three 0.5 mm voids: 1.5 mm total, inside the 2 mm budget → one run.
    let mut tight = plate(1, 0, 0.2, 0.3);
    tight.extend(plate(2, 0, 0.3005, 0.4));
    tight.extend(plate(3, 0, 0.4005, 0.5));
    assert_eq!(walk(&corridor(2.0, tight), &volumes).runs.len(), 1);

    // Three 1.5 mm voids: each welds alone, but 3 mm cumulative exceeds the budget → the chain
    // breaks rather than swallowing the fence.
    let mut loose = plate(1, 0, 0.2, 0.3);
    loose.extend(plate(2, 0, 0.3015, 0.4));
    loose.extend(plate(3, 0, 0.4015, 0.5));
    let result = walk(&corridor(2.0, loose), &volumes);
    assert_eq!(result.runs.len(), 2);
    assert_eq!(result.runs[0].joints, 1);
    assert_eq!(result.runs[1].joints, 0);
}

/// Welded overmatch reads the run's SUMMED factor-weighted thickness — the sandwich behaves as one
/// plate for the surface laws even though each chord charges its own factor.
#[test]
fn welded_overmatch_reads_the_summed_factor_weighted_thickness() {
    let volumes = table(&[(1, 1000.0), (2, 900.0)]);
    // 30 mm RHA + a 0.5 mm void + 30 mm cast = 58.5 mm steel-equivalent; an 88 does not overmatch it
    // (3 × 58.5 = 175 mm > 88), while either half alone (30 / 27 mm) it would.
    let mut hits = plate(1, 0, 0.5, 0.53);
    hits.extend(plate(2, 0, 0.5305, 0.5605));
    let disc = point_disc(2.0, sample(Vec3::ZERO, hits));
    let walked = walk_disc(&disc, &volumes, &WalkLaws::default()).unwrap();
    let shot = Shot {
        caliber: 0.088,
        capability: 250.0,
    };
    match begin(&walked, &shot, &WalkLaws::default()) {
        Begin::Transit(request) => {
            assert!(!request.entrance.overmatched);
            // 30 mm × 1000 + 30 mm × 900 = 57 reference-mm, so 57 mm steel-equivalent — the
            // SUMMED run, not either half (30 mm / 27 mm), each of which an 88 would overmatch.
            assert!(
                (request.entrance.steel_equivalent - 0.057).abs() < 1.0e-4,
                "{}",
                request.entrance.steel_equivalent
            );
        }
        other => panic!("expected Transit, got {other:?}"),
    }
}

/// Downward, equal and upward factor joints inside one welded run, with the exact spall count and
/// budget each produces.
///
/// FLAGGED (§13 spec conflict): §13.2 says spall fires at every downward step, §13.4 says "one exit,
/// spall once per welded run". Implemented conservatively as the field law — welding deletes only
/// the void's face pair, and the material step it exposes still obeys §13.2. An equal-factor joint
/// therefore emits nothing at all, which is where the two readings agree.
#[test]
fn weld_joints_obey_the_field_law_for_spall() {
    let laws = WalkLaws::default();

    // Equal factors across the void: no step, so one entrance and one exit, budget = SUMMED cost.
    let volumes = table(&[(1, 1000.0), (2, 1000.0)]);
    let mut equal = plate(1, 0, 0.2, 0.3);
    equal.extend(plate(2, 0, 0.3005, 0.4));
    let result = walk_ray(0, &corridor(2.0, equal), &volumes, &laws).unwrap();
    let downward: Vec<_> = result
        .events
        .iter()
        .filter(|e| e.factor_after < e.factor_before)
        .collect();
    assert_eq!(downward.len(), 1, "equal-factor weld emits one exit only");
    assert!((downward[0].spall_budget - 199.5).abs() < 0.1);

    // Downward across the void (RHA → cast): the exposed step fires, and so does the exit.
    let volumes = table(&[(1, 1000.0), (2, 900.0)]);
    let mut downhill = plate(1, 0, 0.2, 0.3);
    downhill.extend(plate(2, 0, 0.3005, 0.4));
    let result = walk_ray(0, &corridor(2.0, downhill), &volumes, &laws).unwrap();
    let downward: Vec<_> = result
        .events
        .iter()
        .filter(|e| e.factor_after < e.factor_before)
        .collect();
    assert_eq!(downward.len(), 2);
    assert!(
        downward[0].welded,
        "the interior step exists only because of the weld"
    );
    assert!((downward[0].spall_budget - 100.0).abs() < 0.1);

    // Upward across the void (cast → RHA): the joint throws nothing; only the exit does.
    let mut uphill = plate(2, 0, 0.2, 0.3);
    uphill.extend(plate(1, 0, 0.3005, 0.4));
    let result = walk_ray(0, &corridor(2.0, uphill), &volumes, &laws).unwrap();
    assert_eq!(
        result
            .events
            .iter()
            .filter(|e| e.factor_after < e.factor_before)
            .count(),
        1
    );
}

// ---------------------------------------------------------------------------------------------
// §13.5 — the disc
// ---------------------------------------------------------------------------------------------

/// A flat plate met mid-face by the whole disc resolves to the point-sample answer: k rays that all
/// see the same slab aggregate back to one crossing at full coverage.
#[test]
fn a_flat_plate_disc_equals_the_point_sample() {
    let volumes = table(&[(1, 1000.0)]);
    let laws = WalkLaws::default();
    let single = walk(&corridor(2.0, plate(1, 0, 0.5, 0.6)), &volumes);

    let frame = DiscFrame::from_axis_and_reference(AXIS, Vec3::Y).unwrap();
    let samples = disc_offsets(&frame, 0.044, DEFAULT_RING)
        .into_iter()
        .map(|offset| sample(offset, plate(1, 0, 0.5, 0.6)))
        .collect();
    let walked = walk_disc(
        &DiscCorridor {
            anchor: Vec3::ZERO,
            origin: Vec3::ZERO,
            axis: AXIS,
            length: 2.0,
            radius: 0.044,
            frame,
            samples,
        },
        &volumes,
        &laws,
    )
    .unwrap();

    assert_eq!(walked.events.len(), 1, "one slab is one crossing");
    let event = &walked.events[0];
    assert_eq!(event.coverage, 1.0);
    assert!((event.cost - single.cost).abs() < 1.0e-2, "{}", event.cost);
    assert!((event.entry_normal - -AXIS).length() < 1.0e-5);
}

/// A thin OBLIQUE slab is the case that breaks longitudinal-overlap clustering: the ring's chords
/// shift by `r·tan(incidence)`, so one sample can exit before another enters. Surface-compatible
/// association keeps it one event.
#[test]
fn a_thin_oblique_slab_is_one_event_not_several() {
    let volumes = table(&[(1, 1000.0)]);
    let laws = WalkLaws::default();
    let frame = DiscFrame::from_axis_and_reference(AXIS, Vec3::Y).unwrap();
    // ~72° from the normal: the longitudinal shift across the disc dwarfs the chord.
    let normal = Vec3::new(0.0, 0.95, -0.31).normalize();
    let chord = 0.02f32;

    let samples: Vec<SampleCorridor> = disc_offsets(&frame, 0.044, DEFAULT_RING)
        .into_iter()
        .map(|offset| {
            // One plane: each sample enters where its own ray meets it.
            let shift = -offset.dot(normal) / AXIS.dot(normal);
            sample(offset, slab(1, 0, 0.5 + shift, 0.5 + shift + chord, normal))
        })
        .collect();
    let starts: Vec<f32> = samples.iter().map(|s| s.hits[0].t).collect();
    let (lo, hi) = (
        starts.iter().copied().fold(f32::INFINITY, f32::min),
        starts.iter().copied().fold(f32::NEG_INFINITY, f32::max),
    );
    assert!(
        hi - lo > chord,
        "fixture must actually spread the samples past one chord ({lo}..{hi})"
    );

    let walked = walk_disc(
        &DiscCorridor {
            anchor: Vec3::ZERO,
            origin: Vec3::ZERO,
            axis: AXIS,
            length: 2.0,
            radius: 0.044,
            frame,
            samples,
        },
        &volumes,
        &laws,
    )
    .unwrap();
    assert_eq!(
        walked.events.len(),
        1,
        "one physical slab must resolve to one crossing"
    );
    assert_eq!(walked.events[0].coverage, 1.0);
    assert!((walked.events[0].cost - chord * 1000.0).abs() < 1.0);
}

/// A watertight CONCAVE primitive — the two arms of a bracket, 400 mm apart — is one mesh crossed
/// at two genuinely separate places. Sharing a primitive says the samples met the same BODY; it
/// must not be mistaken for meeting the same CROSSING, or two events collapse into one: one
/// entrance law instead of two, one summed overmatch thickness, and one exit where there are two.
#[test]
fn a_concave_primitive_crossed_twice_stays_two_events() {
    let volumes = table(&[(1, 1000.0)]);
    let laws = WalkLaws::default();
    let frame = DiscFrame {
        u: Vec3::X,
        v: Vec3::Y,
    };
    let walked = walk_disc(
        &DiscCorridor {
            anchor: Vec3::ZERO,
            origin: Vec3::ZERO,
            axis: AXIS,
            length: 2.0,
            radius: 0.02,
            frame,
            samples: vec![
                sample(Vec3::ZERO, plate(1, 0, 0.2, 0.3)),
                sample(Vec3::X * 0.02, plate(1, 0, 0.7, 0.8)),
            ],
        },
        &volumes,
        &laws,
    )
    .unwrap();

    assert_eq!(walked.events.len(), 2, "400 mm apart is not one crossing");
    for event in &walked.events {
        assert_eq!(event.coverage, 0.5, "each arm is met by one sample of two");
        assert!((event.cost - 50.0).abs() < 0.1, "{}", event.cost);
    }
    assert!(walked.events[0].end < walked.events[1].start);
}

/// Sharing a primitive is not a merge licence, and neither is "within the weld LOOKAHEAD".
///
/// The two arms of one bracket, 40 mm of air apart, are the same mesh and comfortably inside the
/// 50 mm lookahead — and they are still two crossings. The lookahead bounds where a weld may be
/// LOOKED for; only a weld-CLASS void (one §13.4 would actually have deleted) makes two runs one.
#[test]
fn a_weld_lookahead_sized_gap_is_not_a_merge_licence() {
    let volumes = table(&[(1, 1000.0)]);
    let laws = WalkLaws::default();
    let walked = walk_disc(
        &DiscCorridor {
            anchor: Vec3::ZERO,
            origin: Vec3::ZERO,
            axis: AXIS,
            length: 2.0,
            radius: 0.02,
            frame: DiscFrame {
                u: Vec3::X,
                v: Vec3::Y,
            },
            samples: vec![
                sample(Vec3::ZERO, plate(1, 0, 0.20, 0.30)),
                sample(Vec3::X * 0.02, plate(1, 0, 0.34, 0.44)),
            ],
        },
        &volumes,
        &laws,
    )
    .unwrap();

    // 40 mm of air: inside the lookahead, twenty times the weld tolerance.
    assert!(0.04 < laws.weld_max_lookahead);
    assert!(0.04 > laws.weld_perp);
    assert_eq!(walked.events.len(), 2, "40 mm of air is not one crossing");
    for event in &walked.events {
        assert_eq!(event.coverage, 0.5);
    }
}

/// The coplanar branch is bounded longitudinally too — by where ONE surface would have put things.
///
/// At grazing incidence two runs far apart ALONG THE RAY can sit within a couple of millimetres of
/// one plane, and the plane test alone says "one surface" and means it. What bounds it is neither a
/// constant nor a worst-case reach: it is the RESIDUAL between where sample `b`'s run starts and
/// where the shared surface predicts it, `−(d·n̄)/(axis·n̄)` for that pair's own lateral offset.
///
/// So the fixture states the bound from both sides at ONE incidence, with the perpendicular reading
/// held inside tolerance throughout — the longitudinal conjunct is the only thing under test — and
/// both arms are placed by the RESIDUAL, not by a reach. The previous form of this test derived its
/// arms from the implementation's capped reach, which is how it inherited that cap's half-scale
/// error; a fixture must not take its expected values from the code it is checking.
///
/// The original 80 mm-at-87° assertion is restored as the far arm, because under the residual
/// relation it is true again: those two runs are 40 mm from where one plane would put them.
#[test]
fn the_coplanar_branch_is_bounded_by_where_one_surface_would_put_things() {
    let volumes = table(&[(1, 1000.0), (2, 1000.0)]);
    let laws = WalkLaws::default();
    let normal = Vec3::new(0.0, 0.999, -0.045).normalize();
    let radius = 0.02f32;
    let near = 0.20f32;
    let near_end = 0.25f32;

    // What ONE plane predicts for a pair whose lateral offset is `d`, written out here rather than
    // read from the implementation — a fixture must not take its expected values from the code it is
    // checking, which is how the previous form of this test inherited a mis-scaled cap.
    let secant = (1.0 / AXIS.dot(normal).abs()).min(laws.event_secant_cap);
    let sign = if AXIS.dot(normal) < 0.0 { -1.0 } else { 1.0 };
    let predicted = |d: Vec3| -d.dot(normal) * secant * sign;

    // BOTH arms sit the SAME distance apart along the ray — 130 mm, five times the near run's whole
    // length — and differ only in how far off the shared plane the far one is. That is the point:
    // longitudinal distance is not the question, and a bound written in it cannot ask the right one.
    let far = 0.33f32;
    // Lateral offset solved so the two entry points land `perpendicular` apart along the shared
    // normal. At this incidence `cos ≈ 0.045`, so the plane tolerance alone would buy 44 mm of
    // along-ray slack — the runaway the residual exists to bound.
    let offset_for = |perpendicular: f32| {
        Vec3::new(
            0.0,
            ((near - far) * normal.z - perpendicular) / normal.y,
            0.0,
        )
    };
    let events_at = |perpendicular: f32| {
        let offset = offset_for(perpendicular);
        let separation = ((AXIS * near) - (offset + AXIS * far)).dot(normal).abs();
        assert!(
            separation < laws.event_plane_tolerance,
            "BOTH arms are coplanar within tolerance, or this tests the wrong conjunct: {separation}"
        );
        assert!(
            offset.length() <= radius,
            "and the offset must be a sample this disc actually has, got {}",
            offset.length()
        );
        walk_disc(
            &DiscCorridor {
                anchor: Vec3::ZERO,
                origin: Vec3::ZERO,
                axis: AXIS,
                length: 2.0,
                radius,
                frame: DiscFrame {
                    u: Vec3::X,
                    v: Vec3::Y,
                },
                samples: vec![
                    sample(
                        Vec3::ZERO,
                        oblique_plate(1, 0, near, near_end, normal, -normal),
                    ),
                    sample(
                        offset,
                        oblique_plate(2, 0, far, far + 0.05, normal, -normal),
                    ),
                ],
            },
            &volumes,
            &laws,
        )
        .unwrap()
        .events
        .len()
    };
    let residual_at = |perpendicular: f32| (far - near) - predicted(offset_for(perpendicular));

    assert!(
        far - near_end > 0.05,
        "the runs really are far apart along the ray: {} m",
        far - near_end,
    );

    // WHERE ONE SURFACE WOULD PUT IT.
    let on_surface = 0.00015_f32;
    assert!(
        residual_at(on_surface).abs() <= laws.event_residual_tolerance,
        "the near arm must sit where one plane would put it, got residual {}",
        residual_at(on_surface),
    );
    assert_eq!(
        events_at(on_surface),
        1,
        "130 mm apart and still one surface, because that is where one plane puts them",
    );

    // AND WHERE IT WOULD NOT — the original 80 mm-at-87° assertion, now true for the right reason:
    // 1.8 mm off the plane is 40 mm off along the ray, and the plane test alone would believe it.
    let displaced = 0.0018_f32;
    assert!(
        residual_at(displaced).abs() > laws.event_residual_tolerance,
        "the far arm must be off the shared surface, got residual {}",
        residual_at(displaced),
    );
    assert_eq!(
        events_at(displaced),
        2,
        "{} m from where one plane would put it, the plane test alone must not merge them",
        residual_at(displaced).abs(),
    );
}

/// The other half of the same rule: the SAME primitive crossed by two samples at overlapping
/// depths is one crossing. Concavity must not shatter an ordinary hit.
#[test]
fn a_concave_primitive_crossed_together_stays_one_event() {
    let volumes = table(&[(1, 1000.0)]);
    let laws = WalkLaws::default();
    // Deliberately NOT coplanar entry faces (a curved cast surface), so only the shared-primitive
    // branch can associate them.
    let a = slab(1, 0, 0.2, 0.3, Vec3::new(0.0, 0.7, -0.7));
    let b = slab(1, 0, 0.22, 0.32, Vec3::new(0.0, -0.7, -0.7));
    let walked = walk_disc(
        &DiscCorridor {
            anchor: Vec3::ZERO,
            origin: Vec3::ZERO,
            axis: AXIS,
            length: 2.0,
            radius: 0.02,
            frame: DiscFrame {
                u: Vec3::X,
                v: Vec3::Y,
            },
            samples: vec![sample(Vec3::ZERO, a), sample(Vec3::X * 0.02, b)],
        },
        &volumes,
        &laws,
    )
    .unwrap();
    assert_eq!(walked.events.len(), 1);
    assert_eq!(walked.events[0].coverage, 1.0);
}

/// A half-covered disc engages half the world: `η ≈ 0.5` and half the cost. Armor-zone boundaries
/// grade over one caliber (§13.5's accepted cost), rather than snapping at a sharp line.
#[test]
fn a_half_covered_disc_gives_half_coverage_and_half_cost() {
    let volumes = table(&[(1, 1000.0)]);
    let laws = WalkLaws::default();
    let frame = DiscFrame::from_axis_and_reference(AXIS, Vec3::Y).unwrap();
    let offsets = disc_offsets(&frame, 0.044, DEFAULT_RING);
    let k = offsets.len();
    let samples: Vec<SampleCorridor> = offsets
        .into_iter()
        .map(|offset| {
            if offset.y > 0.0 {
                sample(offset, plate(1, 0, 0.5, 0.6))
            } else {
                sample(offset, Vec::new())
            }
        })
        .collect();
    let covered = samples.iter().filter(|s| !s.hits.is_empty()).count();
    let walked = walk_disc(
        &DiscCorridor {
            anchor: Vec3::ZERO,
            origin: Vec3::ZERO,
            axis: AXIS,
            length: 2.0,
            radius: 0.044,
            frame,
            samples,
        },
        &volumes,
        &laws,
    )
    .unwrap();
    let event = &walked.events[0];
    let expected = covered as f32 / k as f32;
    assert!((event.coverage - expected).abs() < 1.0e-6);
    assert!(
        (event.cost - 100.0 * expected).abs() < 0.5,
        "{}",
        event.cost
    );
}

/// A FRAGMENT is a shell with r → 0 and k = 1 (§13.5), so it must reduce EXACTLY to the single-ray
/// walk — bitwise, not approximately. `cast_spall_fragment` then needs no law of its own.
#[test]
fn a_fragment_is_the_single_ray_walk_bitwise() {
    let volumes = table(&[(1, 1000.0), (2, 200.0)]);
    let laws = WalkLaws::default();
    let mut hits = plate(1, 0, 0.25, 0.375);
    hits.extend(plate(2, 0, 0.5, 0.75));
    let single = walk(&corridor(2.0, hits.clone()), &volumes);
    let walked = walk_disc(&point_disc(2.0, sample(Vec3::ZERO, hits)), &volumes, &laws).unwrap();
    let total: f32 = walked.events.iter().map(|e| e.cost).sum();
    assert_eq!(total.to_bits(), single.cost.to_bits());
    assert_eq!(walked.events[0].entry_normal, single.runs[0].entry_normal);
    assert_eq!(walked.events[0].coverage, 1.0);
}

/// Rolling the ring phase cannot change a homogeneous case — it would mean the sample frame had
/// leaked into the physics.
#[test]
fn rolling_the_ring_phase_leaves_a_flat_plate_unchanged() {
    let volumes = table(&[(1, 1000.0)]);
    let laws = WalkLaws::default();
    let mut costs = Vec::new();
    for reference in [Vec3::Y, Vec3::X, Vec3::new(1.0, 1.0, 0.0), -Vec3::Y] {
        let frame = DiscFrame::from_axis_and_reference(AXIS, reference).unwrap();
        let samples = disc_offsets(&frame, 0.044, DEFAULT_RING)
            .into_iter()
            .map(|offset| sample(offset, plate(1, 0, 0.5, 0.6)))
            .collect();
        let walked = walk_disc(
            &DiscCorridor {
                anchor: Vec3::ZERO,
                origin: Vec3::ZERO,
                axis: AXIS,
                length: 2.0,
                radius: 0.044,
                frame,
                samples,
            },
            &volumes,
            &laws,
        )
        .unwrap();
        costs.push(walked.events[0].cost.to_bits());
    }
    assert!(costs.windows(2).all(|w| w[0] == w[1]), "{costs:?}");
}

/// The frame is TRANSPORTED, never rebuilt: a direction change that would cross any world-axis
/// fallback branch must carry the roll with it, or the sample pattern snaps mid-flight.
#[test]
fn the_disc_frame_transports_rather_than_re_rolling() {
    let frame = DiscFrame::from_axis_and_reference(Vec3::Z, Vec3::Y).unwrap();
    let mut axis = Vec3::Z;
    let mut carried = frame;
    // Walk the axis all the way onto +Y, straight through where a "cross with world Y" rule is
    // singular.
    for step in 1..=90 {
        let angle = (step as f32).to_radians();
        let next = Vec3::new(0.0, angle.sin(), angle.cos()).normalize();
        carried = carried.transport(axis, next);
        axis = next;
    }
    assert!((carried.u.length() - 1.0).abs() < 1.0e-4);
    assert!(carried.u.dot(axis).abs() < 1.0e-4);
    assert!(carried.u.cross(carried.v).dot(axis) > 0.99);
}

/// Axis-hit/ring-miss and its converse: the aggregate reports the coverage each really has, and the
/// cost scales with it.
#[test]
fn axis_only_and_ring_only_coverage_are_reported_honestly() {
    let volumes = table(&[(1, 1000.0)]);
    let laws = WalkLaws::default();
    let frame = DiscFrame::from_axis_and_reference(AXIS, Vec3::Y).unwrap();
    let offsets = disc_offsets(&frame, 0.044, DEFAULT_RING);
    let k = offsets.len();

    let axis_only: Vec<SampleCorridor> = offsets
        .iter()
        .enumerate()
        .map(|(index, offset)| {
            sample(
                *offset,
                if index == 0 {
                    plate(1, 0, 0.5, 0.6)
                } else {
                    Vec::new()
                },
            )
        })
        .collect();
    let walked = walk_disc(
        &DiscCorridor {
            anchor: Vec3::ZERO,
            origin: Vec3::ZERO,
            axis: AXIS,
            length: 2.0,
            radius: 0.044,
            frame,
            samples: axis_only,
        },
        &volumes,
        &laws,
    )
    .unwrap();
    assert!((walked.events[0].coverage - 1.0 / k as f32).abs() < 1.0e-6);
    assert!((walked.events[0].cost - 100.0 / k as f32).abs() < 0.01);

    let ring_only: Vec<SampleCorridor> = offsets
        .iter()
        .enumerate()
        .map(|(index, offset)| {
            sample(
                *offset,
                if index == 0 {
                    Vec::new()
                } else {
                    plate(1, 0, 0.5, 0.6)
                },
            )
        })
        .collect();
    let walked = walk_disc(
        &DiscCorridor {
            anchor: Vec3::ZERO,
            origin: Vec3::ZERO,
            axis: AXIS,
            length: 2.0,
            radius: 0.044,
            frame,
            samples: ring_only,
        },
        &volumes,
        &laws,
    )
    .unwrap();
    assert!((walked.events[0].coverage - (k - 1) as f32 / k as f32).abs() < 1.0e-6);
}

/// Two samples meeting DIFFERENT stacks: the shared outer plate makes them one event, and each
/// downward step carries its own coverage — entrance η, internal-step η and exit η are different
/// subsets and must not be collapsed into one scalar.
#[test]
fn different_stacks_on_different_samples_carry_per_boundary_coverage() {
    let volumes = table(&[(1, 1000.0), (2, 200.0)]);
    let laws = WalkLaws::default();
    let frame = DiscFrame {
        u: Vec3::X,
        v: Vec3::Y,
    };
    // Both samples cross the same outer plate; only one continues into the ammunition behind it.
    let mut deep = plate(1, 0, 0.5, 0.6);
    deep.extend(plate(2, 0, 0.6, 0.7));
    let walked = walk_disc(
        &DiscCorridor {
            anchor: Vec3::ZERO,
            origin: Vec3::ZERO,
            axis: AXIS,
            length: 2.0,
            radius: 0.02,
            frame,
            samples: vec![
                sample(Vec3::ZERO, plate(1, 0, 0.5, 0.6)),
                sample(Vec3::X * 0.02, deep),
            ],
        },
        &volumes,
        &laws,
    )
    .unwrap();
    assert_eq!(walked.events.len(), 1);
    let event = &walked.events[0];
    assert_eq!(event.coverage, 1.0);
    // Two distinct downward steps: the 1000 → 200 the deep sample alone saw, and the exits.
    let step = event
        .spall
        .iter()
        .find(|mark| mark.from_factor == 1000.0 && mark.to_factor == 200.0)
        .expect("the interior step must be its own spall source");
    assert_eq!(step.coverage, 0.5);
    let coverage = |vol: u32| {
        event
            .shares
            .iter()
            .find(|share| share.entity == volume(vol))
            .expect("both volumes must appear")
            .coverage
    };
    assert_eq!(coverage(1), 1.0);
    assert_eq!(coverage(2), 0.5);
}

/// `n̄` cannot degenerate. Maximally opposed entry patches — a shell straddling a knife edge — still
/// leave an aggregate normal that leans against the axis, because the tangent gate admits a face as
/// an ENTRY only when it does. This is §13.5's repair of the point model stated as an invariant: the
/// patch average is smooth everywhere, and corners average to their bisector.
#[test]
fn the_aggregate_entry_normal_cannot_degenerate() {
    let volumes = table(&[(1, 1000.0)]);
    let laws = WalkLaws::default();
    let frame = DiscFrame {
        u: Vec3::X,
        v: Vec3::Y,
    };
    let a = slab(1, 0, 0.5, 0.55, Vec3::new(0.0, -1.0, -0.001));
    let b = slab(1, 0, 0.5, 0.55, Vec3::new(0.0, 1.0, -0.001));
    let walked = walk_disc(
        &DiscCorridor {
            anchor: Vec3::ZERO,
            origin: Vec3::ZERO,
            axis: AXIS,
            length: 2.0,
            radius: 0.02,
            frame,
            samples: vec![sample(Vec3::ZERO, a), sample(Vec3::X * 0.02, b)],
        },
        &volumes,
        &laws,
    )
    .expect("opposed patches still average to a usable bisector");
    let normal = walked.events[0].entry_normal;
    assert!((normal.length() - 1.0).abs() < 1.0e-5);
    assert!(AXIS.dot(normal) < 0.0, "n̄ always leans against the ray");
}

// ---------------------------------------------------------------------------------------------
// §13.3 / §13.5 / §3 — the staged resolution
// ---------------------------------------------------------------------------------------------

const AP_88: Shot = Shot {
    caliber: 0.088,
    capability: 250.0,
};

fn transit_request(walked: &DiscWalk, shot: &Shot, laws: &WalkLaws) -> TransitRequest {
    match begin(walked, shot, laws) {
        Begin::Transit(request) => request,
        other => panic!("expected the round to bite in, got {other:?}"),
    }
}

/// §13.3's motivating pathology: an exposed forearm is ~80 mm "thick", so a GEOMETRIC overmatch test
/// leaves it un-overmatched and past 70° an arm ricochets an 88. Factor-weighted, 80 mm of flesh is
/// ~0.2 mm steel-equivalent — overmatched by everything, deflecting nothing.
#[test]
fn an_arm_does_not_ricochet_an_88() {
    let laws = WalkLaws::default();
    // ~76° incidence, well past the ricochet threshold.
    let normal = Vec3::new(0.0, 0.97, -0.242).normalize();
    let arm = entrance_disc(&[Slab::at(1, 0, 0.5, normal, 0.08)], 2.0);

    let walked = walk_disc(&arm, &table(&[(1, 10.0)]), &laws).unwrap();
    let request = transit_request(&walked, &AP_88, &laws);
    assert!(request.entrance.overmatched);
    assert!(request.entrance.incidence > laws.ricochet_angle);

    // Real armour at the same obliquity DOES deflect it — the law still works where it should.
    let plate = entrance_disc(&[Slab::at(1, 0, 0.5, normal, 0.4)], 2.0);
    let walked = walk_disc(&plate, &table(&[(1, 1000.0)]), &laws).unwrap();
    assert!(matches!(
        begin(&walked, &AP_88, &laws),
        Begin::Ricochet { .. }
    ));
}

/// The COVERED-sample mean is what overmatch reads (`cost ÷ η`), not the disc mean. Dropping the
/// division makes a lightly-engaged plate look thin, and a graze that should bounce punches through
/// instead — the exact inversion of §13.5's graded weakspot.
///
/// Sized to straddle the threshold: 200 mm of oblique steel is 48 mm steel-equivalent along the
/// normal (3 × 48 > 88, so no overmatch), but scaled by a 4/13 coverage it reads 15 mm (3 × 15 < 88,
/// false overmatch).
#[test]
fn a_lightly_covered_plate_is_not_thin_enough_to_overmatch() {
    let laws = WalkLaws::default();
    let volumes = table(&[(1, 1000.0)]);
    let normal = Vec3::new(0.0, 0.97, -0.242).normalize();
    let frame = DiscFrame::from_axis_and_reference(AXIS, Vec3::Y).unwrap();
    let steel = [Slab::at(1, 0, 0.5, normal, 0.2)];
    let corridor = disc_along(Vec3::ZERO, AXIS, frame, 0.044, 2.0, &steel, |index| {
        index % 4 == 0
    });
    let covered = corridor
        .samples
        .iter()
        .filter(|s| !s.hits.is_empty())
        .count();
    let walked = walk_disc(&corridor, &volumes, &laws).unwrap();
    let event = &walked.events[0];
    assert_eq!(covered, 4, "fixture must stay lightly covered");
    assert!(event.coverage < 0.35, "η = {}", event.coverage);

    match begin(&walked, &AP_88, &laws) {
        Begin::Ricochet { entrance, .. } => {
            assert!(!entrance.overmatched);
            // The covered-sample thickness, NOT the η-diluted one.
            assert!(
                (entrance.steel_equivalent - 0.0484).abs() < 2.0e-3,
                "{}",
                entrance.steel_equivalent
            );
            assert!(
                entrance.steel_equivalent * event.coverage * laws.overmatch_ratio < AP_88.caliber,
                "the fixture must actually straddle the threshold"
            );
        }
        other => panic!("a 200 mm oblique plate must deflect an 88: {other:?}"),
    }
}

/// §13.5 (RULED 2026-08-07): η scales the DEFLECTION ANGLE as well as the bleed. A graze is a
/// partial ricochet in direction too, so 1 mm of aim can never flip the outcome between "flies past"
/// and "full bounce".
#[test]
fn partial_coverage_scales_the_deflection_angle_and_the_bleed() {
    let laws = WalkLaws::default();
    let volumes = table(&[(1, 1000.0)]);
    let normal = Vec3::new(0.0, 0.97, -0.242).normalize();
    let frame = DiscFrame::from_axis_and_reference(AXIS, Vec3::Y).unwrap();
    let steel = [Slab::at(1, 0, 0.5, normal, 0.4)];

    let full = disc_along(Vec3::ZERO, AXIS, frame, 0.044, 2.0, &steel, |_| true);
    let full = walk_disc(&full, &volumes, &laws).unwrap();
    let (full_direction, full_scale) = match begin(&full, &AP_88, &laws) {
        Begin::Ricochet {
            direction,
            speed_scale,
            ..
        } => (direction, speed_scale),
        other => panic!("expected a ricochet, got {other:?}"),
    };
    // At η = 1 the blend IS the classic bounce, returned bit-for-bit — the pre-ruling behaviour is
    // preserved exactly at full coverage.
    let specular = Vec3::from(super::super::reflect(
        Dir3::new(AXIS).unwrap(),
        Dir3::new(full.events[0].entry_normal).unwrap(),
    ));
    assert_eq!(full_direction.to_array(), specular.to_array());
    assert!((full_scale - laws.ricochet_bleed).abs() < 1.0e-6);
    let full_turn = AXIS.angle_between(full_direction);

    // Partial coverage turns the round proportionally less, and bleeds proportionally less.
    let mut previous_turn = full_turn;
    for stride in [2usize, 4, 6] {
        let partial = disc_along(Vec3::ZERO, AXIS, frame, 0.044, 2.0, &steel, |index| {
            index % stride == 0
        });
        let partial = walk_disc(&partial, &volumes, &laws).unwrap();
        let coverage = partial.events[0].coverage;
        match begin(&partial, &AP_88, &laws) {
            Begin::Ricochet {
                direction,
                speed_scale,
                ..
            } => {
                let turn = AXIS.angle_between(direction);
                assert!(
                    (turn - coverage * full_turn).abs() < 1.0e-3,
                    "η = {coverage}: turned {turn}, expected {}",
                    coverage * full_turn
                );
                assert!(turn < previous_turn, "less coverage must turn it less");
                let expected = 1.0 - coverage * (1.0 - laws.ricochet_bleed);
                assert!((speed_scale - expected).abs() < 1.0e-5, "{speed_scale}");
                previous_turn = turn;
            }
            other => panic!("expected a partial ricochet at η = {coverage}: {other:?}"),
        }
    }
}

/// Normalization scales by η the same way: a barely-engaged penetrating entry is barely bent.
#[test]
fn partial_coverage_scales_the_normalization_bend() {
    let laws = WalkLaws::default();
    let volumes = table(&[(1, 1000.0)]);
    // 45°, inside the ricochet threshold so the round bites in.
    let normal = Vec3::new(0.0, 1.0, -1.0).normalize();
    let frame = DiscFrame::from_axis_and_reference(AXIS, Vec3::Y).unwrap();
    let steel = [Slab::at(1, 0, 0.5, normal, 0.02)];

    let full = walk_disc(
        &disc_along(Vec3::ZERO, AXIS, frame, 0.044, 2.0, &steel, |_| true),
        &volumes,
        &laws,
    )
    .unwrap();
    let full_request = transit_request(&full, &AP_88, &laws);
    let full_bend = AXIS.angle_between(full_request.axis);
    // At η = 1 the bend is the classic `normalization × incidence`, unscaled.
    assert!(
        (full_bend - laws.normalization * full_request.entrance.incidence).abs() < 1.0e-4,
        "{full_bend}"
    );

    let partial = walk_disc(
        &disc_along(Vec3::ZERO, AXIS, frame, 0.044, 2.0, &steel, |index| {
            index % 3 == 0
        }),
        &volumes,
        &laws,
    )
    .unwrap();
    let coverage = partial.events[0].coverage;
    let partial_bend = AXIS.angle_between(transit_request(&partial, &AP_88, &laws).axis);
    assert!(
        (partial_bend - coverage * full_bend).abs() < 1.0e-3,
        "η = {coverage}: bent {partial_bend}, expected {}",
        coverage * full_bend
    );
}

/// The transit ray is anchored on the DISC AXIS, never on the covered-sample centroid — a centroid
/// would drag the shell's line of flight sideways toward whatever part of the disc touched geometry,
/// which is the lateral asymmetry §13.5 rules out (re-affirmed 2026-08-07).
#[test]
fn the_transit_ray_is_anchored_on_the_axis_not_the_coverage_centroid() {
    let laws = WalkLaws::default();
    let volumes = table(&[(1, 1000.0)]);
    let frame = DiscFrame::from_axis_and_reference(AXIS, Vec3::Y).unwrap();
    // Head-on plate seen by a lop-sided quarter of the disc: the covered samples all sit on one side.
    let plate = [Slab::at(1, 0, 0.5, -AXIS, 0.02)];
    let offsets = disc_offsets(&frame, 0.044, DEFAULT_RING);
    let corridor = disc_along(Vec3::ZERO, AXIS, frame, 0.044, 2.0, &plate, |index| {
        offsets[index].y > 0.01
    });
    let walked = walk_disc(&corridor, &volumes, &laws).unwrap();
    let event = &walked.events[0];
    assert!(event.coverage < 0.5);
    // The REPORTED surface is a mean and is lop-sided, exactly as §13.5 says an aggregate should be…
    assert!(event.entry_position.y > 0.01, "{}", event.entry_position);

    // …but the shell's own axis is not moved by it.
    let request = transit_request(&walked, &AP_88, &laws);
    assert!(
        request.origin.xy().length() < 1.0e-6,
        "the transit ray drifted laterally to {}",
        request.origin
    );
    assert!((request.origin.z - 0.5).abs() < 1.0e-5);
}

/// The handoff exports per-sample SEED state, and the seed is a LOOKUP on a ray already walked
/// rather than an inference — each sample resumes from the point where its own entrance ray met the
/// surface.
///
/// Two flush plates staggered by 1 mm — exactly the sub-caliber authoring slop §13.7 tolerates — are
/// one crossing whose mean surface sits between them. The samples that met the nearer plate are
/// therefore already inside it at the handoff and must be seeded; the ones that met the further
/// plate are not yet in and must not be, because their entry face arrives in the transit corridor's
/// own hit list at `t = 0`.
#[test]
fn the_handoff_seeds_every_sample_that_starts_inside_material() {
    let laws = WalkLaws::default();
    let volumes = table(&[(1, 1000.0), (2, 1000.0)]);
    let frame = DiscFrame::from_axis_and_reference(AXIS, Vec3::Y).unwrap();
    let offsets = disc_offsets(&frame, 0.044, DEFAULT_RING);
    let near = Slab::at(1, 0, 0.499, -AXIS, 0.1);
    let far = Slab::at(2, 0, 0.5, -AXIS, 0.1);
    let samples = offsets
        .iter()
        .map(|offset| {
            let slab = if offset.y > 0.0 { near } else { far };
            SampleCorridor {
                offset: *offset,
                initial_presence: Vec::new(),
                hits: slab.hits(*offset, AXIS, 3.0, false),
            }
        })
        .collect();
    let walked = walk_disc(
        &DiscCorridor {
            anchor: Vec3::ZERO,
            origin: Vec3::ZERO,
            axis: AXIS,
            length: 3.0,
            radius: 0.044,
            frame,
            samples,
        },
        &volumes,
        &laws,
    )
    .unwrap();
    assert_eq!(walked.events.len(), 1, "1 mm of stagger is one crossing");
    let request = transit_request(&walked, &AP_88, &laws);

    assert_eq!(request.seeds.len(), request.samples);
    assert_eq!(request.seeds[0].offset, Vec3::ZERO, "sample 0 IS the axis");
    let seeded = request
        .seeds
        .iter()
        .filter(|s| !s.inside.is_empty())
        .count();
    assert!(
        seeded > 0 && seeded < request.samples,
        "a staggered surface seeds some samples and not others, got {seeded}/{}",
        request.samples
    );
    // Every seeded sample names the NEAR plate — the one it is actually inside.
    for seed in request.seeds.iter().filter(|s| !s.inside.is_empty()) {
        assert_eq!(
            seed.inside,
            vec![PrimitiveKey {
                volume: volume(1),
                primitive: prim(0)
            }]
        );
    }

    // The seeded corridor resolves; the same corridor WITHOUT the seeds reports the unmatched exits
    // it inherits, which is the whole point of exporting them.
    let transit = DiscCorridor {
        anchor: request.anchor,
        origin: request.origin,
        axis: request.axis,
        length: 3.0,
        radius: request.radius,
        frame: request.frame,
        samples: request
            .seeds
            .iter()
            .map(|seed| {
                let slab = if offsets[seed.sample].y > 0.0 {
                    near
                } else {
                    far
                };
                SampleCorridor {
                    offset: seed.offset,
                    initial_presence: seed.inside.clone(),
                    hits: slab.hits(
                        request.origin + seed.offset,
                        request.axis,
                        3.0,
                        seed.inside.contains(&slab.key()),
                    ),
                }
            })
            .collect(),
    };
    assert!(walk_disc(&transit, &volumes, &laws).is_ok());
    let mut unseeded = transit.clone();
    for sample in &mut unseeded.samples {
        sample.initial_presence.clear();
    }
    assert!(matches!(
        walk_disc(&unseeded, &volumes, &laws),
        Err(WalkError::UnexpectedExit { .. })
    ));
}

/// `finish` validates the corridor it is handed against the corridor it asked for. Nothing in the
/// type system stops a caller collecting along the unbent axis, from the wrong origin, with a
/// re-derived frame, or around a different contact — and each of those silently resolves one
/// surface's geometry against another surface's entrance verdict.
#[test]
fn finish_rejects_a_corridor_that_is_not_the_one_it_asked_for() {
    let laws = WalkLaws::default();
    let volumes = table(&[(1, 1000.0)]);
    let normal = Vec3::new(0.0, 0.5, -0.866).normalize();
    let slabs = [Slab::at(1, 0, 0.5, normal, 0.05)];
    let walked = walk_disc(&entrance_disc(&slabs, 3.0), &volumes, &laws).unwrap();
    let request = transit_request(&walked, &AP_88, &laws);
    let honest = walk_disc(&transit_disc(&request, &slabs, 3.0), &volumes, &laws).unwrap();
    assert!(finish(&honest, &request, &AP_88, &laws).is_ok());

    // The entrance walk itself: right geometry, wrong axis and origin.
    assert!(matches!(
        finish(&walked, &request, &AP_88, &laws),
        Err(WalkError::CorridorMismatch { .. })
    ));

    // A re-derived frame instead of the transported one.
    let mut rolled = honest.clone();
    rolled.frame = DiscFrame::from_axis_and_reference(request.axis, Vec3::X).unwrap();
    assert!(matches!(
        finish(&rolled, &request, &AP_88, &laws),
        Err(WalkError::CorridorMismatch { .. })
    ));

    // A corridor collected around a different contact entirely: right axis and origin, other volume.
    let elsewhere = Slab::at(9, 0, 1.5, normal, 0.05);
    let mut unrelated = transit_disc(&request, &[elsewhere], 3.0);
    for (sample, seed) in unrelated.samples.iter_mut().zip(&request.seeds) {
        sample.initial_presence.clear();
        sample.hits = elsewhere.hits(request.origin + seed.offset, request.axis, 3.0, false);
    }
    let other = walk_disc(&unrelated, &table(&[(9, 1000.0)]), &laws).unwrap();
    assert!(matches!(
        finish(&other, &request, &AP_88, &laws),
        Err(WalkError::CorridorMismatch { .. })
    ));
}

/// The disc's aggregate geometry matching is not enough: the SAMPLES must resume where the handoff
/// put them. A corridor can carry the right axis, origin, frame, radius and sample count while one
/// ray samples 10 mm away — and the seeds are only sound for the rays they were read on.
#[test]
fn finish_rejects_a_corridor_whose_samples_moved() {
    let laws = WalkLaws::default();
    let volumes = table(&[(1, 1000.0)]);
    let normal = Vec3::new(0.0, 0.5, -0.866).normalize();
    let slabs = [Slab::at(1, 0, 0.5, normal, 0.05)];
    let walked = walk_disc(&entrance_disc(&slabs, 3.0), &volumes, &laws).unwrap();
    let request = transit_request(&walked, &AP_88, &laws);

    let mut moved = transit_disc(&request, &slabs, 3.0);
    // One ray, 10 mm across. Everything the aggregate check looks at is untouched.
    moved.samples[3].offset += Vec3::X * 0.01;
    let moved = walk_disc(&moved, &volumes, &laws).unwrap();
    assert_eq!(moved.axis, request.axis);
    assert_eq!(moved.origin, request.origin);
    assert_eq!(moved.samples(), request.samples);
    assert!(matches!(
        finish(&moved, &request, &AP_88, &laws),
        Err(WalkError::CorridorMismatch { .. })
    ));
}

/// First-crossing identity is checked on the geometry a crossing is MADE of and on where it sits —
/// not on entity alone. One hull presents primitives metres apart, so "names a volume the entrance
/// also named" is satisfied by a crossing of a completely different part of the same tank.
#[test]
fn finish_rejects_a_far_crossing_of_the_same_entity() {
    let laws = WalkLaws::default();
    let volumes = table(&[(1, 1000.0)]);
    let front = Slab::at(1, 0, 0.5, -AXIS, 0.05);
    let walked = walk_disc(&entrance_disc(&[front], 3.0), &volumes, &laws).unwrap();
    let request = transit_request(&walked, &AP_88, &laws);

    // Same VOLUME, a different primitive of it, two metres downrange.
    let elsewhere = Slab::at(1, 7, 2.5, -AXIS, 0.05);
    let mut corridor = transit_disc(&request, &[front], 3.0);
    for (sample, seed) in corridor.samples.iter_mut().zip(&request.seeds) {
        sample.initial_presence.clear();
        sample.hits = elsewhere.hits(request.origin + seed.offset, request.axis, 3.0, false);
    }
    let far = walk_disc(&corridor, &volumes, &laws).unwrap();
    assert_eq!(
        far.events[0].shares[0].entity,
        volume(1),
        "the fixture must keep the entity identical — that is the point"
    );
    assert!(matches!(
        finish(&far, &request, &AP_88, &laws),
        Err(WalkError::CorridorMismatch { .. })
    ));
}

/// Capability exhausted partway through a factor CHANGE: the embed point is the inverse of the
/// prefix integral, not `span × cap/cost`, and per-entity damage is CLIPPED at that progress — a
/// round that dies in the plate deposits nothing in the crewman behind it.
#[test]
fn capability_exhausted_midway_inverts_the_prefix_integral_and_clips_damage() {
    let laws = WalkLaws::default();
    let volumes = table(&[(1, 1000.0), (2, 10.0)]);
    // 100 mm of RHA head-on, then a crewman.
    let slabs = [
        Slab::at(1, 0, 0.5, -AXIS, 0.1),
        Slab::at(2, 0, 0.6, -AXIS, 0.3),
    ];
    let walked = walk_disc(&entrance_disc(&slabs, 3.0), &volumes, &laws).unwrap();
    let shot = Shot {
        caliber: 0.088,
        capability: 60.0,
    };
    let request = transit_request(&walked, &shot, &laws);
    let transit = walk_disc(&transit_disc(&request, &slabs, 3.0), &volumes, &laws).unwrap();
    let plan = finish(&transit, &request, &shot, &laws).unwrap();

    // Head-on, so the handoff is the plate's own face: 60 reference-mm dies 60 mm in.
    match plan.outcome {
        Outcome::Embedded { t, .. } => assert!((t - 0.06).abs() < 1.0e-3, "{t}"),
        other => panic!("expected an embed, got {other:?}"),
    }
    assert!((plan.cost_spent - 60.0).abs() < 0.1);
    let armour = plan
        .deposits
        .iter()
        .find(|d| d.entity == volume(1))
        .expect("the plate it died in must be charged");
    assert!((armour.chord - 0.06).abs() < 1.0e-3, "{}", armour.chord);
    assert!(
        !plan.deposits.iter().any(|d| d.entity == volume(2)),
        "a round that stopped in the plate cannot wound the crewman behind it"
    );
    assert!(plan.spall.is_empty(), "no exit, no spall (§5)");
}

/// Enough capability and the same stack perforates: full cost spent, the crewman charged for HIS
/// chord at HIS factor (§13.2's damage law — no ownership, no argmax), and the exit throws spall.
#[test]
fn a_perforation_charges_every_entity_for_its_own_material() {
    let laws = WalkLaws::default();
    let volumes = table(&[(1, 1000.0), (2, 10.0)]);
    let slabs = [
        Slab::at(1, 0, 0.5, -AXIS, 0.1),
        Slab::at(2, 0, 0.6, -AXIS, 0.3),
    ];
    let walked = walk_disc(&entrance_disc(&slabs, 3.0), &volumes, &laws).unwrap();
    let request = transit_request(&walked, &AP_88, &laws);
    let transit = walk_disc(&transit_disc(&request, &slabs, 3.0), &volumes, &laws).unwrap();
    let plan = finish(&transit, &request, &AP_88, &laws).unwrap();

    assert!(matches!(plan.outcome, Outcome::Perforated { .. }));
    assert!((plan.cost_spent - 103.0).abs() < 0.5, "{}", plan.cost_spent);
    let crew = plan
        .deposits
        .iter()
        .find(|d| d.entity == volume(2))
        .expect("the crewman transited must be charged");
    assert!((crew.chord - 0.3).abs() < 1.0e-3);
    assert!(
        (crew.cost - 3.0).abs() < 0.05,
        "flesh charges its own factor"
    );
    assert!(!plan.spall.is_empty());
}

/// Overmatch charges the PERPENDICULAR projection rather than the oblique chord (§4: it cannot
/// present its slope to a round that dwarfs it) — and the projection reaches every CONSEQUENCE.
/// §13.5 defines the spall budget as the event's cost and §6 defines transit damage from the cost
/// paid, so a projection that stopped at `cost_spent` would spend 15 while throwing spall and
/// depositing damage for 30.
#[test]
fn overmatch_charges_the_perpendicular_projection_everywhere() {
    let laws = WalkLaws::default();
    let volumes = table(&[(1, 1000.0)]);
    // 15 mm plate at 60°: a 30 mm chord head-on, and 88 mm overmatches it.
    let normal = Vec3::new(0.0, 0.866, -0.5).normalize();
    let slabs = [Slab::at(1, 0, 0.5, normal, 0.03)];
    let walked = walk_disc(&entrance_disc(&slabs, 3.0), &volumes, &laws).unwrap();
    let request = transit_request(&walked, &AP_88, &laws);
    assert!(request.entrance.overmatched);

    let transit = walk_disc(&transit_disc(&request, &slabs, 3.0), &volumes, &laws).unwrap();
    let plan = finish(&transit, &request, &AP_88, &laws).unwrap();

    // Whatever the normalization bend did to the chord, the CHARGE is the 15 mm perpendicular
    // thickness — which is the point of the projection.
    assert!((plan.cost_spent - 15.0).abs() < 0.2, "{}", plan.cost_spent);
    let exit = plan
        .spall
        .iter()
        .find(|mark| mark.to_factor == 0.0)
        .expect("a perforation exits");
    assert!(
        (exit.budget - 15.0).abs() < 0.2,
        "spall budget {}",
        exit.budget
    );
    let deposit = &plan.deposits[0];
    assert!(
        (deposit.cost - 15.0).abs() < 0.2,
        "deposit {}",
        deposit.cost
    );
    assert!(
        (deposit.chord - 0.015).abs() < 5.0e-4,
        "the charged chord is the perpendicular one: {}",
        deposit.chord
    );
    // The GEOMETRY is untouched — the round really did travel its slope chord.
    match plan.outcome {
        Outcome::Perforated { t, .. } => assert!(t > 0.02, "{t}"),
        other => panic!("expected a perforation, got {other:?}"),
    }
}

/// An empty disc decides nothing — no fabricated entrance, no fabricated terminal (§13.6's "no
/// fabricated events").
#[test]
fn an_empty_disc_is_a_miss() {
    let laws = WalkLaws::default();
    let volumes = table(&[(1, 1000.0)]);
    let walked = walk_disc(&entrance_disc(&[], 2.0), &volumes, &laws).unwrap();
    assert!(walked.events.is_empty());
    assert_eq!(begin(&walked, &AP_88, &laws), Begin::Miss);
}

// ---------------------------------------------------------------------------------------------
// Mutant-ledger fixtures
// ---------------------------------------------------------------------------------------------
//
// Each of these was written because an adversarial review MUTATED the law it names and the whole
// suite stayed green. A test that cannot fail when its law is deleted is not testing the law, and
// the gap is always the same shape: the fixture happened to sit where the mutation makes no
// difference. They are kept apart from the tests they harden only so that the reason they exist
// stays legible.

/// `max` over the covering volumes, mutated to "whichever active entity comes last", survived the
/// monotonicity test — because the entity iteration order happened to end on the steel. The fixture
/// has to be run in BOTH id orders, and the assertion has to be equality, not merely "not lower":
/// flesh clipped into a turret wall must change the cost by exactly nothing.
#[test]
fn a_weaker_volume_inside_steel_cannot_dilute_it_in_either_id_order() {
    for (steel, flesh) in [(9u32, 1u32), (1u32, 9u32)] {
        let volumes = table(&[(steel, 1000.0), (flesh, 10.0)]);
        let bare = walk(&corridor(2.0, plate(steel, 0, 0.2, 0.8)), &volumes);
        let mut clipped = plate(steel, 0, 0.2, 0.8);
        clipped.extend(plate(flesh, 0, 0.3, 0.5));
        let clipped = walk(&corridor(2.0, clipped), &volumes);
        assert_eq!(
            bare.cost.to_bits(),
            clipped.cost.to_bits(),
            "steel id {steel}, flesh id {flesh}: {} vs {}",
            bare.cost,
            clipped.cost
        );
        assert_eq!(bare.spans, clipped.spans);
        // …and the flesh is still charged for ITS OWN chord, at its own factor (§13.2's damage law).
        let share = clipped
            .presence
            .iter()
            .find(|p| p.entity == volume(flesh))
            .expect("the clipped volume is still present");
        assert!((share.cost - 2.0).abs() < 1.0e-3, "{}", share.cost);
    }
}

/// Deleting the weld FACE-COMPATIBILITY guard left the whole suite green: the existing
/// unrelated-plate fixture is rejected by the perpendicular-gap guard first, so it never exercised
/// this rule at all.
///
/// Here both faces are near-tangent and face the SAME way — a wedge tip, not two sides of a void.
/// Every other guard passes (0.9 mm perpendicular, 20 mm lookahead, inside the chain budget), so
/// only face compatibility can reject it.
#[test]
fn a_micro_gap_between_same_facing_faces_does_not_weld() {
    let volumes = table(&[(1, 1000.0), (2, 1000.0)]);
    let grazing_out = Vec3::new(0.0, 0.999, 0.045);
    let grazing_in = Vec3::new(0.0, 0.999, -0.045);

    // Same-facing: `n_exit · n_entry ≈ +1`. Not a gap with two sides.
    let mut same = oblique_plate(1, 0, 0.2, 0.3, Vec3::new(0.0, -0.999, -0.045), grazing_out);
    same.extend(oblique_plate(
        2,
        0,
        0.32,
        0.42,
        grazing_in,
        Vec3::new(0.0, -0.999, 0.045),
    ));
    let same = walk(&corridor(2.0, same), &volumes);
    assert_eq!(same.runs.len(), 2, "same-facing micro-gaps are not one run");

    // The control: identical geometry and identical gap, faces OPPOSED. Now it welds, so the
    // rejection above is the face test and nothing else.
    let mut opposed = oblique_plate(1, 0, 0.2, 0.3, Vec3::new(0.0, -0.999, -0.045), grazing_out);
    opposed.extend(oblique_plate(
        2,
        0,
        0.32,
        0.42,
        Vec3::new(0.0, -0.999, -0.045),
        Vec3::new(0.0, 0.999, 0.045),
    ));
    let opposed = walk(&corridor(2.0, opposed), &volumes);
    assert_eq!(opposed.runs.len(), 1);
    assert_eq!(opposed.runs[0].joints, 1);
}

/// Deleting the weld LOOKAHEAD ceiling also left the suite green. At grazing incidence
/// `|axis · n| → 0` turns any along-ray distance into a small perpendicular one, so without a hard
/// bound the weld reaches arbitrarily far downrange. 60 mm along the ray is 1.2 mm perpendicular
/// here — inside the 2 mm tolerance, and rejected only by the 50 mm ceiling.
#[test]
fn a_long_grazing_gap_is_rejected_by_the_lookahead_ceiling() {
    let volumes = table(&[(1, 1000.0), (2, 1000.0)]);
    let out = Vec3::new(0.0, 0.9998, 0.02);
    let into = Vec3::new(0.0, -0.9998, -0.02);
    let pair = |gap: f32| {
        let mut hits = oblique_plate(1, 0, 0.2, 0.3, into, out);
        hits.extend(oblique_plate(2, 0, 0.3 + gap, 0.5, into, out));
        walk(&corridor(2.0, hits), &volumes)
    };
    // 60 mm along the ray: 1.2 mm perpendicular, so every other guard would let it through.
    assert_eq!(pair(0.06).runs.len(), 2, "60 mm of void is not a micro-gap");
    // 40 mm, same faces, same 0.8 mm perpendicular reading: inside the ceiling, so it welds.
    assert_eq!(pair(0.04).runs.len(), 1);
}

/// A single covered sample IS the disc mean, and re-normalizing it changes the low bits. The
/// axis-aligned fragment fixture could not see that — its normal is exactly `-Z`, which normalizes
/// to itself. Search for an oblique normal where `normalize` is genuinely not idempotent, then
/// assert the fragment degeneracy on THAT.
#[test]
fn the_fragment_degeneracy_survives_a_normal_that_normalization_would_move() {
    let laws = WalkLaws::default();
    let volumes = table(&[(1, 1000.0)]);
    // Search on the value that actually REACHES the walk: the fixture normalizes what it is given,
    // the walk's aggregation normalizes again, and the mutant normalizes a third time. Only the
    // last of those may move the bits, so the candidate has to be tested at that exact depth.
    let front = (1..200_000u32)
        .map(|step| Vec3::new(0.0, step as f32 * 3.7e-5, -1.0))
        .find(|raw| {
            let once = unit_or_zero(raw.normalize());
            unit_or_zero(once).to_array() != once.to_array()
        })
        .expect("some oblique f32 normal must survive normalization with different bits");

    let hits = oblique_plate(1, 0, 0.25, 0.375, front, -front);
    let single = walk(&corridor(2.0, hits.clone()), &volumes);
    let walked = walk_disc(&point_disc(2.0, sample(Vec3::ZERO, hits)), &volumes, &laws).unwrap();

    assert_eq!(walked.events.len(), 1);
    assert_eq!(
        walked.events[0].entry_normal.to_array(),
        single.runs[0].entry_normal.to_array(),
        "r → 0, k = 1 must be the single-ray walk to the bit, not to a tolerance"
    );
    assert_eq!(walked.events[0].cost.to_bits(), single.cost.to_bits());
}

/// Seam invisibility (§13.6) on the nastiest arrangement the review could build: three abutting
/// plates at non-binary coordinates, a lower-factor volume buried across two of the seams, entity
/// ids running BACKWARDS through the stack, and every rotation and reversal of the hit list. All of
/// it must be byte-identical to one thick slab.
///
/// The factors are 997 and 311 — prime, not powers of two — so no arithmetic here is exact by
/// accident. What makes it come out identical is structural: canonical spans are cut only where the
/// union maximum changes, so three plates of one substance are ONE span and the seams never enter
/// the arithmetic at all.
#[test]
fn three_abutting_plates_are_byte_identical_to_one_slab_in_every_order() {
    let volumes = table(&[(7, 997.0), (5, 997.0), (3, 997.0), (11, 311.0)]);
    let (a, b, c, d) = (0.137_f32, 0.291_f32, 0.447_8_f32, 0.601_3_f32);

    let one = walk(&corridor(2.0, plate(7, 0, a, d)), &volumes);

    let mut split = plate(7, 0, a, b);
    split.extend(plate(5, 0, b, c));
    split.extend(plate(3, 0, c, d));
    // A softer volume buried straight through two seams: more boundaries, same maximum.
    split.extend(plate(11, 0, 0.2, 0.5));

    for shift in 0..split.len() {
        let mut permuted = split.clone();
        permuted.rotate_left(shift);
        for reversed in [false, true] {
            if reversed {
                permuted.reverse();
            }
            let result = walk(&corridor(2.0, permuted.clone()), &volumes);
            assert_eq!(
                result.cost.to_bits(),
                one.cost.to_bits(),
                "shift {shift}, reversed {reversed}: {} vs {}",
                result.cost,
                one.cost
            );
            assert_eq!(result.spans, one.spans);
            assert_eq!(result.events, one.events);
        }
    }
}

/// ONE PLANE IS ONE EVENT, AT EVERY CALIBRE AND EVERY INCIDENCE.
///
/// The calibre/incidence cliff §13.5 anticipated, and codex MEASURED on 2026-08-07: with a fixed
/// 50 mm longitudinal ceiling, one planar surface split into THREE events for an 88 mm round at 80°
/// and for a 120 mm round at 75°. Nothing about the geometry changed between those and a head-on
/// hit; only the disc's spread along the ray did, and a constant cannot follow it.
///
/// Three events out of one plate is not a cosmetic defect. Each carries its own entrance read, so
/// the round is charged three entrance laws, offered three ricochet decisions and three overmatch
/// thicknesses, and throws spall three times, for a plate it crossed once.
///
/// The bound that holds is the disc's own reach, `2·r·tan(incidence)`: 0.50 m for the 88 at 80° and
/// 0.45 m for the 120 at 75°, both an order above the constant they were shattered by.
#[test]
fn one_plane_stays_one_event_across_the_calibre_incidence_cliff() {
    let volumes = table(&[(1, 1000.0)]);
    let laws = WalkLaws::default();
    // A whole disc laid on one plane: the axis plus the full ring, each meeting it where its own
    // ray does.
    let disc_on_one_plane = |radius: f32, incidence_deg: f32| {
        let incidence = incidence_deg.to_radians();
        let normal = Vec3::new(0.0, incidence.sin(), -incidence.cos());
        let frame = DiscFrame {
            u: Vec3::X,
            v: Vec3::Y,
        };
        // A 10 mm plate presents this much line of sight at that incidence. Thickness matters to
        // the defect, not just angle: a thick plate's chords still OVERLAP between adjacent ring
        // samples, so the fixed ceiling chained them anyway and the cliff hid. Ten millimetres is
        // where the chords part and the constant has to answer for itself — and it answered with
        // exactly the three events codex measured.
        let chord = 0.010 / incidence.cos();
        let slab = Slab::at(1, 0, 1.0, normal, chord);
        let samples = disc_offsets(&frame, radius, DEFAULT_RING)
            .into_iter()
            .map(|offset| sample(offset, slab.hits(offset, AXIS, 4.0, false)))
            .collect();
        DiscCorridor {
            anchor: Vec3::ZERO,
            origin: Vec3::ZERO,
            axis: AXIS,
            length: 4.0,
            radius,
            frame,
            samples,
        }
    };

    for (caliber, incidence_deg) in [(0.088f32, 80.0f32), (0.120, 75.0)] {
        let radius = caliber * 0.5;
        let corridor = disc_on_one_plane(radius, incidence_deg);
        // The spread the constant could not follow: an order of magnitude past 50 mm.
        let spread = 2.0 * radius * incidence_deg.to_radians().tan();
        assert!(
            spread > 0.4,
            "{caliber} m at {incidence_deg}° spreads {spread} m along the ray",
        );

        let walked = walk_disc(&corridor, &volumes, &laws).expect("one plane resolves");
        assert_eq!(
            walked.events.len(),
            1,
            "{caliber} m at {incidence_deg}°: one plane is one crossing, not {}",
            walked.events.len(),
        );
        assert_eq!(
            walked.events[0].coverage, 1.0,
            "and the whole disc engaged it",
        );
    }
}

/// THE SEED CONTRACT'S OTHER FLOAT BOUNDARY.
///
/// `admit` already handles a face a hair BEHIND a corridor origin: seeded, the seed owns it;
/// unseeded, the ray is sitting on it and it lands at `t = 0`. Nothing handled the same face a hair
/// in FRONT while seeded — and that is the same physical situation with the rounding falling the
/// other way.
///
/// It is reachable, and not rarely. The restart `t` is computed from the AGGREGATE entrance plane
/// while each span's boundary came from that sample's OWN ray, so the two agree only to within
/// rounding; at oblique incidence, where the ring's crossings spread along the ray, a live 72°
/// crossing put them 1.4e-8 apart. Read as an exact comparison that hair decided between "the seed
/// owns this entry" and "the corridor does", they both claimed it, and `UnexpectedEntry` stopped an
/// 88 dead on a 20 mm plate it should have gone straight through.
///
/// So "ON the boundary" is a tolerance here, the same [`coincident`] the rest of the module uses.
#[test]
fn a_boundary_at_the_restart_belongs_to_the_corridor_not_the_seed() {
    let volumes = table(&[(1, 1000.0)]);
    let laws = WalkLaws::default();
    let walked = walk(
        &RayCorridor {
            anchor: Vec3::ZERO,
            origin: Vec3::ZERO,
            axis: AXIS,
            length: 1.0,
            initial_presence: Vec::new(),
            hits: plate(1, 0, 0.20, 0.30),
        },
        &volumes,
    );
    let key = PrimitiveKey {
        volume: volume(1),
        primitive: prim(0),
    };
    // A hair either side of the boundary is the boundary. Well inside the topology tolerance, and
    // an order above the 1.4e-8 the live crossing produced.
    let hair = 1.0e-7;

    for t in [0.20, 0.20 + hair, 0.20 - hair] {
        assert!(
            walked.inside_at(t, &laws).is_empty(),
            "an entry AT the restart is the new corridor's to process, not the seed's (t = {t})",
        );
    }
    for t in [0.30, 0.30 + hair, 0.30 - hair, 0.25] {
        assert_eq!(
            walked.inside_at(t, &laws),
            vec![key],
            "an exit at or after the restart must be seeded, or it has nothing to pair with \
             (t = {t})",
        );
    }
    assert!(
        walked.inside_at(0.35, &laws).is_empty(),
        "and past the exit the ray is out",
    );
}

/// RING-ONLY CONTACT: the two OPPOSITE rim samples, and nothing between them.
///
/// Codex's production-layout probe, made permanent. An 88 at 80° meets a 10 mm plane, but its axis
/// and every intermediate sample thread an opening — only the two diametrically opposite rim samples
/// touch, 0.499 m apart along the ray. They are on ONE plane, so it is one crossing.
///
/// The full-disc cliff fixture cannot see this. There, the intermediate samples bridge the extremes
/// transitively in small steps, so a bound that is too tight for the diameter still yields one event
/// and the defect hides. Here there is exactly one pair, and it sits at the worst case a disc can
/// present — which is precisely where a bound cut to the worst case has no margin left. Codex
/// measured two events: DERIVED gap 0.441 m against a capped reach of 0.440 m, a valid contact
/// refused by a millimetre.
///
/// Under the residual relation the worst case is not a threshold at all. `−(d·n̄)/(axis·n̄)` predicts
/// 0.499 m for this pair, the runs are 0.499 m apart, and the residual is zero — the same answer it
/// gives for two samples a millimetre apart on the same plate.
#[test]
fn a_ring_only_contact_on_one_plane_is_one_event() {
    let volumes = table(&[(1, 1000.0)]);
    let laws = WalkLaws::default();
    let radius = 0.044f32;
    let incidence = 80.0_f32.to_radians();
    let normal = Vec3::new(0.0, incidence.sin(), -incidence.cos());
    let frame = DiscFrame {
        u: Vec3::X,
        v: Vec3::Y,
    };
    let slab = Slab::at(1, 0, 1.0, normal, 0.010 / incidence.cos());

    // Sample 0 is the axis; ring sample k is at angle `TAU·k/12`. Indices 4 and 10 are the ±v
    // extremes — diametrically opposite, and the pair the plane spreads furthest along the ray.
    let touching = [4usize, 10];
    let offsets = disc_offsets(&frame, radius, DEFAULT_RING);
    let samples: Vec<SampleCorridor> = offsets
        .iter()
        .enumerate()
        .map(|(index, &offset)| {
            if touching.contains(&index) {
                sample(offset, slab.hits(offset, AXIS, 4.0, false))
            } else {
                // Through the opening: this ray meets nothing at all.
                sample(offset, Vec::new())
            }
        })
        .collect();

    let separation = (offsets[touching[1]] - offsets[touching[0]]).length();
    assert!(
        (separation - 2.0 * radius).abs() < 1.0e-6,
        "the pair really is diametrically opposite: {separation} m apart",
    );
    let spread = 2.0 * radius * incidence.tan();
    assert!(
        (spread - 0.499).abs() < 1.0e-3,
        "and one plane spreads them {spread} m along the ray",
    );

    let walked = walk_disc(
        &DiscCorridor {
            anchor: Vec3::ZERO,
            origin: Vec3::ZERO,
            axis: AXIS,
            length: 4.0,
            radius,
            frame,
            samples,
        },
        &volumes,
        &laws,
    )
    .expect("one plane resolves");

    assert_eq!(
        walked.events.len(),
        1,
        "two rim samples on ONE plane are one crossing, {spread} m apart or not",
    );
    assert!(
        (walked.events[0].coverage - 2.0 / 13.0).abs() < 1.0e-6,
        "and it engaged exactly the two samples that touched: {}",
        walked.events[0].coverage,
    );
}

// -------------------------------------------------------------------------------------------
// §13.7 — several closed shells inside ONE primitive
// -------------------------------------------------------------------------------------------
//
// The road wheels are the standing precedent: bodies and axle authored as one MildSteel primitive.
// A ray through one meets `enter, enter, exit, exit`, and the §13.6 fuzzer measured 0.47% of a
// million rays failing closed on exactly that shape — the sixteen wheels, `Hull_Rear` and
// `Turret_Cupola`. These state what the three arrangements must resolve to.

/// A shell NESTED inside another, 3 mm in: one presence, one run, charged once.
///
/// This is the wheel. Presence is presence — §13.2 takes `max(factor)` over what is present, and a
/// primitive cannot be more present for being doubly so — which is the same answer the per-ENTITY
/// union already gives when two primitives of one volume overlap.
#[test]
fn a_shell_nested_inside_another_in_one_primitive_is_one_presence() {
    let volumes = table(&[(1, 1000.0)]);
    // Outer shell [0.20, 0.60); inner shell 3 mm inside it, entirely contained.
    let mut hits = plate(1, 0, 0.20, 0.60);
    hits.extend(plate(1, 0, 0.203, 0.597));
    let walked = walk(&corridor(4.0, hits), &volumes);

    assert_eq!(
        walked.runs.len(),
        1,
        "one primitive, one continuous presence"
    );
    assert_eq!(walked.presence.len(), 1, "and one entity presence, not two",);
    assert_eq!(
        walked.presence[0].spans,
        vec![(0.20, 0.60)],
        "the union of the shells — the outermost pair",
    );
    // Charged ONCE across the union, not twice across the overlap. (Not a byte assertion: these
    // distances are not binary-exact, so the tolerance is arithmetic noise, four orders below the
    // 394 reference-mm a double charge would have added.)
    assert!(
        (walked.cost - 0.40 * 1000.0).abs() < 1.0e-3,
        "{}",
        walked.cost
    );
}

/// Shells that OVERLAP without nesting — `enter, enter, exit, exit` with the second starting 92 mm
/// into the first — are still one presence, for the same reason.
#[test]
fn overlapping_shells_in_one_primitive_are_one_presence() {
    let volumes = table(&[(1, 1000.0)]);
    let mut hits = plate(1, 0, 0.20, 0.40);
    hits.extend(plate(1, 0, 0.292, 0.50));
    let walked = walk(&corridor(4.0, hits), &volumes);

    assert_eq!(walked.runs.len(), 1);
    assert_eq!(walked.presence[0].spans, vec![(0.20, 0.50)]);
    // The 108 mm the two shells share is charged once. Summing would give 0.30 + 0.108.
    assert!(
        (walked.cost - 0.30 * 1000.0).abs() < 1.0e-3,
        "{}",
        walked.cost
    );
}

/// DISJOINT shells 92 mm apart in one primitive are TWO crossings, because that is what the
/// association law says — 92 mm of air is not a weld, whoever authored the two shells.
///
/// Legalizing multi-shell primitives is about the PAIRING, and it must not become a licence to merge:
/// two events here, two entrance reads, two exits.
#[test]
fn disjoint_shells_in_one_primitive_are_still_two_crossings() {
    let volumes = table(&[(1, 1000.0)]);
    let laws = WalkLaws::default();
    let mut hits = plate(1, 0, 0.20, 0.30);
    hits.extend(plate(1, 0, 0.392, 0.50));
    assert!(
        0.092 > laws.weld_max_lookahead,
        "the gap is past the lookahead, so no weld can even be looked for",
    );
    let walked = walk(&corridor(4.0, hits), &volumes);

    assert_eq!(walked.runs.len(), 2, "92 mm of air is two crossings");
    assert_eq!(
        walked.presence[0].spans,
        vec![(0.20, 0.30), (0.392, 0.50)],
        "two separate presences of the one primitive",
    );
    assert!(
        (walked.cost - (0.10 + 0.108) * 1000.0).abs() < 1.0e-3,
        "{}",
        walked.cost
    );
}

/// §13.6 IDEMPOTENCE, on the shape that made depth counting necessary. A duplicated shell coincides
/// face-for-face with the original, so the topology reduction collapses it before the field ever
/// sees it: depth two and depth one are the same presence, and the walk is bit-identical.
#[test]
fn a_duplicated_shell_in_one_primitive_changes_nothing() {
    let volumes = table(&[(1, 1000.0)]);
    let once = walk(&corridor(4.0, plate(1, 0, 0.25, 0.75)), &volumes);
    let mut doubled = plate(1, 0, 0.25, 0.75);
    doubled.extend(plate(1, 0, 0.25, 0.75));
    let twice = walk(&corridor(4.0, doubled), &volumes);

    assert_eq!(
        once.cost.to_bits(),
        twice.cost.to_bits(),
        "bit-identical cost"
    );
    assert_eq!(once.spans, twice.spans);
    assert_eq!(once.presence, twice.presence);
    assert_eq!(once.primitives, twice.primitives);
}

/// A SHELL THINNER THAN THE WINDOW IS CHARGED, NOT ERASED.
///
/// The manifold gate asks a ballistic volume for closure and positive signed volume. It does NOT
/// ask for a minimum thickness, so a closed, outward-wound `1 m × 1 m × 0.953674 µm` plate is legal
/// geometry — and a head-on ray meets its two faces eight f32 ULP apart, inside the 1.4 µm topology
/// window at that distance.
///
/// Reduced by tolerance alone, that pair is one boundary and the primitive reads entry-AND-exit: a
/// touch, toggling nothing. The walk then returns success having charged nothing — no cost, no run,
/// no presence, no event — for 0.0009536743 reference-metres of steel it certainly crossed. That is
/// the same free-penetration class as the bridged plate, reached from inside one primitive instead
/// of through a third party.
///
/// Only bit equality may collapse the pair, because only bit equality is a statement about the
/// geometry rather than about the tolerance that grouped it.
#[test]
fn a_shell_thinner_than_the_window_is_charged_not_erased() {
    let volumes = table(&[(1, 1000.0)]);
    let laws = WalkLaws::default();
    let (enter, exit) = (1.0f32, ulps(1.0, 8));
    assert!(
        coincident(enter, exit, &laws),
        "the shell must be INSIDE one window, or there is no trap to spring",
    );

    let walked = walk_ray(0, &corridor(2.0, plate(1, 1, enter, exit)), &volumes, &laws)
        .expect("a thin shell resolves");

    let truth = 1000.0 * (exit - enter) as f64;
    assert!(
        walked.cost as f64 >= truth * (1.0 - 1.0e-6),
        "charged {} against an exact {truth}",
        walked.cost,
    );
    assert_eq!(
        walked.runs.len(),
        1,
        "the shell is one crossing: {walked:#?}"
    );
    assert_eq!(
        walked.runs[0].entry_volume,
        volume(1),
        "a charged run names the volume that charged it",
    );
    assert_eq!(
        walked.presence.iter().map(|p| p.entity).collect::<Vec<_>>(),
        vec![volume(1)],
        "the shell is present: {walked:#?}",
    );
    assert!(
        walked.presence[0].chord >= exit - enter,
        "chord {} lost material",
        walked.presence[0].chord,
    );
    assert!(!walked.events.is_empty(), "a crossing is an event");
}

/// A CORNER IS ONE CLUSTER, HOWEVER MANY FACES MEET AT IT.
///
/// The faces meeting at one geometric point do not share a `t`: each comes from its own triangle's
/// plane, so the four surfaces around a corner between two abutting shells arrive spread over
/// several ULP. MEASURED on the bound Tiger (seed 7, ray 3412876, two shards of `Wheel_R_0`): four
/// faces at 8.5646715 / 8.5646725 / 8.5646753 / 8.5646763, a spread of 4.8 µm against a 4.4 µm
/// window at that distance.
///
/// Anchored on the first face, the window ends mid-corner: the fourth face starts a new cluster,
/// one shard's exit is reduced alone, and it faces a depth that has not opened. Chained, the corner
/// is one batch and both shards read entry-AND-exit.
///
/// What the corner is NOT is free. The brushed shard's two faces are 4 µm apart, not bit-equal, so
/// the reduction cannot tell a corner from a 4 µm plate and charges it as one (`Toggle::Graze`) —
/// microns of steel it may not have crossed, which is the direction the doctrine allows. The claim
/// the test still holds is the one clustering exists for: ONE contact, whatever the corner's face
/// count, and no error.
#[test]
fn a_corner_graze_spread_wider_than_the_window_is_one_contact() {
    let volumes = table(&[(1, 1000.0), (2, 800.0)]);
    let (near, far) = (Vec3::new(0.1, 0.0, -1.0), Vec3::new(-0.1, 0.0, -1.0));
    let mut hits = Vec::new();
    // The shard the ray genuinely crosses, from well before the corner to well after it.
    hits.extend(oblique_plate(1, 1, 8.371_457, 8.754_433, near, -near));
    // Its own second shell, opening and closing INSIDE the corner.
    hits.extend(oblique_plate(1, 1, 8.5646715, 8.564_675, near, -far));
    // The abutting shard, which the ray only touches: it leaves at one corner face and re-enters at
    // the next, and is never inside between them.
    hits.extend(oblique_plate(2, 2, 8.564_676, 8.564_672, far, -near));

    let walked = walk_ray(0, &corridor(14.0, hits), &volumes, &WalkLaws::default())
        .expect("a corner graze resolves");
    assert_eq!(
        walked.runs.len(),
        1,
        "one crossing, not one per corner face: {:#?}",
        walked.runs
    );

    // The shard the ray really crosses is charged in full …
    let crossed = walked
        .presence
        .iter()
        .find(|presence| presence.entity == volume(1))
        .expect("the crossed shard is present");
    assert!(
        crossed.chord >= 8.754_433 - 8.371_457,
        "chord {} lost the plate itself",
        crossed.chord,
    );
    // … and the brushed one for the corner's own width, at most, and never for less than the 4 µm
    // its own two faces bracket.
    let brushed = walked
        .presence
        .iter()
        .find(|presence| presence.entity == volume(2))
        .expect("the brushed shard is charged, not erased");
    let corner = 8.564_676 - 8.5646715;
    assert!(
        (8.564_676 - 8.564_672..=corner).contains(&brushed.chord),
        "chord {} is not the corner's own width",
        brushed.chord,
    );
    assert!(
        walked.cost <= 1000.0 * (8.754_433 - 8.371_457) + 800.0 * corner,
        "cost {} charged more than the crossing plus the whole corner",
        walked.cost,
    );
}

// ---------------------------------------------------------------------------------------------
// Coincidence clustering: what a cluster may and may not swallow
// ---------------------------------------------------------------------------------------------

/// `t` moved by `steps` f32 ULP — the finest perturbation a corridor can express at that distance,
/// and the scale the topology window is written against.
fn ulps(t: f32, steps: i32) -> f32 {
    f32::from_bits((t.to_bits() as i32 + steps) as u32)
}

/// A FACE BETWEEN A PLATE'S TWO FACES MUST NOT ERASE THE PLATE.
///
/// The plate is 20 ULP thick at `t = 1 m` — 2.4 µm against a 1.4 µm window, so its entry and its
/// exit are NOT one boundary and the crossing is real. A single tangent face at the midpoint is
/// within the window of each of them, and nothing else.
///
/// If coincidence is transitive, that one face merges the entry with the exit, the plate reduces to
/// a zero-measure touch, and the walk returns success having charged nothing: no cost, no run, no
/// presence, no event. That is the free-penetration class the whole module exists to refuse.
#[test]
fn a_bridging_face_cannot_erase_the_plate_it_sits_inside() {
    let volumes = table(&[(1, 1000.0), (2, 800.0)]);
    let laws = WalkLaws::default();
    let (enter, exit) = (1.0f32, ulps(1.0, 20));
    let mid = ulps(1.0, 10);
    assert!(
        !coincident(enter, exit, &laws),
        "the plate must be thicker than one window, or there is nothing to preserve",
    );
    assert!(
        coincident(enter, mid, &laws) && coincident(mid, exit, &laws),
        "the bridging face must be within the window of BOTH plate faces",
    );

    let mut hits = plate(1, 1, enter, exit);
    hits.push(FaceHit {
        volume: volume(2),
        primitive: prim(2),
        triangle: 7,
        t: mid,
        true_normal: Vec3::X,
    });
    let walked = walk_ray(0, &corridor(2.0, hits), &volumes, &laws).expect("the plate resolves");

    assert_eq!(
        walked.runs.len(),
        1,
        "the plate is one crossing: {walked:#?}"
    );
    assert_eq!(
        walked.presence.iter().map(|p| p.entity).collect::<Vec<_>>(),
        vec![volume(1)],
        "the plate is present",
    );
    assert_eq!(
        walked.presence[0].chord,
        exit - enter,
        "the whole 20 ULP is charged",
    );
    assert!(
        walked.cost >= 1000.0 * (exit - enter),
        "cost {} lost material",
        walked.cost,
    );
}

/// THE SAME, WITH EVERY FACE PAIRED — nothing here is a graze, a tangent or a stray triangle.
///
/// A high-factor plate 20 ULP thick, and a low-factor solid whose ENTRY happens to land between its
/// two faces. Transitive coincidence merges the plate's entry and exit through that entry face; the
/// plate then toggles nothing and vanishes from the walk entirely, while the soft volume around it
/// is charged in full. The round reads 800 where it should read 1000.
#[test]
fn a_bridging_entry_cannot_erase_a_high_factor_plate() {
    let volumes = table(&[(1, 1000.0), (2, 800.0)]);
    let laws = WalkLaws::default();
    let (enter, exit) = (1.0f32, ulps(1.0, 20));
    let mut hits = plate(1, 1, enter, exit);
    hits.extend(plate(2, 2, ulps(1.0, 10), 1.5));
    let walked = walk_ray(0, &corridor(3.0, hits), &volumes, &laws).expect("both solids resolve");

    let mut present: Vec<Entity> = walked.presence.iter().map(|p| p.entity).collect();
    present.sort();
    let mut expected = vec![volume(1), volume(2)];
    expected.sort();
    assert_eq!(
        present, expected,
        "the plate is a primitive of its own and must be reported: {walked:#?}",
    );
    let plate = walked
        .presence
        .iter()
        .find(|presence| presence.entity == volume(1))
        .expect("the plate is present");
    assert_eq!(plate.chord, exit - enter, "the plate's own chord");
    // 800 over the soft volume, and the plate's 20 ULP charged at 1000 rather than at 800.
    assert!(
        walked.cost > 800.0 * (1.5 - enter),
        "cost {} charged the plate at the soft factor or not at all",
        walked.cost,
    );
}

/// A CHAIN OF BRIDGING FACES CANNOT REACH ACROSS AN AIR GAP.
///
/// Coincidence chains, so a face every window links the next: forty tangent faces a fraction of a
/// window apart cover twenty-seven windows of ray. Unbounded, that chain is one cluster, and the
/// plate that ends where it starts and the plate that begins where it ends are one boundary — the
/// air between them, and both plates' own faces, reduced to a single event.
///
/// `topology_cluster_windows` is the ceiling that stops it. The two plates keep their own boundaries
/// and the gap survives as air.
#[test]
fn a_chain_of_bridging_faces_cannot_span_more_than_the_ceiling() {
    let volumes = table(&[(1, 1000.0), (2, 800.0), (3, 600.0)]);
    let laws = WalkLaws::default();
    let (exit, entry) = (1.0f32, ulps(1.0, 320));
    let mut hits = plate(1, 1, 0.5, exit);
    hits.extend(plate(2, 2, entry, 1.5));
    for step in 0..40 {
        let t = ulps(exit, step * 8);
        assert!(
            coincident(ulps(exit, step * 8 - 8), t, &laws),
            "each link must be within a window of the last, or the chain is not a chain",
        );
        hits.push(FaceHit {
            volume: volume(3),
            primitive: prim(3),
            triangle: step as u32,
            t,
            true_normal: Vec3::X,
        });
    }
    let walked = walk_ray(0, &corridor(3.0, hits), &volumes, &laws).expect("both plates resolve");

    let air = walked
        .spans
        .iter()
        .find(|span| span.factor == 0.0 && span.start > 0.5 && span.end < 1.5)
        .unwrap_or_else(|| panic!("the gap between the plates was swallowed: {walked:#?}"));
    assert!(
        air.end - air.start > 0.5 * (entry - exit),
        "most of the gap must survive: {air:?}",
    );
    assert_eq!(
        walked.presence.len(),
        2,
        "two plates, whatever bridges them: {walked:#?}",
    );
}

/// AN EXIT CLOSES AT THE CLUSTER'S LAST FACE, NOT AT ITS FIRST.
///
/// The direction is the whole reason a cluster cannot erase cost: whatever a cluster's width, the
/// material inside it is charged at the larger of the factors either side. Collapsing an exit onto
/// the cluster's anchor instead would decline to charge everything between the two.
#[test]
fn a_spread_exit_closes_at_the_far_face() {
    let volumes = table(&[(1, 1000.0)]);
    let far = ulps(1.0, 3);
    let mut hits = plate(1, 1, 0.5, 1.0);
    // The same face's second triangle, three ULP downrange — one boundary, two crossings.
    hits.push(FaceHit {
        volume: volume(1),
        primitive: prim(1),
        triangle: 77,
        t: far,
        true_normal: AXIS,
    });
    let walked = walk(&corridor(2.0, hits), &volumes);

    assert_eq!(walked.presence[0].spans, vec![(0.5, far)]);
    assert_eq!(
        walked.runs.len(),
        1,
        "one boundary, one run: {:#?}",
        walked.runs
    );
}

/// A CLUSTER THAT LEAVES A PRIMITIVE OPEN MUST LEAVE IT OPEN.
///
/// §13.7 legalizes several disconnected closed shells inside one primitive, so a cluster may hold
/// shell A's entry, shell A's exit and shell B's entry — `E X E`. Reduced to the pair
/// `(has_entry, has_exit)` that is indistinguishable from a point graze: the walk charges the
/// cluster's own 0.00047683716 reference-metres, reports air from there to the corridor's end, and
/// returns `Ok` having declined ~499.9995 of the 500 reference-metres shell B bounds.
///
/// The signed net occupancy is what separates them. `+1 − 1 + 1 = +1`: the primitive is OPEN after
/// the cluster, so the corridor ends inside it, and a corridor that ends inside material is
/// [`WalkError::IncompleteCorridor`] — the fail-closed answer, not a silent one.
#[test]
fn a_re_entrant_shell_in_one_cluster_stays_open() {
    let volumes = table(&[(1, 1000.0)]);
    let laws = WalkLaws::default();
    // Shell A is two ULP thick; shell B opens two ULP past its exit and closes past the corridor.
    let (a_enter, a_exit, b_enter) = (1.0f32, ulps(1.0, 2), ulps(1.0, 4));
    assert!(
        coincident(a_enter, a_exit, &laws) && coincident(a_exit, b_enter, &laws),
        "the three faces must chain into ONE cluster, or there is no reduction to defeat",
    );
    let mut hits = plate(1, 1, a_enter, a_exit);
    hits.extend(plate(1, 1, b_enter, 2.0));

    let error = walk_ray(0, &corridor(1.5, hits), &volumes, &laws)
        .expect_err("the corridor ends inside shell B");
    let WalkError::IncompleteCorridor { open, .. } = error else {
        panic!("a primitive left open must fail closed, not resolve: {error:?}");
    };
    assert_eq!(
        open,
        vec![PrimitiveKey {
            volume: volume(1),
            primitive: prim(1),
        }],
        "and the open primitive is named",
    );
}

/// THE SAME CLUSTER, CLOSED — `E X E X` inside one cluster is net zero and no more.
///
/// Two complete grazes of one primitive: whatever the reduction charges for the cluster's own
/// width, the field is shut again on its far side, and a corridor that continues past it is not
/// left holding an open shell.
#[test]
fn two_complete_grazes_in_one_cluster_are_net_zero() {
    let volumes = table(&[(1, 1000.0)]);
    let laws = WalkLaws::default();
    let mut hits = plate(1, 1, 1.0, ulps(1.0, 2));
    hits.extend(plate(1, 1, ulps(1.0, 4), ulps(1.0, 6)));
    let walked =
        walk_ray(0, &corridor(1.5, hits), &volumes, &laws).expect("net zero closes the field");

    assert!(
        walked.spans.last().is_some_and(|span| span.factor == 0.0),
        "the field is shut past the cluster: {:#?}",
        walked.spans,
    );
    // Never LESS than the material the two shells bound, and never more than the cluster's width.
    let bound = 1000.0 * (ulps(1.0, 6) - 1.0);
    assert!(
        (1000.0 * (ulps(1.0, 2) - 1.0)..=bound).contains(&walked.cost),
        "cost {} is not the cluster's own width",
        walked.cost,
    );
}

/// TWO TRIANGLES CLAIMING ONE CROSSING ARE ONE CROSSING.
///
/// `collect::cross_triangle` deliberately lets BOTH triangles incident on a shared edge claim a ray
/// that runs through it — a duplicate is recoverable, a dropped crossing is not — and the two
/// compute `t` from their own planes, so the duplicate arrives a few ULP off, not bit-equal. Raw
/// multiplicity is therefore not surface multiplicity, and a net that counted it would read `+2`
/// for one entry and leave the primitive open for ever.
///
/// The reduction is over the SIGN SEQUENCE in `t` order: consecutive same-sign claims of one
/// primitive inside one cluster are one claim. `E E` is `+1`; the `E X E` above is still `+1`.
#[test]
fn duplicate_claims_of_one_crossing_are_one_entry() {
    let volumes = table(&[(1, 1000.0)]);
    let laws = WalkLaws::default();
    let mut hits = plate(1, 1, 1.0, 1.5);
    // The second incident triangle of the entry face, two ULP downrange.
    hits.push(FaceHit {
        volume: volume(1),
        primitive: prim(1),
        triangle: 77,
        t: ulps(1.0, 2),
        true_normal: -AXIS,
    });
    let walked = walk_ray(0, &corridor(2.0, hits), &volumes, &laws)
        .expect("a duplicated entry claim is one entry");

    assert_eq!(walked.runs.len(), 1, "one crossing: {:#?}", walked.runs);
    assert_eq!(walked.presence[0].spans, vec![(1.0, 1.5)]);
}

/// FACES AT ONE `t` HAVE NO ORDER, SO THEY MAY NOT DECIDE THE NET.
///
/// The corner of a box, as the shared-vertex fan really reports it (`collect`'s sweep, one origin in
/// 4 225): the near half of the top face claims an entry, and one ULP downrange the far half claims
/// the SAME entry while a side face claims the exit — two faces at one bit-equal `t`. The corridor's
/// total order breaks that tie on triangle index, so reading the pair in index order makes the net
/// `+1` for one triangulation and `0` for the other: the same corner is a brush or a round stopped
/// dead depending on how the exporter numbered two triangles.
#[test]
fn a_tie_at_one_t_reads_the_same_in_either_triangle_order() {
    let volumes = table(&[(1, 1000.0)]);
    let laws = WalkLaws::default();
    let (near, far) = (1.0f32, ulps(1.0, 2));
    let corner = |exit_first: bool| {
        let (a, b) = if exit_first { (7u32, 9u32) } else { (9, 7) };
        vec![
            FaceHit {
                volume: volume(1),
                primitive: prim(1),
                triangle: 5,
                t: near,
                true_normal: -AXIS,
            },
            FaceHit {
                volume: volume(1),
                primitive: prim(1),
                triangle: a,
                t: far,
                true_normal: AXIS,
            },
            FaceHit {
                volume: volume(1),
                primitive: prim(1),
                triangle: b,
                t: far,
                true_normal: -AXIS,
            },
        ]
    };
    let exit_first = walk_ray(0, &corridor(2.0, corner(true)), &volumes, &laws)
        .expect("the corner resolves in either order");
    let entry_first = walk_ray(0, &corridor(2.0, corner(false)), &volumes, &laws)
        .expect("the corner resolves in either order");

    assert_eq!(exit_first.spans, entry_first.spans, "the field is the same");
    assert_eq!(exit_first.cost.to_bits(), entry_first.cost.to_bits());
    assert!(
        exit_first
            .spans
            .last()
            .is_some_and(|span| span.factor == 0.0),
        "a corner brush closes the field: {:#?}",
        exit_first.spans,
    );
}

/// CLUSTERING NEVER CHARGES LESS THAN THE EXACT UNION FIELD.
///
/// Four hundred randomised fields of six overlapping plates, drawn onto a lattice of hot spots a
/// few ULP wide so their boundaries pile up inside the topology window and the reduction has plenty
/// to collapse.
///
/// TWO FAMILIES, because the reduction has two ways to lose material. The first two hundred cases
/// are centimetres thick — four orders above the window — so no plate is a graze and the risk is a
/// cluster swallowing a boundary. The second two hundred are one to eight ULP thick, entirely
/// INSIDE the window, so every plate is a pairwise-coincident pair and the whole charge rests on
/// the graze rule.
///
/// The reference is `∫ max(factor) dt` over the intervals as authored. The walk's own cost must
/// DOMINATE it: a cluster may charge the wider of the factors either side of it across its own
/// width, and may never charge the narrower.
#[test]
fn clustering_never_charges_less_than_the_exact_union_field() {
    let factors = [1000.0f32, 800.0, 600.0, 1200.0];
    let volumes = table(&[
        (1, factors[0]),
        (2, factors[1]),
        (3, factors[2]),
        (4, factors[3]),
    ]);
    let mut state = 0x2026_0809_u64;
    let mut rng = move || {
        state = state
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        (state >> 33) as u32
    };

    for case in 0..400 {
        // The second family: every plate thinner than the window it lives in.
        let sub_window = case >= 200;
        let mut hits = Vec::new();
        let mut authored: Vec<(f32, f32, f32)> = Vec::new();
        for p in 0..6u32 {
            let vol = 1 + p % 4;
            let enter = ulps(1.0 + (rng() % 5) as f32 * 2.0e-5, (rng() % 24) as i32 - 12);
            let exit = if sub_window {
                ulps(enter, 1 + (rng() % 8) as i32)
            } else {
                ulps(1.05 + (rng() % 5) as f32 * 2.0e-5, (rng() % 24) as i32 - 12)
            };
            if sub_window {
                assert!(
                    coincident(enter, exit, &WalkLaws::default()),
                    "case {case}: the family is only a test of the graze rule if the pair is one \
                     window wide",
                );
            }
            hits.extend(plate(vol, p, enter, exit));
            authored.push((enter, exit, factors[(vol - 1) as usize]));
        }
        let walked = walk_ray(0, &corridor(2.0, hits), &volumes, &WalkLaws::default())
            .unwrap_or_else(|error| panic!("case {case}: {error:?}"));

        let mut edges: Vec<f32> = authored.iter().flat_map(|(a, b, _)| [*a, *b]).collect();
        edges.sort_by(f32::total_cmp);
        edges.dedup();
        let mut truth = 0.0f64;
        for pair in edges.windows(2) {
            let (lo, hi) = (pair[0] as f64, pair[1] as f64);
            let mid = 0.5 * (lo + hi);
            let factor = authored
                .iter()
                .filter(|(a, b, _)| (*a as f64) <= mid && mid < (*b as f64))
                .fold(0.0f32, |max, (_, _, factor)| max.max(*factor));
            truth += factor as f64 * (hi - lo);
        }

        assert!(
            walked.cost as f64 >= truth * (1.0 - 1.0e-6),
            "case {case}: charged {} against an authored {truth}",
            walked.cost,
        );
        // And the other side of it: what a cluster's width can over-charge is the cluster's width,
        // which is microns.
        assert!(
            walked.cost as f64 <= truth + 0.1,
            "case {case}: charged {} against an authored {truth}",
            walked.cost,
        );
    }
}
