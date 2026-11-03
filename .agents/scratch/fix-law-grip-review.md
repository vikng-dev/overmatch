# Grip-law review dispositions

Subject: the game's driving model, ground contact on tracks.

| reviewer | finding | verified / not a defect | fixed in (subject) |
|---|---|---|---|
| Claude | Drag spends engine pull but never competes with grip | verified: rolling entered only belt reaction | driving: make rolling drag compete with grip (#198) |
| Claude | Driver forecast and delivered hull force disagree above the drag/grip crossover | verified: forecast subtracts full rolling after capping pull | driving: make rolling drag compete with grip (#198) |
| Claude | Heavy fixture cannot distinguish grip from engine limitation | verified: its original crawl was engine-limited | driving: make rolling drag compete with grip (#198) |
| Claude | No slope witness on the shipped brake/transmission composition | verified: locked force pads alone cannot witness brake breakaway | driving: make rolling drag compete with grip (#198) |
| Claude, Codex | Zero nominal footprint span reaches the simulation pressure assertion | verified: positive span was never admitted | driving: admit positive running-gear footprints before spawn (#198) |
| Claude, Codex | Idle-catch refusal does not isolate the added slope terms | verified: both original inertia probes clear or fail both bounds | driving: make rolling drag compete with grip (#198) |
| Claude | Per-soil K readers survive as weighted reads of a constant | verified: every column carried the same K | driving: make rolling drag compete with grip (#198) |
| Claude | Vanishing-patch downshift witness cannot fail | verified: its added patch carries identical coefficients; retired witness removed, nominal observation and mixed-ground tests retain the live evidence | driving: make rolling drag compete with grip (#198) |
| Claude | Loose-piece rise and energy ceilings cannot detect the claimed defect | verified: removed the ceilings; retain actual piece support and settled footprint checks, report rise and piece motion | driving: make rolling drag compete with grip (#198) |
| Claude | Ground-table fixtures pin the shared hard rolling datum | verified: valid fixtures and the below-hard invalid case must read that owner | driving: make rolling drag compete with grip (#198) |
| Claude, Codex | Rolling-fade stability prose prices a retired belt damper | verified: rolling now damps hull travel; the scalar hull bound replaces it | driving: make rolling drag compete with grip (#198) |
| Claude | BeltContact.mu still claims soil-strength conversion | verified: terrain supplies direct grip; solids supply their material pair | driving: make rolling drag compete with grip (#198) |
| Claude | Common K comment points to deleted binding evidence | verified: current invariant and retained explicit-mode rationale replace the claim | driving: make rolling drag compete with grip (#198) |
| Claude | DerivedModel.mass and RigGeom.build call mass only a readout | verified: nominal pressure consumes mass in the simulation | driving: admit positive running-gear footprints before spawn (#198) |
| Claude | ContactOracle.ground_at claims a rigid-pair no-ground test path | verified: the path supplies an explicit calibration coefficient | driving: make rolling drag compete with grip (#198) |
| Codex | nominal_pressure claims to write commands and advance time | verified: the comment belongs on drive_tick | driving: make rolling drag compete with grip (#198) |
| Claude, Codex | Pressure witness claims no engine can sustain any translation | verified: the fade permits a limited crawl | driving: make rolling drag compete with grip (#198) |
| Claude | Section 5a calls historical capacity less drag delivered drawbar | verified: relabel the historical budget; state the corrected delivery law and its fade limit | driving: make rolling drag compete with grip (#198) |
| Claude | Steering analyzer and ADR-0026 still advertise cohesion | verified: capacity is coefficient times elastic normal load | driving: make rolling drag compete with grip (#198) |
| Claude | Edited-file ratchet rows remain open | verified historical omission; load-27..32 and net-21..22 are explicitly reserved to the turret chain here; no other open row names a file edited here | scope reservation in both fix-law-grip commits |
| Claude | FixtureConstruction borrows terrain grip from the track-material slot | verified: give the fixture a separate ground coefficient | driving: make rolling drag compete with grip (#198) |
| Claude | FixtureConstruction.grip is optional but always present | verified: require it in the arithmetic fixture | driving: make rolling drag compete with grip (#198) |
| Claude | Slope entry-speed factor has no stated job | verified: label the fixture's nonzero entry and its arrest-coverage job | driving: make rolling drag compete with grip (#198) |

The footprint/tooling, protocol and authored-number decisions are recorded in track-model
architecture section 5a, which also owns the law narrative and measurements. No other open
backlog row names an edited file. The shear form, K magnitude, damping, membership, dwell,
breakaway, belt step, recoil and solver constraints remain outside this lane.
