//! Track model — the game's tracked-locomotion foundation (architecture:
//! `.agents/docs/design/track-model/architecture.md`).
//!
//! One geometric core (the tagged route over a side's running-gear circles) feeds the belt-physics
//! forces (phase B) and the view. The track view is the memory-enabled kinematic wrap (`wrap`), and
//! the game and the sandbox both run it: the pure math lives here, `track_sandbox` is the lab that
//! consumes it behind its own rig/course/harness, and the game's `view` plugin (phase A) consumes it
//! behind the tank rig.
//!
//! The math modules (`oracle`/`route`/`wheels`/`wrap`) are pure (no ECS, no assets); `view` is the
//! game's ECS adapter over `wrap`, mounted by the windowed clients only.

pub mod drive;
pub mod forces;
pub mod oracle;
// The marker-driven suspension/track model, promoted out of `track_sandbox` (mirrors `forces`):
// `derive` = the universal laws (pure f32 math), `marker_model` = the glb marker read (the
// `DerivedModel`), `rig_geom` = the assembled geometry contract. Crate-internal: `sim`, `view` and
// the sandbox all consume them.
pub(crate) mod derive;
pub(crate) mod envelope;
// The running gear's phase law (sprocket tooth lock + rolling spin + the tooth-tip measurement) and
// the shoe-instancing render layer, both shared verbatim by the game's `view` and the sandbox's
// `wheel_view`/`link_view` adapters. They used to be duplicated per consumer — a phase lock whose
// whole point is that ONE measured constant seats twenty teeth cannot survive two copies of it.
pub(crate) mod gear_phase;
pub(crate) mod link_view;
pub(crate) mod loop_geom;
pub(crate) mod marker_model;
pub(crate) mod rig_geom;
pub mod route;
// The belt's SHADOW CASTER: a low-poly ribbon swept along the same drawn polyline the shoes are
// placed on, so the 1.08 M-triangle shoe pool can stop being re-submitted into every cascade. Pure
// geometry + the mode knob; `view` owns the entities.
pub(crate) mod shadow_proxy;
pub mod side;
pub mod sim;
pub mod terrain;
pub mod transmission;
pub mod view;
pub mod wheels;
pub mod wrap;

pub use sim::sim_plugin;
pub use terrain::terrain_plugin;
pub use view::view_plugin;
