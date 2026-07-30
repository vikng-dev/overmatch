//! Server-sanctioned shot outcomes: the client-side reconciliation bookkeeping.
//!
//! A net client's shells are cosmetic — the authority decides every armor outcome (ADR-0016/0021).
//! This module is the buffer those verdicts land in and the bounded reconstruction that turns them
//! back into a drawable flight. It owns three facts and one law:
//!
//! * [`SanctionedBounce`] / [`SanctionedTerminal`] — the authority's verdicts, keyed by [`ShotId`].
//! * [`SanctionedShots`] — the bounded, self-evicting buffer `net::client` fills and the ballistics
//!   march drains, ordered strictly by bounce sequence.
//! * [`catch_up_sanctioned_chain`] — the ONLY path that turns buffered verdicts into free flight,
//!   partitioned at every outcome the client already knows and bounded by the cosmetic horizon.
//!
//! Nothing here decides ballistics: it consumes flight math from the parent module
//! ([`fast_forward_shell`]) and never touches health, impulse, or the wire.

use bevy::prelude::*;

use super::{MAX_COSMETIC_CATCH_UP_TICKS, elapsed_ticks, fast_forward_shell};
use crate::ShotId;

/// A server-sanctioned ricochet consumed by a client cosmetic shell.
#[derive(Clone, Copy)]
pub(crate) struct SanctionedBounce {
    /// The exact server bounce point — where the re-seeded shell restarts.
    pub origin: Vec3,
    /// The post-bounce travel direction (unit; the receiver guards it before use).
    pub direction: Vec3,
    /// The post-bounce speed (m/s).
    pub speed: f32,
    /// Server tick where this bounce resolved.
    pub bounce_tick: u32,
    /// Zero-based ordinal, consumed strictly in order.
    pub sequence: u32,
    /// The combatant whose body the authority gave this bounce's impulse to, if any. Carried so the
    /// spark this bounce draws can be matched to the `HullShock` episode it belongs to.
    pub victim: Option<crate::CombatantId>,
}

/// A server-sanctioned armor terminal consumed by a client cosmetic shell.
#[derive(Clone, Copy)]
pub(crate) struct SanctionedTerminal {
    /// The server's impact position (embed point, or the perforation's entry face).
    pub position: Vec3,
    /// The struck face's outward normal, straight from the server's raycast.
    pub normal: Vec3,
    /// The server's penetration verdict — gates the flame lick, exactly as the authority's read did.
    pub penetrated: bool,
    /// Server tick where this terminal resolved.
    pub impact_tick: u32,
    /// Required number of prior bounces before this terminal may be consumed.
    pub after_bounces: u32,
    /// The combatant whose body the authority gave this terminal's impulse to, if any. See
    /// [`SanctionedBounce::victim`].
    pub victim: Option<crate::CombatantId>,
}

/// Per-shot sanctioned state: ordered bounces + the (at most one) terminal, plus an age for expiry.
struct SanctionedShot {
    bounces: Vec<SanctionedBounce>,
    terminal: Option<SanctionedTerminal>,
    /// Seconds since last touched — evicted once it outlives any shell that could still consume it.
    age: f32,
}

/// Bounded client buffer of server-sanctioned outcomes, keyed by [`ShotId`].
#[derive(Resource, Default)]
pub(crate) struct SanctionedShots {
    shots: std::collections::HashMap<ShotId, SanctionedShot>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SanctionedBounceInsert {
    Inserted,
    Duplicate,
    Capacity,
}

impl SanctionedShots {
    /// Configured expiry for unconsumed authority outcomes; recorded in trace metadata.
    pub(crate) const MAX_AGE_SECS: f32 = 3.0;
    /// DERIVED backstop: `30 combatants * 2 weapons * 750 rounds/min * 3 s / 60 = 2,250` shots;
    /// 4,096 is the next power of two.
    const MAX_SHOTS: usize = 4_096;
    /// DERIVED: a shot cannot consume more bounces than the cosmetic segment-work horizon.
    pub(crate) const MAX_BOUNCES_PER_SHOT: usize = MAX_COSMETIC_CATCH_UP_TICKS as usize;

