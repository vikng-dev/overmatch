# Deploy — the dedicated server

The server runs on **Edgegap**, one deployment per match, started by the matchmaking Worker
(`backend/`) for the game version a joining client reports. Release clients carry the Worker URL
(`OVERMATCH_DEFAULT_API`, baked by `.github/workflows/release.yml`) and no server address.

## Release → Edgegap

A `vX.Y.Z` tag builds the Linux `overmatch-server` once (`.github/actions/build-server`), wraps that
tarball in the container image `ghcr.io/vikng-dev/overmatch-server:<version>-<sha7>`
(`.github/actions/server-image`), and registers it as Edgegap version `<version>` — the client's
`CARGO_PKG_VERSION`. The GitHub Release stays a draft until that registration and both archive
smoke lanes have passed. The registration's request bodies, its settings and the on-demand path
(`edgegap-version.yml`, a commit without a release) are in `backend/README.md`, Deploy step 7.

- **Versions do not collide.** Each release registers its own version name, so tagging never
  touches the servers of the version already in play.
- **`main` past the last tag** builds a client of the same version whose fingerprint differs
  (`.agents/docs/adr/0018-wire-surface-fingerprinted-and-refused.md`); the server Edgegap starts for
  that version refuses it. Playtest `main` against a local server, or cut a release.

## The server payload

The image is the tarball's tree at `/opt/overmatch-server/{overmatch-server,assets/}`, which the
server resolves beside its binary with no env, listening on **UDP 5888**. `assets/` there is **only what
the server opens**, an allowlist stated in `.github/actions/build-server/action.yml` rather than the
client's whole tree:

| Path | Opened by |
|---|---|
| `assets/maps/*/level.json`, `assets/maps/*/*.png` | `crate::map::parse`, the terrain height decode, and the handshake content digest. Every map: `$OVERMATCH_MAP` picks one at runtime, and the heightmap's file name is inside `level.json`. |
| `assets/maps/*/prototypes.ron` | `crate::map::read_prototypes` and the content digest (`map::load` panics without it) |
| `assets/trees/**/*.ron` | `crate::trees::TreeLibrary::load` (species table, wood row, each `<id>.tree.ron`) and the content digest |
| `assets/<id>/<id>.sim.glb` | the bake's consumer contract, the `geometry_lod` trio fingerprint, and the track rig — the one geometry artifact the server reads (ADR-0035) |
| `assets/<id>/<id>.lod.json` | `geometry_lod::load_certificate` (panics without it) |
| `assets/<id>/<id>.tank.ron` | the spec sheet, through the `AssetServer` |

The 64 MB view glb, `shell/shell.glb`, the terrain KTX2, `terrain/ground_types.ron`, the vfx
atlases, the shaders and the fonts stay out: every one is loaded by a plugin the server does not
mount, or behind the `!windows.is_empty()` branch a window-less server never takes. The shell model
in particular belongs to `ballistics::view_plugin` — the server mounts `ballistics::sim_plugin`,
whose projectiles carry no scene root at all. `materials/materials.ron` and the `ballistics/`,
`guns/` and `rounds/` RON are `include_str!`d into the binary.

## Local and LAN play

Without a Worker URL the client connects directly. `OVERMATCH_SERVER=<host>[:port]` names any
server (port 5888 by default) and wins over a baked or `OVERMATCH_API` Worker URL; with no target
at all the client dials `127.0.0.1:5888`. The server binary runs as is:
`cargo run --bin overmatch-server`.
