use super::*;
use crate::damage_cam::shapes::TankAsset;

/// Every recorded scenario, by name, name-sorted.
fn records() -> Vec<(String, PenetrationReport)> {
    let dir = crate::assets::asset_root().join("dev/pen_cam");
    let mut out: Vec<(String, PenetrationReport)> = std::fs::read_dir(&dir)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| {
            path.extension().is_some_and(|ext| ext == "ron")
                && path.file_name().is_some_and(|name| name != "scenarios.ron")
        })
        .map(|path| {
            let name = path.file_stem().unwrap().to_string_lossy().into_owned();
            (name, PenetrationReport::load(&path).unwrap())
        })
        .collect();
    out.sort_by(|a, b| a.0.cmp(&b.0));
    assert!(!out.is_empty(), "no recorded scenario in {}", dir.display());
    out
}

/// The vocabulary of every vehicle the recorded scenarios name.
fn vocabulary(records: &[(String, PenetrationReport)]) -> ShotVocabulary {
    let mut files: Vec<&str> = records
        .iter()
        .filter_map(|(_, report)| report.vehicle.as_deref())
        .collect();
    files.sort();
    files.dedup();
    let assets: Vec<(&str, TankAsset)> = files
        .into_iter()
        .map(|file| (file, TankAsset::load(file).unwrap()))
        .collect();
    ShotVocabulary::new(
        assets
            .iter()
            .map(|(file, asset)| (*file, asset.geometry(), asset.spec())),
        RoundRegistry::shipped(),
        ConeShape::shipped(),
    )
}

/// Fixture identities: the shooter combatant 7, the struck combatant 9 in its second life.
fn recorded(report: &PenetrationReport) -> RecordedShot {
    RecordedShot {
        tank: Entity::PLACEHOLDER,
        life: Some(TankLife {
            combatant: CombatantId(9),
            life: CombatantLife(1),
        }),
        shooter: None,
        shot: Some(ShotId {
            combatant: CombatantId(7),
            weapon: 0,
            fire_tick: 1200,
        }),
        encounter: 0,
        contact_tick: 1201,
        completion_tick: 1203,
        hull: Some((
            Vec3::new(12.3456, 0.8, -40.25),
            Quat::from_rotation_y(0.7) * Quat::from_rotation_x(0.05),
        )),
        report: report.clone(),
    }
}

/// The angle (rad) between `a` and `b`, in f64 by `atan2`: an f32 `acos` cannot tell apart angles
/// under 3.5e-4 rad, five times the octahedral bound.
fn angle(a: Vec3, b: Vec3) -> f32 {
    let (a, b) = (a.as_dvec3(), b.as_dvec3());
    a.cross(b).length().atan2(a.dot(b)) as f32
}

/// The angle (rad) of the rotation from `a` to `b`, in f64 as [`angle`].
fn rotation_angle(a: Quat, b: Quat) -> f32 {
    let d = a.as_dquat().inverse() * b.as_dquat();
    (2.0 * d.xyz().length().atan2(d.w.abs())) as f32
}

/// Half an f32 ulp of `value`, the most an f64-exact quantum loses on its way back to f32.
fn half_ulp(value: f32) -> f32 {
    (value.abs().max(f32::MIN_POSITIVE) * f32::EPSILON) * 0.5
}

#[track_caller]
fn assert_point(what: &str, decoded: [f32; 3], recorded: [f32; 3]) {
    for (d, r) in decoded.into_iter().zip(recorded) {
        assert!(
            (d - r).abs() <= 0.000_5 + half_ulp(r) + half_ulp(d),
            "{what}: {decoded:?} vs {recorded:?}"
        );
    }
}

#[track_caller]
fn assert_time(what: &str, decoded: f32, recorded: f32) {
    assert!(
        (decoded - recorded).abs() <= 0.5e-6 + half_ulp(recorded) + half_ulp(decoded),
        "{what}: {decoded} vs {recorded}"
    );
}

