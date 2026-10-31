//! The witness pass (ADR-0040): what the hard world sent through each witness volume.
//!
//! A witness ([`super::WitnessVolume`]) is no layer of the walk, so nothing here feeds back into
//! the hard world: the round's state, every field direction's strength and reach are decided
//! without it, and this pass only reads what they were. Two intersections dose a witness:
//!
//! - [`line`]: the round's axis legs ([`Leg`]), each sampled by the walk's own disc; per
//!   witness, the disc-mean chord in the open (none inside a hard volume, [`solid`]), dosed
//!   `chord × wound` (the round's crew line rate, reference-mm per metre).
//! - [`entries`]: where a field direction enters each witness, which
//!   [`super::resolve::field_direction`] doses at the direction's strength there times its share.
//!
//! Both read witness presence by the collector and the pairing law of every other corridor
//! ([`walk::witness_spans`]); a witness shell that does not alternate is the same structured
//! [`WalkError`] a hard one would be. The laws are the design doc's §6
//! (`.agents/docs/design/armor-penetration-and-damage.md`).

use avian3d::prelude::SimpleCollider;
use bevy::prelude::*;

use super::collect::{self, Corridor};
use super::resolve::{FieldReach, Leg, MAX_CORRIDOR, PROBE_MARGIN, ResolveContext, field_presence};
use super::walk::{self, WalkError, WitnessSpans};
use super::{VolumeCrossing, hit_ancestor};

/// The witness colliders `admit` keeps whose bounds meet the stretch from `from` along `axis` for
/// `length` m, grown by `radius`: `(witness, collider)` pairs in entity order.
fn candidates(
    context: &ResolveContext<'_, '_, '_>,
    admit: &dyn Fn(Entity) -> bool,
    from: Vec3,
    axis: Vec3,
    length: f32,
    radius: f32,
) -> Vec<(Entity, Entity)> {
    let world = context.world;
    let mut out = Vec::new();
    world.spatial.aabb_intersections_with_aabb_callback(
        collect::swept_aabb(from, axis, length, radius),
        |entity| {
            if world.witness_surfaces.contains(entity)
                && admit(entity)
                && let Some((node, _)) = hit_ancestor(entity, &world.witnesses, &world.parents)
            {
                out.push((node, entity));
            }
            // Every tree is walked whatever this returns; `true` walks this one whole.
            true
        },
    );
    out.sort();
    out
}

/// Where the line `t·axis` lies inside the box `[min, max]` (both relative to the line's origin):
/// the parameter interval, `None` where it misses.
fn line_in_box(axis: Vec3, min: Vec3, max: Vec3) -> Option<(f32, f32)> {
    let (mut enter, mut leave) = (f32::NEG_INFINITY, f32::INFINITY);
    for i in 0..3 {
        if axis[i] == 0.0 {
            if min[i] > 0.0 || max[i] < 0.0 {
                return None;
            }
        } else {
            let (a, b) = (min[i] / axis[i], max[i] / axis[i]);
            enter = enter.max(a.min(b));
            leave = leave.min(a.max(b));
        }
    }
    (enter <= leave).then_some((enter, leave))
}

/// Each witness of `near` that the line `at + t·axis` is inside for some `t ∈ [0, length]`: its
/// presence intervals in metres from `at`, clipped to that stretch, disjoint and ordered; in
/// entity order.
///
/// Each witness is read on a corridor of its own, so what one witness reads is bit for bit what it
/// would read with every other witness absent: a witness behind a witness is dosed as it would be
/// alone.
fn spans(
    context: &ResolveContext<'_, '_, '_>,
    near: &[(Entity, Entity)],
    at: Vec3,
    axis: Vec3,
    length: f32,
) -> Result<Vec<WitnessSpans>, WalkError> {
    let mut out = Vec::new();
    // `near` is sorted, so each witness's colliders are one run of it.
    for run in near.chunk_by(|a, b| a.0 == b.0) {
        if let Some(found) = presence(context, run, at, axis, length)? {
            out.push(found);
        }
    }
    Ok(out)
}

