//! View-only MG tracer streak maintenance.
//!
//! Invariant: spawn and maintenance derive the same clamped transform from distance since the
//! muzzle or latest ricochet.

use bevy::prelude::*;

use crate::ballistics::{PenetrationMarks, ShellPath, TracerStreak};

pub(super) fn plugin(app: &mut App) {
    app.add_systems(Update, clamp_tracer_streaks);
}

/// Re-derive each tracer streak child from the round's distance-flown-since-anchor (muzzle or last
/// ricochet), so the head stays on the round and the tail stops at the anchor.
fn clamp_tracer_streaks(
    // The projectiles carrying a streak; `Without<TracerStreak>` keeps this disjoint from the child
    // transform query below (the parent has no streak marker, the child does).
    projectiles: Query<
        (&Transform, &ShellPath, &PenetrationMarks, &Children),
        Without<TracerStreak>,
    >,
    mut streaks: Query<(&TracerStreak, &mut Transform), With<TracerStreak>>,
) {
    for (proj, path, marks, children) in &projectiles {
        // Anchor: the most recent ricochet if the round has bounced, else the muzzle (the first
        // recorded path point). No anchor (empty path) → nothing to clamp against.
        let Some(anchor) = marks
            .ricochets
            .last()
            .copied()
            .or_else(|| path.points.first().copied())
        else {
            continue;
        };
        let flown = proj.translation.distance(anchor);
        for &child in children {
            if let Ok((streak, mut transform)) = streaks.get_mut(child) {
                // The SAME derivation the spawn seeded the child with — one definition, so the two
                // can't drift apart (see [`TracerStreak::drawn_transform`]).
                *transform = streak.drawn_transform(flown);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::ballistics::{FireShell, FireShellOrigin};
    use avian3d::prelude::*;
    use bevy::asset::AssetPlugin;
    use bevy::time::TimeUpdateStrategy;
    use std::collections::BTreeSet;
    use std::time::Duration;

    /// The MG tracer round both spawn paths fire (7.9 mm, 11.8 g, 755 m/s).
    const MUZZLE: Vec3 = Vec3::new(0.0, 2.0, 0.0);

    fn fire_shell(catch_up: u32, shot: Option<crate::ShotId>) -> FireShell {
        FireShell {
            origin: MUZZLE,
            direction: Dir3::NEG_Z,
            speed: 755.0,
            caliber: 0.0079,
            mass: 0.0118,
            mechanism: crate::spec::FireMechanism::Automatic,
            tracer: true,
            // Identity, not authority — both paths name their shooter (the coax self-exclusion).
            shooter: Some(crate::ballistics::ShotSource {
                tank: Entity::PLACEHOLDER,
                weapon: 0,
            }),
            shot_origin: if shot.is_some() {
                FireShellOrigin::Reconstructed
            } else {
                FireShellOrigin::Local
            },
            catch_up_ticks: catch_up,
            shot,
        }
    }

    /// A shell-spawning world running the REAL `ballistics` observer and the REAL streak maintainer.
    fn world() -> App {
        let mut app = App::new();
        app.add_plugins((
            MinimalPlugins,
            AssetPlugin::default(),
            PhysicsPlugins::default(),
        ))
        .init_asset::<Mesh>()
        .init_asset::<StandardMaterial>()
        .init_asset::<bevy::world_serialization::WorldAsset>()
        .insert_resource(TimeUpdateStrategy::ManualDuration(Duration::from_millis(
            16,
        )))
        .add_plugins(crate::ballistics::plugin)
        .add_plugins(plugin);
        while app.plugins_state() == bevy::app::PluginsState::Adding {
            std::thread::sleep(Duration::from_millis(1));
        }
        app.finish();
        app.cleanup();
        app
    }

    /// The two spawn SCHEDULES, which is the axis the bug lived on: `shooting::fire` raises `FireShell`
    /// from `FixedUpdate` (a locally-fired round), `net::client::receive_fire_events` from `Update` (a
    /// net observer's). Both flush their `trigger` at their own schedule's command flush.
    #[derive(Clone, Copy, PartialEq, Debug)]
    enum Born {
        Local,
        Remote,
    }

    #[derive(Resource)]
    struct Pending(Option<FireShell>);

    fn raise_pending(mut pending: ResMut<Pending>, mut commands: Commands) {
        if let Some(fire) = pending.0.take() {
            commands.trigger(fire);
        }
    }

    fn spawn_via(app: &mut App, born: Born, fire: FireShell) {
        app.insert_resource(Pending(Some(fire)));
        match born {
            Born::Local => app.add_systems(FixedUpdate, raise_pending),
            Born::Remote => app.add_systems(Update, raise_pending),
        };
    }

    /// The streak's tail offset along the bore from the muzzle, at the END of a frame — i.e. exactly the
    /// geometry the renderer draws that frame. Negative ⇒ the tail pokes BEHIND the muzzle.
    fn tail_offset_from_muzzle(app: &mut App) -> Option<f32> {
        let mut q = app
            .world_mut()
            .query_filtered::<(&Transform, &ShellPath, &Children), Without<TracerStreak>>();
        let (tf, path, children) = q.iter(app.world()).next()?;
        let muzzle = *path.points.first()?;
        let child = *children.first()?;
        let len = app.world().get::<Transform>(child)?.scale.y;
        let travel = (tf.rotation * Vec3::NEG_Z).normalize();
        let tail = tf.translation - travel * len;
        Some((tail - muzzle).dot(travel))
    }

    /// Regression: every spawn schedule must seed a streak whose tail is not behind the muzzle.
    #[test]
    fn a_streak_never_pokes_behind_the_muzzle_from_either_spawn_path() {
        for born in [Born::Local, Born::Remote] {
            // 0 and 1 ticks are the catch-ups that put the round INSIDE the streak's nominal length,
            // where an unclamped seed drags the tail back through the turret; 4 and 10 are the routine
            // net catch-ups (`fast_forward_shell`), where the round is already far downrange.
            for catch_up in [0u32, 1, 4, 10] {
                let mut app = world();
                spawn_via(&mut app, born, fire_shell(catch_up, None));
                for frame in 0..5 {
                    app.update();
                    let Some(tail) = tail_offset_from_muzzle(&mut app) else {
                        continue; // not born yet
                    };
                    assert!(
                        tail >= -1.0e-3,
                        "{born:?} shell (catch_up {catch_up}) drew its streak tail {:.2} m BEHIND the \
                         muzzle on frame {frame} — back through the shooter's turret",
                        -tail,
                    );
                }
            }
        }
    }

    /// The clamp is only ever a SHORTENING: once the round has flown past `nominal_len` the full streak
    /// draws, and it re-anchors at a ricochet so the tail stops at the bounce, not the muzzle.
    #[test]
    fn streak_clamps_to_flown_distance() {
        let mut app = App::new();
        app.add_systems(Update, clamp_tracer_streaks);

        // A round 3 m past the muzzle with a 13 m nominal streak.
        let child_spawn = |len: f32| {
            (
                TracerStreak { nominal_len: 13.0 },
                Transform {
                    translation: Vec3::Z * (len * 0.5),
                    scale: Vec3::new(1.0, len, 1.0),
                    ..default()
                },
            )
        };
        let near = app
            .world_mut()
            .spawn((
                Transform::from_translation(Vec3::new(3.0, 0.0, 0.0)),
                ShellPath {
                    points: vec![Vec3::ZERO],
                    segment_starts: Vec::new(),
                },
                PenetrationMarks::default(),
            ))
            .with_child(child_spawn(13.0))
            .id();
        app.update();

        let child = app.world().get::<Children>(near).expect("streak child")[0];
        let tf = app
            .world()
            .get::<Transform>(child)
            .expect("child transform");
        assert!(
            (tf.scale.y - 3.0).abs() < 1.0e-4,
            "streak clamps to the 3 m flown, not its 13 m nominal (got {})",
            tf.scale.y
        );
        assert!(
            (tf.translation.z - 1.5).abs() < 1.0e-4,
            "the trailing offset re-centers to len/2"
        );

        // A ricochet resets the anchor: a round 2 m past its last bounce clamps to 2 m.
        let bounced = app
            .world_mut()
            .spawn((
                Transform::from_translation(Vec3::new(50.0, 0.0, 0.0)),
                ShellPath {
                    points: vec![Vec3::ZERO, Vec3::new(48.0, 0.0, 0.0)],
                    segment_starts: Vec::new(),
                },
                PenetrationMarks {
                    ricochets: vec![Vec3::new(48.0, 0.0, 0.0)],
                    ..default()
                },
            ))
            .with_child(child_spawn(13.0))
            .id();
        app.update();
        let child = app.world().get::<Children>(bounced).expect("streak child")[0];
        let tf = app
            .world()
            .get::<Transform>(child)
            .expect("child transform");
        assert!(
            (tf.scale.y - 2.0).abs() < 1.0e-4,
            "after a ricochet the streak clamps to distance since the bounce (got {})",
            tf.scale.y
        );

        // A round well downrange (30 m flown) draws the full 13 m streak — the clamp is a no-op.
        let far = app
            .world_mut()
            .spawn((
                Transform::from_translation(Vec3::new(30.0, 0.0, 0.0)),
                ShellPath {
                    points: vec![Vec3::ZERO],
                    segment_starts: Vec::new(),
                },
                PenetrationMarks::default(),
            ))
            .with_child(child_spawn(13.0))
            .id();
        app.update();
        let child = app.world().get::<Children>(far).expect("streak child")[0];
        let tf = app
            .world()
            .get::<Transform>(child)
            .expect("child transform");
        assert!(
            (tf.scale.y - 13.0).abs() < 1.0e-4,
            "past nominal length the full streak shows (got {})",
            tf.scale.y
        );
    }

    fn component_names(app: &App, entity: Entity) -> BTreeSet<String> {
        app.world()
            .inspect_entity(entity)
            .expect("entity exists")
            .map(|info| info.name().to_string())
            .collect()
    }

    fn only_shell(app: &mut App) -> (Entity, Entity) {
        let mut q = app
            .world_mut()
            .query_filtered::<(Entity, &Children), Without<TracerStreak>>();
        let (shell, children) = q.iter(app.world()).next().expect("a shell was spawned");
        (shell, children[0])
    }

    /// CLASS GUARD: the two spawn paths must stay compositionally IDENTICAL wherever the view layer is
    /// concerned. Both a locally-fired round and a net observer's cosmetic round are dressed by the one
    /// `ballistics::on_fire_shell` observer, so every view system's query matches both — and this pins
    /// that, so a component added to one path alone (the failure mode that would silently make a view
    /// system skip remote shells entirely) fails here instead of in a playtest.
    ///
    /// The ONE sanctioned difference is [`ballistics::Shot`], the shell's network identity: both
    /// local and reconstructed network shells carry it at spawn. It is correlation, not view state — nothing
    /// in `vfx` reads it. Damage authority is NOT a component difference at all: it is gated on the
    /// `ClientReplica` RESOURCE, so it cannot skew the shell's composition.
    #[test]
    fn local_and_remote_shells_are_compositionally_identical_to_the_view() {
        let shot = crate::ShotId {
            combatant: crate::CombatantId(1),
            weapon: 0,
            fire_tick: 100,
        };

        let mut local = world();
        spawn_via(&mut local, Born::Local, fire_shell(0, None));
        let mut remote = world();
        spawn_via(&mut remote, Born::Remote, fire_shell(6, Some(shot)));
        for _ in 0..3 {
            local.update();
            remote.update();
        }

        let (local_shell, local_streak) = only_shell(&mut local);
        let (remote_shell, remote_streak) = only_shell(&mut remote);

        // The network identity an observer's shell carries at spawn (see the doc above) — the only
        // sanctioned asymmetry, and deliberately not a view component.
        let identity = "overmatch::ballistics::Shot".to_string();
        let mut local_components = component_names(&local, local_shell);
        let mut remote_components = component_names(&remote, remote_shell);
        assert!(
            remote_components.remove(&identity),
            "an observer's shell carries its wire `Shot` identity",
        );
        local_components.remove(&identity);

        assert_eq!(
            local_components, remote_components,
            "local and remote shells diverged in composition — every view system queries both, so a \
             component on only one path silently makes that system skip the other (this is exactly \
             how the tracer clamp came to skip remote shells)",
        );
        assert_eq!(
            component_names(&local, local_streak),
            component_names(&remote, remote_streak),
            "the tracer streak CHILD diverged between the two spawn paths",
        );
    }
}
