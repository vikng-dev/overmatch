//! Shared controlled-tank drive HUD for the offline and server-stream clients.
//!
//! This is view state only: readouts use server-stream components and the gauge uses the
//! client's effective lever output in `Update`. It owns one local F3
//! toggle, and writes only UI text, nodes and visibility. It never writes a tank component or enters a
//! fixed schedule.

use avian3d::prelude::{LinearVelocity, Rotation};
use bevy::prelude::*;

use crate::flow::{Flow, FlowScoped, InMatchSystems, MatchResourceExt};
use crate::tank::Controlled;
use crate::track::sim::{DriveSignal, TankTransmission, TrackDrive, TrackGear};
use crate::track::transmission::{DriveReadout, TransmissionState, readout};
use crate::ui_font::{UiFonts, set_text};

/// HUD look is authored data; positions and colours are tastes, never drivetrain parameters.
#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct HudLook {
    standard_font_px: crate::provenance::Sourced<f32>,
    debug_font_px: crate::provenance::Sourced<f32>,
    standard_bottom_px: crate::provenance::Sourced<f32>,
    left_px: crate::provenance::Sourced<f32>,
    debug_top_px: crate::provenance::Sourced<f32>,
    debug_right_px: crate::provenance::Sourced<f32>,
    standard_rgb: crate::provenance::Sourced<[f32; 3]>,
    debug_rgb: crate::provenance::Sourced<[f32; 3]>,
    gauge_above_row_gap_px: crate::provenance::Sourced<f32>,
    gauge_width_px: crate::provenance::Sourced<f32>,
    gauge_height_px: crate::provenance::Sourced<f32>,
    gauge_label_px: crate::provenance::Sourced<f32>,
    gauge_gap_px: crate::provenance::Sourced<f32>,
    rail_px: crate::provenance::Sourced<f32>,
    mark_px: crate::provenance::Sourced<f32>,
}

fn hud_look() -> &'static HudLook {
    static LOOK: std::sync::LazyLock<HudLook> = std::sync::LazyLock::new(|| {
        let look: HudLook = crate::provenance::from_shipped_str(
            "assets/input/drive_hud.ron",
            include_str!("../assets/input/drive_hud.ron"),
        )
        .expect(
            "assets/input/drive_hud.ron: author all HUD dimensions and colours with provenance",
        );
        for (field, value) in [
            ("standard_font_px", look.standard_font_px.0),
            ("debug_font_px", look.debug_font_px.0),
            ("standard_bottom_px", look.standard_bottom_px.0),
            ("left_px", look.left_px.0),
            ("debug_top_px", look.debug_top_px.0),
            ("debug_right_px", look.debug_right_px.0),
            ("gauge_above_row_gap_px", look.gauge_above_row_gap_px.0),
            ("gauge_width_px", look.gauge_width_px.0),
            ("gauge_height_px", look.gauge_height_px.0),
            ("gauge_label_px", look.gauge_label_px.0),
            ("gauge_gap_px", look.gauge_gap_px.0),
            ("rail_px", look.rail_px.0),
            ("mark_px", look.mark_px.0),
        ] {
            assert!(
                value.is_finite() && value > 0.0,
                "assets/input/drive_hud.ron: {field} must be finite and positive; author a HUD dimension"
            );
        }
        assert!(
            look.mark_px.0 <= look.gauge_width_px.0 && look.rail_px.0 <= look.gauge_height_px.0,
            "assets/input/drive_hud.ron: mark_px and rail_px must fit gauge_width_px and gauge_height_px; shrink the marks or enlarge the gauge"
        );
        for (field, rgb) in [
            ("standard_rgb", look.standard_rgb.0),
            ("debug_rgb", look.debug_rgb.0),
        ] {
            assert!(
                rgb.into_iter()
                    .all(|v| v.is_finite() && (0.0..=1.0).contains(&v)),
                "assets/input/drive_hud.ron: {field} must contain finite channels in [0, 1]; author an sRGB colour"
            );
        }
        look
    });
    &LOOK
}

#[derive(Component)]
struct DriveNotchGauge;

#[derive(Component, Clone, Copy)]
enum NotchMark {
    Throttle,
    Steer,
}

