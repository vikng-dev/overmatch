# Penetration proxy: the minimal model (#147)

Subject: the game's damage model; game state only.

**Owner decision, 2026-11-02.** The per-type preset proxy replaces the per-round construction facts and the shatter gate. It amends the package accepted the same day: the pressure contact law, a straight bite and the geometry rebuild stand; per-round hit length and "the round type holds no numbers" go.

The reason: most of the spread the four-gun survey found inside a type comes from speed, mass and calibre, which stay per round. What remains moves a skid limit by up to about 5° (DERIVED), about the law's own error. A player cannot perceive it.

## Laws

| Law | Form |
|---|---|
| Flight | Drag from mass, calibre and the type's flight nose |
| Potency | The existing closed-form capability law: mass, calibre, speed, hit nose |
| Armour in the way | Line-of-sight chord × material factor × slope charge |
| Skid | `ρ v² cos²θ < R_eff sin θ`, `R_eff = min(f·R, 4τ·x·√(x/s))`, `x = t_n/d`, `s = R/(4τ)` |
| What's left | Leftover potency and the energy-lost dose (existing) |

**Slope charge.** `charge = cos β cos θ / cos(θ + β) = cos θ / (cos θ − sin θ tan β)`, with β the type's slope penalty. It is exactly 1 head-on, so potency keeps its normal-impact meaning, and it rises with obliquity. A face softer than the reference pays a share of it: `1 + min(1, f)·(charge − 1)`. It is finite by a `Resolution` ceiling, because `θ + β → 90°` has no meaning in a bite the skid law has not already caught.

## Data

- Per round: mass, calibre, muzzle speed; a core round adds its core's mass and calibre.
- Per type: flight nose, hit nose, whole-or-core, `contact_density` (ρ above), `slope_penalty` (β).
- Per material: its resistance factor.
- Global: the reference steel's strength term, shear flow stress and density.

| Type | ρ, kg/m³ | β | Basis (DERIVED from the survey) |
|---|---|---|---|
| Capped steel | 6,000 | +8° | ρ passes 47 of 50 capped firing rows with the thin-plate term; the misses are t/d 0.16 at 140–237 m/s. Every row is a bite, so it bounds ρ from below only. β averages +8.8° at 590 m/s and up |
| Uncapped sharp steel | 6,000 | +8° | ρ near the class's measured m/(A·L). β averages +4° slow to +12° fast |
| Uncapped blunt steel | 6,000 | +2° | Survey +1 to +2° ([S], calculated). No shipped round uses this row yet |
| Tungsten core | 14,000 | +16.5° | ρ from a core's m/(A·L). β is the survey's tungsten-carbide ratios restated in this charge's own form |

Every preset is a `StandIn` naming the deformation and shatter law it replaces. So is the thin-plate term: the missing law is plate capacity below one calibre, where a pointed round opens the plate rather than plugging it. It rests on by-eye 0° limits from NPG 1125 Fig. 3, and it is unchanged at `x ≥ s`. Rows and limits: `cap-contact-sources-2026-11-02.md`.

## What it reproduces (DERIVED)

A round with 120 mm of normal potency defeats:

| | 30° | 60° |
|---|---|---|
| The former entry bend | 110 mm | 80 mm |
| Straight, line of sight | 104 mm | 60 mm |
| Capped steel, β +8° | 95.5 mm | 45.4 mm |
| Tungsten core, β +16.5° | 86.2 mm | 29.2 mm |
| Measured class ratios | 85–97 mm | 31–49 mm |

Steel ρ 6,000 at 773 m/s, 88 mm, skid limit: 82.0° on 10 mm, 65.7° on 45 mm, 53.5° on 80 mm, 37.5° on thick plate; 53.3° on 45 mm at 500 m/s.

## Given up

- Two rounds of one type differ only by mass, calibre and speed.
- No visible shatter event; uncapped rounds lose their speed dependence.
- A new construction needs a preset rather than emerging from its facts.
- Capped and uncapped steel share one contact density, so they skid alike and differ in flight. No row at t/d ≥ 1.6, θ ≥ 40°, ≥ 600 m/s tests that: such a pair is the one thing that could restore a capped premium.

Rejected: a flat skid angle plus an overmatch ratio per type. It reads easily but needs more numbers, and a single angle cannot span the capped 88 mm's 77° on thin plate to 43° on thick. Per-round construction facts: `round-type-profiles-2026-11-02.md` §7 keeps the field set if a later round needs it.
