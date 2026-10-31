//! The victim kill cam's law: what the struck player sees when a record knocks their own tank out,
//! which the game's driver ([`super::game`]'s kill cam) plays on the real clock. Four beats,
//! counted from the killing shot's world impact ([`Beats`]):
//!
//! 1. **Hit** — the player's own orbit view, free, the world showing the hit in real time.
//! 2. **Lesson** — captive: the look eases to a three-quarter from the shot's side; the main camera
//!    swaps to [`crate::render_policy::CameraProfile::KillCamLesson`], so the tank it rides leaves the view and the pen
//!    cam stage, which sits on it in the record's frame, draws the record over the live world at
//!    [`LESSON_RATE`] of the corner's tempo. It runs until the record has settled and held
//!    ([`lesson_length`]; `demo/ux-spec.md` §3, "Time"). The world clock is untouched.
//! 3. **Killer** — captive: back on the victim's own orbit at the play radius, the look turns
//!    toward the shooter and tracks it, and the killer card shows name, vehicle, range and round.
//! 4. **Death** — free: the orbit about the victim, opening on the shooter where beat 3 played,
//!    with the death tint under the UI, the death panel and the respawn prompt.
//!
//! The kill cam never shows more of the enemy than the victim could see from where they died:
//! every pose orbits the victim's own pivot at the game's play radius, with no zoom, so beat 3
//! frames the shooter from the victim's place alone.
//!
//! Beat 3 plays only for a shooter the driver stages ([`Beats::shooter`]); without one the death
//! screen follows the lesson with the killer card showing what the record names.
//!
//! Respawn is live from [`HIT_S`] after the impact. Every pose is the game's own orbit
//! ([`crate::camera::orbit_rotation`], [`crate::camera::orbit_body`],
//! [`crate::camera::orbit_pivot`]) about the victim's pivot. Plan and owner record:
//! `.agents/scratch/damage-cam-2026-10-08/demo/killcam-plan.md`; the shooter framing's open
//! alternative: `.agents/scratch/playtest-forks/killcam-shooter-framing.md`. Every duration, angle
//! and colour here is TUNED (a look-test choice).

use bevy::prelude::*;

use super::plan::Plan;
use super::stage;
use super::timeline::{self, CornerKind};
use super::{Showing, Surface};
use crate::overlay::Overlay;
use crate::spec::TankSpec;
use crate::ui_font::{PANEL_BG, TEXT, TEXT_DIM, UiFonts, drawable};

/// Beat 1 runs this long after the world impact; respawn is live from its end.
pub const HIT_S: f32 = 0.5;
/// The look eases into the lesson's bearing over this; every camera move here runs rest to rest.
const MOVE_S: f32 = 0.6;
/// The lesson's shortest length (real seconds).
pub const LESSON_S: f32 = 3.6;
/// The record's tempo in the lesson, as a fraction of the corner's.
pub const LESSON_RATE: f32 = 0.5;
/// The lesson holds on the settled record this long (real seconds) before it ends.
pub const LESSON_HOLD_S: f32 = 1.0;
/// Beat 3's length: the look's turn toward the shooter over [`MOVE_S`], then the track.
const KILLER_S: f32 = 3.0;
/// The lesson's bearing: yawed off the shot's side, the body above the pivot by this slope (the
/// corner's three-quarter).
const LESSON_YAW_DEG: f32 = 55.0;
const LESSON_SLOPE: f32 = 0.5;
/// The killer card's enter.
pub(super) const CARD_ENTER_S: f32 = 0.18;
/// The card's accent bar.
const ACCENT_PX: f32 = 3.0;

/// Who fired the killing round, as the victim's copy of the record names them: each part only
/// where the record and the files it names supply it, so a card shows what is known.
#[derive(Clone, Debug, PartialEq)]
pub struct Killer {
    /// The shooter's name, where the record names a shooter.
    pub player: Option<String>,
    /// The shooter's vehicle as its tank file names it to players, where that file reads.
    pub vehicle: Option<String>,
    /// The round's display name and muzzle speed (m/s), where the registry has the round; never
    /// its registry key.
    pub round: Option<(String, f32)>,
}