/// The signed bounded axis maps onto the usable rail, keeping the full mark inside both ends.
fn notch_left(value: f32) -> f32 {
    let look = hud_look();
    (value + 1.0) / 2.0 * (look.gauge_width_px.0 - look.mark_px.0)
}

/// The normal-play drive row. One instance is mounted by this plugin in both client roots.
#[derive(Component)]
struct StandardDriveHudText;

/// The detailed drive panel controlled by [`DriveDebugVisible`].
#[derive(Component)]
struct DriveDebugHudText;

/// View-local F3 latch. It is deliberately neither a component on the tank nor replicated state.
#[derive(Resource, Default)]
struct DriveDebugVisible(bool);

pub(crate) fn plugin(app: &mut App) {
    crate::flow::require(app, "drive_hud");
    hud_look();
    app.init_match_resource::<DriveDebugVisible>()
        .add_systems(OnEnter(Flow::InMatch), spawn_drive_hud)
        .add_systems(
            Update,
            (toggle_drive_debug, update_drive_hud, update_notch_gauge)
                .chain()
                .in_set(InMatchSystems),
        );
}

/// Mount the real fixed panels without their game-state readers for HUD layout tests.
#[cfg(test)]
pub(crate) fn mount_layout_test(app: &mut App) {
    app.add_systems(Startup, spawn_drive_hud);
}

fn spawn_drive_hud(mut commands: Commands, fonts: Res<UiFonts>) {
    let look = hud_look();
    let [r, g, b] = look.standard_rgb.0;
    let standard_color = Color::srgb(r, g, b);
    let [r, g, b] = look.debug_rgb.0;
    let debug_color = Color::srgb(r, g, b);
    // Bottom-left, one line above `crew_ui`'s crew row (bottom 10 px). This is ordinary HUD, not an
    // overlay: its fixed-HUD layer sits below the overlay authority's layers, and its dock
    // reserves space for modal HUD content.
    commands.spawn((
        FlowScoped(Flow::InMatch),
        StandardDriveHudText,
        crate::hud::HudDock::Fixed,
        Text::new(""),
        TextFont {
            font: fonts.body.clone().into(),
            font_size: FontSize::Px(look.standard_font_px.0),
            ..default()
        },
        TextColor(standard_color),
        Node {
            position_type: PositionType::Absolute,
            bottom: Val::Px(look.standard_bottom_px.0),
            left: Val::Px(look.left_px.0),
            ..default()
        },
        Visibility::Hidden,
    ));

    // Diagnostic block: top-right, bare text, ordinary non-interactive HUD (never an overlay/scrim).
    commands.spawn((
        FlowScoped(Flow::InMatch),
        DriveDebugHudText,
        crate::hud::HudDock::Fixed,
        Text::new(""),
        TextFont {
            font: fonts.body.clone().into(),
            font_size: FontSize::Px(look.debug_font_px.0),
            ..default()
        },
        TextColor(debug_color),
        Node {
            position_type: PositionType::Absolute,
            top: Val::Px(look.debug_top_px.0),
            right: Val::Px(look.debug_right_px.0),
            ..default()
        },
        Visibility::Hidden,
    ));
    commands
        .spawn((
            FlowScoped(Flow::InMatch),
            DriveNotchGauge,
            crate::hud::HudDock::Fixed,
            Node {
                position_type: PositionType::Absolute,
                left: Val::Px(look.left_px.0),
                bottom: Val::Px(
                    look.standard_bottom_px.0
                        + match bevy::text::LineHeight::default() {
                            bevy::text::LineHeight::Px(px) => px,
                            bevy::text::LineHeight::RelativeToFont(scale) => {
                                scale * look.standard_font_px.0
                            }
                        }
                        + look.gauge_above_row_gap_px.0,
                ),
                flex_direction: FlexDirection::Column,
                row_gap: Val::Px(look.gauge_gap_px.0),
                ..default()
            },
            Visibility::Hidden,
        ))
        .with_children(|gauge| {
            for (label, axis) in [("THR", NotchMark::Throttle), ("STEER", NotchMark::Steer)] {
                gauge
                    .spawn(Node {
                        align_items: AlignItems::Center,
                        ..default()
                    })
                    .with_children(|row| {
                        row.spawn((
                            Text::new(label),
                            TextFont {
                                font: fonts.body.clone().into(),
                                font_size: FontSize::Px(look.debug_font_px.0),
                                ..default()
                            },
                            TextColor(standard_color),
                            Node {
                                width: Val::Px(look.gauge_label_px.0),
                                ..default()
                            },
                        ));
                        row.spawn(Node {
                            width: Val::Px(look.gauge_width_px.0),
                            height: Val::Px(look.gauge_height_px.0),
                            ..default()
                        })
                        .with_children(|rail| {
                            rail.spawn((
                                Node {
                                    position_type: PositionType::Absolute,
                                    top: Val::Px((look.gauge_height_px.0 - look.rail_px.0) / 2.0),
                                    width: Val::Px(look.gauge_width_px.0),
                                    height: Val::Px(look.rail_px.0),
                                    ..default()
                                },
                                BackgroundColor(debug_color),
                            ));
                            // The neutral reference and the live notch are shapes, not font glyphs.
                            rail.spawn((
                                Node {
                                    position_type: PositionType::Absolute,
                                    left: Val::Px((look.gauge_width_px.0 - look.rail_px.0) / 2.0),
                                    width: Val::Px(look.rail_px.0),
                                    height: Val::Px(look.gauge_height_px.0),
                                    ..default()
                                },
                                BackgroundColor(debug_color),
                            ));
                            rail.spawn((
                                axis,
                                Node {
                                    position_type: PositionType::Absolute,
                                    left: Val::Px(notch_left(0.0)),
                                    width: Val::Px(look.mark_px.0),
                                    height: Val::Px(look.gauge_height_px.0),
                                    ..default()
                                },
                                BackgroundColor(standard_color),
                            ));
                        });
                    });
            }
        });
}

