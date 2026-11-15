//! The game's avian guard files (`src/tests/avian_*.rs`), as siblings so their `super::` paths
//! resolve as in the game.

#[path = "../../../src/tests/avian_child_pose_churn.rs"]
mod avian_child_pose_churn;

#[path = "../../../src/tests/avian_prepared_mass.rs"]
mod avian_prepared_mass;

#[path = "../../../src/tests/avian_spawn_batch.rs"]
mod avian_spawn_batch;

#[path = "../../../src/tests/avian_broad_phase_layers.rs"]
mod avian_broad_phase_layers;

#[path = "../../../src/tests/avian_island_sleep.rs"]
mod avian_island_sleep;

#[path = "../../../src/tests/avian_body_query_gap.rs"]
mod avian_body_query_gap;

#[path = "../../../src/tests/avian_narrow_phase.rs"]
mod avian_narrow_phase;

#[path = "../../../src/tests/avian_moved_aabb_prune.rs"]
mod avian_moved_aabb_prune;

#[path = "../../../src/tests/avian_collider_of_removal.rs"]
mod avian_collider_of_removal;

#[path = "../../../src/tests/avian_moved_lists.rs"]
mod avian_moved_lists;

#[path = "../../../src/tests/avian_warm_start.rs"]
mod avian_warm_start;
