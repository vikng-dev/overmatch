# Plugin-per-feature architecture, thin binary over a library crate

Game logic lives in a **library crate** (`lib.rs`) as `GamePlugin`, composed of one plugin per feature (`state`, `world`, `tank`, `camera`, `aim`, `shooting`); `main.rs` only adds the runtime (`DefaultPlugins`) and runs it. Each feature module owns its components, systems, and its own wiring (`pub fn plugin(app)`), so `main`/`lib` never reach in to register a feature's systems.

Two supporting patterns:
- **Shared `GameplaySet`** — play-only systems join this set, which is gated `run_if(in_state(Playing))` once per schedule, instead of every system repeating the run condition. Pausing freezes everything from one place.
- **Reactive behaviour attachment** — `tank` binds *structural* markers (`Turret`, `GunBarrel`, …) from the rig; features attach their *behaviour* (`AimPoint`, `Recoil`) reactively via `Added<Marker>`. So dependencies point at `tank`/`world`/`state` and never sideways or back — a clean DAG.

Chosen so the game can also be mounted headless (`MinimalPlugins`) for tests, features stay decoupled, and `main` reads as a table of contents. Considered keeping everything inline in `main.rs`; deferred the split deliberately until natural seams emerged (~600 lines), then did it as one rewrite.