/// Pure toggle transition used by the input system and the offline HUD regression tests.
pub(crate) const fn debug_visible_after_f3(current: bool, f3_just_pressed: bool) -> bool {
    if f3_just_pressed { !current } else { current }
}

fn toggle_drive_debug(keys: Res<ButtonInput<KeyCode>>, mut visible: ResMut<DriveDebugVisible>) {
    visible.0 = debug_visible_after_f3(visible.0, keys.just_pressed(KeyCode::F3));
}

/// Compact normal-play row: `P` is the parking lever.
/// A paid shift has no extra marker. Before construction is ready only speed is shown.
pub(crate) fn standard_drive_row(
    transmission: Option<(&TransmissionState, &DriveReadout)>,
    ground_speed_mps: f32,
) -> String {
    let speed = format!("Speed {:>3.0} km/h", ground_speed_mps * 3.6);
    let Some((state, operating)) = transmission else {
        return speed;
    };

    let mut gear = operating.gear_label.clone();
    if state.park {
        gear.push('P');
    }
    let rpm = format!("{:>3.1}k", operating.rpm / 1_000.0);
    // Display rounding must not give zero a direction.
    let rpm = if rpm == "-0.0k" { "0.0k" } else { &rpm };
    format!("Gear {gear:<4}  RPM {rpm}  {speed}")
}

/// Horizontal world-space velocity magnitude. Vertical motion and hull facing are intentionally
/// excluded: the normal HUD promises ground speed, not signed forward speed.
pub(crate) fn horizontal_ground_speed(velocity: Vec3) -> f32 {
    Vec3::new(velocity.x, 0.0, velocity.z).length()
}

/// Fixed-radius steering visibility from the live detent state and the source radius table retained
/// by [`TransmissionParams`](crate::track::transmission::TransmissionParams).
pub(crate) fn steering_hud_line(
    st: &TransmissionState,
    params: Option<&crate::track::transmission::TransmissionParams>,
) -> String {
    if st.steer_step == 0 {
        return String::new();
    }
    let Some(params) = params else {
        return String::new(); // No running gear is built yet: no steering table to read.
    };
    let (tight, wide) = params.steering_radii(st.reverse, st.gear);
    let (detent, radius) = match st.steer_step {
        1 => ("I", wide),
        2 => ("II", tight),
        _ => panic!("the transmission admits only straight, wide and tight detents"),
    };
    format!("STEER {detent} R~{radius:.0}m")
}

