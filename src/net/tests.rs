//! The netcode boundary scan and the tripwires on the lightyear behaviour `net` compensates for.
//! Each file's module doc says what it guards and what its failure means. They live under
//! `src/net/` because each names lightyear, and only the netcode layer may (`net_boundary`).

mod net_boundary;
mod net_fire_release;
mod net_input_buffer_wrap;
mod net_interp_delay;
mod net_sync_margin;