#[track_caller]
fn assert_direction(what: &str, decoded: [f32; 3], recorded: [f32; 3]) {
    let off = angle(Vec3::from(decoded), Vec3::from(recorded));
    assert!(
        off <= DIRECTION_BOUND,
        "{what}: {off} rad off ({decoded:?} vs {recorded:?})"
    );
}

/// `decoded` is `recorded` up to the quanta of the module's table, but for the shooter's name,
/// which no explanation carries.
fn assert_round_trip(name: &str, decoded: &PenetrationReport, recorded: &PenetrationReport) {
    assert_eq!(decoded.synthetic, recorded.synthetic, "{name}");
    assert_eq!(decoded.vehicle, recorded.vehicle, "{name}");
    assert_eq!(decoded.round, recorded.round, "{name}");
    assert_eq!(decoded.shooter, None, "{name}");
    assert_eq!(decoded.projectile, recorded.projectile, "{name}");
    let turn = std::f32::consts::PI / 65_536.0;
    for (servo, &deg) in &decoded.servo_deg {
        let want = recorded.servo_deg.get(servo).copied().unwrap_or(0.0);
        let off = (deg - want).to_radians().rem_euclid(std::f32::consts::TAU);
        let off = off.min(std::f32::consts::TAU - off);
        assert!(off <= turn * 1.01, "{name}: servo {servo} {deg} vs {want}");
    }
    assert!(
        recorded
            .servo_deg
            .keys()
            .all(|servo| decoded.servo_deg.contains_key(servo)),
        "{name}"
    );
    assert_point(name, decoded.origin, recorded.origin);
    assert_eq!(
        decoded.muzzle.is_some(),
        recorded.muzzle.is_some(),
        "{name}"
    );
    if let (Some(d), Some(r)) = (decoded.muzzle, recorded.muzzle) {
        assert_point(name, d, r);
    }

    let (d, r) = (&decoded.impact, &recorded.impact);
    assert_point(name, d.point, r.point);
    assert_direction(name, d.normal, r.normal);
    assert_direction(name, d.incoming, r.incoming);
    assert_eq!(d.verdict, r.verdict, "{name}");
    assert_eq!(
        d.incidence_deg.to_bits(),
        r.incidence_deg.to_bits(),
        "{name}: incidence {} vs {}",
        d.incidence_deg,
        r.incidence_deg
    );

    assert_eq!(decoded.plates.len(), recorded.plates.len(), "{name}");
    for (d, r) in decoded.plates.iter().zip(&recorded.plates) {
        assert_eq!(
            (
                &d.name,
                d.class,
                d.overmatched,
                d.chord_mm,
                d.steel_equiv_mm,
                d.cost_mm
            ),
            (
                &r.name,
                r.class,
                r.overmatched,
                r.chord_mm,
                r.steel_equiv_mm,
                r.cost_mm
            ),
            "{name}"
        );
        assert_time(name, d.t, r.t);
        assert_point(name, d.entry, r.entry);
        assert_eq!(d.exit.is_some(), r.exit.is_some(), "{name}");
        if let (Some(de), Some(re)) = (d.exit, r.exit) {
            assert_point(name, de, re);
        }
        assert!(
            (d.factor - r.factor).abs() <= 4.0 * f32::EPSILON * r.factor.abs(),
            "{name}: factor {} vs {}",
            d.factor,
            r.factor
        );
    }

    assert_eq!(decoded.ricochets.len(), recorded.ricochets.len(), "{name}");
    for (d, r) in decoded.ricochets.iter().zip(&recorded.ricochets) {
        assert_eq!(d.volume, r.volume, "{name}");
        assert_time(name, d.t, r.t);
        assert_point(name, d.point, r.point);
        assert_direction(name, d.normal, r.normal);
        assert_direction(name, d.incoming, r.incoming);
        assert_direction(name, d.outgoing, r.outgoing);
    }

    assert_eq!(decoded.path.len(), recorded.path.len(), "{name}");
    for (d, r) in decoded.path.iter().zip(&recorded.path) {
        assert_time(name, d.t, r.t);
        assert_point(name, d.point, r.point);
    }

    assert_eq!(decoded.spall.len(), recorded.spall.len(), "{name}");
    for (d, r) in decoded.spall.iter().zip(&recorded.spall) {
        assert_time(name, d.t, r.t);
        assert_point(name, d.origin, r.origin);
        assert_direction(name, d.axis, r.axis);
        assert_eq!(
            (d.half_angle_deg, d.strength, d.fragments.len()),
            (r.half_angle_deg, r.strength, r.fragments.len()),
            "{name}"
        );
        let (d_origin, r_origin) = (Vec3::from(d.origin), Vec3::from(r.origin));
        for (df, rf) in d.fragments.iter().zip(&r.fragments) {
            assert_eq!((df.strength, &df.hits), (rf.strength, &rf.hits), "{name}");
            let reach = (Vec3::from(rf.end) - r_origin).length();
            let off = (Vec3::from(df.end) - Vec3::from(rf.end)).length();
            assert!(
                off <= end_bound(reach),
                "{name}: a ray of reach {reach} m ends {off} m off"
            );
            let decoded_reach = (Vec3::from(df.end) - d_origin).length();
            assert!(
                (decoded_reach - reach).abs() <= 0.000_5 + 4.0 * half_ulp(reach.max(1.0)),
                "{name}: reach {decoded_reach} vs {reach}"
            );
        }
    }

    match (&decoded.end, &recorded.end) {
        (RoundEnd::Left { at: d }, RoundEnd::Left { at: r }) => assert_point(name, *d, *r),
        (d, r) => assert_eq!(d, r, "{name}"),
    }

    assert_eq!(decoded.doses.len(), recorded.doses.len(), "{name}");
    for (d, r) in decoded.doses.iter().zip(&recorded.doses) {
        assert_eq!(
            (&d.volume, d.line, d.field, d.lost),
            (&r.volume, r.line, r.field, r.lost),
            "{name}"
        );
        assert_eq!(d.entered.is_some(), r.entered.is_some(), "{name}");
        if let (Some(de), Some(re)) = (d.entered, r.entered) {
            assert_point(name, de, re);
        }
    }

    assert_eq!(
        decoded.consequences.len(),
        recorded.consequences.len(),
        "{name}"
    );
    for (d, r) in decoded.consequences.iter().zip(&recorded.consequences) {
        assert_eq!(
            (&d.volume, d.state, &d.function),
            (&r.volume, r.state, &r.function),
            "{name}"
        );
        assert_time(name, d.t, r.t);
    }

    assert_eq!(decoded.components, recorded.components, "{name}");
    assert_eq!(decoded.killed, recorded.killed, "{name}");
}

