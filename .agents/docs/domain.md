# Domain Docs

How the engineering skills should consume this repo's domain documentation when exploring the codebase.

## Before exploring, read these

- **`.agents/PRODUCT.md`** for the current product target and explicit deferrals.
- **`.agents/GLOSSARY.md`** for canonical vocabulary.
- **`.agents/docs/adr/`** for accepted decisions that touch the area you're about to work in.
- **`.agents/scratch/playtest-forks/`** when the work touches a deliberately provisional feel decision.

Historical research and implementation logs are evidence, not current truth. Follow their successor links and verify their claims against the code and accepted ADRs before relying on them.

If any of these files don't exist, **proceed silently**. Don't flag their absence; don't suggest creating them upfront. The `/domain-modeling` skill (reached via `/grill-with-docs` and `/improve-codebase-architecture`) creates them lazily when terms or decisions actually get resolved.

## File structure

Single-context repo (this repo):

```
/
├── .agents/
│   ├── PRODUCT.md
│   ├── GLOSSARY.md
│   └── docs/adr/
│       ├── 0001-some-decision.md
│       └── 0002-another-decision.md
└── src/
```

## Use the glossary's vocabulary

When your output names a domain concept (in an issue title, a refactor proposal, a hypothesis, a test name), use the term as defined in `.agents/GLOSSARY.md`. Don't drift to synonyms the glossary explicitly avoids.

If the concept you need isn't in the glossary yet, that's a signal — either you're inventing language the project doesn't use (reconsider) or there's a real gap (note it for `/domain-modeling`).

## Flag ADR conflicts

If your output contradicts an existing ADR, surface it explicitly rather than silently overriding:

> _Contradicts ADR-0007 (event-sourced orders) — but worth reopening because…_
