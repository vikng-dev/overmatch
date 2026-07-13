# Overmatch

A realistic 3D multiplayer tank game (Bevy 0.19 + Avian 0.7).

## Project map

- [Architecture and debt map](ARCHITECTURE.md) — canonical repository structure, dependency direction, migration sequence, and open architectural debt.
- [Product target](.agents/PRODUCT.md) — current player experience, authority model, scale, and intentionally deferred capabilities.
- [Glossary](.agents/GLOSSARY.md) — canonical game and simulation vocabulary.
- [Architecture decisions](.agents/docs/adr/) — durable decisions and their rationale.
- [Playtest forks](.agents/scratch/playtest-forks/README.md) — provisional feel decisions that remain deliberately reversible.
- [Historical roadmap](ROADMAP.md) — early sequencing preserved as history; it is not current status.

## Releasing

Bump `version` in `Cargo.toml`, then tag it — the tag triggers CI to build and publish a GitHub Release:

```bash
git commit -am "Release vX.Y.Z" && git tag vX.Y.Z && git push origin main --follow-tags
```

CI (`.github/workflows/release.yml`) produces, per platform: Linux `.tar.gz`, Windows `.zip`, and a signed + notarized macOS universal `.dmg` (binary + assets). See `.agents/docs/adr/0009-release-artifacts-and-repo-layout.md` for the full layout, and `scripts/` for local builds (`build-linux.sh`, `package-macos.sh`) and icon generation (`gen-icons.sh`).
