# Ground-contact delivery review (#198)

Scope: the driving model. The contact law and its parameters are unchanged.

| reviewer | finding | verified / not a defect | fixed in (subject) |
|---|---|---|---|
| Claude | Stop forecast omits delivered rolling; its test repeats the omission | Verified: a saturated adapter stop forecast 98,396.8 N against delivered 159,326.58 N, including 60,929.89 N rolling (MEASURED red regression). The forecast now reads the contact law's delivered rolling vector, projected opposite lane travel; its regression compares saturated delivery, and the caller witness varies rolling independently of grip. | driving: forecast stops from delivered rolling drag (#198) |

Forecast gates: CI-feature workspace/all-target clippy, formatting and 39 targeted
adapter/caller/citation tests pass. The saturated-stop regression was red before the fix.
