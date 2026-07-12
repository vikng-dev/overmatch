//! MG tracer streak origin clamp (view-only). The sim spawns each tracer round a fixed-length
//! capsule streak ([`ballistics::TracerStreak`], ~13 m at the MG's 755 m/s), whose tail trails the
//! round. At the spawn instant — and again right after a ricochet — the round has barely moved, so a
//! full-length tail extends BACKWARD through the muzzle/turret (or back past the bounce point). This
//! system clamps the drawn streak to the distance the round has actually flown since its last anchor
//! (the muzzle, or the most recent ricochet), so the tail can never poke behind where the round came
//! from. Past `nominal_len` of travel the clamp is a no-op and the full streak shows.
//!
//! All data is already on the sim shell — [`ShellPath`] (`points[0]` = muzzle) and
//! [`PenetrationMarks`] (`ricochets`) — so this reads sim state and writes only the cosmetic child's
//! `Transform` (ADR-0014). Client-mounted with the rest of `vfx`; the headless server never runs it.

use bevy::prelude::*;

use crate::ballistics::{PenetrationMarks, ShellPath, TracerStreak};

pub(super) fn plugin(app: &mut App) {
    app.add_systems(Update, clamp_tracer_streaks);
}

/// Shorten each tracer streak child to the round's distance-flown-since-anchor (muzzle or last
/// ricochet), adjusting scale.y and the trailing offset together so the head stays on the round and
/// the tail stops at the anchor.
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
                // Clamp to the flown distance; the streak axis is the child's local +Z (tail
                // trailing), so scale.y is the length and +Z·(len/2) re-centers it.
                let len = streak.nominal_len.min(flown).max(0.0);
                transform.scale.y = len;
                transform.translation = Vec3::Z * (len * 0.5);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A streak whose round has barely left the muzzle is clamped SHORT (to the flown distance), so
    /// its tail can't reach back through the turret; once the round has flown past its nominal
    /// length the streak is drawn full.
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
}
