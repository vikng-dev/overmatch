# Tank shadow proxies, #171: parity stills

Hidden 1920×1080 captures from the #171 lane; full frames and logs were not kept. The lever is `OVERMATCH_TANK_SHADOW_PROXY=off|on|always|<metres>`. Pixels count as changed above 8/255; two identical runs changed 0 (MEASURED).

| Distance | `on` (normal) vs `off` | `always` vs `off` |
|---|---|---|
| 10 m | 132 px, all on the settling track belt, not shadow | 14,647 px |
| 50 m | 12 px | 2,121 px |
| 200 m | 50 px (3 above 32/255), on shadow edges and turret tops | — |

- **Silhouette** (`always`): the collider shapes fill the gaps at the fenders and the turret bin. Shadow edges move a few px at 50 m and about 1 px at 200 m. The barrel tube keeps the taper and the muzzle brake.
- **Switch range:** 120 m, ASSUMED, at the last cascade's near edge (√(40×350) ≈ 118 m, DERIVED). To be tuned by pricing.