/// Every recorded scenario comes back from its victim's explanation as it was recorded, up to the
/// quanta, and each ray's reach is the record's to the millimetre on the wire.
#[test]
fn every_recorded_scenario_round_trips() {
    let records = records();
    let vocabulary = vocabulary(&records);
    for (name, report) in &records {
        let built = BuiltExplanation::build(&vocabulary, &recorded(report))
            .unwrap_or_else(|err| panic!("{name}: {err}"));
        let decoded = built.full.decode(&vocabulary).unwrap();
        assert_round_trip(name, &decoded, report);
        let bytes = postcard::to_allocvec(&built.full).unwrap();
        let read: ShotExplanation = postcard::from_bytes(&bytes).unwrap();
        assert_eq!(read, built.full, "{name}: postcard round trip");

        for (wire, cone) in built.full.record.cones.iter().zip(&report.spall) {
            let origin = Vec3::from(cone.origin);
            let mut overflow = wire.overflow.iter();
            for (&reach, fragment) in wire.reaches.iter().zip(&cone.fragments) {
                let mm = if reach == REACH_OVERFLOW {
                    *overflow.next().unwrap()
                } else {
                    reach as u32
                };
                let want = ((Vec3::from(fragment.end) - origin).length() as f64 * 1000.0).round();
                assert_eq!(mm as f64, want, "{name}");
            }
        }
    }
}