    /// Stable tie-break among already-buffered entries: the greatest tuple is evicted.
    fn eviction_key(shot: ShotId) -> (u64, u8, u32) {
        (shot.combatant.0, shot.weapon, shot.fire_tick)
    }

    /// This shot's entry, fresh-touched, with the over-cap eviction applied.
    fn entry(&mut self, shot: ShotId) -> &mut SanctionedShot {
        if self.shots.len() >= Self::MAX_SHOTS && !self.shots.contains_key(&shot) {
            // Capacity overflow evicts one oldest entry; normal removal remains time-based. Equal
            // ages use the stable ShotId-derived order so cosmetic traces reproduce across runs.
            if let Some(oldest) = self
                .shots
                .iter()
                .max_by(|(a_shot, a), (b_shot, b)| {
                    a.age.total_cmp(&b.age).then_with(|| {
                        Self::eviction_key(**a_shot).cmp(&Self::eviction_key(**b_shot))
                    })
                })
                .map(|(k, _)| *k)
            {
                self.shots.remove(&oldest);
            }
        }
        let entry = self.shots.entry(shot).or_insert_with(|| SanctionedShot {
            bounces: Vec::new(),
            terminal: None,
            age: 0.0,
        });
        entry.age = 0.0;
        entry
    }

    /// Insert a server-sanctioned bounce idempotently by `(shot, sequence)`.
    pub(crate) fn insert(
        &mut self,
        shot: ShotId,
        bounce: SanctionedBounce,
    ) -> SanctionedBounceInsert {
        let entry = self.entry(shot);
        if entry.bounces.iter().any(|b| b.sequence == bounce.sequence) {
            return SanctionedBounceInsert::Duplicate;
        }
        if entry.bounces.len() >= Self::MAX_BOUNCES_PER_SHOT {
            return SanctionedBounceInsert::Capacity;
        }
        entry.bounces.push(bounce);
        SanctionedBounceInsert::Inserted
    }

    /// Record a shot's terminal, idempotently by [`ShotId`].
    ///
    /// INVARIANT: [`TerminalReport`] permits at most one authority terminal, so first insert wins.
    pub(crate) fn insert_terminal(&mut self, shot: ShotId, terminal: SanctionedTerminal) -> bool {
        let entry = self.entry(shot);
        if entry.terminal.is_some() {
            return false;
        }
        entry.terminal = Some(terminal);
        true
    }

    /// Whether anything is buffered under this exact [`ShotId`].
    #[cfg(test)]
    pub(crate) fn has_shot(&self, shot: ShotId) -> bool {
        self.shots.contains_key(&shot)
    }

    /// The next ordered bounce, if it has arrived.
    pub(super) fn next(&self, shot: ShotId, consumed: usize) -> Option<SanctionedBounce> {
        self.shots
            .get(&shot)
            .and_then(|e| e.bounces.iter().find(|b| b.sequence as usize == consumed))
            .copied()
    }

    /// The terminal only after all of its preceding bounces have been consumed.
    pub(super) fn terminal(&self, shot: ShotId, consumed: usize) -> Option<SanctionedTerminal> {
        self.shots
            .get(&shot)
            .and_then(|e| e.terminal)
            .filter(|t| t.after_bounces as usize == consumed)
    }

