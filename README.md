# Overmatch

A realistic 3D multiplayer tank game (Bevy 0.19 + Avian 0.7).

## Project map

- [Product](.agents/PRODUCT.md) — the top-level authority: values, current milestone, player experience, authority model, scale, and intentional deferrals.
- [Architecture and debt map](ARCHITECTURE.md) — canonical repository structure, dependency direction, migration sequence, and open architectural debt.
- [Glossary](.agents/GLOSSARY.md) — canonical game and simulation vocabulary.
- [Architecture decisions](.agents/docs/adr/) — durable decisions and their rationale.
- [Playtest forks](.agents/scratch/playtest-forks/README.md) — provisional feel decisions that remain deliberately reversible.

## Controls

W/A/S/D drive; left mouse fires the primary weapon; Space fires secondary weapons.
Q/E choose the primary weapon's round, V changes its fuze, and 1–5 select crew stations.
Shift changes view, X toggles x-ray, M opens the spawn map, Tab holds the scoreboard, and Esc opens the menu.

The game's damage model: **F starts or cancels module repair**. The tank brakes and cannot
drive while repairing; fire and turret controls remain available. The own-tank panel shows
each module's progress, then READY when it returns whole. A knocked-out tank cannot repair;
R requests respawn when available. These are game-client controls; sandboxes have their own.

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
a Windows x86_64 client `.zip`, registers the server image as the Edgegap version the new clients
report, smoke-tests both packaged clients, and only then publishes the GitHub Release — so a failed
registration or smoke publishes nothing. Release clients join through the matchmaking Worker
(`backend/`), whose URL is baked in. The signed + notarized Apple-Silicon macOS `.dmg` is cut on
demand, not on the tag: dispatch `Release` with the `tag` input and it is attached to that tag's
existing Release. See `.agents/docs/adr/0009-release-artifacts-and-repo-layout.md` for the full
layout, `DEPLOY.md` for the server, and `scripts/` for local builds (`build-linux.sh`,
`package-macos.sh`) and icon generation (`gen-icons.sh`).
