# Overmatch

A realistic 3D multiplayer tank game (Bevy 0.19 + Avian 0.7).

## Project map

- [Product](.agents/PRODUCT.md) — the top-level authority: values, current milestone, player experience, authority model, scale, and intentional deferrals.
- [Architecture and debt map](ARCHITECTURE.md) — canonical repository structure, dependency direction, migration sequence, and open architectural debt.
- [Glossary](.agents/GLOSSARY.md) — canonical game and simulation vocabulary.
- [Architecture decisions](.agents/docs/adr/) — durable decisions and their rationale.
- [Playtest forks](.agents/scratch/playtest-forks/README.md) — provisional feel decisions that remain deliberately reversible.

## Releasing

Bump `version` in `Cargo.toml` on a release branch and refresh `Cargo.lock`:

```bash
cargo check
git add Cargo.toml Cargo.lock
git commit -m "Release vX.Y.Z"
git push origin HEAD
```

Merge that branch through its required PR, then tag that exact commit — the tag is the only thing
that builds, deploys, and publishes a release:

```bash
git switch main
git pull --ff-only
git tag vX.Y.Z
git push origin vX.Y.Z
```

`.github/workflows/release.yml` then produces Linux x86_64 client and server `.tar.gz` archives and
a Windows x86_64 client `.zip`, deploys the built server to the droplet, smoke-tests both packaged
clients, and only then publishes the GitHub Release — so no visible release is ever ahead of the
droplet, and a failed deploy or smoke publishes nothing. The droplet can be ahead of the latest
visible release: a deploy that succeeds before a smoke failure is not rolled back (see `DEPLOY.md`).
The signed + notarized Apple-Silicon macOS `.dmg` is cut on demand, not on the tag: dispatch
`Release` with the `tag` input and it is attached to that tag's existing Release. See `.agents/docs/adr/0009-release-artifacts-and-repo-layout.md` for the full
layout, `DEPLOY.md` for the droplet, and `scripts/` for local builds (`build-linux.sh`,
`package-macos.sh`) and icon generation (`gen-icons.sh`).
