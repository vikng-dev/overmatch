# Contact fixes re-review

Subject: the multiplayer tank game's server contact physics; game state only.

## Decisions and guards

The distance/depth and area selections consider only bearing points when any point bears;
construction visits preferred indices before input-order fill. All-bearing selection retains
upstream's exact output. A lone bearing point has no second endpoint, so it supplies no area
extremes and spare slots come from the fill pass.

The mixed/subset relation compares membership when pruning the bearing subset selects four
points. Exact all-bearing compatibility and the retained mixed fill rule preclude unconditional
equality for degenerate subsets: upstream can return fewer than four even with more than four
bearing candidates. Degenerate mixed manifolds can retain additional bearing candidates.

Projected deduplication uses exact equality and applies to speculative candidates; bearing
candidates still take priority. The duplicate relation adds a separated point at an existing
projected anchor and requires the retained stops to remain unchanged, then makes that point
bearing and requires both bearing points to survive.

`normal_speed` justifies speculative capacity for the contact reducer; separation monotonicity
forbids rewarding separation. The scorecard excludes sensor and disabled-body pairs because they
generate no solver constraints. Its impulse citation names experiment commit `1a488c66`.

## Fork inventory

MEASURED against the cached published avian3d 0.8.0 archive, SHA-256
`ca7479ccec5f1440675946bada8c805f2b6d514af78e1d2b0937ca53b1e2c543`;
the pristine source tree was checked byte-for-byte against that archive.
Count both added and removed source lines against upstream: **938 → 968** total,
**890** in upstream files plus **78** fork-only. The contact-types file is **58** lines,
including **53** pruning lines (previously **23**).

## Validation

The initial relation run failed on the existing mixed selection. The final relation compares
membership, since output order has no downstream contract.

The requested input-order mutant was applied: preferred indices were ignored and bearing
points admitted by input order. `scripts/serial-cargo test --lib --features dev_ui,bitprobe
avian_contact_pruning` failed the membership relation (`n=6, mask=47, rotation=0`); the mutant
was reverted before final gates. Both other pruning guards passed under the mutant.

Formatting and CI clippy passed, followed by **43 passed, 7 ignored** targeted tests
(pruning, contact reduction and the scorecard module). One fresh complete scorecard capture
passed every law check. All builds used the serial queue and `dev_ui,bitprobe`, offline;
no timing measurements were made.

## Scorecard observations

MEASURED by the single fresh capture alongside the saved earlier columns, without acceptance
targets. Earlier-after values come from the preceding lane report; before and d71637c3 have
saved JSON captures. The baseline counted all solver pairs/manifolds (**26.6328** per tick on
the layer), so its pair count is not a bearing-support comparison.

| Observation | Before | Earlier after | d71637c3 | Re-review |
|---|---:|---:|---:|---:|
| Full-width wall normal, kN | 280.792 | 367.760 | 372.579 | 372.417 |
| Wall vertical friction, kN | −32.650 | −82.154 | −89.929 | -89.639 |
| Left/right wall normal, kN | 197.664 / 197.852 | 193.694 / 195.217 | 193.694 / 195.240 | 193.694 / 195.240 |
| Layer support / weight | 1.000010 | 0.999992 | 1.000004 | 1.000005 |
| Layer reaction ratio | 0.999955 | 0.999991 | 0.999973 | 0.999974 |
| Layer vertical jitter, mm RMS | 28.922 | 28.928 | 28.854 | 28.870 |
| Layer peak piece speed, m/s | 0.23249 | 0.20167 | 0.15328 | 0.15328 |
| Layer bearing pairs/manifolds per tick | — | 15.4766 | 15.4141 | 15.4453 |
| Layer bearing points per tick | — | 60.2813 | 60.0117 | 60.1992 |
| Layer total solver points per tick | 99.8320 | — | 106.6055 | 106.3398 |
| Highest step climbed, m | 0.6 | 0.6 | 0.6 | 0.6 |
| 0.6 m step peak pitch, degrees | 14.834 | 14.936 | 15.196 | 15.094 |
| Pivot yaw, rad/s | −0.128220 | −0.128220 | −0.128220 | -0.128220 |