/// One witness's presence along the line, from the colliders `run` of it ([`spans`]).
///
/// The corridor runs from behind the union of the met colliders' bounds, grown by
/// [`PROBE_MARGIN`], to beyond it, so every shell it meets is entered and left inside it and the
/// witness's presence at `at` is the parity of crossings behind, never assumed.
fn presence(
    context: &ResolveContext<'_, '_, '_>,
    run: &[(Entity, Entity)],
    at: Vec3,
    axis: Vec3,
    length: f32,
) -> Result<Option<WitnessSpans>, WalkError> {
    let margin = Vec3::splat(PROBE_MARGIN);
    let mut kept = Vec::new();
    let (mut lo, mut hi) = (Vec3::INFINITY, Vec3::NEG_INFINITY);
    for &(node, primitive) in run {
        let Ok((position, rotation, collider, _)) = context.colliders.get(primitive) else {
            return Err(WalkError::CollectorFailed {
                volume: node,
                reason: "a witness collider has no resolved pose",
            });
        };
        let bounds = collider.aabb(position.0, *rotation);
        let (min, max) = (bounds.min - at - margin, bounds.max - at + margin);
        if line_in_box(axis, min, max).is_none_or(|(enter, leave)| leave < 0.0 || enter > length) {
            continue;
        }
        lo = lo.min(min);
        hi = hi.max(max);
        kept.push((node, primitive));
    }
    let Some(&(witness, _)) = kept.first() else {
        return Ok(None);
    };
    let Some((enter, leave)) = line_in_box(axis, lo, hi) else {
        return Err(WalkError::UncertifiedStart {
            volume: witness,
            reason: "a witness line misses the bounds its colliders were admitted by",
        });
    };
    let behind = (-enter).max(0.0);
    let total = behind + leave.max(0.0);
    if !total.is_finite() || total > MAX_CORRIDOR {
        return Err(WalkError::UncertifiedStart {
            volume: witness,
            reason: "a witness line through its colliders' bounds exceeds the corridor ceiling",
        });
    }
    let mut hits = Vec::new();
    collect::collect(
        &Corridor {
            anchor: at,
            origin: Vec3::ZERO,
            axis,
            length: total,
            behind,
            seeded: &[],
            handoff: None,
            laws: &context.laws,
        },
        &kept,
        context.colliders,
        &mut hits,
    )?;
    let (behind, length) = (behind as f64, length as f64);
    let spans: Vec<(f64, f64)> = walk::witness_spans(0, total as f64, &hits)?
        .into_iter()
        .flat_map(|found| found.spans)
        .map(|(a, b)| ((a - behind).max(0.0), (b - behind).min(length)))
        .filter(|(a, b)| b > a)
        .collect();
    Ok((!spans.is_empty()).then_some(WitnessSpans { witness, spans }))
}

/// Where the line `at + t·axis` lies inside any hard volume `admit` keeps, for `t ∈ [0, length]`:
/// disjoint, ordered intervals in metres from `at`. Read by the field's presence walk
/// ([`field_presence`]) over every hard collider whose bounds meet the stretch, so it is the
/// collector and pairing law of every corridor, walked from outside those bounds, and what `at`
/// starts inside is the parity of crossings behind it, never assumed.
fn solid(
    context: &ResolveContext<'_, '_, '_>,
    admit: &dyn Fn(Entity) -> bool,
    at: Vec3,
    axis: Dir3,
    length: f32,
) -> Result<Vec<(f64, f64)>, WalkError> {
    let reach = FieldReach::within(
        context,
        admit,
        collect::swept_aabb(at, Vec3::from(axis), length, 0.0),
    );
    let Some((behind, walked)) = field_presence(context, &reach, at, axis)? else {
        return Ok(Vec::new());
    };
    let mut out: Vec<(f64, f64)> = Vec::new();
    for layer in walked
        .layers
        .iter()
        .filter(|layer| !layer.present.is_empty())
    {
        let (a, b) = (
            (layer.start - behind).max(0.0),
            (layer.end - behind).min(length as f64),
        );
        if b <= a {
            continue;
        }
        match out.last_mut() {
            Some(last) if last.1 >= a => last.1 = last.1.max(b),
            _ => out.push((a, b)),
        }
    }
    Ok(out)
}

/// What of `spans` lies outside every interval of `solid`, both disjoint and ordered: the
/// witness's presence in the open. With no solid interval among them it is `spans` unchanged.
fn open_spans(spans: &[(f64, f64)], solid: &[(f64, f64)]) -> Vec<(f64, f64)> {
    let mut out = Vec::new();
    for &(start, end) in spans {
        let mut from = start;
        for &(enter, leave) in solid {
            if leave <= from {
                continue;
            }
            if enter >= end {
                break;
            }
            if enter > from {
                out.push((from, enter));
            }
            from = leave;
            if from >= end {
                break;
            }
        }
        if from < end {
            out.push((from, end));
        }
    }
    out
}