    /// Age every tracked shot and evict those past [`Self::MAX_AGE_SECS`]. Driven by `net::client`.
    pub(crate) fn age(&mut self, dt: f32) {
        for entry in self.shots.values_mut() {
            entry.age += dt;
        }
        self.shots.retain(|_, e| e.age <= Self::MAX_AGE_SECS);
    }
}

/// One authority-bounded free-flight segment beginning at a sanctioned bounce.
pub(super) struct SanctionedFlightSegment {
    pub(super) bounce: SanctionedBounce,
    pub(super) points: Vec<Vec3>,
}

/// A client catch-up through every already-buffered authority outcome up to `present`.
pub(super) struct SanctionedCatchUp {
    pub(super) segments: Vec<SanctionedFlightSegment>,
    pub(super) position: Vec3,
    pub(super) velocity: Vec3,
    pub(super) terminal: Option<SanctionedTerminal>,
}

/// Why an authority-outcome chain cannot be reconstructed safely on this client.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum SanctionedCatchUpReject {
    IntervalBeyondCosmeticHorizon,
    ChainBeyondCosmeticHorizon,
}

impl SanctionedCatchUpReject {
    pub(super) fn trace_reason(self) -> &'static str {
        match self {
            Self::IntervalBeyondCosmeticHorizon => "interval_beyond_cosmetic_horizon",
            Self::ChainBeyondCosmeticHorizon => "chain_beyond_cosmetic_horizon",
        }
    }
}

/// Reserve one disconnected authority segment before integrating it.
///
/// INVARIANT: one cosmetic reconstruction may integrate at most
/// [`MAX_COSMETIC_CATCH_UP_TICKS`] total steps and materialize at most that many authority
/// segments. The segment limit also bounds same-tick bounce chains, whose elapsed steps are zero.
fn reserve_sanctioned_catch_up_work(
    integrated_ticks: &mut u32,
    segments: &mut u32,
    steps: u32,
) -> Result<(), SanctionedCatchUpReject> {
    if *segments >= MAX_COSMETIC_CATCH_UP_TICKS
        || steps > MAX_COSMETIC_CATCH_UP_TICKS.saturating_sub(*integrated_ticks)
    {
        return Err(SanctionedCatchUpReject::ChainBeyondCosmeticHorizon);
    }
    *integrated_ticks += steps;
    *segments += 1;
    Ok(())
}

