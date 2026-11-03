//! Inline driving fixture shared by arithmetic and spec/contact composition tests.

/// Fixture: a lighter three-speed construction using the seed's declared running-gear geometry.
/// Its engine has a tail past the governor, lower power/weight and a different recirculation path.
pub(crate) fn compact_spec() -> crate::spec::TankSpec {
    let mut spec = super::rig_geom::tiger_spec();
    spec.mass = 18_000.0;
    let tr = &mut spec.track.powertrain.transmission;
    tr.engine.idle_rpm = 700.0;
    tr.engine.governed_rpm = 2200.0;
    tr.engine.crank_limit_rpm = crate::provenance::Sourced(2600.0); // Fixture limit.
    tr.engine.rated_rpm = 2200.0;
    tr.engine.torque_curve = vec![
        (700.0, 600.0),
        (1400.0, 800.0),
        (2200.0, 500.0),
        (2600.0, 400.0),
    ];
    tr.engine.drag_fraction = 0.3;
    tr.engine.inertia_kgm2 = 2.0;
    tr.engine.clutch_capacity_nm = 1100.0;
    tr.gearbox.forward_speeds_kmh = vec![5.0, 12.0, 25.0];
    tr.gearbox.reverse_speeds_kmh = vec![10.0];
    tr.gearbox.launch_gears = 2;
    tr.gearbox.max_step = 1;
    tr.gearbox.shift_secs = 0.2;
    tr.steering.radii = vec![(3.0, 8.0), (7.2, 19.2), (15.0, 40.0)];
    tr.steering.reverse_radii = vec![(6.0, 16.0)];
    tr.steering.capacity = 80_000.0;
    tr.steering.recirculation = 0.65;
    tr.brake_force = 50_000.0;
    tr.brake_static_factor = 1.4;
    spec
}

/// Fixture: a firmer loaded surface with a friction coefficient distinct from the lab's.
pub(crate) fn ground() -> crate::ground_field::ContactGround {
    crate::ground_field::ContactGround {
        friction: 0.65,
        rolling_resistance: 0.04,
    }
}