/// A reach past `u16` mm rides the overflow list and comes back to the millimetre.
#[test]
fn a_reach_past_u16_millimetres_overflows() {
    let records = records();
    let vocabulary = vocabulary(&records);
    let (name, mut report) = records
        .into_iter()
        .find(|(_, report)| !report.spall.is_empty())
        .expect("a recorded scenario throws a cone");
    let cone = &mut report.spall[0];
    let origin = Vec3::from(cone.origin);
    for (ray, far) in [(3, 70.0), (200, 4_000.25)] {
        let span = Vec3::from(cone.fragments[ray].end) - origin;
        cone.fragments[ray].end = (origin + span.normalize() * far).to_array();
    }
    let built = BuiltExplanation::build(&vocabulary, &recorded(&report)).unwrap();
    assert_eq!(built.full.record.cones[0].overflow, [70_000, 4_000_250]);
    let decoded = built.full.decode(&vocabulary).unwrap();
    assert_round_trip(&name, &decoded, &report);
}

/// The quanta's bounds hold over a dense sweep: every direction of a Fibonacci sphere, every angle
/// of a fine sweep, every rotation of a grid of axes and angles.
#[test]
fn the_quanta_hold_their_bounds() {
    let count = 200_000;
    let golden = std::f64::consts::PI * (3.0 - 5f64.sqrt());
    let mut worst = 0.0f32;
    for k in 0..count {
        let y = 1.0 - 2.0 * (k as f64 + 0.5) / count as f64;
        let r = (1.0 - y * y).sqrt();
        let phi = golden * k as f64;
        let v = Vec3::new((r * phi.cos()) as f32, y as f32, (r * phi.sin()) as f32);
        worst = worst.max(angle(Oct::of(v.to_array()).unwrap().direction(), v));
    }
    for axis in [
        Vec3::X,
        Vec3::NEG_X,
        Vec3::Y,
        Vec3::NEG_Y,
        Vec3::Z,
        Vec3::NEG_Z,
    ] {
        worst = worst.max(angle(Oct::of(axis.to_array()).unwrap().direction(), axis));
    }
    assert!(worst <= DIRECTION_BOUND, "octahedral: {worst} rad");

    let mut worst = 0.0f64;
    for k in -400_000..=400_000 {
        let deg = k as f32 * 0.000_9;
        let back = Turn::of_degrees(deg).unwrap().degrees() as f64;
        let off = (back - deg as f64).rem_euclid(360.0);
        worst = worst.max(off.min(360.0 - off));
        let rad = deg.to_radians();
        let back = Turn::of_radians(rad).unwrap().radians();
        assert!((-std::f32::consts::PI..=std::f32::consts::PI).contains(&back));
    }
    assert!(worst <= 0.002_8, "turn: {worst}°");

    let mut worst = 0.0f32;
    for i in 0..400 {
        let axis = Vec3::new((i as f32 * 0.37).sin(), (i as f32 * 0.91).cos(), 0.3).normalize();
        for j in 0..100 {
            let q = Quat::from_axis_angle(axis, j as f32 * 0.0634 - 3.1);
            worst = worst.max(rotation_angle(
                Rotation::of(q).unwrap().rotation().unwrap(),
                q,
            ));
        }
    }
    assert!(worst <= ROTATION_BOUND, "rotation: {worst} rad");
}

