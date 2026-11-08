# A per-manifold compliant contact law

Subject: the video game's tank driving physics (the track belt's support); game state only.

**Target:** vendored Avian 0.7.0, Bevy 0.19.1. **Status:** LOCAL DRAFT, not filed; upstream head
was not checked. It serves the game's track belt, where a compliant contact
carries the belt's support against loose pieces and a separate rigid collider is its hard limit.

Avian's contact softness is global (`ContactSoftnessCoefficients`), mass-normalised, and only
governs how fast overlap is pushed out: the relaxing pass solves every contact rigidly along its
normal (`-m_eff · v_n`, `ContactNormalPart::solve_impulse` with `use_bias = false`). A contact
therefore cannot be a spring. Under a steady load its overlap decays to zero, whatever the
softness, so a vehicle cannot rest on its contacts at a load-proportional deflection.

The patch adds `ContactManifold::compliance: Option<ContactCompliance>`, an absolute normal law
per point (`stiffness` N/m, `damping` N·s/m), reset by the narrow phase each step and set by a
contact-modification hook. Constraint preparation turns it into soft-constraint coefficients from
the point's own effective mass and the substep length, and the normal solve keeps those
coefficients in the relaxing pass and does not cap them by `max_overlap_solve_speed`.

DERIVED: with `a = h(c + hk)` the coefficients are bias `k/(c + hk)`, mass scale `a/(m + a)` and
impulse scale `m/(m + a)`, the exact rewrite of `SoftnessParameters::compute_coefficients` at
`ω² = k/m`, `2ζω = c/m`. The fixed point of the soft update under a steady load is
`impulse = k · overlap · h`, the same in both passes, so the static overlap is `F/k`. Absolute
laws sum over points; a per-contact frequency would make the total stiffness grow with the number
of points and with the masses involved.

One further rule: a compliant point at positive separation applies no impulse. The stock
speculative branch would stop the bodies rigidly at the spring's surface, which MEASURED as a
2.4 MN single-tick impulse against a 0.08 m bar.

Files: `collision/contact_types/mod.rs` (field, type, coefficients), `collision/mod.rs` (prelude),
`collision/narrow_phase/system_param.rs` (reset), `dynamics/solver/contact/mod.rs` and
`normal_part.rs` (coefficients, compliant branch), `dynamics/solver/plugin.rs` (substep length).
Each site is marked `OVERMATCH PATCH`.
