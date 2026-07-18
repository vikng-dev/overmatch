//! Track model — the game's tracked-locomotion foundation (architecture:
//! `.agents/docs/design/track-model/architecture.md`).
//!
//! One geometric core (the tagged route over a side's running-gear circles) feeds three
//! consumers: the belt-physics forces (phase B), the simulated-chain view tier, and the
//! route/render view tier. The pure math lives here; `track_sandbox` is the lab that consumes
//! it behind its own rig/course/harness, and the game's `view` plugin (phase A) consumes it
//! behind the tank rig.
//!
//! The math modules (`chain`/`oracle`/`route`/`wheels`) are pure (no ECS, no assets); `view` is
//! the game's ECS adapter, mounted by the windowed clients only.

pub mod chain;
pub mod drive;
pub mod forces;
pub mod oracle;
pub mod route;
pub mod side;
pub mod sim;
pub mod terrain;
pub mod transmission;
pub mod view;
pub mod wheels;

pub use sim::sim_plugin;
pub use terrain::terrain_plugin;
pub use view::view_plugin;
