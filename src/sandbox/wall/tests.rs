//! The wall prototype's invariants, about the game's environment destruction (game state only),
//! on the game's laws over the baked wall. Fixture numbers are fixture data; the brick is the
//! shipped row's ([`super::brick`]).

use bevy::math::Vec3;

use super::model::*;

// Fixture numerical step, s, for direct law probes outside a running app.
const STEP: f32 = 1.0 / 64.0;
use super::{Charge, Deposit, View, Wall, WallControls, apply_deposit, blast_field, brick, layout};

/// The 88 mm HE fixture burst (fixture data): 1.47 MJ, its charge the TNT that carries it, so its
/// reach (23.2 m, DERIVED) holds every standoff here; a coupling is its hits' energy over its own.
const FIXTURE_BLAST: Blast = Blast {
    energy: 1.47e6,
    charge: 1.47e6 / 4.184e6,
    radius: 1e-3,
    products: FIT_ONLY_PRODUCTS,
};

/// SUPPORT IS CONNECTIVITY TO AN ANCHOR THROUGH INTACT JOINTS, on the baked wall. An intact wall
/// has no loose group (a joint the bake missed would drop chunks the moment the wall stands);
/// cracking every joint across a region's boundary frees exactly that region, as one group; and
/// cracking every anchor frees the whole wall as one. The sandbox's falls, and the game's later,
/// rely on it.
#[test]
fn a_group_with_no_intact_path_to_the_ground_falls_as_one() {
    let layout = layout();
    let structure = &layout.cut.structure;
    let state = State::new(structure);
    assert!(
        loose_groups(structure, &state).is_empty(),
        "an intact wall stands"
    );

    let top = layout.min.y + 0.8 * layout.size.y;
    let region: Vec<usize> = (0..structure.chunks.len())
        .filter(|&i| structure.chunks[i].centroid.y > top)
        .collect();
    assert!(!region.is_empty());
    let mut state = State::new(structure);
    for (j, joint) in structure.joints.iter().enumerate() {
        let inside = |side: Side| match side {
            Side::Chunk(i) => region.contains(&i),
            Side::Ground => false,
        };
        if inside(Side::Chunk(joint.a)) != inside(joint.b) {
            state.cracked[j] = true;
        }
    }
    assert_eq!(loose_groups(structure, &state), vec![region]);

    let mut state = State::new(structure);
    for (j, joint) in structure.joints.iter().enumerate() {
        state.cracked[j] = joint.b == Side::Ground;
    }
    let all: Vec<usize> = (0..structure.chunks.len()).collect();
    assert_eq!(loose_groups(structure, &state), vec![all]);
}

/// A BLAST'S COUPLING IS ITS GEOMETRY: a charge at the face of a wide wall sends about half its
/// rays into it, and fewer the farther it stands off. No coupling constant enters.
#[test]
fn blast_coupling_falls_with_standoff() {
    let field = blast_field();
    let slab = [Block::cuboid(
        Vec3::new(-50.0, -50.0, -0.25),
        Vec3::new(50.0, 50.0, 0.0),
    )];
    let coupled = |standoff: f32| -> f32 {
        sample_blocks(
            &slab,
            |_| true,
            |_| brick(),
            Vec3::Z * standoff,
            FIXTURE_BLAST,
            &field,
        )
        .iter()
        .map(|(_, hit)| hit.energy)
        .sum::<f32>()
            / FIXTURE_BLAST.energy
    };
    let contact = coupled(1.0e-4);
    assert!((contact - 0.5).abs() < 0.01, "contact couples {contact}");
    let wall = [Block::cuboid(
        Vec3::new(-2.0, -1.0, -0.25),
        Vec3::new(2.0, 1.0, 0.0),
    )];
    let on_wall = |standoff: f32| -> f32 {
        sample_blocks(
            &wall,
            |_| true,
            |_| brick(),
            Vec3::Z * standoff,
            FIXTURE_BLAST,
            &field,
        )
        .iter()
        .map(|(_, hit)| hit.energy)
        .sum::<f32>()
            / FIXTURE_BLAST.energy
    };
    let (near, mid, far) = (on_wall(0.1), on_wall(1.0), on_wall(3.0));
    assert!(near > mid && mid > far && far > 0.0, "{near} {mid} {far}");
}