/// The shooter hears of no component the shot did not reach, and nothing from which one's prior
/// damage could be read: every byte of its projection is the same whatever HP the unreached
/// components held at the shot.
#[test]
fn the_shooter_learns_nothing_of_an_unreached_component() {
    let records = records();
    let vocabulary = vocabulary(&records);
    let mut perturbed_any = false;
    for (name, report) in &records {
        let built = BuiltExplanation::build(&vocabulary, &recorded(report)).unwrap();
        let (_, shooter) = built
            .projections()
            .into_iter()
            .find(|(_, explanation)| explanation.audience == Audience::Shooter)
            .unwrap();
        let decoded = shooter.decode(&vocabulary).unwrap();
        for component in &decoded.components {
            assert!(
                decoded
                    .doses
                    .iter()
                    .any(|dose| dose.volume == component.name),
                "{name}: the shooter hears of {}, which no dose names",
                component.name
            );
        }

        let mut damaged = report.clone();
        for component in &mut damaged.components {
            if !report
                .doses
                .iter()
                .any(|dose| dose.volume == component.name)
            {
                component.hp /= 2;
                perturbed_any |= component.count > 1;
            }
        }
        let rebuilt = BuiltExplanation::build(&vocabulary, &recorded(&damaged)).unwrap();
        let (_, again) = rebuilt
            .projections()
            .into_iter()
            .find(|(_, explanation)| explanation.audience == Audience::Shooter)
            .unwrap();
        assert_eq!(
            postcard::to_allocvec(&again).unwrap(),
            postcard::to_allocvec(&shooter).unwrap(),
            "{name}: an unreached component's prior damage reaches the shooter"
        );
    }
    assert!(perturbed_any, "some scenario leaves a component unreached");
}

/// The struck tank's combatant gets its full record, with the kill cam's lesson when the shot
/// killed it; the shooter gets the shooter's projection; one message each; nobody else.
#[test]
fn each_party_gets_its_own_projection_once() {
    let records = records();
    let vocabulary = vocabulary(&records);
    let (_, report) = &records[0];
    for killed in [None, Some(KnockoutReason::CrewLoss)] {
        let mut report = report.clone();
        report.killed = killed;
        let shot = recorded(&report);
        let built = BuiltExplanation::build(&vocabulary, &shot).unwrap();
        let projections = built.projections();
        assert_eq!(projections.len(), 2);
        let victim = shot.life.unwrap().combatant;
        let shooter = shot.shot.unwrap().combatant;
        let (to_victim, to_shooter) = (&projections[0], &projections[1]);
        assert_eq!(to_victim.0, victim);
        assert_eq!(to_shooter.0, shooter);
        let expected = if killed.is_some() {
            Audience::KilledVictim
        } else {
            Audience::Victim
        };
        assert_eq!(to_victim.1.audience, expected);
        assert_eq!(to_victim.1.lesson(), killed.is_some());
        assert_eq!(to_victim.1.decode(&vocabulary).unwrap().killed, killed);
        assert_eq!(to_victim.1.record, built.full.record);
        assert_eq!(to_shooter.1.audience, Audience::Shooter);
        assert!(!to_shooter.1.lesson());
        assert_eq!(to_shooter.1.decode(&vocabulary).unwrap().killed, killed);
        for (_, explanation) in &projections {
            assert_eq!(
                (explanation.shot, explanation.encounter, explanation.struck),
                (built.full.shot, built.full.encounter, built.full.struck)
            );
            assert_eq!(
                (explanation.contact_tick, explanation.completion_tick),
                (1201, 1203)
            );
        }
    }
}

/// A combatant both shooter and victim — a round of one life striking its next — gets one
/// message, the victim's.
#[test]
fn a_combatant_in_both_roles_gets_one_message() {
    let records = records();
    let vocabulary = vocabulary(&records);
    let mut shot = recorded(&records[0].1);
    shot.shot.as_mut().unwrap().combatant = shot.life.unwrap().combatant;
    let projections = BuiltExplanation::build(&vocabulary, &shot)
        .unwrap()
        .projections();
    assert_eq!(projections.len(), 1);
    assert_eq!(projections[0].0, shot.life.unwrap().combatant);
    assert_eq!(projections[0].1.audience, Audience::Victim);
}