impl Killer {
    /// The killer with no name, fired from the tank file `vehicle` (relative to the asset root)
    /// with the round `round` (a round registry key): the vehicle's display name read from that
    /// tank file, and the round's display name and muzzle speed from the registry. A tank file
    /// that does not read or a round the registry lacks leaves that part unknown.
    pub fn of_vehicle(vehicle: Option<&str>, round: &str) -> Self {
        let vehicle = vehicle.and_then(|vehicle| {
            let path = crate::assets::asset_root().join(vehicle);
            let spec = std::fs::read_to_string(&path)
                .map_err(|e| e.to_string())
                .and_then(|text| ron::de::from_str::<TankSpec>(&text).map_err(|e| e.to_string()));
            spec.inspect_err(|e| warn!("kill cam: the shooter's tank file {}: {e}", path.display()))
                .ok()
                .map(|spec| spec.display_name)
        });
        let round = crate::rounds::RoundRegistry::shipped()
            .name(round)
            .map_err(crate::guns::GunError::Round)
            .and_then(|name| {
                let fired = crate::guns::GunRegistry::shipped().round_by_key(round)?;
                Ok((name.to_owned(), fired.muzzle_speed))
            })
            .inspect_err(|e| warn!("kill cam: the round {round}: {e}"))
            .ok();
        Self {
            player: None,
            vehicle,
            round,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Beat {
    Hit,
    Lesson,
    Killer,
    Death,
}

impl Beat {
    /// Whether the camera is the kill cam's, not the player's.
    #[cfg(test)]
    pub fn captive(self) -> bool {
        matches!(self, Self::Lesson | Self::Killer)
    }
}

/// The lesson's length (real seconds) for the record of `plan`: until the record has settled in
/// the glass at [`LESSON_RATE`] ([`stage::pace`]) and held [`LESSON_HOLD_S`] on it, never under
/// [`LESSON_S`]; that minimum when the stage has no plan to draw.
pub fn lesson_length(plan: Option<&Plan>) -> f32 {
    plan.map_or(LESSON_S, |plan| {
        let settle = stage::pace(plan, CornerKind::Glass).settle;
        LESSON_S.max(settle / LESSON_RATE + LESSON_HOLD_S)
    })
}

/// The sequence's beats in run seconds, from the killing shot's world impact.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Beats {
    pub impact: f32,
    /// Whether beat 3 plays: only for a shooter the kill cam can stage. Without one the death
    /// screen follows the lesson.
    pub shooter: bool,
    /// The lesson's length (real seconds): [`lesson_length`] of the record it draws.
    pub lesson_s: f32,
}

impl Beats {
    pub fn lesson(self) -> f32 {
        self.impact + HIT_S
    }
    pub fn killer(self) -> f32 {
        self.lesson() + self.lesson_s
    }
    /// Where the killer card enters: the look's arrival on the shooter, or the death screen's
    /// start without beat 3.
    pub fn arrive(self) -> f32 {
        if self.shooter {
            self.killer() + MOVE_S
        } else {
            self.killer()
        }
    }
    pub fn death(self) -> f32 {
        if self.shooter {
            self.killer() + KILLER_S
        } else {
            self.killer()
        }
    }
    pub fn beat(self, t: f32) -> Beat {
        if t < self.lesson() {
            Beat::Hit
        } else if t < self.killer() {
            Beat::Lesson
        } else if t < self.death() {
            Beat::Killer
        } else {
            Beat::Death
        }
    }

    /// The record's corner time in the lesson.
    pub fn lesson_c(self, t: f32) -> Option<f32> {
        (self.beat(t) == Beat::Lesson).then(|| (t - self.lesson()) * LESSON_RATE)
    }
}

/// The turn index the lesson's stage is keyed by: no corner turn has it.
const LESSON_TURN: usize = usize::MAX - 1;

/// The stage's record and clock in the lesson: the glass, drawn on the world.
pub fn lesson_showing(beats: Beats, t: f32, record: usize) -> Option<Showing> {
    beats.lesson_c(t).map(|c| Showing {
        record,
        turn: LESSON_TURN,
        kind: CornerKind::Glass,
        c,
        opacity: 1.0,
        scale: 1.0,
        remaining: 1.0,
        surface: Surface::World,
    })
}

/// What the kill cam's orbit is placed from, in the battlefield's frame.
#[derive(Clone, Copy, Debug)]
pub struct Rig {
    pub victim_pivot: Vec3,
    /// What the look turns toward in beat 3; never an orbit centre. `None` where no shooter is
    /// staged ([`Beats::shooter`]).
    pub shooter_pivot: Option<Vec3>,
    pub impact: Vec3,
    pub muzzle: Vec3,
}

/// A look whose view axis is `forward`.
fn look_along(forward: Vec3) -> Quat {
    Transform::IDENTITY.looking_to(forward, Vec3::Y).rotation
}

impl Rig {
    /// The round's travel, shooter to victim.
    fn incoming(&self) -> Vec3 {
        (self.impact - self.muzzle).normalize_or(Vec3::NEG_Z)
    }

    fn incoming_flat(&self) -> Vec3 {
        let d = self.incoming();
        Vec3::new(d.x, 0.0, d.z).normalize_or(Vec3::NEG_Z)
    }

    /// The lesson's look: a three-quarter from the shot's side, drifted by `drift` (rad).
    fn lesson_look(&self, drift: f32) -> Quat {
        let side =
            Quat::from_rotation_y(LESSON_YAW_DEG.to_radians() + drift) * -self.incoming_flat();
        look_along(-(side + Vec3::Y * LESSON_SLOPE))
    }

    /// From the victim's pivot toward the shooter's where one is staged: the look the victim's own
    /// orbit takes to see the shooter, recomputed from the rig each frame so it tracks the shooter
    /// wherever the stage places it.
    fn shooter_look(&self) -> Option<Quat> {
        self.shooter_pivot
            .map(|shooter| look_along((shooter - self.victim_pivot).normalize_or(Vec3::NEG_Z)))
    }

    /// The lesson's drift (rad) at run time `t`.
    fn drift(beats: Beats, t: f32, reduce: bool) -> f32 {
        if reduce {
            0.0
        } else {
            timeline::ORBIT_DEG_S.to_radians() * (t - beats.lesson()).max(0.0)
        }
    }

    /// The pivot and look at run time `t`, given the player's free looks (`hit` in beat 1, `death`
    /// in beat 4), with the lesson's drift taken at `drift_t` rather than `t`: a held clock's orbit
    /// keeps drifting.
    pub fn orbit_drifting(
        &self,
        beats: Beats,
        t: f32,
        drift_t: f32,
        hit: Quat,
        death: Quat,
        reduce: bool,
    ) -> (Vec3, Quat) {
        let drift = |at: f32| Self::drift(beats, at + drift_t - t, reduce);
        match beats.beat(t) {
            Beat::Hit => (self.victim_pivot, hit),
            Beat::Lesson => {
                let w = timeline::ease_standard(timeline::progress(t, beats.lesson(), MOVE_S));
                (self.victim_pivot, hit.slerp(self.lesson_look(drift(t)), w))
            }
            Beat::Killer => {
                let w = timeline::ease_standard(timeline::progress(t, beats.killer(), MOVE_S));
                let from = self.lesson_look(drift(beats.killer()));
                (
                    self.victim_pivot,
                    from.slerp(self.shooter_look().unwrap_or(from), w),
                )
            }
            Beat::Death => (self.victim_pivot, death),
        }
    }
}

/// The killer card's three data lines, each reduced to what the font draws and named by what is
/// missing where the killer has nothing legible; the range only where the record has the
/// shooter's muzzle (`range_m`).
pub fn card_lines(killer: &Killer, range_m: Option<f32>) -> [String; 3] {
    let shown = |text: Option<&str>, missing: &str| {
        text.and_then(drawable)
            .unwrap_or_else(|| missing.to_owned())
    };
    let round = match &killer.round {
        Some((name, speed)) => format!("{}    {speed:.0} m/s", shown(Some(name), "UNKNOWN ROUND")),
        None => "UNKNOWN ROUND".to_owned(),
    };
    [
        shown(killer.player.as_deref(), "UNKNOWN PLAYER"),
        shown(killer.vehicle.as_deref(), "UNKNOWN VEHICLE"),
        match range_m {
            Some(range_m) => format!("{range_m:.0} m    {round}"),
            None => round,
        },
    ]
}

#[derive(Component)]
pub(super) struct Card;

#[derive(Component)]
pub(super) struct CardAccent;

#[derive(Component)]
pub(super) struct CardText;

#[derive(Component)]
pub(super) struct DeathPanel;

#[derive(Component)]
pub(super) struct DeathText;

#[derive(Component)]
pub(super) struct Prompt;

/// The death screen's wash over the world: shown from the kill cam's handoff to the death screen
/// or the respawn, never during the kill cam's own beats.
#[derive(Component)]
pub(super) struct DeathTint;

/// A text row's place in its block.
#[derive(Component)]
pub(super) struct Index(pub(super) usize);

/// A text style of the kill cam's UI.
fn text(font: &Handle<Font>, px: f32, color: Color) -> (TextFont, TextColor) {
    (
        TextFont {
            font: font.clone().into(),
            font_size: FontSize::Px(px),
            ..default()
        },
        TextColor(color),
    )
}

/// The death tint: the whole screen, on the bottom UI rung so it washes the world alone; hidden.
pub(super) fn spawn_death_tint(commands: &mut Commands) -> Entity {
    commands
        .spawn((crate::overlay::death_tint(), Visibility::Hidden, DeathTint))
        .id()
}

/// The killer card, lower left, hidden: clear of the shooter the look centres in beat 3.
pub(super) fn spawn_card(commands: &mut Commands, fonts: &UiFonts) -> Entity {
    commands
        .spawn((
            Node {
                position_type: PositionType::Absolute,
                left: Val::Px(48.0),
                bottom: Val::Px(150.0),
                ..default()
            },
            GlobalZIndex(Overlay::PEN_CAM_CORNER_Z),
            Visibility::Hidden,
            UiTransform::from_scale(Vec2::splat(0.96)),
            Card,
        ))
        .with_children(|root| {
            root.spawn((
                Node {
                    flex_direction: FlexDirection::Column,
                    padding: UiRect::axes(Val::Px(18.0), Val::Px(12.0)),
                    min_width: Val::Px(320.0),
                    overflow: Overflow::clip(),
                    ..default()
                },
                BackgroundColor(PANEL_BG),
            ))
            .with_children(|card| {
                card.spawn((
                    Node {
                        position_type: PositionType::Absolute,
                        top: Val::Px(0.0),
                        left: Val::Px(0.0),
                        width: Val::Percent(100.0),
                        height: Val::Px(ACCENT_PX),
                        ..default()
                    },
                    BackgroundColor(Color::WHITE),
                    CardAccent,
                ));
                for (index, (font, px, color)) in [
                    (&fonts.hud, 13.0, TEXT_DIM),
                    (&fonts.hud, 26.0, TEXT),
                    (&fonts.body, 18.0, TEXT),
                    (&fonts.body, 16.0, TEXT_DIM),
                ]
                .into_iter()
                .enumerate()
                {
                    card.spawn((Text::new(""), text(font, px, color), CardText, Index(index)));
                }
            });
        })
        .id()
}

/// The death panel, centre, over the live world, hidden.
pub(super) fn spawn_death_panel(commands: &mut Commands, fonts: &UiFonts) -> Entity {
    commands
        .spawn((
            Node {
                position_type: PositionType::Absolute,
                width: Val::Percent(100.0),
                top: Val::Percent(30.0),
                flex_direction: FlexDirection::Column,
                align_items: AlignItems::Center,
                ..default()
            },
            GlobalZIndex(Overlay::PEN_CAM_CORNER_Z),
            Visibility::Hidden,
            DeathPanel,
        ))
        .with_children(|panel| {
            panel.spawn((
                Text::new(""),
                text(&fonts.hud, 48.0, TEXT),
                DeathText,
                Index(0),
            ));
            panel.spawn((
                Text::new(""),
                text(&fonts.body, 20.0, TEXT_DIM),
                DeathText,
                Index(1),
            ));
        })
        .id()
}

/// A prompt reading `label`, bottom-centre, hidden.
pub(super) fn spawn_prompt(commands: &mut Commands, fonts: &UiFonts, label: &str) -> Entity {
    commands
        .spawn((
            Node {
                position_type: PositionType::Absolute,
                width: Val::Percent(100.0),
                bottom: Val::Px(96.0),
                justify_content: JustifyContent::Center,
                ..default()
            },
            GlobalZIndex(Overlay::PEN_CAM_CORNER_Z),
            Visibility::Hidden,
        ))
        .with_children(|root| {
            root.spawn((Text::new(label), text(&fonts.hud, 20.0, TEXT), Prompt));
        })
        .id()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::camera::{ORBIT_FAR, orbit_body};

    /// The fixture ground's height (m) under both tanks, which pulls the orbit body in.
    const GROUND_Y: f32 = -0.6;

    /// A rig staged with the victim at the origin, the shooter 700 m back along a level
    /// line, both pivots above their hulls. Fixture numbers.
    fn rig() -> Rig {
        let incoming = Vec3::new(0.5736, 0.0, 0.8191).normalize();
        let impact = Vec3::new(-1.63, 1.5, -0.31);
        let muzzle = impact - incoming * 700.0;
        Rig {
            victim_pivot: Vec3::new(0.0, 4.4, 0.1),
            shooter_pivot: Some(Vec3::new(muzzle.x, 4.4, muzzle.z)),
            impact,
            muzzle,
        }
    }

    /// The player's look at the hit, before any input: along the hull's forward, 12 degrees down.
    /// Fixture data; in play it is wherever the player was looking.
    fn player_look() -> Quat {
        let down = 12f32.to_radians();
        look_along(Vec3::new(0.0, -down.sin(), -down.cos()))
    }

    /// The death screen's opening look: toward the shooter, or without one the lesson's look as
    /// the lesson ends.
    fn death_look(rig: &Rig, beats: Beats) -> Quat {
        rig.shooter_look()
            .unwrap_or_else(|| rig.lesson_look(Rig::drift(beats, beats.killer(), false)))
    }

    /// The camera at run time `t`: [`Rig::orbit_drifting`] on the live clock, its body the game's
    /// orbit body at the play radius ([`ORBIT_FAR`]), pulled in by the flat fixture ground.
    fn pose(rig: &Rig, beats: Beats, t: f32, hit: Quat, death: Quat) -> Transform {
        let (pivot, look) = rig.orbit_drifting(beats, t, t, hit, death, false);
        let forward = Dir3::new(look * Vec3::NEG_Z).unwrap_or(Dir3::NEG_Z);
        let body = orbit_body(pivot, forward, ORBIT_FAR, |ray, max| {
            let down = ray.direction.y;
            if down < -1e-6 {
                let hit = (GROUND_Y - ray.origin.y) / down;
                if hit >= 0.0 {
                    return hit.min(max);
                }
            }
            max
        });
        Transform::from_translation(body).with_rotation(look)
    }

    /// The beats run in order; the camera is captive exactly in the lesson and the killer beats;
    /// by the lesson's end the record's corner clock is past its settle for every recorded
    /// scenario (the nose at the span's end, a field's front at its longest reach, a component's
    /// fill), held there [`LESSON_HOLD_S`], and the lesson is never under [`LESSON_S`]. The
    /// records hold one that settles too late for the minimum alone, so a lesson blind to its
    /// record fails here.
    #[test]
    fn the_beats_run_in_order() {
        let beats = Beats {
            impact: 2.0,
            shooter: true,
            lesson_s: LESSON_S,
        };
        assert!(beats.lesson() < beats.killer());
        assert!(beats.killer() < beats.arrive() && beats.arrive() < beats.death());
        for (t, beat) in [
            (0.0, Beat::Hit),
            (2.4, Beat::Hit),
            (2.6, Beat::Lesson),
            (beats.killer() + 0.1, Beat::Killer),
            (beats.death() + 0.1, Beat::Death),
        ] {
            assert_eq!(beats.beat(t), beat, "t {t}");
            assert_eq!(beat.captive(), matches!(beat, Beat::Lesson | Beat::Killer));
        }
        assert!(beats.lesson_c(beats.killer()).is_none());
        assert_eq!(lesson_length(None), LESSON_S);
        let library = super::super::plan::tests::Library::load();
        let longer = library.records.iter().filter(|record| {
            let beats = Beats {
                lesson_s: lesson_length(Some(&record.plan)),
                ..beats
            };
            let settle = stage::pace(&record.plan, CornerKind::Glass).settle;
            let held = beats.killer() - LESSON_HOLD_S + 1e-3;
            assert!(
                beats.lesson_c(held).unwrap() >= settle,
                "{}: settles at {settle}",
                record.stem
            );
            assert!(beats.lesson_s >= LESSON_S, "{}", record.stem);
            settle / LESSON_RATE > LESSON_S
        });
        assert!(
            longer.count() > 0,
            "a record settles past the minimum lesson"
        );
    }

    /// The camera never jumps: at every beat boundary its pose is continuous.
    #[test]
    fn the_camera_is_continuous_across_beats() {
        let rig = rig();
        let beats = Beats {
            impact: 2.0,
            shooter: true,
            lesson_s: LESSON_S,
        };
        let hit = player_look();
        let death = death_look(&rig, beats);
        for boundary in [
            beats.lesson(),
            beats.lesson() + MOVE_S,
            beats.killer(),
            beats.arrive(),
            beats.death(),
        ] {
            let a = pose(&rig, beats, boundary - 1e-4, hit, death);
            let b = pose(&rig, beats, boundary + 1e-4, hit, death);
            assert!(
                a.translation.distance(b.translation) < 0.05,
                "position jumps at {boundary}: {} m",
                a.translation.distance(b.translation)
            );
            assert!(
                a.rotation.angle_between(b.rotation).to_degrees() < 0.5,
                "look jumps at {boundary}"
            );
        }
    }

    /// Beat 3's look follows the shooter's pivot wherever it stands, off the round's line too, and
    /// its orbit stays the victim's: through the whole beat the camera stands no farther than the
    /// play radius from the victim's pivot, and from the look's arrival on it stands on that radius
    /// looking through the victim's pivot at the shooter's. The 60 m sidestep and the 0.1 s sweep
    /// step are fixture data.
    #[test]
    fn the_killer_look_follows_the_shooter_not_the_line() {
        let mut rig = rig();
        let beats = Beats {
            impact: 2.0,
            shooter: true,
            lesson_s: LESSON_S,
        };
        let side = rig.incoming_flat().cross(Vec3::Y) * 60.0;
        rig.shooter_pivot = rig.shooter_pivot.map(|pivot| pivot + side);
        let hit = player_look();
        let death = death_look(&rig, beats);
        let shooter = rig.shooter_pivot.unwrap();
        let mut t = beats.killer();
        while t < beats.death() {
            let pose = pose(&rig, beats, t, hit, death);
            let r = pose.translation.distance(rig.victim_pivot);
            assert!(r <= ORBIT_FAR + 1e-3, "t {t}: {r} m off the victim's pivot");
            if t >= beats.arrive() {
                let to_shooter = (shooter - pose.translation).normalize();
                assert!(
                    pose.forward().dot(to_shooter) > 0.9999,
                    "t {t}: off the shooter"
                );
                let to_victim = (rig.victim_pivot - pose.translation).normalize();
                assert!(
                    pose.forward().dot(to_victim) > 0.9999,
                    "t {t}: off the victim"
                );
                assert!(
                    (r - ORBIT_FAR).abs() < 1e-2,
                    "t {t}: {r} m, not the play radius"
                );
            }
            t += 0.1;
        }
        let pose = pose(&rig, beats, beats.arrive() + 0.5, hit, death);
        let to_shooter = (shooter - pose.translation).normalize();
        assert!(pose.forward().dot(to_shooter) > 0.9999);
        assert!((pose.translation.distance(rig.victim_pivot) - ORBIT_FAR).abs() < 1e-2);
    }

    /// The card's data lines draw in the shipped font: an undrawable name falls back to saying it
    /// is unknown, a partly drawable one keeps what draws; a record with no shooter names neither,
    /// and one with no muzzle shows no range. A round the registry lacks reads as unknown, and every
    /// registered round is named by its display name, never by its registry key.
    #[test]
    fn the_card_draws_only_what_the_font_covers() {
        let mut killer = Killer {
            player: Some("\u{864E}\u{738B}".into()),
            vehicle: Some("Tiger \u{864E} I".into()),
            round: Some(("Probe Mk. 1".into(), 773.0)),
        };
        let [name, vehicle, round] = card_lines(&killer, Some(700.0));
        assert_eq!(name, "UNKNOWN PLAYER");
        assert_eq!(vehicle, "Tiger I");
        assert!(round.contains("Probe Mk. 1") && round.contains("700 m"));
        (killer.player, killer.vehicle) = (None, None);
        let [name, vehicle, round] = card_lines(&killer, None);
        assert_eq!(
            (name.as_str(), vehicle.as_str()),
            ("UNKNOWN PLAYER", "UNKNOWN VEHICLE")
        );
        assert_eq!(round, "Probe Mk. 1    773 m/s");
        killer.round = None;
        assert_eq!(card_lines(&killer, None)[2], "UNKNOWN ROUND");
        assert_eq!(
            card_lines(&Killer::of_vehicle(None, "<unregistered>"), None)[2],
            "UNKNOWN ROUND"
        );
        let registry = crate::rounds::RoundRegistry::shipped();
        for key in registry.keys() {
            let [_, _, round] = card_lines(&Killer::of_vehicle(None, key), None);
            let name = registry.name(key).expect("a registered round's name");
            assert!(round.contains(name), "{key}: {round}");
            assert!(!round.contains(key), "{round} shows the key {key}");
        }
    }

    /// A kill record that names no shooter plays the hit, the lesson and the death screen with no
    /// beat 3: the death screen starts where the killer beat would, and orbits the victim,
    /// continuing the lesson's look.
    #[test]
    fn a_kill_without_a_shooter_skips_beat_three() {
        let beats = Beats {
            impact: 2.0,
            shooter: false,
            lesson_s: LESSON_S,
        };
        assert_eq!(beats.death(), beats.killer());
        let mut rig = rig();
        rig.shooter_pivot = None;
        let hit = player_look();
        let death = death_look(&rig, beats);
        let lesson_end = pose(&rig, beats, beats.killer() - 1e-4, hit, death);
        let death_start = pose(&rig, beats, beats.death() + 1e-4, hit, death);
        assert!(
            death_start.translation.distance(lesson_end.translation) < 0.05,
            "the death screen continues the lesson about the victim"
        );
    }
}