/// A HULL'S RAY ENTRY IS THE SHAPE'S, NOT ITS BOX'S: a ray that passes the cut corner of a
/// wedge-shaped chunk misses it, though the wedge's bounding box would be hit; one through the
/// body enters at the face. The sandbox's blast rays and the wall's picks rely on it.
#[test]
fn a_ray_enters_the_hull_not_its_bounding_box() {
    // A unit cube with the corner at (1, 1, *) sliced off.
    let wedge = Block::hull(vec![
        Vec3::new(0.0, 0.0, 0.0),
        Vec3::new(1.0, 0.0, 0.0),
        Vec3::new(0.0, 1.0, 0.0),
        Vec3::new(0.5, 1.0, 0.0),
        Vec3::new(1.0, 0.5, 0.0),
        Vec3::new(0.0, 0.0, 1.0),
        Vec3::new(1.0, 0.0, 1.0),
        Vec3::new(0.0, 1.0, 1.0),
        Vec3::new(0.5, 1.0, 1.0),
        Vec3::new(1.0, 0.5, 1.0),
    ])
    .expect("the wedge is a closed solid");
    assert!(
        wedge
            .entry(Vec3::new(0.95, 0.95, 5.0), Vec3::NEG_Z)
            .is_none()
    );
    let hit = wedge.entry(Vec3::new(0.25, 0.25, 5.0), Vec3::NEG_Z);
    assert!((hit.expect("through the body") - 4.0).abs() < 1e-4);
}

/// The game's environment blast tool needs casing construction for its cased setting. The
/// operator must see the missing burst block at initialization, before a readout or load can
/// invent zero products momentum. A silent bare-only fallback fails this refusal.
#[test]
#[should_panic(expected = "rounds need a burst block")]
fn the_blast_tool_refuses_missing_casing_construction_at_initialization() {
    Charge::of(&[]);
}

/// The game's environment blast tool shares the production ray driver and deposit pass.
/// ASSUMED fixture placement; the charge uses the tool's cased round's construction, not a fitted
/// chemical-energy-to-wear conversion. Pressure can remove volume while cavity energy is zero.
#[test]
fn the_shared_blast_path_reports_pressure_work_and_rear_scabs() {
    let layout = layout();
    let rounds = super::wall_rounds();
    let he = rounds
        .iter()
        .find(|round| round.key == super::wall_tool().cased_round)
        .expect("the tool's cased round is shipped");
    let charge = Charge::of(std::slice::from_ref(he));
    let mass = he
        .round
        .burst
        .expect("the fixture states a burst")
        .core_mass;
    for (mass, cased, standoff) in [
        (mass, true, 0.1),
        (mass, true, 0.3),
        (mass, true, 2.0),
        (0.9, false, 0.1),
    ] {
        let mut wall = Wall {
            cut: layout.cut.clone(),
            live: crate::construction::breakage::Live::from_structure(
                layout.cut.structure.clone(),
                layout.cut.elements,
            ),
            structural: vec![false; layout.cut.structure.joints.len()],
            entities: Vec::new(),
            meshes: Vec::new(),
            material: brick(),
        };
        let controls = WallControls {
            mass: 0,
            standoff: 0,
            cased,
            round: 0,
            view: View::Front,
            help: false,
        };
        let deposit = Deposit::Blast {
            mass,
            standoff,
            at: super::aim_point(&wall),
            into: Vec3::NEG_Z,
        };
        let result = apply_deposit(
            &mut wall,
            &controls,
            &charge,
            &blast_field(),
            &deposit,
            STEP,
        );
        let (_, _, intercepted, impulse, _) = result.blast.unwrap();
        if standoff == 0.1 {
            assert!(
                result.crushed > 0.0 || result.scabs > 0,
                "a near-contact field on the aimed solid face must exercise pressure removal"
            );
        }
        assert_eq!(
            result.deposited, 0.0,
            "a blast supplies no kinetic cavity energy"
        );
        assert!(
            result.chunks > 0 && impulse > 0.0,
            "the charge reaches the wall"
        );
        println!(
            "MEASURED shared sandbox: {mass:.3} kg, cased {cased}, standoff {standoff:.2} m: kinetic {} J, audit {intercepted:.1} J, impulse {impulse:.1} N s, crushed {:.6e} m3, scabs {}, cracks {}, freed {} groups {:?}",
            result.deposited,
            result.crushed,
            result.scabs,
            result.local + result.failures.len(),
            result.groups.iter().map(|g| g.0.len()).sum::<usize>(),
            result.groups.iter().map(|g| g.0.len()).collect::<Vec<_>>()
        );
    }
}