/// The hull's pose comes back within the point and rotation quanta.
#[test]
fn the_hull_pose_rides_the_explanation() {
    let records = records();
    let vocabulary = vocabulary(&records);
    let shot = recorded(&records[0].1);
    let built = BuiltExplanation::build(&vocabulary, &shot).unwrap();
    let (at, turn) = built.full.hull_pose().unwrap().unwrap();
    let (want_at, want_turn) = shot.hull.unwrap();
    assert_point("hull", at.to_array(), want_at.to_array());
    assert!(rotation_angle(turn, want_turn) <= ROTATION_BOUND);
}

/// The vocabulary folds the same from the same data, and a client whose vocabulary folds another
/// refuses the explanation.
#[test]
fn a_foreign_vocabulary_is_refused() {
    let records = records();
    let vocabulary = vocabulary(&records);
    assert_eq!(
        vocabulary.fingerprint(),
        self::vocabulary(&records).fingerprint()
    );
    let built = BuiltExplanation::build(&vocabulary, &recorded(&records[0].1)).unwrap();
    let mut other = vocabulary.clone();
    other.cone = ConeShape::shipped_at(128);
    other.fingerprint = other.fold();
    assert_ne!(other.fingerprint(), vocabulary.fingerprint());
    assert!(built.full.decode(&other).is_err());
}

/// No explanation is built for a record no match produced, nor for a synthetic one.
#[test]
fn only_a_matched_real_record_is_explained() {
    let records = records();
    let vocabulary = vocabulary(&records);
    let report = &records[0].1;
    let mut shot = recorded(report);
    shot.shot = None;
    assert!(BuiltExplanation::build(&vocabulary, &shot).is_err());
    let mut shot = recorded(report);
    shot.life = None;
    assert!(BuiltExplanation::build(&vocabulary, &shot).is_err());
    let mut shot = recorded(report);
    shot.report.synthetic = true;
    assert!(BuiltExplanation::build(&vocabulary, &shot).is_err());
}

/// MEASURED: the postcard size of each recorded scenario's projections, and of its cones.
/// `cargo test --lib damage_cam::wire::tests::encoded_sizes -- --nocapture` prints the table the
/// module documentation quotes.
#[test]
fn encoded_sizes() {
    let records = records();
    let vocabulary = vocabulary(&records);
    println!("| scenario | total B | shooter B | fields B | cones | rays | hits | B per cone |");
    println!("|---|---|---|---|---|---|---|---|");
    for (name, report) in &records {
        let built = BuiltExplanation::build(&vocabulary, &recorded(report)).unwrap();
        let projections = built.projections();
        let size = |audience: Audience| {
            projections
                .iter()
                .find(|(_, explanation)| explanation.audience == audience)
                .map_or(0, |(_, explanation)| {
                    postcard::to_allocvec(explanation).unwrap().len()
                })
        };
        let victim = size(Audience::Victim).max(size(Audience::KilledVictim));
        let shooter = size(Audience::Shooter);
        let cones = &built.full.record.cones;
        let fields: usize = cones
            .iter()
            .map(|cone| postcard::to_allocvec(cone).unwrap().len())
            .sum();
        let rays: usize = cones.iter().map(|cone| cone.reaches.len()).sum();
        let hits: usize = cones.iter().map(|cone| cone.hits.len()).sum();
        let per_cone = if cones.is_empty() {
            "–".to_owned()
        } else {
            (fields / cones.len()).to_string()
        };
        println!(
            "| {name} | {victim} | {shooter} | {fields} | {} | {rays} | {hits} | {per_cone} |",
            cones.len()
        );
        assert!(shooter <= victim, "{name}");
    }
}

