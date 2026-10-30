# Kalinovo's houses as matter (#182): before and after

Subject: the game's environment destruction model, game state only.

These stills compare Kalinovo's houses before and after #182, for the owner to judge by eye.
`before/` is today's house: one brick-and-timber box per house, a `TerrainMap` block.
`after/` is `house_1`, baked by the asset tools and spawned from `assets/buildings/house_1/house_1.building.ron`.
Each house is a hollow one-storey brick shell with windows, a door, wall plates and a gable roof of timber boards.
Its walls are ballistic colliders.
`compare/` shows each pair side by side, before on the left.

| Scene | What it shows |
|---|---|
| `near-a`, `near-b` | One house at 11–13 m, from a camera 2.6 m above the ground (the `houses-pbr` poses) |
| `mid-a`, `mid-b` | A row of houses at 45–55 m |
| `wide` | The western village from 170 m away and 45 m up |
| `roofs` | The north-west cluster from 22 m away and 16 m up |
| `window` | house_0027's east wall: the windows and the door, through which the far wall's window and the terrain behind show |
| `after/tracers-mg` | MG fire at house_0027's east wall from a Tiger 25 m away, seen from behind the tank with the HUD up. The bore dot and the strike flash sit on the pier beside the first window. |
| `after/tracers-88-strike`, `after/tracers-88-after` | The 8.8 cm round's tracer striking that pier, and 0.1 s later, from beside the house |

## How they were made

Every frame comes from a hidden, muted offline run (`capture.sh`).
The house scenes use a fixed 1/128 s step, take their frame at app time 12 s, and show no UI or gizmos.
The tracer scenes spawn the player's tank with `OVERMATCH_START` and fire on `SPIKE_AUTO_FIRE`'s real-time schedule.
Their times are app seconds, which track real time, so those frames are not reproducible to the frame.
`poses.py` computes the house frames and terrain heights.
`compare.py` writes the JPEGs and the per-pixel numbers.

- **Before binary:** built from main at `c804cfd`.
  It runs on an asset root whose `prototypes.ron` and `building_surface.wgsl` are main's, and whose other files are this lane's, symlinked.
- **After binary:** built from the lane at `6d05971`.

Capture quirk: the third-person aim picks along the hidden window's centre ray, not the capture target's (`aim::commit_aim` reads `window.size() / 2`).
A frame smaller than the window's logical 1280 × 720 at the capture's scale of 2 therefore lays the gun off the frame's centre.
The tracer scenes are captured at 2560 × 1440 for that reason.

The pen cam did not open on a wall strike in these runs.

## Per-pixel check (MEASURED)

A pixel counts as changed when any channel moves by more than 12/255.

| Scene | Changed share of the frame |
|---|---|
| near-a | 19.5 % |
| near-b | 24.7 % |
| mid-a | 2.0 % |
| mid-b | 2.1 % |
| wide | 1.1 % |
| roofs | 10.8 % |
| window | 72.8 % |

The changes fall on the houses, their shadows, and the ground seen through the openings.

## Static colliders (DERIVED)

These figures cover houses only, from the shipped manifest's 74 `house_proxy` instances.

- **Before:** 74 box colliders, all `TerrainMap` blocks.
- **After:** 74 × 31 = 2294 element colliders, none of them a block. Per house: 11 piers, 7 sills, 7 lintels, 2 gables, 2 wall plates and 2 roof planes.

The 1163 chunks per house stay data: no chunk is a collider.