/// THE SANDBOX REACHES THE HOLE CAP AS THE GAME DOES, AND THE CAP DUSTS NOTHING: each subcritical
/// channel through one chunk keeps its hole up to [`HOLES_PER_CHUNK`]; one past it wears the pool
/// and leaves the chunk standing. A sandbox that cut no holes, or a count that dusts, fails this.
/// ASSUMED fixture: the largest chunk, the crush energy of a quarter of its pool (m³) spread over
/// the channels.
#[test]
fn the_sandbox_chunk_outlives_the_hole_cap() {
    use crate::construction::breakage::HOLES_PER_CHUNK;
    let layout = layout();
    let charge = Charge::of(&super::wall_rounds());
    let mut wall = Wall {
        cut: layout.cut.clone(),
        live: crate::construction::breakage::Live::from_structure(
            layout.cut.structure.clone(),
            layout.cut.elements,
        ),
        structural: vec![false; layout.cut.structure.joints.len()],
        entities: Vec::new(),
        meshes: Vec::new(),
        material: brick(),
    };
    let controls = WallControls {
        mass: 0,
        standoff: 0,
        cased: true,
        round: 0,
        view: View::Front,
        help: false,
    };
    let chunks = &wall.cut.structure.chunks;
    let c = (0..chunks.len())
        .max_by(|&a, &b| chunks[a].volume.total_cmp(&chunks[b].volume))
        .expect("the wall has chunks");
    let at = chunks[c].centroid;
    let energy =
        wall.live.state.pool[c] * chunks[c].material.crush / (4.0 * (HOLES_PER_CHUNK + 1) as f32);
    let channel = Deposit::Line {
        what: "fixture channel".to_owned(),
        lines: vec![(
            c,
            crate::ballistics::DeliveredLine {
                contacts: vec![(at - Vec3::Z * 0.05, at + Vec3::Z * 0.05)],
                crossing: crate::ballistics::VolumeCrossing {
                    volume: bevy::prelude::Entity::PLACEHOLDER,
                    entry: at - Vec3::Z * 0.05,
                    end: at + Vec3::Z * 0.05,
                    chord: 0.1,
                    cost: 0.0,
                    energy,
                    perforated: true,
                },
                momentum: Vec3::ZERO,
                entrance_momentum: Vec3::ZERO,
                entrance_energy: 0.0,
            },
        )],
        spall: 0.0,
    };
    for kept in 1..=HOLES_PER_CHUNK {
        let result = apply_deposit(
            &mut wall,
            &controls,
            &charge,
            &blast_field(),
            &channel,
            STEP,
        );
        assert!(!result.dusted.contains(&c), "channel {kept} is subcritical");
        assert_eq!(
            wall.live.holes[c].len(),
            kept,
            "each channel keeps its hole"
        );
    }
    let pool = wall.live.state.pool[c];
    let result = apply_deposit(
        &mut wall,
        &controls,
        &charge,
        &blast_field(),
        &channel,
        STEP,
    );
    assert!(
        !result.dusted.contains(&c) && wall.live.state.pool[c] < pool,
        "the channel past the cap wears the pool and dusts nothing"
    );
    assert_eq!(wall.live.holes[c].len(), HOLES_PER_CHUNK);
}