/// The round's line through every witness `admit` keeps, along `legs` in order: per witness, the
/// disc-mean chord in the open summed over the legs (each leg's chord the mean over all its
/// samples, each sample read along its own stretch, a sample that misses counting zero), its dose
/// `chord × wound` reference-mm, the axis points where the line first entered it in the open and
/// where it last left it or the legs end (each sample's point projected onto its leg's axis), and
/// whether every sample that entered it on the last leg that met it came out within its own
/// stretch. In order of first entry.
///
/// THE STRONGEST-ONLY RULE (§13.2's damage law). A witness's chord inside any hard volume `admit`
/// keeps ([`solid`]) takes no dose: a crew box clipped into a wall is dosed only over the chord
/// it presents in the open, and one wholly inside armour is not met at all. A field direction
/// already stops at the hard face that hides it ([`super::resolve::field_direction`]).
///
/// With them, the axis of the first leg that met a witness (`None` when none did).
///
/// A leg ends where the round came to rest, so a round at rest inside a witness cuts its chord
/// there.
pub(crate) fn line(
    context: &ResolveContext<'_, '_, '_>,
    admit: &dyn Fn(Entity) -> bool,
    legs: &[Leg],
    wound: f32,
) -> Result<(Vec<VolumeCrossing>, Option<Dir3>), WalkError> {
    let mut out: Vec<VolumeCrossing> = Vec::new();
    let mut incoming = None;
    for leg in legs {
        let longest = leg
            .samples
            .iter()
            .map(|(_, length)| *length)
            .fold(0.0, f32::max);
        if longest <= 0.0 {
            continue;
        }
        let axis = Vec3::from(leg.axis);
        // Every sample's stretch lies within its offset's length of the axis from `from` for the
        // longest stretch.
        let reach = leg
            .samples
            .iter()
            .map(|(offset, _)| offset.length())
            .fold(0.0, f32::max);
        let near = candidates(context, admit, leg.from, axis, longest, reach);
        if near.is_empty() {
            continue;
        }
        // Per witness: summed chord over the samples, first entry and last exit along the axis,
        // all samples left.
        let mut met: Vec<(Entity, f64, f64, f64, bool)> = Vec::new();
        for &(offset, length) in &leg.samples {
            if length <= 0.0 {
                continue;
            }
            let along = offset.dot(axis) as f64;
            let at = leg.from + offset;
            let found = spans(context, &near, at, axis, length)?;
            if found.is_empty() {
                continue;
            }
            let hard = solid(context, admit, at, leg.axis, length)?;
            for found in found {
                let open = open_spans(&found.spans, &hard);
                let (Some(&(first, _)), Some(&(_, last))) = (open.first(), open.last()) else {
                    continue;
                };
                let chord: f64 = open.iter().map(|(a, b)| b - a).sum();
                let entry = along + first;
                let exit = along + last;
                let left = last < length as f64;
                match met.iter_mut().find(|m| m.0 == found.witness) {
                    Some(m) => {
                        m.1 += chord;
                        m.2 = m.2.min(entry);
                        m.3 = m.3.max(exit);
                        m.4 &= left;
                    }
                    None => met.push((found.witness, chord, entry, exit, left)),
                }
            }
        }
        if met.is_empty() {
            continue;
        }
        incoming.get_or_insert(leg.axis);
        met.sort_by(|a, b| a.2.total_cmp(&b.2).then(a.0.cmp(&b.0)));
        let samples = leg.samples.len() as f64;
        for (witness, chord, entry, exit, left) in met {
            let chord = (chord / samples) as f32;
            let entry = leg.from + axis * entry as f32;
            let end = leg.from + axis * exit as f32;
            match out.iter_mut().find(|crossed| crossed.volume == witness) {
                Some(crossed) => {
                    crossed.chord += chord;
                    crossed.cost += chord * wound;
                    crossed.end = end;
                    crossed.perforated = left;
                }
                None => out.push(VolumeCrossing {
                    volume: witness,
                    entry,
                    end,
                    chord,
                    cost: chord * wound,
                    energy: 0.0,
                    perforated: left,
                }),
            }
        }
    }
    Ok((out, incoming))
}

/// Where the field direction thrown from `at` along `dir` enters each witness `admit` keeps
/// within `reach` m: `(witness, t)`, `t` the metres from `at` to its first presence (0 for a
/// witness that holds `at`), in order of `t`.
pub(crate) fn entries(
    context: &ResolveContext<'_, '_, '_>,
    admit: &dyn Fn(Entity) -> bool,
    at: Vec3,
    dir: Dir3,
    reach: f32,
) -> Result<Vec<(Entity, f64)>, WalkError> {
    let swept = collect::swept_aabb(at, Vec3::from(dir), reach, 0.0);
    entries_in(
        context,
        &FieldReach::within(context, admit, swept),
        at,
        dir,
        reach,
    )
}

/// [`entries`] for one direction of a throw, its witnesses read from the throw's `field`, which
/// must hold every collider whose bounds meet the direction's swept box.
pub(crate) fn entries_in(
    context: &ResolveContext<'_, '_, '_>,
    field: &FieldReach,
    at: Vec3,
    dir: Dir3,
    reach: f32,
) -> Result<Vec<(Entity, f64)>, WalkError> {
    let axis = Vec3::from(dir);
    let near = field.witnesses_meeting(collect::swept_aabb(at, axis, reach, 0.0));
    if near.is_empty() {
        return Ok(Vec::new());
    }
    let mut found: Vec<(Entity, f64)> = spans(context, &near, at, axis, reach)?
        .into_iter()
        .map(|found| (found.witness, found.spans[0].0))
        .collect();
    found.sort_by(|a, b| a.1.total_cmp(&b.1).then(a.0.cmp(&b.0)));
    Ok(found)
}

#[cfg(test)]
mod tests;
