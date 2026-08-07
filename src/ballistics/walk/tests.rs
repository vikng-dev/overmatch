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
            primitive,
            triangle: primitive * 100,
            t: enter,
            true_normal: entry_normal.normalize(),
        },
        FaceHit {
            volume: volume(vol),
            primitive,
            triangle: primitive * 100 + 1,
            t: exit,
            true_normal: exit_normal.normalize(),
        },
    ]
}

fn corridor(length: f32, hits: Vec<FaceHit>) -> RayCorridor {
    RayCorridor {
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
            primitive: 0,
            triangle: 0,
            t: 1.0,
            true_normal: -AXIS,
        },
        FaceHit {
            volume: volume(1),
            primitive: 0,
            triangle: 1,
            t: 1.0,
            true_normal: -AXIS,
        },
        FaceHit {
            volume: volume(1),
            primitive: 0,
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
            primitive: 0,
            triangle: index as u32,
            t: 1.0,
            true_normal: normal.normalize(),
        });
    }
    hits.push(FaceHit {
        volume: volume(1),
        primitive: 0,
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
            primitive: 0,
            triangle: 0,
            t: 1.0,
            true_normal: -AXIS,
        },
        FaceHit {
            volume: volume(1),
            primitive: 0,
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
        primitive: 0,
    };
    let mut corridor = corridor(
        4.0,
        vec![FaceHit {
            volume: volume(1),
            primitive: 0,
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
            primitive: 0,
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

/// A second entry into a primitive already open means the mesh is not the closed positively-oriented
/// shell the bake gate promises.
#[test]
fn a_second_entry_into_an_open_primitive_is_a_structured_error() {
    let volumes = table(&[(1, 1000.0)]);
    let hits = vec![
        FaceHit {
            volume: volume(1),
            primitive: 0,
            triangle: 0,
            t: 0.25,
            true_normal: -AXIS,
        },
        FaceHit {
            volume: volume(1),
            primitive: 0,
            triangle: 1,
            t: 0.5,
            true_normal: -AXIS,
        },
        FaceHit {
            volume: volume(1),
            primitive: 0,
            triangle: 2,
            t: 1.0,
            true_normal: AXIS,
        },
    ];
    assert!(matches!(
        walk_ray(1, &corridor(4.0, hits), &volumes, &WalkLaws::default()),
        Err(WalkError::UnexpectedEntry { sample: 1, .. })
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
            primitive: 0,
            triangle: 0,
            t: 0.125,
            true_normal: AXIS,
        }],
    );
    corridor.initial_presence = vec![PrimitiveKey {
        volume: volume(1),
        primitive: 0,
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
// §13.3 / §3 — the staged resolution
// ---------------------------------------------------------------------------------------------

fn disc_of(hits: Vec<FaceHit>, length: f32) -> DiscCorridor {
    let frame = DiscFrame::from_axis_and_reference(AXIS, Vec3::Y).unwrap();
    let samples = disc_offsets(&frame, 0.044, DEFAULT_RING)
        .into_iter()
        .map(|offset| sample(offset, hits.clone()))
        .collect();
    DiscCorridor {
        origin: Vec3::ZERO,
        axis: AXIS,
        length,
        radius: 0.044,
        frame,
        samples,
    }
}

/// §13.3's motivating pathology: an exposed forearm is ~80 mm "thick", so a GEOMETRIC overmatch test
/// leaves it un-overmatched and past 70° an arm ricochets an 88. Factor-weighted, 80 mm of flesh is
/// ~1.6 mm steel-equivalent — overmatched by everything, deflecting nothing.
#[test]
fn an_arm_does_not_ricochet_an_88() {
    let laws = WalkLaws::default();
    let shot = Shot {
        caliber: 0.088,
        capability: 250.0,
    };
    // ~76° incidence — well past the ricochet threshold. The forearm's chord is 80 mm, so the
    // GEOMETRIC read is a fat plate; factor-weighted it is 80 mm × 10 ÷ 1000 × cos76° ≈ 0.19 mm.
    let normal = Vec3::new(0.0, 0.97, -0.242).normalize();
    let flesh = table(&[(1, 10.0)]);
    let arm = disc_of(slab(1, 0, 0.5, 0.58, normal), 2.0);
    let walked = walk_disc(&arm, &flesh, &laws).unwrap();
    match begin(&walked, &shot, &laws) {
        Begin::Transit(request) => {
            assert!(request.entrance.overmatched);
            assert!(request.entrance.incidence > laws.ricochet_angle);
        }
        other => panic!("an arm must not deflect an 88: {other:?}"),
    }

    // Real armour at the same obliquity DOES deflect it — the law still works where it should.
    let steel = table(&[(1, 1000.0)]);
    let plate = disc_of(slab(1, 0, 0.5, 0.9, normal), 2.0);
    let walked = walk_disc(&plate, &steel, &laws).unwrap();
    match begin(&walked, &shot, &laws) {
        Begin::Ricochet { speed_scale, .. } => {
            assert!((speed_scale - laws.ricochet_bleed).abs() < 1.0e-5)
        }
        other => panic!("expected a ricochet off steel, got {other:?}"),
    }
}

/// A graze is a PARTIAL ricochet (§13.5): the same oblique steel met at half coverage bleeds half as
/// much speed. Continuous, no cliff, no lottery.
#[test]
fn a_partial_graze_bleeds_proportionally_to_coverage() {
    let laws = WalkLaws::default();
    let shot = Shot {
        caliber: 0.088,
        capability: 250.0,
    };
    let steel = table(&[(1, 1000.0)]);
    let normal = Vec3::new(0.0, 0.97, -0.242).normalize();
    let frame = DiscFrame::from_axis_and_reference(AXIS, Vec3::Y).unwrap();
    let offsets = disc_offsets(&frame, 0.044, DEFAULT_RING);
    let k = offsets.len();
    let samples: Vec<SampleCorridor> = offsets
        .iter()
        .enumerate()
        .map(|(index, offset)| {
            sample(
                *offset,
                if index % 2 == 0 {
                    slab(1, 0, 0.5, 0.9, normal)
                } else {
                    Vec::new()
                },
            )
        })
        .collect();
    let covered = samples.iter().filter(|s| !s.hits.is_empty()).count() as f32 / k as f32;
    let walked = walk_disc(
        &DiscCorridor {
            origin: Vec3::ZERO,
            axis: AXIS,
            length: 2.0,
            radius: 0.044,
            frame,
            samples,
        },
        &steel,
        &laws,
    )
    .unwrap();
    match begin(&walked, &shot, &laws) {
        Begin::Ricochet { speed_scale, .. } => {
            let expected = 1.0 - covered * (1.0 - laws.ricochet_bleed);
            assert!((speed_scale - expected).abs() < 1.0e-5, "{speed_scale}");
            assert!(speed_scale > laws.ricochet_bleed, "a graze bleeds less");
        }
        other => panic!("expected a partial ricochet, got {other:?}"),
    }
}

/// Capability exhausted partway through a factor CHANGE: the embed point is the inverse of the
/// prefix integral, not `span × cap/cost`, and per-entity damage is CLIPPED at that progress — a
/// round that dies in the plate deposits nothing in the crewman behind it.
#[test]
fn capability_exhausted_midway_inverts_the_prefix_integral_and_clips_damage() {
    let laws = WalkLaws::default();
    let volumes = table(&[(1, 1000.0), (2, 10.0)]);
    // 100 mm of RHA, then a crewman.
    let mut hits = plate(1, 0, 0.5, 0.6);
    hits.extend(plate(2, 0, 0.6, 0.9));
    let disc = disc_of(hits, 2.0);
    let walked = walk_disc(&disc, &volumes, &laws).unwrap();
    // 60 reference-mm: dies 60 % of the way through the plate.
    let shot = Shot {
        caliber: 0.088,
        capability: 60.0,
    };
    let request = match begin(&walked, &shot, &laws) {
        Begin::Transit(request) => request,
        other => panic!("expected Transit, got {other:?}"),
    };
    let plan = finish(&walked, &request, &shot, &laws).unwrap();
    match plan.outcome {
        Outcome::Embedded { t, .. } => assert!((t - 0.56).abs() < 1.0e-3, "{t}"),
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
    let mut hits = plate(1, 0, 0.5, 0.6);
    hits.extend(plate(2, 0, 0.6, 0.9));
    let disc = disc_of(hits, 2.0);
    let walked = walk_disc(&disc, &volumes, &laws).unwrap();
    let shot = Shot {
        caliber: 0.088,
        capability: 250.0,
    };
    let request = match begin(&walked, &shot, &laws) {
        Begin::Transit(request) => request,
        other => panic!("expected Transit, got {other:?}"),
    };
    let plan = finish(&walked, &request, &shot, &laws).unwrap();
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
/// present its slope to a round that dwarfs it).
#[test]
fn overmatch_charges_the_perpendicular_projection() {
    let laws = WalkLaws::default();
    let volumes = table(&[(1, 1000.0)]);
    // 15 mm plate at 60°: a 30 mm chord, but 15 mm perpendicular, and 88 mm overmatches it.
    let normal = Vec3::new(0.0, 0.866, -0.5).normalize();
    let disc = disc_of(slab(1, 0, 0.5, 0.53, normal), 2.0);
    let walked = walk_disc(&disc, &volumes, &laws).unwrap();
    let shot = Shot {
        caliber: 0.088,
        capability: 250.0,
    };
    let request = match begin(&walked, &shot, &laws) {
        Begin::Transit(request) => request,
        other => panic!("expected Transit, got {other:?}"),
    };
    assert!(request.entrance.overmatched);
    let plan = finish(&walked, &request, &shot, &laws).unwrap();
    // 30 mm chord × 1000 = 30 reference-mm oblique; the perpendicular projection is half of it.
    assert!((plan.cost_spent - 15.0).abs() < 0.2, "{}", plan.cost_spent);
}

/// An empty disc decides nothing — no fabricated entrance, no fabricated terminal (§13.6's "no
/// fabricated events").
#[test]
fn an_empty_disc_is_a_miss() {
    let laws = WalkLaws::default();
    let volumes = table(&[(1, 1000.0)]);
    let walked = walk_disc(&disc_of(Vec::new(), 2.0), &volumes, &laws).unwrap();
    assert!(walked.events.is_empty());
    assert_eq!(
        begin(
            &walked,
            &Shot {
                caliber: 0.088,
                capability: 250.0
            },
            &laws
        ),
        Begin::Miss
    );
}