/// Fixture: one baked chunk, either soil-held or unsupported, and the production inspector
/// observer and deposit system. Work is one percent of its pool; the two independent impulses
/// make entrance work, linear launch and angular launch observable without a round registry.
fn inspector_app(held: bool) -> (bevy::prelude::App, crate::ballistics::DeliveredLine) {
    use crate::ballistics::{DeliveredLine, ShellMarched, SpallMarks, VolumeCrossing};
    use bevy::prelude::*;
    let cut = &layout().cut;
    let c = (0..cut.blocks.len())
        .filter(|&c| !cut.structure.chunks[c].buried)
        .max_by(|&a, &b| {
            cut.structure.chunks[a]
                .volume
                .total_cmp(&cut.structure.chunks[b].volume)
        })
        .unwrap();
    let block = cut.blocks[c].clone();
    let mut chunk = cut.structure.chunks[c];
    chunk.buried = held;
    let structure = Structure {
        chunks: vec![chunk],
        joints: Vec::new(),
    };
    let live = crate::construction::breakage::Live::from_structure(structure.clone(), 1);
    let energy = live.state.pool[0] * chunk.material.crush * 0.01;
    let reach = (block.max - block.min).length();
    let from = block.center() - Vec3::Z * reach;
    let (near, far) = block.span(from, Vec3::Z).unwrap();
    let (entry, end) = (from + Vec3::Z * near, from + Vec3::Z * far);
    let wall = Wall {
        cut: Cut {
            structure,
            blocks: vec![block],
            faces: Vec::new(),
            element_of: vec![0],
            elements: 1,
        },
        live,
        structural: Vec::new(),
        entities: vec![None],
        meshes: vec![Handle::default()],
        material: chunk.material,
    };
    let mut app = App::new();
    app.add_plugins((
        MinimalPlugins,
        AssetPlugin::default(),
        bevy::transform::TransformPlugin,
        crate::offline_physics(),
    ))
    .init_asset::<Mesh>()
    .insert_resource(avian3d::prelude::Gravity::ZERO)
    .insert_resource(bevy::time::TimeUpdateStrategy::ManualDuration(
        std::time::Duration::from_micros(15_625),
    ))
    .insert_resource(wall)
    .insert_resource(WallControls {
        mass: 0,
        standoff: 0,
        cased: true,
        round: 0,
        view: View::Front,
        help: false,
    })
    .insert_resource(Charge::of(&super::wall_rounds()))
    .insert_resource(super::BlastField(blast_field()))
    .insert_resource(super::WallAssets {
        heat: vec![Handle::default(); 11],
        loose: Handle::default(),
        toppled: Handle::default(),
        witness: Vec::new(),
        puff: Handle::default(),
    })
    .init_resource::<Assets<StandardMaterial>>()
    .init_resource::<super::WallQueue>()
    .init_resource::<super::WallLog>()
    .add_observer(super::read_hits)
    .add_systems(Update, super::apply_deposits);
    app.finish();
    app.cleanup();
    let volume = app.world_mut().spawn(super::WallChunk(0)).id();
    let shell = app.world_mut().spawn(SpallMarks::default()).id();
    let line = DeliveredLine {
        crossing: VolumeCrossing {
            volume,
            entry,
            end,
            chord: entry.distance(end),
            cost: 0.0,
            energy,
            perforated: true,
        },
        contacts: vec![(entry, end)],
        entrance_energy: energy * (5.0 / 7.0),
        entrance_momentum: Vec3::X * 12.0,
        momentum: Vec3::X * 12.0 + Vec3::Z * 30.0,
    };
    app.world_mut().trigger(ShellMarched {
        shell,
        marks_from: 0,
        cones_from: 0,
        lines: vec![line.clone()],
        burst: None,
    });
    (app, line)
}

/// The game's environment destruction inspector launches and spawns the same occupied mass the
/// structure weighed. Feeding its launch a contact inset, or dropping spawned mass overrides,
/// changes the body's momentum or spin and fails this production apply_deposits path.
#[test]
fn the_sandbox_launch_reads_the_chunk_the_structure_weighed() {
    use avian3d::dynamics::rigid_body::mass_properties::bevy_heavy::ComputeMassProperties3d;
    use avian3d::prelude::*;
    use bevy::prelude::*;
    let (mut app, line) = inspector_app(false);
    let wall = app.world().resource::<Wall>();
    let chunk = wall.cut.structure.chunks[0];
    let block = &wall.cut.blocks[0];
    let props = super::chunk_collider(block).mass_properties(chunk.material.density);
    let centre = block.center() + props.center_of_mass;
    let inertia = props.angular_inertia_tensor().to_mat3();
    let angular = (line.crossing.entry - centre).cross(line.entrance_momentum)
        + ((line.crossing.entry + line.crossing.end) / 2.0 - centre)
            .cross(line.momentum - line.entrance_momentum);
    app.update();
    // Launch is observed before this new body's first integration; free rotation can change
    // its angular velocity afterwards even with no external torque.
    let mut motion = app
        .world_mut()
        .query_filtered::<(&LinearVelocity, &AngularVelocity), With<super::WallPiece>>();
    let (velocity, spin) = motion.single(app.world()).unwrap();
    assert!((velocity.0 * chunk.mass - line.momentum).length() < 1e-3);
    assert!(
        (spin.0 - inertia.inverse() * angular).length() < 1e-3,
        "initial spin {:?}, expected {:?}",
        spin.0,
        inertia.inverse() * angular
    );
    for _ in 0..2 {
        app.update();
    }
    let mut bodies = app
        .world_mut()
        .query_filtered::<(&ComputedMass, &ComputedAngularInertia), With<super::WallPiece>>();
    let (mass, actual_inertia) = bodies.single(app.world()).unwrap();
    assert!((mass.value() / chunk.mass - 1.0).abs() < 1e-3);
    let error = (actual_inertia.tensor().to_mat3() - inertia)
        .to_cols_array()
        .map(f32::abs)
        .into_iter()
        .fold(0.0, f32::max);
    assert!(
        error
            < 1e-3
                * inertia
                    .to_cols_array()
                    .map(f32::abs)
                    .into_iter()
                    .fold(0.0, f32::max)
    );
}

