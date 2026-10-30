# Kalinovo houses: graybox to brick and timber

These stills compare the scatter buildings before and after the change, for the owner to judge by eye.
`before/` is the flat graybox wall material. `after/` is brick walls under a timber-board top. `compare/` shows each pair side by side, before on the left.

| Scene | What it shows |
|---|---|
| `near-a`, `near-b` | one house at 11–13 m, from a camera 2.6 m above the ground |
| `mid-a`, `mid-b` | a row of houses at 45–55 m |
| `wide` | the western village from 170 m away and 45 m up |
| `roofs` | the north-west cluster from 22 m away and 16 m up, with the top faces in view |

## How they were made

Every frame comes from a hidden, muted offline run (`capture.sh`). Each run uses a fixed 1/128 s step, takes its frame at app time 12 s, and has no UI or gizmos. The before binary is built from `6e28eb6`, the after binary from the lane branch.

## Per-pixel check (MEASURED)

A pixel counts as changed when any channel moves by more than 12/255 between before and after. Changed pixels fall only on the house faces. The terrain, the trees and the houses' shadows show no change above that threshold, so each building's silhouette and shadow are the graybox's own.

Changed share of the frame: near-a 17.6 %, near-b 18.8 %, mid-a 2.0 %, mid-b 2.4 %, wide 0.8 %, roofs 5.7 %.

Below that threshold, the whole frame shows low-bit differences between the two runs.

## Packs

The packs are Poly Haven, CC0. Each pack's `cc.txt` under `assets/buildings/` holds its attribution and authored size.

- Walls: `brick_wall_006`, 3 m repeat, https://polyhaven.com/a/brick_wall_006
- Roof: `dark_planks`, 2 m repeat, https://polyhaven.com/a/dark_planks