/// Rebuild both shared HUD surfaces from the controlled tank's server-stream components.
/// The standard row is always visible once a controlled body exists; detailed formatting is skipped
/// entirely while F3 is closed.
fn update_drive_hud(
    gear: Option<Res<TrackGear>>,
    debug_visible: Res<DriveDebugVisible>,
    controlled: Query<
        (
            &TrackDrive,
            &DriveSignal,
            &TankTransmission,
            &LinearVelocity,
            &Rotation,
        ),
        With<Controlled>,
    >,
    mut standard_label: Query<
        (&mut Text, &mut Visibility),
        (With<StandardDriveHudText>, Without<DriveDebugHudText>),
    >,
    mut debug_label: Query<
        (&mut Text, &mut Visibility),
        (With<DriveDebugHudText>, Without<StandardDriveHudText>),
    >,
) {
    let (mut standard_text, mut standard_visibility) = standard_label
        .single_mut()
        .expect("the drive HUD mounts exactly one standard row for this match");
    let (mut debug_text, mut debug_visibility) = debug_label
        .single_mut()
        .expect("the drive HUD mounts exactly one debug row for this match");
    let Some((drive, signal, transmission, velocity, rotation)) =
        crate::tank::controlled_one(controlled.single())
    else {
        set_text(standard_text.reborrow(), "");
        set_text(debug_text.reborrow(), "");
        standard_visibility.set_if_neq(Visibility::Hidden);
        debug_visibility.set_if_neq(Visibility::Hidden);
        return;
    };

    let params = gear.as_deref().map(TrackGear::trans);
    let operating = params.map(|params| readout(&transmission.0, params));
    let standard_operating = operating
        .as_ref()
        .map(|operating| (&transmission.0, operating));
    let ground_speed = horizontal_ground_speed(velocity.0);
    set_text(
        standard_text.reborrow(),
        &standard_drive_row(standard_operating, ground_speed),
    );
    standard_visibility.set_if_neq(Visibility::Visible);

    if !debug_visible.0 {
        debug_visibility.set_if_neq(Visibility::Hidden);
        return;
    }
    debug_visibility.set_if_neq(Visibility::Visible);

    // The debug panel preserves the former offline diagnostics: signed hull-forward speed, per-belt
    // speed/slip, shaped commands, and the authored L600 steering detent/radius.
    let forward = rotation.0 * Vec3::NEG_Z;
    let forward_horizontal = Vec3::new(forward.x, 0.0, forward.z).normalize_or_zero();
    let horizontal = Vec3::new(velocity.0.x, 0.0, velocity.0.z);
    let projected = horizontal.dot(forward_horizontal);
    let signed_ground = horizontal.length() * projected.signum();
    let speeds = [drive.sides[0].speed, drive.sides[1].speed];
    let steering = steering_hud_line(&transmission.0, params);

    let debug = format!(
        "hull {signed_ground:+5.2} m/s ({:+6.1} km/h)\n\
         belt L {:+5.2} R {:+5.2} | slip L {:+5.2} R {:+5.2}\n\
         cmd thr {:+5.2} steer {:+5.2} | {steering:<15}",
        signed_ground * 3.6,
        speeds[0],
        speeds[1],
        speeds[0] - projected,
        speeds[1] - projected,
        signal.throttle,
        signal.steer,
    );
    set_text(debug_text.reborrow(), &debug);
}

