# Dev sweep review

Subject: the game’s dev tooling; rounds, crew and impacts mentioned here are the game’s damage
model and game state only. Lane `lane/sweep-dev-1`, starting at `48f9df93`.
MEASURED clock start: 2026-11-01 07:58:39 +07; deadline: 09:28:39 +07.
No agents, captures, pricing runs, deployments or pushes.

## Review of sweep-view-2

Read the six commits after `8904a30e` against the three lenses and the #215 brief.
Standards and correctness: **P1 V-RATION, fixed**. A single global smoke counter let a shot from
another mount change this mount’s authored ration. Cadence now belongs to `(tank, weapon)`;
changing ammunition preserves it, other mounts cannot advance it, far shots still count, and
stale presentations do not. Unattributed developer shots form one named stream. Despawned
sources leave the table at the next aging pass. No wire type changes.
MEASURED validation: workspace CI clippy and formatting passed; 16 muzzle tests and 3 citation
tests passed, including interleaved sources and ammunition selection. The macOS test linker
reports its existing compact-unwind size warning; the gates exit successfully.

Spec and base truths: no additional P1/P2 found. Required gun `muzzle` blocks and ground
`dust.erodibility` refuse omission and invalid domains before play. Their values explicitly say
ASSUMED, and belong to the firing gun or the painted ground, with no calibre-based recipe.
The dust law rolls the authored count and multiplies by blended erodibility within the authored
reach. This remains provisional presentation data, not measured gas or soil physics.
`controlled_one` enforces HUD control cardinality; the reverse steering-table clamp matches the
transmission. A perforation mark reads penetrator diameter; a stop or bounce reads the flying
assembly; the record cutoff still reads the flying body. Keeping the APCR fireball independent
of the smaller penetrator is correct: the gun’s muzzle emission supplies it.

P3s retained from that review: VFX admission reads whole texture files for a prefix check;
`rounds::effects::resolve` resists rustfmt; sprite playback and fill-glow appearance remain
shared renderer facts. A misplaced terminal-type introductory sentence above
`face_contact_caliber` is a documentation P3. Owner’s batched eye review still owns calibration.
The existing ballistics P2 (`ballistics::view` choosing a scene/streak by calibre), and the impact
emission geometry P2, remain assigned to the ballistics sweep.

## Dev inventory

Disposition IDs below identify the inventory’s symbol groups (the inventory gave no numeric IDs).
The sandbox wall and its subtree belong to part 2 and are excluded from this lane.
