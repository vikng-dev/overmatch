# Track lane 1 review

Subject: the game’s tank driving physics, game state only.

Reviewed `7dbbe78f`, `076e5f83` and `633cbe40` at `b23c322c` against the
error-handling, code-branching and base-truths lenses and issue #238’s first lane.

No P1 or P2 findings. The spawn constructor `assemble_tank_body` receives cached
`BumpStops` through `TankContent` and queues them with the body’s other colliders.
It neither reads a GLB nor waits for a view asset. The running-gear GLB read in
`BumpStops::of_blueprint` belongs to startup extraction of the certified sim
artifact, alongside the blueprint; it is not a body-spawn or per-tick path.
The sandbox calls the same constructor, including on retune; its separate hard-stop
collider builder and marker are gone. The top-speed gate derives its bounds from
`TransmissionParams`, not a fixture speed: the top gear, governed speed, and torque
cut-off all follow the fixture’s ladder. Its acceptance bands are explicitly assumed.

The reconnect concern is not a defect. The client link has `Client` and
`ReplicationReceiver`, without `Persistent` or `HostClient`.
Lightyear 0.30.1’s `on_replication_disconnect` observes removal of `Connected` and
queues despawn of every nonpersistent `Replicated` entity. Cleanup runs in the
removal’s command flush. `drive_connection` retries only after the disconnected
state and its backoff; a replacement snapshot cannot precede that cleanup.
The authority installs its ledger once in startup, not on client joins.
Thus `destruction::apply_ledger`’s single-authority invariant survives reconnect.

P3s:

- The tank data’s transmission comment rounded the DERIVED governed speed to
  10.48 m/s although the ladder yields 10.509 m/s. Corrected here.
- `marker_model` still describes its consumer as sandbox-only and says measurement
  will move into the bake when promoted. Startup extraction already uses it; this
  provenance prose is stale. Its file reader is still shared with the development
  measurement tool, so no construction-path change is required by this lane.

Driving: no P1/P2; two P3s, one corrected. Net: no findings.