/// A hull rotation the encoder could not have written — the zero quaternion, one of length two,
/// one just past the slack — is refused by the pose and by the whole decode, never normalized.
#[test]
fn a_malformed_hull_rotation_is_refused() {
    let records = records();
    let vocabulary = vocabulary(&records);
    let built = BuiltExplanation::build(&vocabulary, &recorded(&records[0].1)).unwrap();
    let rotation = |q: [i16; 4]| {
        let mut bytes = [0; 8];
        for (index, value) in q.into_iter().enumerate() {
            bytes[2 * index..2 * index + 2].copy_from_slice(&value.to_le_bytes());
        }
        Rotation(bytes)
    };
    let past_slack = (32_767.0 * (1.0 + 2.0 * ROTATION_SLACK) / 2f32.sqrt()).round() as i16;
    for hostile in [
        rotation([0; 4]),
        rotation([i16::MAX; 4]),
        rotation([i16::MIN; 4]),
        rotation([0, 0, past_slack, past_slack]),
    ] {
        let mut explanation = built.full.clone();
        explanation.hull.as_mut().unwrap().turn = hostile;
        let bytes = postcard::to_allocvec(&explanation).unwrap();
        let received: ShotExplanation = postcard::from_bytes(&bytes).unwrap();
        assert!(received.hull_pose().is_err(), "{hostile:?}");
        assert!(received.decode(&vocabulary).is_err(), "{hostile:?}");
    }
    let mut explanation = built.full.clone();
    explanation.hull.as_mut().unwrap().turn = rotation([0, 0, 0, i16::MAX]);
    assert_eq!(explanation.hull_pose().unwrap().unwrap().1, Quat::IDENTITY);
}

/// A record holding a number that is not finite is refused.
#[test]
fn a_non_finite_number_is_refused() {
    let records = records();
    let vocabulary = vocabulary(&records);
    let (_, report) = records
        .iter()
        .find(|(_, report)| {
            !report.spall.is_empty() && !report.doses.is_empty() && !report.plates.is_empty()
        })
        .expect("a recorded scenario crosses a plate and doses through a cone");
    let built = BuiltExplanation::build(&vocabulary, &recorded(report)).unwrap();
    assert!(built.full.decode(&vocabulary).is_ok());
    let hostile: [fn(&mut Record); 5] = [
        |record| record.speed_mps = f32::NAN,
        |record| record.impact.incidence_deg = f32::INFINITY,
        |record| record.cones[0].strength = f32::NEG_INFINITY,
        |record| record.doses[0].field = f32::NAN,
        |record| record.plates[0].cost_mm = f32::NAN,
    ];
    for (index, spoil) in hostile.into_iter().enumerate() {
        let mut explanation = built.full.clone();
        spoil(&mut explanation.record);
        assert!(explanation.decode(&vocabulary).is_err(), "spoiler {index}");
    }
}

/// Incidence comes back exactly as recorded, near normal impact too, where an `acos` of the decoded
/// directions would not: a sweep of normals, each struck head-on and up to 1° off it, the record's
/// incidence computed as the record computes it.
#[test]
fn incidence_is_the_records_near_normal_impact() {
    let records = records();
    let vocabulary = vocabulary(&records);
    let report = &records[0].1;
    let mut normals = vec![Vec3::new(0.059_412_46, -0.542_756_44, 0.837_786_2).normalize()];
    let count = 400;
    let golden = std::f32::consts::PI * (3.0 - 5f32.sqrt());
    for k in 0..count {
        let y = 1.0 - 2.0 * (k as f32 + 0.5) / count as f32;
        let r = (1.0 - y * y).sqrt();
        let phi = golden * k as f32;
        normals.push(Vec3::new(r * phi.cos(), y, r * phi.sin()));
    }
    for normal in normals {
        let side = normal.any_orthonormal_vector();
        for off_deg in [0.0f32, 0.001, 0.01, 0.1, 1.0] {
            let incoming = -(Quat::from_axis_angle(side, off_deg.to_radians()) * normal);
            let mut report = report.clone();
            report.impact.normal = normal.to_array();
            report.impact.incoming = incoming.to_array();
            report.impact.incidence_deg =
                (-incoming).dot(normal).clamp(-1.0, 1.0).acos().to_degrees();
            let built = BuiltExplanation::build(&vocabulary, &recorded(&report)).unwrap();
            let decoded = built.full.decode(&vocabulary).unwrap();
            assert_eq!(
                decoded.impact.incidence_deg.to_bits(),
                report.impact.incidence_deg.to_bits(),
                "{normal} {off_deg}°"
            );
        }
    }
}
