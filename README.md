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

Bump `version` in `Cargo.toml` on a release branch and refresh `Cargo.lock`:

```bash
cargo check
git add Cargo.toml Cargo.lock
git commit -m "Release vX.Y.Z"
git push origin HEAD
```

Merge that branch through its required PR without `[skip deploy]`. Wait for the **Deploy to
droplet** workflow to succeed, and verify its logged `Deployed SHA on droplet` equals the merged
`main` SHA. Only then tag that exact commit; the release gate independently requires the tag to
name current `main` and a successful, non-skipped deploy job for the same SHA before it publishes:

```bash
git switch main
git pull --ff-only
git tag vX.Y.Z
git push origin vX.Y.Z
```

CI (`.github/workflows/release.yml`) produces Linux x86_64 client and server `.tar.gz` archives,
a Windows x86_64 client `.zip`, and a signed + notarized Apple-Silicon macOS `.dmg` (binary +
assets). See `.agents/docs/adr/0009-release-artifacts-and-repo-layout.md` for the full layout, and
`scripts/` for local builds (`build-linux.sh`, `package-macos.sh`) and icon generation
(`gen-icons.sh`).
