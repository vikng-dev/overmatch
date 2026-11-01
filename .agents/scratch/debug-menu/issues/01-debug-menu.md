# Debug menu — consolidate debug visualisations behind a proper UI

Status: needs-triage

Subject: the game’s damage-model debug view, game state only.

## Current surface

The debug helpers ship in both client compositions. They have no build-feature switch:

- `G`, off by default, toggles belt-contact support/traction arrows and Avian collider gizmos
  (`debug::ShowGizmos`, `debug::draw_wheel_forces`). A capped impact-marker ring follows the
  same switch. Avian raycast gizmos are disabled; the arrows read the current contact telemetry.
- `X` belongs to the pen cam’s x-ray, outside this debug master switch.
- The orbit camera follows the controlled tank; there is no detach key.
- The sandboxes have their own `dev_ui` panels and independent layer controls.

## Goal

Provide one menu with per-visualisation toggles and controls such as force-arrow scale.
Possible future entries include thrust/lateral-friction vectors, a centre-of-mass marker and
station load readouts. Bevy UI and the optional egui dependency are implementation candidates.

## Notes

This is a proposed consolidation, not a second live control path. Availability of the proposed
menu in packaged builds belongs to triage; the current debug helpers are included there.
Do not block driving work on this proposal.
