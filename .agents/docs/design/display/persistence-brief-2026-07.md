# Local persistence / settings file — research brief (2026-07-27)

Source-verified brief for the player-settings persistence track. Produced by a research agent from
first-party source reading (bevy 0.19.0 `bevy_settings` + `bevy_platform::dirs`, ron 0.12,
serde 1.0.229, `std::fs`), Apple/Microsoft primary docs, crates.io API metadata, and a
shipped-Bevy-game survey. Commissioned while `src/settings.rs` was in flight, to settle the config
shape before it ossifies.

**The verdict in one line: the in-flight architecture — RON + `version` field + `#[serde(default)]`
+ platform config dir + atomic tmp-and-rename — is CORRECT and must not be redesigned. What follows
is a change-list, plus one reversal (item 11).**

## 0. Scope and status

`src/settings.rs` (699 lines) + `src/settings/ui.rs` landed during this research. It is *ahead of*
both community crates and Bevy 0.19's own built-in on every axis verified: correct macOS directory
(Bevy's violates Apple guidance, §5), atomic writes (`bevy-persistent`'s are not, §1),
human-readable format (`bevy_pkv`'s is not, §1), and loud corruption handling.

Two items below are corrections to earlier drafts of this brief and are marked **⚠️ CORRECTION** /
**⛔ SUPERSEDED**. If any of this doc is relayed piecemeal, those two must not be quoted from the
first pass.

## 1. Ecosystem 2026 — no crate earns its dependency

**Bevy 0.19 ships a first-party settings framework.** Crate `bevy-settings`, non-default cargo
feature `bevy_settings`, re-exported as `bevy::settings`. Cart got the crates.io name donated
("A special thanks to Andhrimnir for giving Bevy ownership of the `bevy-settings` crate name").
[Release notes](https://bevy.org/news/bevy-0-19/) ·
[persisting_window_settings example](https://github.com/bevyengine/bevy/blob/v0.19.0/examples/window/persisting_window_settings.rs)

**Do not adopt it.** Read from
[`store_fs.rs`](https://github.com/bevyengine/bevy/blob/v0.19.0/crates/bevy_settings/src/store_fs.rs)
and `lib.rs` at the v0.19.0 tag:

- Writes `{preferences_dir()}/{app_name}/settings.toml`. On macOS `preferences_dir()` is
  `$HOME/Library/Preferences` — which Apple explicitly forbids for self-written files (§5).
- `resources_to_toml` builds a fresh `toml::Table::new()` every save, so **comments and unknown
  keys in the player's file are destroyed on every write**.
- `load_properties` iterates the *struct's* field names and does
  `if let Ok(field_value) = deserializer.deserialize(...)` — a bad value **silently reverts that one
  field to default with no log and no error**. Very tolerant, entirely silent.
- **No version field, no migration story at all.**
- `toml::Value::try_from(serializer).unwrap()` — a panic on any non-TOML-representable type, i.e.
  exactly the shape a keybindings map takes.
- `store_fs::save` logs the temp-write error and then **renames anyway**.
- `warn!("Filename {filename}.toml not found")` fires on every first launch.

| Option | Latest | Total DL | Why not here |
|---|---|---|---|
| `bevy_settings` (first-party) | 0.19.0, 2026-06-19 | 18.8k | Above. |
| [`bevy_pkv`](https://github.com/johanhelsing/bevy_pkv) | 0.16.0, 2026-06-20 | 80.7k | MessagePack in a **redb** embedded DB — not hand-editable, not diffable. [`path.rs`](https://github.com/johanhelsing/bevy_pkv/blob/main/src/path.rs) **silently falls back to `"."`** when `ProjectDirs` resolution fails, i.e. writes its database into the cwd. |
| [`bevy-persistent`](https://github.com/umut-sahin/bevy-persistent) | 0.11.0, 2026-06-22 | 41.3k | No RON path opinion; **no migration support** ([issue #45](https://github.com/umut-sahin/bevy-persistent/issues/45), open; maintainer: "any trick you can do [in serde], you should be able to do here"). **Native write is truncate-in-place, not atomic tmp+rename** ([`storage.rs`](https://github.com/umut-sahin/bevy-persistent/blob/main/src/storage.rs)) — our `save()` is strictly better. Its one feature, `revert_to_default_on_deserialization_errors`, is already hand-implemented with a better complaint string. |
| [`moonshine-save`](https://github.com/Zeenobit/moonshine_save) | 0.7.0, 2026-06-21 | 38.1k | Whole-world entity save/load. Different problem. |

**No community norm exists to defer to** — every candidate sits at 18k–80k lifetime downloads, and
the official quickstart template [`TheBevyFlock/bevy_new_2d`](https://github.com/TheBevyFlock/bevy_new_2d)
**persists nothing at all**: verified in
[`src/menus/settings.rs`](https://github.com/TheBevyFlock/bevy_new_2d/blob/main/src/menus/settings.rs),
volume is a plain in-memory `ResMut<GlobalVolume>` nudged ±0.1, no `fs::write`, no dirs crate,
resets every launch. (`bevy.org/assets/games` is 404; the real shipped-game inventories are the
[Fourth](https://bevy.org/news/bevys-fourth-birthday/) /
[Fifth Birthday](https://bevy.org/news/bevys-fifth-birthday/) posts and
[bevy_awesome_prod](https://github.com/Vrixyz/bevy_awesome_prod/).)

### What shipped Bevy games actually do

- **Tunnet** ([Steam 2286390](https://store.steampowered.com/app/2286390/Tunnet/)) — the flagship
  shipped commercial Bevy game — **hand-rolled TOML**, player-editable, no persistence crate.
  See §6 for its load-bearing lesson.
- **Jumpy** (Fish Folk) — `directories::ProjectDirs…data_dir().join("storage.yml")`, namespace
  `("org","fishfolk","jumpy")`.
- **Thetawave** — hand-rolled SQLite via rusqlite at `ProjectDirs…data_local_dir()`.
- **moar_ants** — `bevy-persistent`, JSON, with a versioned settings enum
  `#[serde(tag="version")] enum UserSettings { V1 {…} }` + `migrate()`.
  **Good pattern, wrong format for us** — that is precisely the shape §3 rules out in RON.
- **Unhaunter** — `bevy-persistent`, five split RON files under `dirs::config_dir()/unhaunter-game/config/`.
  **Pico-TD** — bevy-persistent RON. **mageanoid** — bevy-persistent bincode.
  **sandy-factry**, **steks** — `bevy_pkv`.

**The hand-roll is what the most successful shipped title does.**

**Free win worth knowing:** `bevy::platform::dirs::preferences_dir()` exists in 0.19.0 behind no
feature (only `std`) — a ~30-line in-house `dirs` clone using `std::env::home_dir` and
`SHGetKnownFolderPath`. But its macOS branch is `$HOME/Library/Preferences`, so
**our hand-rolled `config_dir_from` is strictly better than Bevy's own.**

## 2. Format — RON, with `struct_names` pinned `false`

RON maps Rust types 1:1; `shadows: Medium`, `msaa: X4` is about as hand-editable as a config gets,
free because the enums are unit-only. Comments (`//`, `/* */`) and trailing commas parse.
`ron` 0.12 is already a dependency — **zero new deps**.

TOML's disqualifiers for a file that will grow keybindings: enums beyond unit variants map badly
([toml#965](https://github.com/toml-rs/toml/issues/965),
[#607](https://github.com/toml-rs/toml/issues/607)); `Option` has no representation; and the
**table-ordering trap** — a player appending `fullscreen = true` at the bottom of a file that
already has a `[graphics]` header silently sets `graphics.fullscreen`. That is the likeliest real
player mistake and it fails least helpfully.

TOML's one genuine edge is error rendering (line, column, source line, caret). RON's `SpannedError`
gives `3:14: Expected integer` — position without a snippet. **Already handled correctly**:
`ron::de::from_str` returns `SpannedResult`, so the existing `{err}` interpolation carries line:col.

**Nobody preserves comments through a serde round-trip** — not RON, not `toml` 1.x. The sole
exception is `toml_edit`'s `DocumentMut` surgical-patch flow, and `toml` 1.x dropped that
dependency. Since the UI rewrites this file on every change, **comment preservation is
unachievable and must not be attempted.** Player *edits* survive; player *comments* do not.
Document that where players will see it.

**⚠️ CORRECTION — `struct_names` must stay `false`.** An earlier draft suggested
`PrettyConfig::struct_names(true)` for readability. **Retracted.** RON 0.12 *enforces* the struct
name when one is present — verified: a `VersionProbe` reading `Config(version: 3, …)` fails with
``Expected struct `VersionProbe` but found `Config` ``. "Optional" in
[RON's docs](https://docs.rs/ron/latest/ron/) means *may be omitted*, not *is ignored*.
`PrettyConfig::default()` has `struct_names: false`, and that anonymous `(…)` output is what makes
version probes and frozen snapshot structs work at all. **Keep it `false` and add a test asserting
it** — flipping it silently breaks every probe and snapshot lacking a matching `#[serde(rename)]`.

Also pin **`new_line("\n")`**: `PrettyConfig::default()` uses `"\r\n"` on Windows, so the same file
written by the Windows and macOS builds differs on every line.

## 3. Versioning — right tier, two holes, one bigger flaw

### The dedicated crates are a graveyard

| Crate | Latest | Recent DL | Status |
|---|---|---|---|
| `serde-version` | 0.5.1, **2019-11-26** | 41 | Dead. |
| `serde_flow` | 1.1.1, 2024-03-14 | 230 | Dead — abandoned 8 days after 1.0.0, **1 GitHub star**. |
| `obake` | 1.0.5, **2023-04-20** | 971 | Dormant. Best-designed of the bunch. |
| `serde_versioning` | 2025-12-02 | 2,091 | Alive but **built on the untagged pattern**; its own README calls it "a naive solution… inspired by untagged enum patterns". |
| `versionize` (Firecracker) | 0.2.0, 2024-01-02 | 30,090 | **[Repo archived](https://github.com/firecracker-microvm/versionize).** |
| `savefile` / `revision` | 2026-07 | 10k / 599k | Alive, **binary-only**. `revision` is SurrealDB's storage layer, not a serde text format. |

**The headline is `versionize`.** Firecracker's own crate, archived, and the
[Firecracker CHANGELOG](https://github.com/firecracker-microvm/firecracker/blob/main/CHANGELOG.md)
records the retreat: *"This change renders all previous Firecracker snapshots (up to Firecracker
version v1.6.0) incompatible with the current Firecracker version."* The most-cited serious
versioned-serialization crate in Rust was abandoned by the team that wrote it, in favour of *break
compatibility and say so loudly*. **Every survivor's model reduces to "keep old structs, write
`From` impls."** Write that yourself in ~40 lines.

### Three shapes ruled out, empirically (verified against serde 1.0.229 / ron 0.12.2)

- **`ron::Value` as a migration intermediate — non-starter.** `(q: High)` parses to
  `Map({"q": Unit})`: **the variant name is destroyed.**
  [`ron::Value`](https://docs.rs/ron/latest/ron/value/enum.Value.html) has no enum variant and the
  docs confirm it *"does not support enums"*. `Settings` is all enums. (`serde_json::Value` handles
  the same data fine — this is RON-specific.) Also never migrate by re-serialising a `Value`: RON
  emits map syntax it then refuses to read back into a struct.
- **Internally tagged `#[serde(tag = "version")]` — disqualified.** **The tag value must be a
  string.** RON emits `version:"2"`; a hand-written `version: 2` fails with `Expected string`.
  Adopting it is itself a breaking file change *and* permanently quotes the version. It also forces
  serde's buffering path ([serde#2187](https://github.com/serde-rs/serde/issues/2187),
  [#1183](https://github.com/serde-rs/serde/issues/1183),
  [#1495](https://github.com/serde-rs/serde/issues/1495)), and RON's support for it works by
  **sniffing serde's private `Content` type name** —
  [ron#579](https://github.com/ron-rs/ron/issues/579) records it breaking on a *Rust nightly bump*.
  RON lists it under [limitations](https://docs.rs/ron/latest/ron/index.html#limitations).
- **Untagged — structurally unsafe here.** Verified silent mis-selection: with `#[serde(default)]`
  on every field (which Tier 0 requires), the *newest* struct is always the most permissive and
  swallows every older file, discarding the payload. Not a corner case.

### The ladder

| Phase | Tier | Mechanism |
|---|---|---|
| **Now** | 0.5 — *implemented* | `#[serde(default)]` + serde's ignore-unknown default; `version` as a **refuse-newer gate**. Correct. |
| **Keybindings** | Still 0.5 | Additive. **`#[serde(alias = "old")]` is the entire rename story** — what [rustup](https://github.com/rust-lang/rustup/blob/master/src/settings.rs) and [bevy_pkv's migration example](https://github.com/johanhelsing/bevy_pkv/blob/main/examples/migration.rs) both do. **`BTreeMap`, never `HashMap`** — the randomly-seeded default hasher produces different bytes every save. |
| **Profile/stats** | Tier 1, separate file | Frozen snapshot structs + `From` chain, own `version`. Never `ron::Value`, never tagged, never untagged. |

Serde's guarantees, for the record: `deny_unknown_fields` exists to opt *out* — *"When this
attribute is not present, by default unknown fields are ignored for self-describing formats"*; and
container `#[serde(default)]` — *"any missing fields should be filled in from the struct's
implementation of `Default`"* ([container attrs](https://serde.rs/container-attrs.html)). That is a
*shape* contract, not a *meaning* contract: it covers add/remove, `alias` extends it to rename, and
**nothing in serde handles semantic change**. Semantic change is the only thing that genuinely
requires the version number.

### Hole 1 — the version gate cannot fire

`parse_settings` reads `version` only *after* a successful full deserialize, so a future file whose
*shape* changed reports "corrupt" rather than "from a newer version" — the gate fails exactly where
it exists. Fix:

```rust
#[derive(Deserialize)]
struct VersionProbe { #[serde(default)] version: u32 }   // absent version ⇒ 0, never an error
```

This is Godot's and Minecraft's design.
[Godot's `project_settings.cpp`](https://github.com/godotengine/godot/blob/master/core/config/project_settings.cpp)
reads `config_version` *during* the streaming parse and hard-refuses a future value with an
explanatory `ERR_FAIL_COND_V_MSG`. Minecraft's `Options.java` probes the version out of an
**untyped bag** (`catch (RuntimeException e) { }` ⇒ 0), logs `"Skipping bad option: {}"` per bad
line so one malformed line never kills the file, then runs `DataFixTypes.OPTIONS` on it.

### Hole 2 — silent downgrade data loss

On refuse-newer we fall back to defaults but leave the newer file in place; the player's next Apply
**overwrites their newer file with `version: 1`**. Rename it aside (`settings.ron.newer`).

### The bigger flaw — full serialization freezes defaults forever

`save()` writes every field at its current value, defaults included. Once `shadows: Medium` is on
disk it is indistinguishable from a deliberate choice, so **a shipped default can never be changed
for existing players again.** This is exactly the bug
[television had to write a 318-line archaeology system to undo](https://github.com/alexpasmantier/television/blob/main/television/config/migration.rs)
— reconstructing every default config the project ever shipped from git history and fuzzy-matching
against it, because *"a value like `ui_scale = 80` was the default of one era and a deliberate
choice in any other."*

The fix is RimWorld's rule.
[`Scribe_Values.Look`](https://github.com/Chillu1/RimWorldDecompiled/blob/master/Verse/Scribe_Values.cs)
does `if (value.Equals(defaultValue)) return;` — the file is a **sparse diff against current
defaults**, so new defaults reach existing players and additive evolution is genuinely free. In
serde: `#[serde(skip_serializing_if = …)]` per field. **Caveat: `version` must always be written**
(RimWorld has a `forceSave` flag for exactly this) or a fully-default file probes as `0`.

**This is a bigger practical win than any migration machinery, ~1 line per field, and it must land
before keybindings do** — a keybinding map frozen into a player's file is the worst possible thing
to freeze.

### The keybindings landmine is a dependency problem, not a config problem

From [bevy-persistent PR #48](https://github.com/umut-sahin/bevy-persistent/pull/48): *"when Bevy
v0.13 was released, it changed the names of `KeyCode` enum variants (e.g., `KeyCode::W` ->
`KeyCode::KeyW`)… This meant deserialization of structs with a `KeyCode` property would fail after
the upgrade"* — confirmed by the
[0.12→0.13 migration guide](https://bevy.org/learn/migration-guides/0-12-to-0-13/)
(`KeyCode::Up → ArrowUp`, `Key1 → Digit1`).

**Do not serialise `bevy::input::keyboard::KeyCode`.** Define our own key enum and convert at the
boundary, so a Bevy bump is a compiler error rather than a runtime failure in every player's file.
General rule: **on-disk types contain only primitives, `String`, and types we define.**

### The convergent pattern

Minecraft, Factorio, Godot, RimWorld and Unity independently arrived at the same five rules:
(1) version stamp on every save, read *before* the schema; (2) missing version ⇒ 0, never an error;
(3) ordered N→N+1 steps, never N→latest; (4) **files from the future refused loudly — nobody
guesses forward**; (5) a declarative pass (renames/removals) before an imperative one. In serde
terms: **`alias` + `default` are the declarative pass; the version chain is the imperative pass.
Complements, not alternatives.** Godot's entire chain sits inside `#ifndef DISABLE_DEPRECATED` —
acknowledged dead weight, which is why you do not build it before you need it.

[Factorio migrations](https://lua-api.factorio.com/latest/auxiliary/migrations.html) adds one idea
worth remembering for mod support: *"Each save file remembers (by name) which migrations from which
mods have been applied and will not apply the same migration twice"* — the applied-set, not an
integer. Overkill at alpha.

## 4. Write-safety — four defects in `save()`

[`std::fs::rename`](https://doc.rust-lang.org/std/fs/fn.rename.html) **does** atomic-replace on
Windows: std calls `MoveFileExW` or `SetFileInformationByHandle(FileRenameInfoEx)`. The "fails if
destination exists" folklore is raw `MoveFileW`. **No crate earns its keep** — none of `tempfile` /
`atomicwrites` / `atomic-write-file` handle the Windows retry case that actually matters.

1. **Fixed temp path.** `path.with_extension("ron.tmp")` is a constant. Two instances write the
   *same* temp file; B's partial write can be renamed into place by A. **The only live corruption
   path today.** Fix: pid-suffix it.
   *(This is also why no locking is needed — VS Code runs N windows over one `settings.json` with
   no lock, just atomic `.vsctmp` writes. Last-writer-wins is correct; a unique temp name is what
   makes it safe.)*
2. **No `sync_all()` before rename.** `fs::write` never syncs — the ext4 delayed-allocation shape
   that yields a **zero-length settings file** after power loss
   ([LWN](https://lwn.net/Articles/322823/),
   [Ts'o](https://thunk.org/tytso/blog/2009/03/15/dont-fear-the-fsync/)). One `sync_all()` on an
   explicit Apply, off the frame path; `#[cfg(unix)]` directory sync after is ~free.
   **Skip `F_FULLFSYNC`** on macOS ([SQLite](https://www.sqlite.org/atomiccommit.html): *"profoundly
   slow… not recommended"*) — worst case without it is reverting to the previous whole file.
3. **No Windows retry.** AV, Search Indexer, OneDrive and Dropbox transiently hold handles →
   `ERROR_ACCESS_DENIED`(5) / `ERROR_SHARING_VIOLATION`(32) / `ERROR_LOCK_VIOLATION`(33).
   Well-documented ([rustup #1869](https://github.com/rust-lang/rustup/issues/1869),
   [#2441](https://github.com/rust-lang/rustup/issues/2441)). ~500 ms of doubling backoff suffices
   (rustup needs 28 s only because it replaces a running exe).
4. **Corrupt file destroyed by the next save.** Adopt Firefox's `Invalidprefs.js` policy: rename to
   `settings.ron.bad` before falling back. One line, and it preserves the only evidence.

Rename-aside beats `.bak` rotation (needs promote-only-after-parse discipline) and
write-both-and-pick (needs a generation counter; "parses OK" ≠ "newer"). VS Code's
refuse-to-write policy is right for *user-authored* files and wrong here — this file is
machine-authored, so a hard stop just bricks the settings UI.

**Blast radius:** serde has no partial-recovery mode — **one bad character costs every setting in
the file.** Fine at 4 fields. When keybindings land, use per-action `Option<Key>` + fill-from-default
so one bad binding degrades to one reverted action, not a full reset — Minecraft's
`"Skipping bad option"` instinct.

## 5. Location — implemented choice is right

Next-to-exe is correctly rejected, and the code comment's reasoning is accurate on both counts.

- **macOS App Translocation is real.** A quarantined app launched via Launch Services that was never
  moved by Finder runs from a **read-only, randomized** mount under
  `/private/var/folders/.../AppTranslocation/<UUID>/d/`
  ([lapcat analysis](https://lapcatsoftware.com/articles/app-translocation.html)). Exe-relative
  writes fail *and* the path changes every launch. Cleared only by a Finder move or stripping
  `com.apple.quarantine`. This is exactly the GitHub-release `.dmg`/`.zip` path.
- **Signed bundles are read-only, independent of translocation.** Apple
  [TN2206](https://developer.apple.com/library/archive/technotes/tn2206/_index.html), verbatim:
  *"Bundles should be treated as read-only once they have been signed"* and *"It also won't work to
  write that data after your app first runs. That still breaks the signature. Gatekeeper will check
  your app again…"* `scripts/package-macos.sh` signs and notarizes, so even a user-moved `.app` must
  never write inside itself.
- **Windows Program Files is settled, not merely discouraged.** UAC's VirtualStore redirection is
  **disabled for 64-bit processes and manifested apps**
  ([Registry Virtualization](https://learn.microsoft.com/en-us/windows/win32/sysinfo/registry-virtualization)),
  and Microsoft calls it an interim technology to be removed. A 64-bit game under Program Files gets
  `ACCESS_DENIED` with no fallback.
- **Linux** AppImages are read-only mounts; plain tarballs extracted to `$HOME` are writable, which
  is why *opt-in* portable mode survives there.

**Apple's positive guidance**
([File System Programming Guide](https://developer.apple.com/library/archive/documentation/FileManagement/Conceptual/FileSystemProgrammingGuide/MacOSXDirectories/MacOSXDirectories.html)),
verbatim: Application Support *"Contains all app-specific data and support files… By convention, all
of these items should be put in a subdirectory whose name matches the bundle identifier of the
app"*; Preferences *"Contains the user's preferences. **You should never create files in this
directory yourself.** To get or set preference values, you should always use the NSUserDefaults
class or an equivalent system-provided interface."* Our
`~/Library/Application Support/Overmatch` is a **deliberate deviation** from the reverse-DNS
convention (findable vs `com.vikngdev.overmatch`) — defensible for a game; worth one comment so it
reads as a choice.

**Linux** — [XDG Base Directory spec](https://specifications.freedesktop.org/basedir-spec/latest/):
`$XDG_CONFIG_HOME` (default `~/.config`) for config, `$XDG_DATA_HOME` (`~/.local/share`) for data —
**saves belong in data, not config**, if they ever grow.

**Crate choice.** `dirs` 6.0.0 (279.8M DL) returns raw base dirs; `directories` 6.0.0 (62.6M DL)
`ProjectDirs` applies platform-idiomatic naming but adds a Windows `\config`+`\data` leaf split and
macOS `com.Org.App` mangling — both mildly hostile to players hunting for files. `etcetera` lets you
choose the convention but its Apple strategy resolves to `~/Library/Preferences` (wrong, per above).
**Keep the hand-roll**: ~25 lines, testable on all three OSes from any one OS (which no crate
offers), zero dependencies, and it matches ADR-0010's posture (*"RON is `serde` + a ~25-line
in-crate `AssetLoader`"*). The existing `OVERMATCH_CONFIG_DIR` override mirroring `BEVY_ASSET_ROOT`,
and `config_dir_from` being a pure function, are both exactly right.

**Portable mode**, if ever wanted, is the sentinel pattern —
[Dolphin's `portable.txt`](https://github.com/dolphin-emu/dolphin/blob/master/Source/Core/UICommon/UICommon.cpp)
or [Godot's `_sc_`](https://docs.godotengine.org/en/stable/tutorials/io/data_paths.html) — with the
non-obvious requirement that you **probe writability, not just sentinel existence**, because inside
a translocated or signed bundle the sentinel is readable while the directory is not. Deferrable;
macOS can never be its default.

`std::env::current_exe` caveats, for the record: symlink resolution is platform-dependent, a
rename-while-running may return a stale path, and the Security section warns the result can be
attacker-influenced. Bevy itself uses it only for the read-only asset root — **exe-relative for
read-only assets, platform dirs for anything written** is the correct division and the one we have.

## 6. ⛔ SUPERSEDED — item 11, and the file-split rule that replaces it

**An earlier draft of this brief said: keybindings go IN `settings.ron`; only profile/stats split
out. That is WRONG and is hereby replaced.**

**Tunnet shipped that arrangement and had to hotfix it.** From its
[v1.2.3 notes](https://steamstore-a.akamaihd.net/news/externalpost/steam_community_announcements/5969041959982316167):
*"The system settings are now stored in the system_settings.toml file. This file is ignored by the
cloud saves."* Valve's [Steam Auto-Cloud docs](https://partner.steamgames.com/doc/features/cloud)
state the rule directly — *"avoid machine-specific configurations such as video settings."*
Microsoft's taxonomy says the same thing about roaming profiles:
[CSIDL](https://learn.microsoft.com/en-us/windows/win32/shell/csidl) defines
`CSIDL_LOCAL_APPDATA` as *"a data repository for local (nonroaming) applications"*, while
`FOLDERID_RoamingAppData` is copied across machines.

**The split axis is MACHINE-LOCAL vs PORTABLE — not settings vs stats.** That is a third
independent rationale on top of the two already established (different corruption policy: reset vs
never-lose; different write cadence: per-Apply vs per-match), and it cuts the files differently:

| File | Contents | Nature |
|---|---|---|
| **existing `settings.ron`** | shadows, MSAA, vsync — plus queued window mode, render scale, UI scale, frame cap | **Machine-local.** GPU/monitor facts. Never roam, never cloud-sync. |
| **new `controls.ron`** | keybindings | **Portable.** Follows the player to any machine. |
| **later `profile.ron`** | profile / stats | **Portable**, own version stamp. |

Every field currently in `settings.ron`, and every field queued for it, is machine-local. **So
`settings.ron` is already the machine-local file, and keybindings must NOT be added to it.**

**Minimal correct implementation:** keep ONE config directory; split by **filename** so a future
Steam Cloud manifest can include/exclude by name. That is exactly what Tunnet does. It also
dissolves the Windows Roaming-vs-Local tension: with the split in place, `%APPDATA%` is defensible
for the portable files, and the machine-local file can move to `%LOCALAPPDATA%` if and when it
matters. **No directory decision is needed today — only the filename decision, and that one must be
made before keybindings land.**

Consider renaming the existing file to `video.ron` or `system.ron` while it has effectively zero
players. Free now; a migration later.

## 7. The change-list

| # | Change | Cost |
|---|---|---|
| 1 | **Pid-unique temp filename** — the only live corruption bug | 1 line |
| 2 | **`sync_all()` temp before rename**; `#[cfg(unix)]` dir sync after | 3 lines |
| 3 | **Retry rename on Windows 5/32/33**, doubling backoff to ~512 ms | ~10 lines |
| 4 | **Rename aside before fallback** — `.bad` on parse failure, `.newer` on refuse-newer (closes silent downgrade loss) | 2 lines |
| 5 | **`VersionProbe` before full parse** — makes the refuse-newer gate actually fire | 4 lines |
| 6 | **Pin `PrettyConfig::new_line("\n")`; assert `struct_names == false` in a test** | 2 lines |
| 7 | **`skip_serializing_if` per field, `version` always written** — stop freezing defaults into player files | 1 line/field |
| 8 | **Policy doc line:** renames are `alias`-only and never bump the version; only semantic changes bump | 1 comment |
| 9 | **Fixture test per shipped version** — a real `settings.ron` string constant, extending the existing `unknown_and_missing_fields_both_load_cleanly` / `corrupt_and_future_files_fall_back_loudly` tests, which are already the right instinct | ~10 lines |
| 10 | **Keybindings: own key enum (not `KeyCode`), `BTreeMap`, per-action `Option`** | decision now |
| 11 | ⛔ **SUPERSEDED** — replaced by the §6 three-file machine-local/portable split. Urgency rises from "later" to **"before keybindings"**, because it decides where keybindings are written on day one | decision now |

Items 1–6 are ~25 lines total. **Item 7 is the highest-value single change.** Items 8–11 are
decisions to take now so they do not become migrations later.

## 8. Method and gaps

WebSearch budget was exhausted early, so most of this is primary sources fetched directly
(crates.io API, GitHub raw source at pinned tags, serde.rs, Apple/Microsoft docs, engine source
trees) rather than blog discovery — there may be well-known Rust config-migration write-ups not
surfaced. All Rust behavioural claims marked *verified* were run against serde 1.0.229 / ron 0.12.2
/ toml 0.8 / serde_json 1 in a scratch crate; no repo files were modified during research. The
RimWorld and Minecraft sources are community decompiles — treat the code as accurate in shape and
line numbers as version-dependent. Tunnet is closed-source, so its exact directory is unverified
(the behaviour is quoted from its own patch notes). No reachable persistence postmortems exist for
Times of Progress, jarl, HexLands, Cargo Space or Roids.
