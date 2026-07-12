# Fire mechanism is a spec-level enum

A weapon's fire mechanism — single-shot with a per-round reload versus belt-fed automatic — is one authored fact, `FireMode`, on `WeaponSpec`:

```rust
enum FireMode {
    Single    { reload_secs: f32 },
    Automatic { rpm: f32, belt_size: u32, belt_swap_secs: f32, tracer_every: u32 },
}
```

It replaces the flat `reload: f32` + `tracer_every: u32` pair, under which the 88 and the MGs ran one code path that was quietly wrong for both: the 88 authored a `tracer_every` it only consulted vacuously (`1` = "every round", because the field existed), and the MGs' 750 rpm cyclic rate was smuggled through the *reload* machinery as `reload: 0.08` — which made rate of fire crew-gated. That was a latent trap: `tick_reload` froze the timer whenever the weapon's `load` requirement was unmet, so a dead Loader would have frozen an MG's cyclic rate mid-belt, a thing no crew casualty physically does. The mechanism split makes the crew-gate split expressible.

## The owner's four calls (2026-07-11)

1. **Finite belt, infinite reserve.** An `Automatic` fires from a `belt_size`-round belt tracked as sim state; there is no stowed-ammo inventory behind it. Running dry automatically starts a `belt_swap_secs` swap — ammunition *pressure* (bursts get interrupted; a dry gun is vulnerable for seconds) without an ammo-management meta the alpha doesn't need.
2. **The swap is crew-gated; the cyclic interval is not.** The belt swap is the human act, gated by the same `load: Requirement` machinery as the 88's reload (dead gun crew = frozen swap = silent gun). The 60/rpm interval between rounds is mechanism and ticks unconditionally. `shooting::tick_reload` branches per mode on exactly this line.
3. **No overheat — deferred.** The enum is the extension point: an overheat model adds fields to `Automatic` (or a new variant) without touching `Single` or the flat spec fields.
4. **`belt_remaining` enters the determinism hash.** The new `WeaponState::belt_remaining` gates fire, so it is folded into `trace::hash_tank_state` (the `hrld` stream, next to the fire timer it modulates). Contrast `rounds_fired`, which stays deliberately excluded: it only picks which rounds trace — a cosmetic phase that a dropped predicted shot legitimately skews by one, and hashing it would flag that benign skew as divergence. The rule the pair demonstrates: *fire-gating state hashes; cosmetic-phase state does not.*

## Consequences

**One fire system, a `match` per weapon.** `shooting::fire` stays singular with a `match` on `FireMode`; the mechanisms are arms, not systems. The schedule edges (`tick_reload → fire` before `ConsumeCommandEdges`; `apply_recoil.after(fire)`) are determinism-load-bearing — the 2026-07-10 recoil-order divergence came from exactly one missing edge — and a per-mechanism system split would force every such edge to be re-proven per pair. State stays root-resident in `TankSim::weapons` for rollback, per the existing doctrine.

**One timer, two meanings.** `WeaponState::reload_remaining` is the single fire timer; for an `Automatic` its meaning is derived from `belt_remaining`: belt has rounds → cyclic interval (ungated), belt dry → swap timer (crew-gated, refills the belt inside the gated tick on the tick it bottoms out, so a swap cannot complete while the gate is unmet). No second timer field, no second hash stream.

**`Trigger` demotes to pure input routing.** Which command field a weapon reads (`Primary` → `fire_primary`, `Secondary` → `fire_secondary`) stays `Trigger`; *how* the weapon consumes it derives from the mode — `Single` consumes a click edge, `Automatic` reads a held level. `TankCommand` and the wire are untouched: `fire_primary` was already an edge and `fire_secondary` already a level, so the net protocol's extrapolation invariant (edges cleared on held-last ticks, levels held) holds unchanged. Zero wire changes; `WeaponSpec`/`Weapon`/`WeaponState` are not replicated.

**Belts start full.** `WeaponState::for_mode` spawns an `Automatic` with `belt_remaining = belt_size` — a fresh belt is spawn config, like a loaded 88. Starting at 0 would hand every MG a phantom first swap, or (with the refill inside the crew-gated tick) a crew-dead spawn whose belt never fills.

**Tracer identity per mode.** `tracer_round` (the belt arithmetic) is called only in the `Automatic` arm; a `Single`'s round always traces — its visual is the shell scene, not a streak, so "tracer" is just "visible round".

## Alternatives rejected

- **Component split** (`SingleShot` / `AutomaticFire` components, per-mechanism fire systems): two systems mutating `TankSim` means re-proving the determinism-critical ordering per pair (the recoil-order lesson), and the "which component does this weapon carry" invariant becomes checkable only at runtime. The enum makes the mechanism a closed, exhaustively-matched fact.
- **Optional fields on `WeaponSpec`** (`belt_size: Option<u32>`, keep `reload` + `tracer_every` flat): every invalid combination — an 88 with a belt, an MG without one, a `tracer_every` on a single-shot gun — parses fine and needs prose + validation code to reject. ADR-0010/0011's point is that the schema should refuse what the sim can't mean; the enum does it in the type.
- **Crew-gating the cyclic interval** (status quo): rejected as the trap described above.
- **Finite reserve / stowed belts**: deferred with the load-out UI; the enum's `Automatic` arm is where a `reserve_belts` field would land.
