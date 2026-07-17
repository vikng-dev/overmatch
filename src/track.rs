//! Track model — the game's tracked-locomotion foundation (architecture:
//! `.agents/docs/design/track-model/architecture.md`).
//!
//! One geometric core (the tagged route over a side's running-gear circles) feeds three
//! consumers: the belt-physics forces (phase B), the simulated-chain view tier, and the
//! route/render view tier. The pure math lives here; `track_sandbox` is the lab that consumes
//! it behind its own rig/course/harness, and the game's view plugin (phase A, upcoming)
//! consumes it behind the tank rig.
//!
//! Everything in this module is pure (no ECS, no assets): callers own the adapters.

pub mod chain;
pub mod oracle;
pub mod route;
pub mod wheels;
