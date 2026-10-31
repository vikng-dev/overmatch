# Track lane 2 review

Subject: the game's tank driving physics, game state only.

Reviewed `0ace6e15` (with `b3fcc874` and the Tiger's running gear, `2bad36e4`) against the
error-handling, code-branching and base-truths lenses and issue #238's second lane.

The side plates are built at spawn from data alone: `TrackColliders::of_blueprint` cuts them during
startup extraction of the sim artifact, beside the bump stops, and `spawn_track_colliders` hangs the
cached shapes on every game and sandbox body. `Layer::TrackSide` meets pieces, vehicles, standing
structures and other plates; its filters exclude `Layer::Terrain`, and the pivot gate checks a real
geometric overlap with terrain that forms no contact pair. On the new running gear the three
bump-stop tests and the headless bump-stop gate pass (MEASURED: rest clearance 0.1793 m, was
0.1436 m; the soft-spring tank rests on the stops at −0.0000 m).

P2 (fixed in this lane): the one-rule friction gate had no material class for the plates, so main
went red on `every_collider_carries_its_materials_friction_under_the_one_rule`. The class is now a
rigid track collider's track steel, bump stop or side plate.

Owner decision, not a lane defect: the plates' profile reaches the rest belt's outer face, as #238
specifies, so each plate's bottom (the shoe's depth-wide edge strip, `min(thickness, width / 2)`)
is a rigid floor for a piece under the shoe's edge at the rest face. Under those strips the bottom
is avian's contact, not the belt's push. Keeping only a plate's lateral contacts (a contact hook)
would make "a vertical plate never climbs" literal.

P3s:

- The pivot gate's surface-speed bound (DERIVED 0.7096 m/s) is the largest `|ω × p|` over every
  plate vertex of both sides. It is derived from the tank's own shapes and the fixture pivot, so it
  holds for any tank's data, but it is loose: the piece sits a quarter wheelbase from the pivot,
  and a throw below the plates' far-end speed passes. The plate's speed at the piece is the tight
  bound.
- `bump_stop::SUBSTANCE` is a material named in logic until the track's substance is tank data
  (lane 4).
- `marker_model` described its consumer as sandbox-only; corrected here.

Driving: one P2 (fixed), three P3s (one corrected), one owner decision.
