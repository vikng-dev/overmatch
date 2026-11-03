# Ground-contact delivery review (#198)

Scope: the driving model. The contact law and its parameters are unchanged.

| reviewer | finding | verified / not a defect | fixed in (subject) |
|---|---|---|---|
| Claude | Stop forecast omits delivered rolling; its test repeats the omission | Verified: a saturated adapter stop forecast 98,396.8 N against delivered 159,326.58 N, including 60,929.89 N rolling (MEASURED red regression). The forecast now reads the contact law's delivered rolling vector, projected opposite lane travel; its regression compares saturated delivery, and the caller witness varies rolling independently of grip. | driving: forecast stops from delivered rolling drag (#198) |
| Claude | Recipient reactions and contact travel speed have no distinguishing rolling witness | Verified: recipient fixtures had zero rolling or rolling parallel to grip. Added perpendicular grip/rolling with an independently specified total vector for both reaction routes, and moving-support cases with zero and full support-relative drag. Grip-only whole-force and clamped-direction mutations, and hull-only travel speed, each fail the targeted assertion. | driving: witness complete recipient reactions (#198) |
| Claude | Rubble module reaction and direction prose still says grip alone | Verified: both recipient paths use grip plus rolling. The module now names the total tangent reaction and its complete force. | driving: witness complete recipient reactions (#198) |
| Claude, Codex | Piece-impulse test promises a grip-only magnitude bound | Verified: the bound is `abs(grip + rolling) dt`, alongside the separate Coulomb bound. Corrected the test contract. | driving: witness complete recipient reactions (#198) |
| Claude | Force module order and coast test claim rolling enters belt balance | Verified: support precedes total tangent force, while only longitudinal grip enters belt balance. Corrected both comments, preserving the coupled coast derivation. | driving: witness complete recipient reactions (#198) |
| Claude | Hard-support datum charges hull travel regardless of belt motion | Verified; report only. A locked belt pays it during hull travel, while spinning belts on a stationary hull create no such load on the engine. Its running-gear provenance remains a question for the owner. | Unchanged, as instructed |
