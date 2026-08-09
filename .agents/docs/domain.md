# Domain docs

How the engineering skills consume this repo's domain documentation.

## Read before exploring

- **`.agents/PRODUCT.md`** — the product authority: values, current milestone, and explicit deferrals.
- **`.agents/GLOSSARY.md`** — canonical vocabulary.
- **`.agents/docs/adr/`** — accepted decisions touching the area you are about to work in.
- **`.agents/scratch/playtest-forks/`** — when the work touches a deliberately provisional feel decision.

Everything else under `.agents/docs/` is evidence, not authority: verify its claims against the code and the ADRs before relying on them. If a file listed above does not exist, proceed silently — `/domain-modeling` creates them lazily, when a term or a decision actually resolves.

## Use the glossary's vocabulary

When your output names a domain concept — an issue title, a refactor proposal, a hypothesis, a test name — use the glossary's term, not a synonym it explicitly avoids. A concept missing from the glossary is a signal: either you are inventing language the project does not use, or there is a real gap worth noting for `/domain-modeling`.

## Flag ADR conflicts

If your output contradicts an ADR, surface it rather than silently overriding:

> _Contradicts ADR-0007 (event-sourced orders) — but worth reopening because…_
