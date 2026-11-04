use super::*;
use crate::net::debris_stress::{DebrisStress, install_server_from};

/// Fixture construction is deliberately independent of the shipped fragment and release tastes.
fn parameters() -> Parameters {
    Parameters {
        fragment_hull_m: Sourced(vec![
            [-0.1, -0.1, -0.1],
            [0.1, -0.1, -0.1],
            [-0.1, 0.1, -0.1],
            [0.1, 0.1, -0.1],
            [-0.1, -0.1, 0.1],
            [0.1, -0.1, 0.1],
            [-0.1, 0.1, 0.1],
            [0.1, 0.1, 0.1],
        ]),
        density_kg_m3: Sourced(1000.0),
        lattice_m: Sourced(1.0),
        rows: Sourced(2),
        columns: Sourced(3),
        ground_clearance_m: Sourced(0.2),
        tilt_rad: Sourced(0.1),
        inward_speed_m_s: Sourced([0.5, 2.0]),
        upward_speed_m_s: Sourced(0.6),
        along_speed_m_s: Sourced(0.2),
        spin_rad_s: Sourced(0.3),
        albedo_srgb: Sourced([0.5; 3]),
        roughness: Sourced(1.0),
    }
}

/// Operator scenario admission owns finite, nonoverlapping construction. Neither a new fragment
/// nor a different legal release layout may bypass those invariants and reach body creation.
#[test]
fn construction_faults_name_the_file_field_and_fix_before_play() {
    type Mutation = fn(&mut Parameters);
    let faults: &[(&str, Mutation)] = &[
        ("density_kg_m3", |p| p.density_kg_m3.0 = 0.0),
        ("density_kg_m3", |p| p.density_kg_m3.0 = f32::INFINITY),
        ("density_kg_m3", |p| p.density_kg_m3.0 = f32::MIN_POSITIVE),
        ("lattice_m", |p| p.lattice_m.0 = f32::NAN),
        ("lattice_m", |p| p.lattice_m.0 = f32::MAX),
        ("lattice_m", |p| p.lattice_m.0 = 0.01),
        ("rows", |p| p.rows.0 = 0),
        ("columns", |p| p.columns.0 = 0),
        ("ground_clearance_m", |p| p.ground_clearance_m.0 = 0.0),
        ("tilt_rad", |p| p.tilt_rad.0 = std::f32::consts::PI),
        ("inward_speed_m_s", |p| p.inward_speed_m_s.0 = [2.0, 1.0]),
        ("upward_speed_m_s", |p| p.upward_speed_m_s.0 = -1.0),
        ("along_speed_m_s", |p| p.along_speed_m_s.0 = f32::NAN),
        ("along_speed_m_s", |p| p.along_speed_m_s.0 = f32::MAX),
        ("spin_rad_s", |p| p.spin_rad_s.0 = f32::INFINITY),
        ("roughness", |p| p.roughness.0 = -1.0),
        ("albedo_srgb", |p| p.albedo_srgb.0[0] = 2.0),
        ("fragment_hull_m", |p| p.fragment_hull_m.0.clear()),
        ("fragment_hull_m", |p| p.fragment_hull_m.0[0][1] = f32::NAN),
        ("fragment_hull_m", |p| {
            p.fragment_hull_m.0.iter_mut().for_each(|v| v[1] = 0.0)
        }),
    ];
    assert!(Scenario::admit(parameters()).is_ok());
    let mut boundary = parameters();
    boundary.along_speed_m_s.0 = f32::MAX / 2.0;
    assert!(Scenario::admit(boundary).is_ok());
    for (field, mutate) in faults {
        let mut data = parameters();
        mutate(&mut data);
        let error = Scenario::admit(data).expect_err(field);
        let message = error.to_string();
        assert!(
            message.contains(FILE) && message.contains(field),
            "{message}"
        );
        assert!(matches!(error, ScenarioError::Invalid { fix, .. } if !fix.is_empty()));
    }
}

/// The actual installer must admit required scenario data even when the lever is off. A mutant
/// that creates a default scenario, postpones admission, or ignores the refusal fails here.
#[test]
fn the_server_installer_requires_the_scenario_before_mounting_releases() {
    for lever in [None, Some("3:1")] {
        let mut app = App::new();
        let error = install_server_from(&mut app, lever, "()")
            .unwrap_err()
            .to_string();
        assert!(
            error.contains(FILE) && error.contains("fragment_hull_m"),
            "{error}"
        );
        assert!(!app.world().contains_resource::<DebrisStress>());
        install_server_from(&mut app, lever, RON).unwrap();
        assert_eq!(
            app.world().contains_resource::<DebrisStress>(),
            lever.is_some()
        );
    }
}

/// Every declared scenario quantity is required; inventing even one legal default conceals an
/// author's missing choice. This pins the file interface, independently of the shipped values.
#[test]
fn every_scenario_field_must_be_authored() {
    let fields = [
        "fragment_hull_m",
        "density_kg_m3",
        "lattice_m",
        "rows",
        "columns",
        "ground_clearance_m",
        "tilt_rad",
        "inward_speed_m_s",
        "upward_speed_m_s",
        "along_speed_m_s",
        "spin_rad_s",
        "albedo_srgb",
        "roughness",
    ];
    let starts: Vec<_> = fields
        .iter()
        .map(|field| RON.find(&format!("{field}:")).unwrap())
        .collect();
    for (field, &start) in fields.iter().zip(&starts) {
        let end = starts
            .iter()
            .copied()
            .filter(|&next| next > start)
            .min()
            .unwrap_or_else(|| RON.rfind(')').unwrap());
        let text = format!("{}{}", &RON[..start], &RON[end..]);
        let error = Scenario::parse(&text).unwrap_err().to_string();
        assert!(error.contains(FILE) && error.contains(field), "{error}");
    }
}

/// A collider alone does not certify presentation: tiny face areas can normalize to zero even
/// while their vertices are finite. Refuse them at admission before a view invents zero normals.
#[test]
fn unresolvable_faces_are_refused_before_the_view_builds() {
    let mut data = parameters();
    // Fixture scale/density isolate face representability from the other release choices.
    for point in &mut data.fragment_hull_m.0 {
        for coordinate in point {
            *coordinate *= 1e-12;
        }
    }
    data.density_kg_m3.0 = f32::MAX;
    let error = Scenario::admit(data).unwrap_err().to_string();
    assert!(
        error.contains(FILE) && error.contains("fragment_hull_m") && error.contains("faces"),
        "{error}"
    );
}

/// Shipped-file provenance policy applies in test builds too, with a field path for corrections.
#[test]
fn unlabelled_blank_and_fixture_fields_are_refused_with_their_path() {
    for (value, cause) in [
        ("1000.0", "provenance"),
        ("(1000.0, StandIn(\" \"))", "blank"),
        (
            "(1000.0, Fixture(\"inline fixture\"))",
            "Fixture provenance is test-only",
        ),
    ] {
        let error = Scenario::parse(&format!("(density_kg_m3: {value},)"))
            .unwrap_err()
            .to_string();
        assert!(
            error.contains(FILE) && error.contains("density_kg_m3") && error.contains(cause),
            "{error}"
        );
    }
}
