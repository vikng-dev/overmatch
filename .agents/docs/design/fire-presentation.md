# Announced fire owns its trajectory

Subject: Overmatch's fire presentation and damage model; game state only. Issue #388.

## Decision and alternatives

Only a validated server announcement authors a cosmetic round. Its `ShotId` joins the authority's
path; a belt count cannot supply identity, bore pose, physical birth, fired load or tracer phase.
Own and remote fire share the reconstruction path in `net::client`.

The deleted belt-derived fallback waited for an estimator-sized interval, synthesized an unkeyed
round from the current muzzle, then swallowed the later announcement. This broke two ADR-0021
receive rules: "No receive rule infers causality from cross-channel arrival order" and a cosmetic
shell "never improvises the authority's result." A snapshot's consumption count cannot prove an
announcement lost. A cursor-held announcement could also race the fallback when the cursor crossed
several ticks in one frame.

The deletion also fixes main-gun presentation: a lost packet carrying the reliable announcement
could outlast the fallback's wait. The fallback drew a round without its server path or ricochet,
then swallowed the resent keyed announcement.

**Playtest decision:** omit an own automatic round when every announcement copy is lost. ADR-0021
permits automatic visual expiry but leaves failure presentation to playtest; it does not decide
this omission. [Fork F4](../../scratch/playtest-forks/README.md#f4--automatic-fire-continuity)
owns that choice for owners and observers. The local reload view still consumes and confirms belt
counts; its unconfirmed-reload refresh does not prove refusal or announcement loss.

Rejected alternatives:

- **Wait longer:** unordered delivery supplies no finite maximum lateness. An observed quantile
  cannot become a loss bound.
- **Adopt a synthetic round by belt order:** coalesced counts and reordered announcements do not
  identify a one-to-one mapping; the provisional trajectory also invents the authority's result.
  The removed fallback is preserved in git at `7c0156e3`, with its revert cost recorded in F4.
- **Reliable automatic announcements:** this changes ADR-0021's cosmetic-debt policy to solve a
  client discard that requires no transport change. Bounded copies remain the transport repair.

## Delivery facts

[ADR-0021's delivery table](../adr/0021-fire-replication-architecture.md#delivery-classes)
owns delivery guarantees and their rationale.

## Player outcomes

DERIVED from `net::client` release, `vfx::muzzle`, `sfx`, and `crew_ui`. Ages use the releasing cursor.
The cue window is owned by `assets/vfx/fire_cues.ron`; the catch-up horizon is owned by
`ballistics::MAX_COSMETIC_CATCH_UP_TICKS`. A tracer is drawn only if the announced round's visual
and tracer phase call for it. Reliable paths join independently of announcement arrival order.

| Own-gun case | Machine gun: sees / hears | Main gun: sees / hears |
|---|---|---|
| Announcement on time | Keyed round and path, muzzle flash, report; recoil if the weapon has it. | Keyed round and path, muzzle flash, report and barrel recoil. |
| Late within the cue window | Round catches up along its path; flash and report, recoil if configured. | Round catches up along its path; flash, report and barrel recoil. |
| Late past the cue window, within the catch-up horizon | Round catches up along its path; no flash or report; recoil if configured. | Round catches up along its path; no flash or report; late barrel recoil. |
| Late past the catch-up horizon | No round, flash, report or recoil; traced as `stale_release`. | Same omission and diagnostic. |
| All announcement copies lost | No round, flash, report or recoil. The belt count still drops with local cadence and authoritative confirmation. A reliable damage confirmation may show a hit marker for a round never drawn; its path waits unused until expiry. | A live connection repairs the reliable announcement: it can be late, including past the catch-up horizon, but is not permanently lost. A disconnected connection promises no further presentation. |

The machine-gun belt display follows cadence in every row, independently of the announcement; it
can drop before the round appears. The main gun shows reload state rather than a belt count.
For either gun, a reliable damage confirmation is independent of presentation: a hit marker can
appear even when no round was drawn. A correlated loss window during an own automatic burst can
therefore leave gaps in tracer and sound while the belt display continues counting down.
Other tanks' presentation is unchanged by deleting the own-gun fallback.

## Regression witness

`net::client::tests::belt_arrival_cannot_discard_an_announced_round_or_its_path` mounts the shipped
own-fire and ballistics plugins. It covers both announcement/belt orders and both path/announcement
orders, plus a duplicate announcement. It requires no round before the announcement is released,
then exactly one keyed round, one recoil presentation, and one played sanctioned bounce.

DERIVED by reading the removed fallback: with the belt arriving first, its synthetic projectile
violates the initial zero-projectile assertion before the later identity assertion can run.
The regression guards both assertions.