/// A retained notch is local input feedback. Hide the gauge during spawn/possession gaps and the
/// death flow; the client input state is reset there rather than inherited by another body.
fn update_notch_gauge(
    input: Res<crate::command::DriveInput>,
    controlled: Query<Has<crate::damage::TankKnockedOut>, With<Controlled>>,
    mut gauges: Query<&mut Visibility, With<DriveNotchGauge>>,
    mut marks: Query<(&NotchMark, &mut Node)>,
) {
    let visible = crate::tank::controlled_one(controlled.single()).is_some_and(|dead| !dead);
    for mut visibility in &mut gauges {
        visibility.set_if_neq(if visible {
            Visibility::Visible
        } else {
            Visibility::Hidden
        });
    }
    let notches = input.notches();
    for (axis, mut node) in &mut marks {
        let value = match axis {
            NotchMark::Throttle => notches.throttle,
            NotchMark::Steer => notches.steer,
        };
        let left = Val::Px(notch_left(value));
        if node.left != left {
            node.left = left;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::ecs::system::RunSystemOnce;

    /// Back-driven rotation still needs sound and a tachometer reading. Public audio consumes
    /// magnitude, while the owner row preserves the signed crank state.
    #[test]
    fn backward_crank_keeps_public_rotation_and_signed_hud_rpm() {
        let spec: crate::spec::TankSpec = crate::provenance::from_shipped_str(
            crate::tank::TANK_SPEC_PATH,
            include_str!("../assets/tiger_1/tiger_1.tank.ron"),
        )
        .unwrap();
        let tp = spec
            .track
            .nominal_transmission_params(&spec.roadwheels)
            .unwrap();
        // Fixture: backward rotations covering visible rpm and values rounding to zero.
        for rpm in [-3100.0_f32, -40.0, -0.0] {
            let state = TransmissionState {
                omega_e: rpm * crate::track::transmission::RPM_TO_RAD,
                ..TransmissionState::stopped()
            };
            let public = crate::track::sim::EngineRpm::of(&TankTransmission(state));
            let operating = crate::track::transmission::readout(&state, &tp);
            let row = standard_drive_row(Some((&state, &operating)), 0.0);
            assert_eq!(public.rpm(), operating.rpm.abs());
            assert!(!row.contains("-0.0k"));
            if rpm == -3100.0 {
                assert!(row.contains("-3.1k"));
            }
        }
    }

    /// Players need both signed lever positions, including neutral and both bounds. The marks
    /// stay inside the rails and follow independent axes through the real gather and HUD systems.
    #[test]
    fn gauge_marks_follow_the_two_local_notches_and_hide_without_a_body() {
        let mut world = World::new();
        world.insert_resource(UiFonts {
            hud: Handle::default(),
            body: Handle::default(),
        });
        world.insert_resource(crate::command::Bindings::default());
        world.insert_resource(crate::command::DriveInput::default());
        world.insert_resource(crate::firecontrol::Ranging::default());
        world.insert_resource(Time::<Real>::default());
        world.insert_resource(ButtonInput::<KeyCode>::default());
        world.insert_resource(ButtonInput::<MouseButton>::default());
        world.run_system_once(spawn_drive_hud).unwrap();
        let tank = world
            .spawn((Controlled, crate::command::TankCommand::default()))
            .id();
        for key in [KeyCode::KeyW, KeyCode::KeyA] {
            world.resource_mut::<ButtonInput<KeyCode>>().press(key);
            world.resource_mut::<ButtonInput<KeyCode>>().release(key);
        }
        world
            .run_system_once(crate::command::gather_commands)
            .unwrap();
        world.run_system_once(update_notch_gauge).unwrap();
        let mut marks = world.query::<(&NotchMark, &Node)>();
        assert_eq!(marks.iter(&world).count(), 2);
        for (axis, node) in marks.iter(&world) {
            let expected = match axis {
                NotchMark::Throttle => crate::track::drive::drive_feel().notch_step.0,
                NotchMark::Steer => -crate::track::drive::drive_feel().notch_step.0,
            }; // Fixture tap.
            assert_eq!(node.left, Val::Px(notch_left(expected)));
        }
        assert_eq!(notch_left(-1.0), 0.0);
        let span = hud_look().gauge_width_px.0 - hud_look().mark_px.0;
        assert_eq!(notch_left(0.0), span / 2.0);
        assert_eq!(notch_left(1.0), span);
        let mut gauge = world.query_filtered::<&Visibility, With<DriveNotchGauge>>();
        assert_eq!(*gauge.single(&world).unwrap(), Visibility::Visible);
        world.entity_mut(tank).remove::<Controlled>();
        world.run_system_once(update_notch_gauge).unwrap();
        assert_eq!(*gauge.single(&world).unwrap(), Visibility::Hidden);
    }
}
