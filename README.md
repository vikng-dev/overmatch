# Overmatch

A realistic 3D multiplayer tank game (Bevy 0.19 + Avian 0.7).

## Releasing

Bump `version` in `Cargo.toml`, then tag it — the tag triggers CI to build and publish a GitHub Release:

```bash
git commit -am "Release vX.Y.Z" && git tag vX.Y.Z && git push origin main --follow-tags
```

CI (`.github/workflows/release.yml`) produces, per platform: Linux `.tar.gz`, Windows `.zip`, and a signed + notarized macOS universal `.dmg` (binary + assets). See `.agents/docs/adr/0009-release-artifacts-and-repo-layout.md` for the full layout, and `scripts/` for local builds (`build-linux.sh`, `package-macos.sh`) and icon generation (`gen-icons.sh`).
