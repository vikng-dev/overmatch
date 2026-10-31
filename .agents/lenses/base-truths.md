# Lens: base-truths

Principle: `AGENTS.md`, "Tanks are data" (ADR-0034) and "Behaviour emerges from base truths". A
behaviour comes from material properties, base laws and how a thing is built (its construction, in
data). Systems, laws, gates and tests are generic; per-object truth lives only in declarative data.
Review test: would this line survive tank #2, house #2 and a new material unchanged?

## Finding classes

- **Per-object rule**: behaviour keyed on what a body is rather than what it is made of and how it
  is built: a marker component gating physics ("turrets do not slide", "roofs do not fall"), a
  special case for one body type in contact, friction, weight, collapse or the damage model.
- **Named asset in logic**: a tank, building, round or material name in a branch, a constant or a
  lookup outside the asset data.
- **Per-asset constant**: a number tuned for one asset and living in code rather than in that asset's
  data, or a code default that only one asset's data happens to override.
- **Tuned past its truth**: a law parameter far from its handbook or measured value, set to make one
  case look right. The missing factor is the finding.
- **Asset-specific gate**: a test or gate whose pass condition holds only for one asset (fixture
  numbers are fine when labelled fixture data and the assertion is a class invariant).
- **Inferred construction**: geometry, joints or membership guessed from names, mesh shape or a
  loaded view asset, where construction should be authored data.

## Not a finding

- Data selecting among generic laws, or supplying their parameters.
- Labelled fixture data in tests asserting class invariants.
- A generic law with a parameter the data sets for every asset of its class.

## Fix

Find the missing truth: a material property, a construction fact in data, or a law term. Then
delete the rule. Name the truth the fix supplies; a rule moved to another place is not a fix.