/// The game's damage-model inspector reads delivered work through read_hits, apply_deposits,
/// the shared laws and the hole reader. Dropping entrance work at any of those seams changes
/// the observed pool, readout or channel; no round metadata supplies an inferred replacement.
#[test]
fn the_inspector_reads_delivered_work_through_pool_holes_and_readout() {
    let (mut app, line) = inspector_app(true);
    let wall = app.world().resource::<Wall>();
    let before = wall.live.state.pool[0];
    let crush = wall.cut.structure.chunks[0].material.crush;
    let volume = line.crossing.energy / crush;
    let radius = (volume / (std::f32::consts::PI * line.crossing.chord))
        .sqrt()
        .min((0.75 * volume / std::f32::consts::PI).cbrt());
    app.update();
    let wall = app.world().resource::<Wall>();
    let worn = before - wall.live.state.pool[0];
    assert!((worn / volume - 1.0).abs() < 1e-3);
    assert_eq!(wall.live.holes[0].len(), 1);
    assert_eq!(
        wall.live.holes[0][0].radius,
        (radius * 1000.0).round() as u16
    );
    let outcome = app
        .world()
        .resource::<super::WallLog>()
        .last
        .as_ref()
        .unwrap();
    assert!((outcome.deposited / line.crossing.energy - 1.0).abs() < 1e-4);
}

/// Inspector magnitudes are refused before play, naming the field an author must repair.
/// Removing any appearance gate or admitting a nonfinite spread breaks the firing/view contract.
#[test]
fn the_inspection_tool_refuses_invalid_magnitudes_at_admission() {
    let source = include_str!("../../../assets/dev/wall-tool.ron");
    for field in [
        "spread_rad",
        "seam_m",
        "puff_life_s",
        "view_fill",
        "readout_range_m",
        "puff_start_m",
        "puff_growth_m",
        "heat_emissive",
        "roughness",
        "ray_alpha_floor",
        "blast_ray_alpha",
        "gizmo_width_px",
        "readout_font_px",
        "readout_padding_px",
    ] {
        let prefix = format!("{field}: (");
        let start = source.find(&prefix).unwrap() + prefix.len();
        let end = start + source[start..].find(',').unwrap();
        for bad in ["0.0", "-1.0", "NaN", "inf"] {
            if bad == "0.0"
                && matches!(
                    field,
                    "spread_rad"
                        | "puff_growth_m"
                        | "heat_emissive"
                        | "roughness"
                        | "ray_alpha_floor"
                        | "blast_ray_alpha"
                        | "readout_padding_px"
                )
            {
                continue;
            }
            let mut text = source.to_owned();
            text.replace_range(start..end, bad);
            let error = super::tool_from_ron(&text)
                .err()
                .expect("invalid magnitude is refused");
            assert!(error.contains(field), "{field}: {error}");
        }
    }
}

/// The inspector palette cannot pass nonfinite or out-of-range channels to the renderer.
#[test]
fn the_inspection_tool_refuses_invalid_palette_channels() {
    let source = include_str!("../../../assets/dev/wall-tool.ron");
    for line in source.lines().filter(|line| line.contains("_rgba:")) {
        let field = line.trim().split(':').next().unwrap();
        let prefix = format!("{field}: ((");
        let start = source.find(&prefix).unwrap() + prefix.len();
        let end = start + source[start..].find(',').unwrap();
        for bad in ["-1.0", "NaN", "inf", "2.0"] {
            let mut text = source.to_owned();
            text.replace_range(start..end, bad);
            let error = super::tool_from_ron(&text)
                .err()
                .expect("invalid palette channel is refused");
            assert!(error.contains(field), "{field}: {error}");
        }
    }
}