/// Fast-forward authority outcomes without integrating through a known later outcome.
///
/// INVARIANT: outcome ingress may retain early or out-of-order facts, but this is the sole path to
/// [`fast_forward_shell`] for a sanctioned chain; every segment reserves bounded work before it can
/// allocate or integrate.
pub(super) fn catch_up_sanctioned_chain(
    shot: ShotId,
    consumed: usize,
    first: SanctionedBounce,
    present: Option<u32>,
    fallback_age: u32,
    sanctioned: &SanctionedShots,
    fallback_velocity: Vec3,
    drag_k: f32,
    dt: f32,
) -> Result<SanctionedCatchUp, SanctionedCatchUpReject> {
    enum NextOutcome {
        Bounce(SanctionedBounce, u32),
        Terminal(SanctionedTerminal, u32),
    }

    let mut segments = Vec::new();
    let mut bounce = first;
    let mut seed_velocity =
        Dir3::new(bounce.direction).map_or(fallback_velocity, |dir| Vec3::from(dir) * bounce.speed);
    let mut consumed = consumed + 1;
    let mut integrated_ticks = 0;
    let mut segment_count = 0;
    loop {
        let next = present.and_then(|present| {
            let due_bounce = sanctioned.next(shot, consumed).and_then(|next| {
                elapsed_ticks(present, next.bounce_tick)?;
                let gap = elapsed_ticks(next.bounce_tick, bounce.bounce_tick)?;
                Some((next, gap))
            });
            let due_terminal = sanctioned.terminal(shot, consumed).and_then(|terminal| {
                elapsed_ticks(present, terminal.impact_tick)?;
                let gap = elapsed_ticks(terminal.impact_tick, bounce.bounce_tick)?;
                Some((terminal, gap))
            });
            match (due_bounce, due_terminal) {
                (Some((next, bounce_gap)), Some((terminal, terminal_gap))) => {
                    if terminal_gap <= bounce_gap {
                        Some(NextOutcome::Terminal(terminal, terminal_gap))
                    } else {
                        Some(NextOutcome::Bounce(next, bounce_gap))
                    }
                }
                (Some((next, gap)), None) => Some(NextOutcome::Bounce(next, gap)),
                (None, Some((terminal, gap))) => Some(NextOutcome::Terminal(terminal, gap)),
                (None, None) => None,
            }
        });

        match next {
            Some(NextOutcome::Bounce(next, gap)) => {
                if gap > MAX_COSMETIC_CATCH_UP_TICKS {
                    return Err(SanctionedCatchUpReject::IntervalBeyondCosmeticHorizon);
                }
                let steps = gap.saturating_sub(1);
                reserve_sanctioned_catch_up_work(&mut integrated_ticks, &mut segment_count, steps)?;
                let (_, velocity, points) =
                    fast_forward_shell(bounce.origin, seed_velocity, drag_k, dt, steps);
                segments.push(SanctionedFlightSegment { bounce, points });
                bounce = next;
                seed_velocity = Dir3::new(bounce.direction)
                    .map_or(velocity, |dir| Vec3::from(dir) * bounce.speed);
                consumed += 1;
            }
            Some(NextOutcome::Terminal(terminal, gap)) => {
                if gap > MAX_COSMETIC_CATCH_UP_TICKS {
                    return Err(SanctionedCatchUpReject::IntervalBeyondCosmeticHorizon);
                }
                let steps = gap.saturating_sub(1);
                reserve_sanctioned_catch_up_work(&mut integrated_ticks, &mut segment_count, steps)?;
                let (_, velocity, points) =
                    fast_forward_shell(bounce.origin, seed_velocity, drag_k, dt, steps);
                segments.push(SanctionedFlightSegment { bounce, points });
                return Ok(SanctionedCatchUp {
                    segments,
                    position: terminal.position,
                    velocity,
                    terminal: Some(terminal),
                });
            }
            None => {
                let age = present
                    .and_then(|present| elapsed_ticks(present, bounce.bounce_tick))
                    .unwrap_or(fallback_age);
                if age > MAX_COSMETIC_CATCH_UP_TICKS {
                    return Err(SanctionedCatchUpReject::IntervalBeyondCosmeticHorizon);
                }
                reserve_sanctioned_catch_up_work(&mut integrated_ticks, &mut segment_count, age)?;
                let (position, velocity, points) =
                    fast_forward_shell(bounce.origin, seed_velocity, drag_k, dt, age);
                segments.push(SanctionedFlightSegment { bounce, points });
                return Ok(SanctionedCatchUp {
                    segments,
                    position,
                    velocity,
                    terminal: None,
                });
            }
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanctioned_outcome_buffer_holds_the_derived_thirty_player_automatic_horizon() {
        const COMBATANTS: u64 = 30;
        const WEAPONS: u8 = 2;
        // DERIVED ceiling: 750 rounds/minute × 3 seconds / 60 = 37.5 rounds per weapon.
        const SHOTS_PER_WEAPON: u32 = 38;
        let mut sanctioned = SanctionedShots::default();

        for combatant in 1..=COMBATANTS {
            for weapon in 0..WEAPONS {
                for fire_tick in 0..SHOTS_PER_WEAPON {
                    let shot = ShotId {
                        combatant: crate::CombatantId(combatant),
                        weapon,
                        fire_tick,
                    };
                    assert_eq!(
                        sanctioned.insert(
                            shot,
                            SanctionedBounce {
                                origin: Vec3::ZERO,
                                direction: Vec3::X,
                                speed: 500.0,
                                bounce_tick: fire_tick,
                                sequence: 0,
                                victim: None,
                            }
                        ),
                        SanctionedBounceInsert::Inserted
                    );
                }
            }
        }

        let expected = COMBATANTS as usize * WEAPONS as usize * SHOTS_PER_WEAPON as usize;
        assert_eq!(
            sanctioned.shots.len(),
            expected,
            "the configured outcome lifetime must not evict a valid 30-player automatic-fire horizon"
        );
    }

    /// Equal-age overflow eviction is deterministic even though the buffer is a hash map.
    #[test]
    fn sanctioned_outcome_capacity_evicts_the_stable_highest_shot_id_on_an_age_tie() {
        let mut sanctioned = SanctionedShots::default();
        for fire_tick in 0..SanctionedShots::MAX_SHOTS as u32 {
            let shot = ShotId {
                combatant: crate::CombatantId(1),
                weapon: 0,
                fire_tick,
            };
            assert_eq!(
                sanctioned.insert(
                    shot,
                    SanctionedBounce {
                        origin: Vec3::ZERO,
                        direction: Vec3::X,
                        speed: 500.0,
                        bounce_tick: fire_tick,
                        sequence: 0,
                        victim: None,
                    }
                ),
                SanctionedBounceInsert::Inserted
            );
        }

        let incoming = ShotId {
            combatant: crate::CombatantId(1),
            weapon: 0,
            fire_tick: SanctionedShots::MAX_SHOTS as u32,
        };
        assert_eq!(
            sanctioned.insert(
                incoming,
                SanctionedBounce {
                    origin: Vec3::ZERO,
                    direction: Vec3::X,
                    speed: 500.0,
                    bounce_tick: incoming.fire_tick,
                    sequence: 0,
                    victim: None,
                }
            ),
            SanctionedBounceInsert::Inserted
        );

        assert!(
            sanctioned.has_shot(ShotId {
                combatant: crate::CombatantId(1),
                weapon: 0,
                fire_tick: 0,
            }),
            "the lowest stable tie-break key remains"
        );
        assert!(sanctioned.has_shot(incoming), "the new fact is retained");
        assert!(
            !sanctioned.has_shot(ShotId {
                combatant: crate::CombatantId(1),
                weapon: 0,
                fire_tick: SanctionedShots::MAX_SHOTS as u32 - 1,
            }),
            "the previous highest stable tie-break key is evicted"
        );
    }

    /// One malformed shot cannot grow more buffered bounces than cosmetic reconstruction can
    /// consume. The bound is DERIVED from the shared segment horizon.
    #[test]
    fn sanctioned_outcome_rejects_distinct_bounces_beyond_the_per_shot_bound() {
        let shot = ShotId {
            combatant: crate::CombatantId(1),
            weapon: 0,
            fire_tick: 100,
        };
        let mut sanctioned = SanctionedShots::default();
        for sequence in 0..MAX_COSMETIC_CATCH_UP_TICKS {
            assert_eq!(
                sanctioned.insert(
                    shot,
                    SanctionedBounce {
                        origin: Vec3::ZERO,
                        direction: Vec3::X,
                        speed: 500.0,
                        bounce_tick: 100 + sequence,
                        sequence,
                        victim: None,
                    }
                ),
                SanctionedBounceInsert::Inserted
            );
        }

        assert_eq!(
            sanctioned.insert(
                shot,
                SanctionedBounce {
                    origin: Vec3::ZERO,
                    direction: Vec3::X,
                    speed: 500.0,
                    bounce_tick: 100 + MAX_COSMETIC_CATCH_UP_TICKS,
                    sequence: MAX_COSMETIC_CATCH_UP_TICKS,
                    victim: None,
                }
            ),
            SanctionedBounceInsert::Capacity,
            "the first distinct bounce beyond the reconstruction bound is rejected"
        );
        assert_eq!(
            sanctioned.shots[&shot].bounces.len(),
            MAX_COSMETIC_CATCH_UP_TICKS as usize
        );
    }

    /// A catch-up with bounce 1 and a later terminal already buffered must partition free-flight at
    /// both authority outcomes. It may not integrate bounce 0's outgoing state all the way to the
    /// present and draw through facts it already knows.
    #[test]
    fn sanctioned_chain_stops_each_segment_before_the_next_known_outcome() {
        let shot = ShotId {
            combatant: crate::CombatantId(1),
            weapon: 0,
            fire_tick: 90,
        };
        let first = SanctionedBounce {
            origin: Vec3::ZERO,
            direction: Vec3::X,
            speed: 10.0,
            bounce_tick: 100,
            sequence: 0,
            victim: None,
        };
        let second = SanctionedBounce {
            origin: Vec3::new(3.5, 0.0, 0.0),
            direction: Vec3::Y,
            speed: 10.0,
            bounce_tick: 104,
            sequence: 1,
            victim: None,
        };
        let terminal = SanctionedTerminal {
            position: Vec3::new(3.5, 3.5, 0.0),
            normal: Vec3::NEG_Y,
            penetrated: true,
            impact_tick: 108,
            after_bounces: 2,
            victim: None,
        };
        let mut sanctioned = SanctionedShots::default();
        sanctioned.insert(shot, first);
        sanctioned.insert(shot, second);
        sanctioned.insert_terminal(shot, terminal);

        let caught_up = catch_up_sanctioned_chain(
            shot,
            0,
            first,
            Some(110),
            0,
            &sanctioned,
            Vec3::X * first.speed,
            0.0,
            0.1,
        )
        .expect("the short authority chain is reconstructible");

        assert_eq!(
            caught_up.segments.len(),
            2,
            "both bounces partition the catch-up"
        );
        assert_eq!(caught_up.segments[0].bounce.sequence, 0);
        assert_eq!(caught_up.segments[1].bounce.sequence, 1);
        assert_eq!(
            caught_up.segments[0].points.len(),
            4,
            "DERIVED fixture: origin plus three complete ticks before the tick-104 bounce"
        );
        assert!(
            caught_up.segments[0]
                .points
                .iter()
                .all(|point| point.x < second.origin.x),
            "bounce 0 free-flight never crosses the already-known bounce 1 origin"
        );
        assert!(
            caught_up.segments[1]
                .points
                .iter()
                .all(|point| point.y < terminal.position.y),
            "bounce 1 free-flight never crosses the already-known terminal"
        );
        assert_eq!(
            caught_up.terminal.map(|terminal| terminal.position),
            Some(terminal.position)
        );
        assert_eq!(caught_up.position, terminal.position);
    }

    /// A bogus authority tick must not turn cosmetic recovery into an unbounded integration. The
    /// fallback is intentionally no trajectory: drawing a prefix would claim an authority path the
    /// client cannot safely reconstruct.
    #[test]
    fn sanctioned_chain_rejects_an_implausibly_late_first_bounce() {
        let shot = ShotId {
            combatant: crate::CombatantId(1),
            weapon: 0,
            fire_tick: 90,
        };
        let first = SanctionedBounce {
            origin: Vec3::ZERO,
            direction: Vec3::X,
            speed: 10.0,
            bounce_tick: 100,
            sequence: 0,
            victim: None,
        };
        let sanctioned = SanctionedShots::default();

        let caught_up = catch_up_sanctioned_chain(
            shot,
            0,
            first,
            Some(first.bounce_tick + MAX_COSMETIC_CATCH_UP_TICKS + 1),
            0,
            &sanctioned,
            Vec3::X * first.speed,
            0.0,
            0.1,
        );

        assert!(
            matches!(
                caught_up,
                Err(SanctionedCatchUpReject::IntervalBeyondCosmeticHorizon)
            ),
            "a catch-up beyond the configured cosmetic horizon must reject instead of drawing a partial trajectory"
        );
    }

    /// A later authority fact cannot create an unbounded intermediate segment either. The entire
    /// chain rejects, so the already-seen first bounce is not drawn as a misleading partial result.
    #[test]
    fn sanctioned_chain_rejects_an_implausible_inter_outcome_gap() {
        let shot = ShotId {
            combatant: crate::CombatantId(1),
            weapon: 0,
            fire_tick: 90,
        };
        let first = SanctionedBounce {
            origin: Vec3::ZERO,
            direction: Vec3::X,
            speed: 10.0,
            bounce_tick: 100,
            sequence: 0,
            victim: None,
        };
        let second = SanctionedBounce {
            origin: Vec3::X,
            direction: Vec3::Y,
            speed: 10.0,
            bounce_tick: first.bounce_tick + MAX_COSMETIC_CATCH_UP_TICKS + 1,
            sequence: 1,
            victim: None,
        };
        let mut sanctioned = SanctionedShots::default();
        sanctioned.insert(shot, first);
        sanctioned.insert(shot, second);

        assert!(
            matches!(
                catch_up_sanctioned_chain(
                    shot,
                    0,
                    first,
                    Some(second.bounce_tick),
                    0,
                    &sanctioned,
                    Vec3::X * first.speed,
                    0.0,
                    0.1,
                ),
                Err(SanctionedCatchUpReject::IntervalBeyondCosmeticHorizon)
            ),
            "the chain must not draw its first segment when the next sanctioned boundary is implausible"
        );
    }

    /// Individually plausible segments may not accumulate into unbounded cosmetic work. This chain
    /// would integrate 198 ticks: DERIVED as 99 pre-bounce steps plus 99 pre-terminal steps.
    #[test]
    fn sanctioned_chain_rejects_cumulative_multi_segment_work_beyond_its_horizon() {
        let shot = ShotId {
            combatant: crate::CombatantId(1),
            weapon: 0,
            fire_tick: 90,
        };
        let first = SanctionedBounce {
            origin: Vec3::ZERO,
            direction: Vec3::X,
            speed: 10.0,
            bounce_tick: 100,
            sequence: 0,
            victim: None,
        };
        let second = SanctionedBounce {
            origin: Vec3::X,
            direction: Vec3::Y,
            speed: 10.0,
            bounce_tick: first.bounce_tick + MAX_COSMETIC_CATCH_UP_TICKS,
            sequence: 1,
            victim: None,
        };
        let terminal = SanctionedTerminal {
            position: Vec3::ONE,
            normal: Vec3::Y,
            penetrated: false,
            impact_tick: second.bounce_tick + MAX_COSMETIC_CATCH_UP_TICKS,
            after_bounces: 2,
            victim: None,
        };
        let mut sanctioned = SanctionedShots::default();
        sanctioned.insert(shot, first);
        sanctioned.insert(shot, second);
        sanctioned.insert_terminal(shot, terminal);

        assert!(
            matches!(
                catch_up_sanctioned_chain(
                    shot,
                    0,
                    first,
                    Some(terminal.impact_tick),
                    0,
                    &sanctioned,
                    Vec3::X * first.speed,
                    0.0,
                    0.1,
                ),
                Err(SanctionedCatchUpReject::ChainBeyondCosmeticHorizon)
            ),
            "the complete chain must fail closed once its combined integration exceeds the horizon"
        );
    }

    /// A small true elapsed interval remains valid across the wrapping tick boundary.
    #[test]
    fn sanctioned_chain_accepts_a_small_wraparound_interval() {
        let shot = ShotId {
            combatant: crate::CombatantId(1),
            weapon: 0,
            fire_tick: u32::MAX - 3,
        };
        let first = SanctionedBounce {
            origin: Vec3::ZERO,
            direction: Vec3::X,
            speed: 10.0,
            bounce_tick: u32::MAX - 2,
            sequence: 0,
            victim: None,
        };
        let caught_up = catch_up_sanctioned_chain(
            shot,
            0,
            first,
            Some(3),
            0,
            &SanctionedShots::default(),
            Vec3::X * first.speed,
            0.0,
            0.1,
        )
        .expect("a six-tick wrapping interval is inside the cosmetic horizon");

        assert_eq!(
            caught_up.segments[0].points.len(),
            7,
            "DERIVED: origin plus six integrated ticks"
        );
    }
}
