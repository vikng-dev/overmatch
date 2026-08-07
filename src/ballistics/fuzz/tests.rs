//! The CI-scale §13.6 gate.
//!
//! One test does the gating; the rest prove the gate itself is not vacuous, which for a fuzzer is
//! the failure mode that matters. A ray generator that misses the tank, a target set that resolves
//! to nothing, or a bless list that swallows everything all produce a green run that means nothing.

use super::*;

/// The CI-scale ray count.
///
/// §13.6 asks for 10⁵–10⁶ at bake scale (`cargo run --bin ballistic_fuzzer`); this is the tenth of
/// that which fits in a test suite. MEASURED on the M4: ~1.0 s for the world plus 10⁴ rays, against
/// the 30 s budget — the ceiling is the whole suite's patience, not this test's need.
const CI_RAYS: u64 = 10_000;

/// A run at CI scale with the shipped defaults.
fn ci_report() -> Report {
    fuzz(&FuzzConfig {
        rays: CI_RAYS,
        ..default()
    })
    .expect("the probe world builds")
}

/// THE GATE. Every §13.6 invariant, over 10⁴ rays at the bound Tiger, with every corridor reaching
/// crew or ammunition adjudicated against the checked-in bless list.
#[test]
fn the_union_field_contract_holds_at_ci_scale() {
    let report = ci_report();

    assert!(
        report.violations.is_empty(),
        "§13.6 invariant violated at seed {} — replay the named ray with that seed, it depends on \
         nothing else:\n{:#?}",
        report.seed,
        report.violations,
    );
    assert!(
        report.duplication_checks > 0,
        "the idempotence/monotonicity gate never ran — no ray crossed material, or the stride is \
         larger than the run"
    );
    assert!(
        report.unexplained_walk_errors().is_empty(),
        "a corridor through the bound tank failed to walk on a volume that is NOT one of the \
         measured bake defects (see `DEGENERATE_BAKE_RESIDUE`). Every one of these is a round that \
         stops dead in mid-armour:\n{:#?}",
        report.unexplained_walk_errors(),
    );

    let bless = BlessList::shipped();
    let verdict = adjudicate(&report, &bless);
    assert!(
        verdict.unblessed.is_empty(),
        "UNBLESSED corridor to crew/ammunition. Each is either a hole to fix or a weakspot to add \
         to assets/tiger_1/tiger_1.bless.ron — §13.6: the bless list is where weakspots are \
         decided, not discovered by players as bugs.\n{}",
        render(&report, &bless, &verdict),
    );
}

/// ANTI-VACUITY. A gate that never reaches the crew has not cleared them, it has ignored them.
#[test]
fn every_crew_and_ammunition_volume_is_actually_reached() {
    let report = ci_report();
    for volume in [
        "Commander",
        "Gunner",
        "Loader",
        "Driver",
        "BowGunner",
        "Ammo_L_0",
        "Ammo_L_1",
        "Ammo_R_0",
        "Ammo_R_1",
    ] {
        assert!(
            report.presence_census.get(volume).is_some_and(|n| *n > 0),
            "no ray crossed `{volume}` in {CI_RAYS} rays — the gate is not covering it",
        );
        assert!(
            report.min_reach.contains_key(volume),
            "`{volume}` was crossed but no cost-to-reach was recorded for it",
        );
    }
    assert!(
        report.rays_crossing > report.rays / 10,
        "only {} of {} rays met the tank — the generator has drifted off the target",
        report.rays_crossing,
        report.rays,
    );
}

/// The bless machinery has teeth: raise the finding floor until real corridors appear, and they must
/// come back UNBLESSED rather than being quietly absorbed.
///
/// Without this, an empty bless list and a clean tank would make the gate's headline assertion
/// unfalsifiable — it would pass just as happily if `adjudicate` returned nothing at all.
#[test]
fn a_corridor_above_the_gate_floor_is_reported_unblessed() {
    let report = fuzz(&FuzzConfig {
        rays: 2_000,
        // Above the tank's thinnest authored plate, so ordinary roof and belly shots qualify.
        finding_floor: Some(60.0),
        ..default()
    })
    .expect("the probe world builds");
    assert!(
        !report.regions.is_empty(),
        "raising the floor to 60 reference-mm found no corridor at all — the finding path is dead"
    );
    for region in &report.regions {
        assert!(
            region.admitting.is_some(),
            "a corridor cheap enough to be a finding must be admitted by SOME gun: {region:?}"
        );
        assert_eq!(
            region.measurements.len(),
            PROBE_ROUNDS.len(),
            "every opening is measured against every gun"
        );
    }
    let verdict = adjudicate(&report, &BlessList::shipped());
    assert_eq!(
        verdict.unblessed.len(),
        report.regions.len(),
        "the shipped bless list must not absorb corridors it never named"
    );
}

/// Determinism: the same seed replays exactly, and a different seed does not.
#[test]
fn a_run_is_a_pure_function_of_its_seed() {
    let once = fuzz(&FuzzConfig {
        rays: 400,
        ..default()
    })
    .expect("world");
    let again = fuzz(&FuzzConfig {
        rays: 400,
        ..default()
    })
    .expect("world");
    assert_eq!(
        once.min_reach, again.min_reach,
        "two runs at one seed must measure the same tank"
    );
    assert_eq!(once.presence_census, again.presence_census);
    assert_eq!(
        once.walk_errors.len(),
        again.walk_errors.len(),
        "even the failures replay"
    );

    let other = fuzz(&FuzzConfig {
        rays: 400,
        seed: 0x1234_5678,
        ..default()
    })
    .expect("world");
    assert_ne!(
        once.presence_census, other.presence_census,
        "a different seed must fire different rays"
    );
}

/// The bless list on disk parses and every entry is usable — a blessing with a zero radius or an
/// unstated reason is a decision nobody recorded.
#[test]
fn the_shipped_bless_list_is_well_formed() {
    for opening in &BlessList::shipped().openings {
        assert!(
            opening.radius > 0.0,
            "blessing `{}` covers nothing",
            opening.name
        );
        assert!(
            opening.admits > 0.0,
            "blessing `{}` records no measured admitting caliber",
            opening.name
        );
        assert!(
            opening.reason.len() > 16,
            "blessing `{}` has no reason worth reading",
            opening.name
        );
    }
}

/// SURVEY (`cargo test -- --ignored survey --nocapture`): the full report at CI scale, for a human.
/// Set `FUZZ_FLOOR` to raise the finding floor and see the graded weakspots above the gate's line.
#[test]
#[ignore]
fn survey() {
    let report = fuzz(&FuzzConfig {
        rays: 20_000,
        finding_floor: std::env::var("FUZZ_FLOOR")
            .ok()
            .and_then(|v| v.parse().ok()),
        ..default()
    })
    .expect("world");
    let bless = BlessList::shipped();
    let verdict = adjudicate(&report, &bless);
    println!("{}", render(&report, &bless, &verdict));
}