/// The game's damage-model inspector must read a cone before any line is delivered, and attach
/// its strength only to that shell's line. Foreign sources must not enter the wall log.
/// Fixture: a wall cone, a foreign cone at the same endpoint and an interleaved shell.
#[test]
fn the_inspector_attributes_cones_to_their_wall_and_shell() {
    use crate::ballistics::spall_cone::{AxisRule, ConeShape, FieldDescriptor};
    use crate::ballistics::{
        DeliveredLine, ShellMarched, SpallCone, SpallMarks, SpallRay, VolumeCrossing,
    };
    use bevy::prelude::*;
    let mut app = App::new();
    app.init_resource::<super::WallQueue>()
        .init_resource::<super::WallLog>()
        .add_observer(super::read_hits);
    let volume = app.world_mut().spawn(super::WallChunk(0)).id();
    let spread = ConeShape::new(AxisRule::ExitDirection, 1)
        .unwrap()
        .spread(FieldDescriptor::ball(0.0))
        .unwrap();
    let (direction, profile) = spread.directions(Dir3::NEG_Z).next().unwrap();
    let strength = 17.0; // Fixture: recorded field strength, J.
    let cone = SpallCone {
        exited: vec![volume],
        origin: Vec3::ZERO,
        axis: Dir3::NEG_Z,
        field: spread.field(),
        strength,
        rays: vec![SpallRay {
            direction,
            end: super::witness_tile(0).center(),
            strength: strength * profile,
            doses: Vec::new(),
        }],
    };
    let mut foreign = cone.clone();
    foreign.exited = vec![app.world_mut().spawn_empty().id()];
    let shell = app
        .world_mut()
        .spawn(SpallMarks {
            cones: vec![cone, foreign],
            ..default()
        })
        .id();
    app.world_mut().trigger(ShellMarched {
        shell,
        marks_from: 0,
        cones_from: 0,
        lines: Vec::new(),
        burst: None,
    });
    let log = app.world().resource::<super::WallLog>();
    assert_eq!(
        log.cones.len(),
        1,
        "a cone-only event must advance its reader"
    );
    assert!((log.witness.iter().sum::<f32>() - strength * profile).abs() < 1e-5);
    assert!(app.world().resource::<super::WallQueue>().0.is_empty());
    let line = DeliveredLine {
        crossing: VolumeCrossing {
            volume,
            entry: Vec3::ZERO,
            end: Vec3::Z,
            chord: 1.0,
            cost: 0.0,
            energy: 7.0,
            perforated: true,
        },
        contacts: vec![(Vec3::ZERO, Vec3::Z)],
        momentum: Vec3::Z,
        entrance_energy: 0.0,
        entrance_momentum: Vec3::ZERO,
    };
    let other = app.world_mut().spawn(SpallMarks::default()).id();
    for shell in [other, shell, shell] {
        app.world_mut().trigger(ShellMarched {
            shell,
            marks_from: 0,
            cones_from: if shell == other { 0 } else { 2 },
            lines: vec![line.clone()],
            burst: None,
        });
    }
    let queue = &app.world().resource::<super::WallQueue>().0;
    assert_eq!(queue.len(), 3);
    let strength_of = |deposit: &Deposit| match deposit {
        Deposit::Line { spall, .. } => *spall,
        _ => panic!("the fixture emits line deposits"),
    };
    assert_eq!(
        strength_of(&queue[0]),
        0.0,
        "another shell cannot consume pending strength"
    );
    assert_eq!(strength_of(&queue[1]), strength);
    assert_eq!(strength_of(&queue[2]), 0.0);
}

/// THE WALL IS ITS DECLARED COMPONENT, NOT ITS NAMES: renaming every element of the shipped
/// construction (fixture edit) leaves the sandbox's cut the same, hull for hull. A selection by name
/// prefix loses the wall the moment an element is renamed.
#[test]
fn renaming_the_elements_keeps_the_wall() {
    use crate::construction::schema::ConstructionAsset;
    let asset =
        ConstructionAsset::from_ron(super::CONSTRUCTION).expect("the shipped construction parses");
    let mut renamed = asset.clone();
    for (k, element) in renamed.elements.iter_mut().enumerate() {
        element.name = format!("renamed_{k}");
    }
    let cut = |asset: &ConstructionAsset| {
        super::baked::wall_cut(asset, super::WALL, super::SUBSTANCE, brick(), super::PLACE)
            .expect("the shipped construction has the wall")
            .blocks
    };
    let blocks = cut(&asset);
    assert!(!blocks.is_empty());
    assert_eq!(cut(&renamed), blocks);
}
