//! What a build states about itself at the netcode handshake, and why two builds refuse each other.
//!
//! [`Compatibility`] is the handshake tag's only input ([`Compatibility::tag`]): two builds with one
//! record share a tag, so a peer refused under another tag states a record that differs from ours.
//! The record's text is the stable part of the handshake. A client states it in its connect token's
//! user data; a server answers a request under another tag with its own record, sealed under the
//! requester's tag by the vendored netcode's `ForeignProtocolHandler`, so builds of any revision
//! read each other. Each side then names the [`Incompatibility`] from the two records. The layout,
//! the magic word then space-separated `key=value` fields, must stay readable by every shipped
//! build: add content keys, never rename or reformat `rev`, `ver` or `wire`.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;

use bevy::log::warn;
use lightyear::connection::shared::DeniedReason;
use lightyear::netcode::{ForeignProtocolHandler, USER_DATA_BYTES};
use lightyear::prelude::PeerId;

use super::protocol::{PROTOCOL_FINGERPRINT, PROTOCOL_REV};
use crate::fingerprint::{FNV_OFFSET_BASIS, fingerprint_field, fnv1a_64};

const MAGIC: &str = "overmatch-compat";

/// A netcode custom denial carries at most `u8::MAX` bytes of reason; the token's user data holds
/// [`USER_DATA_BYTES`].
const MAX_RECORD_BYTES: usize = u8::MAX as usize;

/// The longest `ver` a record states. A client renders a refusing server's version, so this bounds
/// the text a server of another build can put on the player's screen.
const MAX_VERSION_BYTES: usize = 32;

/// The longest content key a record states: a client renders a refusing server's differing keys.
const MAX_KEY_BYTES: usize = 16;

/// A build's handshake identity, in the order a difference is acted on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Compatibility {
    /// [`PROTOCOL_REV`]: it orders builds, so the older side of a refusal is knowable.
    rev: u32,
    /// The crate version: a semantic version ([`is_version`]), so it renders within the UI font's
    /// coverage.
    version: String,
    /// [`PROTOCOL_FINGERPRINT`]: the pinned wire manifest, [`PROTOCOL_REV`] and the crate version.
    wire: u64,
    /// Content each peer derives beside the wire, by key ([`is_content_key`]). Key order makes the
    /// text, and so the tag, a function of the set the records are compared as.
    content: BTreeMap<String, u64>,
}

impl Compatibility {
    /// This process's record. Its content terms:
    ///
    /// * `map`: terrain and scatter are DERIVED on each peer from files beside the binary; stale
    ///   files (including half-pulled LFS objects) must refuse before divergent belt contacts,
    ///   spawn heights or hull collisions reach play.
    /// * `materials`: embedded `materials.ron` sets every hard body's resistance and is invisible to
    ///   the map digest and the wire manifest.
    /// * `tank`: the construction digest covers the embedded tank sheet and the adjacent sim
    ///   artifact, which decide the weapon, stowage and health-volume indices.
    /// * `armaments`: the game's damage model binds fuze modes and round facts from the embedded
    ///   round-type, round and gun registries, so a handshake-matched receiver can refuse an
    ///   unresolvable index as malformed input rather than wait for data skew to resolve.
    /// * `aim`: the input admission policy must agree before peers connect.
    /// * `ground`: the ground-type table beside the binary sets the server's ground response and
    ///   each client's prints and drawn belt sink; a peer holding another table draws sinkage the
    ///   server's ground never had.
    ///
    /// Cached per process: map files must stay fixed while running, and the heightmap is tens of MB.
    pub(crate) fn local() -> &'static Self {
        static LOCAL: std::sync::OnceLock<Compatibility> = std::sync::OnceLock::new();
        LOCAL.get_or_init(|| {
            let root = crate::assets::asset_root();
            let ground = crate::ground_types::GroundTypes::shipped_path();
            let ground_types = std::fs::read(&ground).unwrap_or_else(|err| {
                panic!(
                    "the ground-type table {} cannot be read: {err}",
                    ground.display()
                )
            });
            let record = Self {
                rev: PROTOCOL_REV,
                version: super::CLIENT_VERSION.to_owned(),
                wire: PROTOCOL_FINGERPRINT,
                content: [
                    ("map", crate::map::content_digest(&root)),
                    ("materials", bytes_digest(crate::substances::MATERIALS_RON)),
                    ("tank", crate::bake::construction_digest(&root)),
                    (
                        "armaments",
                        armaments_digest(
                            crate::rounds::ROUND_TYPES_RON,
                            crate::armaments_index::ROUND_FILES,
                            crate::armaments_index::GUN_FILES,
                        ),
                    ),
                    ("aim", bytes_digest(crate::aim::INPUT_RON)),
                    ("ground", fnv1a_64(FNV_OFFSET_BASIS, &ground_types)),
                ]
                .map(|(key, digest)| (key.to_owned(), digest))
                .into(),
            };
            assert_eq!(
                Self::parse(&record.text()).as_ref(),
                Ok(&record),
                "this build's record must read back as itself: its version a semantic version \
                 within {MAX_VERSION_BYTES} bytes, its keys `[a-z_]` within {MAX_KEY_BYTES}, and \
                 the whole within {MAX_RECORD_BYTES}",
            );
            record
        })
    }

    /// The netcode handshake tag: the record's text, hashed.
    pub(crate) fn tag(&self) -> u64 {
        fnv1a_64(FNV_OFFSET_BASIS, self.text().as_bytes())
    }

    fn text(&self) -> String {
        let mut text = format!(
            "{MAGIC} rev={} ver={} wire={:016x}",
            self.rev, self.version, self.wire
        );
        for (key, digest) in &self.content {
            write!(text, " {key}={digest:016x}").expect("writing to a String cannot fail");
        }
        text
    }

    /// The token's user data: the text, zero-padded.
    pub(super) fn user_data(&self) -> [u8; USER_DATA_BYTES] {
        let text = self.text();
        let mut data = [0; USER_DATA_BYTES];
        data[..text.len()].copy_from_slice(text.as_bytes());
        data
    }

    /// A peer's record from its token's user data. A record is shorter than the data, so a zero
    /// always ends it; all zeros is a build that states none.
    fn from_user_data(data: &[u8; USER_DATA_BYTES]) -> Result<Self, RecordError> {
        let end = data
            .iter()
            .position(|&byte| byte == 0)
            .ok_or(RecordError::NotARecord)?;
        Self::parse(std::str::from_utf8(&data[..end]).map_err(|_| RecordError::NotARecord)?)
    }

    /// A peer's record from its text: parsed once, here, so every field is within its form and size
    /// before anything renders it.
    pub(super) fn parse(text: &str) -> Result<Self, RecordError> {
        let mut fields = text.split(' ');
        if fields.next() != Some(MAGIC) || text.len() > MAX_RECORD_BYTES {
            return Err(RecordError::NotARecord);
        }
        let (mut rev, mut version, mut wire) = (None, None, None);
        let mut content = BTreeMap::new();
        for field in fields {
            let malformed = || RecordError::Field(field.to_owned());
            let (key, value) = field.split_once('=').ok_or_else(malformed)?;
            let fresh = match key {
                "rev" => rev
                    .replace(value.parse::<u32>().map_err(|_| malformed())?)
                    .is_none(),
                "ver" if is_version(value) => version.replace(value.to_owned()).is_none(),
                "ver" => return Err(RecordError::Version(value.to_owned())),
                "wire" => wire.replace(digest(value).ok_or_else(malformed)?).is_none(),
                _ if is_content_key(key) => content
                    .insert(key.to_owned(), digest(value).ok_or_else(malformed)?)
                    .is_none(),
                _ => return Err(RecordError::Key(key.to_owned())),
            };
            if !fresh {
                return Err(RecordError::Duplicate(key.to_owned()));
            }
        }
        Ok(Self {
            rev: rev.ok_or(RecordError::Missing("rev"))?,
            version: version.ok_or(RecordError::Missing("ver"))?,
            wire: wire.ok_or(RecordError::Missing("wire"))?,
            content,
        })
    }
}

/// A semantic version within [`MAX_VERSION_BYTES`]: `N.N.N`, then an optional `-pre` and an
/// optional `+build`, each dot-separated `[0-9A-Za-z-]` identifiers.
fn is_version(value: &str) -> bool {
    let identifiers = |part: &str| {
        part.split('.').all(|identifier| {
            !identifier.is_empty()
                && identifier
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
        })
    };
    let (release, build) = value
        .split_once('+')
        .map_or((value, None), |(release, build)| (release, Some(build)));
    let (core, pre) = release
        .split_once('-')
        .map_or((release, None), |(core, pre)| (core, Some(pre)));
    value.len() <= MAX_VERSION_BYTES
        && core.split('.').count() == 3
        && core
            .split('.')
            .all(|number| !number.is_empty() && number.bytes().all(|byte| byte.is_ascii_digit()))
        && pre.is_none_or(identifiers)
        && build.is_none_or(identifiers)
}

/// `[a-z_]`, within [`MAX_KEY_BYTES`].
fn is_content_key(key: &str) -> bool {
    (1..=MAX_KEY_BYTES).contains(&key.len())
        && key
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte == b'_')
}

/// Sixteen hex digits, as [`Compatibility::text`] writes a digest.
fn digest(value: &str) -> Option<u64> {
    (value.len() == 16 && value.bytes().all(|byte| byte.is_ascii_hexdigit()))
        .then(|| u64::from_str_radix(value, 16).expect("sixteen hex digits fit a u64"))
}

fn bytes_digest(text: &str) -> u64 {
    fnv1a_64(FNV_OFFSET_BASIS, text.as_bytes())
}

/// The game's damage model: fingerprint every embedded armament key and source, in the index's
/// byte-sorted order. These sources are what the shipped registries parse, not a second manifest.
fn armaments_digest(round_types: &str, rounds: &[(&str, &str)], guns: &[(&str, &str)]) -> u64 {
    let mut tag = fingerprint_field(FNV_OFFSET_BASIS, b"round_types", round_types.as_bytes());
    for (kind, files) in [(b"rounds".as_slice(), rounds), (b"guns".as_slice(), guns)] {
        tag = fingerprint_field(tag, b"registry", kind);
        for (key, text) in files {
            tag = fingerprint_field(tag, b"key", key.as_bytes());
            tag = fingerprint_field(tag, b"ron", text.as_bytes());
        }
    }
    tag
}

/// Why a peer's stated record cannot be read. Outside input: each side logs it and refuses.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum RecordError {
    /// Not UTF-8 text opening with the magic word within the size limit: a build that states no
    /// record, such as one that predates it.
    NotARecord,
    /// A field that is not `key=value`, or a `rev`, `wire` or content value out of its form.
    Field(String),
    /// A `ver` that is not [`is_version`]'s.
    Version(String),
    /// A content key that is not [`is_content_key`]'s.
    Key(String),
    /// A field stated twice.
    Duplicate(String),
    /// A required field is absent.
    Missing(&'static str),
}

/// Why a client and a server refuse each other.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Incompatibility {
    /// Network revisions differ; the lower is the older build.
    Revision { client: u32, server: u32 },
    /// One revision, two releases.
    Version { client: String, server: String },
    /// One revision and release, two wire manifests: a pin moved without a revision bump.
    Wire,
    /// One build, different content: these keys, in key order, differ or one side lacks them.
    Content(Vec<String>),
}

impl Incompatibility {
    /// The first difference in the order that decides what to do: the revision says which side is
    /// older, then the release, the wire build and the content. `None` when the records match,
    /// which a peer refused under another tag shows only by folding its tag another way.
    pub(crate) fn between(client: &Compatibility, server: &Compatibility) -> Option<Self> {
        if client.rev != server.rev {
            return Some(Self::Revision {
                client: client.rev,
                server: server.rev,
            });
        }
        if client.version != server.version {
            return Some(Self::Version {
                client: client.version.clone(),
                server: server.version.clone(),
            });
        }
        if client.wire != server.wire {
            return Some(Self::Wire);
        }
        let differing: BTreeSet<&String> = client
            .content
            .keys()
            .chain(server.content.keys())
            .filter(|key| client.content.get(*key) != server.content.get(*key))
            .collect();
        (!differing.is_empty()).then(|| Self::Content(differing.into_iter().cloned().collect()))
    }

    /// What the player reads: the cause, then who acts.
    pub(crate) fn message(&self) -> String {
        match self {
            Self::Revision { client, server } if client < server => format!(
                "This client is older than the server: network revision {client}, server \
                 {server}. Update the client."
            ),
            Self::Revision { client, server } => format!(
                "The server is older than this client: network revision {server}, client \
                 {client}. The server needs an update."
            ),
            Self::Version { client, server } => format!(
                "This client is version {client}; the server runs {server}. Run the server's \
                 version."
            ),
            Self::Wire => "This client and the server are different builds of one version. Run \
                           the server's build."
                .to_owned(),
            Self::Content(keys) => format!(
                "This client's game data differs from the server's: {}. Update or reinstall the \
                 client, or redeploy the server.",
                keys.join(", ")
            ),
        }
    }
}

/// The server's answer to a key holder under another tag: this build's record as the denial, and
/// a log line naming the difference from the record the peer states.
#[derive(Debug)]
pub(super) struct RefuseOtherBuilds;

impl ForeignProtocolHandler for RefuseOtherBuilds {
    fn refuse(
        &self,
        client: PeerId,
        protocol_id: u64,
        user_data: &[u8; USER_DATA_BYTES],
    ) -> DeniedReason {
        let server = Compatibility::local();
        match Compatibility::from_user_data(user_data)
            .map(|stated| Incompatibility::between(&stated, server))
        {
            Ok(Some(cause)) => warn!(?client, ?cause, "server: refused a client of another build"),
            unexplained => warn!(
                ?client,
                tag = %format!("{protocol_id:#018x}"),
                ?unexplained,
                "server: refused a client of another build that states no readable difference"
            ),
        }
        DeniedReason::Custom(server.text())
    }
}

#[cfg(test)]
impl Compatibility {
    /// FIXTURE: this record at another revision.
    pub(super) fn at_revision(&self, rev: u32) -> Self {
        Self {
            rev,
            ..self.clone()
        }
    }

    /// FIXTURE: this record with `key`'s content digest changed.
    pub(super) fn with_content_changed(&self, key: &str) -> Self {
        let mut record = self.clone();
        *record
            .content
            .get_mut(key)
            .expect("the fixture changes a stated key") ^= 1;
        record
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// FIXTURE: a record with one content term.
    fn fixture() -> Compatibility {
        Compatibility {
            rev: 7,
            version: "1.2.3-alpha.4".into(),
            wire: 0xff,
            content: [("map".into(), 1)].into(),
        }
    }

    /// The layout every shipped build reads: a reformatted field makes earlier builds read a newer
    /// record as unreadable, losing the cause on both sides. Content terms follow in key order, so
    /// builds stating one set state one tag. An interface pin.
    #[test]
    fn the_record_layout_is_the_stable_handshake() {
        let mut record = fixture();
        record.content.insert("ground".into(), 2);
        assert_eq!(
            record.text(),
            "overmatch-compat rev=7 ver=1.2.3-alpha.4 wire=00000000000000ff \
             ground=0000000000000002 map=0000000000000001",
        );
    }

    /// A peer reads exactly what this build states, through both carriers: the server reads a
    /// client's token user data, the client reads the server's denial text.
    #[test]
    fn this_build_reads_back_through_both_carriers() {
        let local = Compatibility::local();
        assert_eq!(
            Compatibility::from_user_data(&local.user_data()).as_ref(),
            Ok(local)
        );
        assert_eq!(
            Compatibility::from_user_data(&[0; USER_DATA_BYTES]),
            Err(RecordError::NotARecord),
            "a build that predates the record states none",
        );
        let text = fixture().text();
        assert_eq!(
            Compatibility::parse(&format!("{text} map=0000000000000002")),
            Err(RecordError::Duplicate("map".into())),
        );
        assert_eq!(
            Compatibility::parse(&text.replace(" rev=7", "")),
            Err(RecordError::Missing("rev")),
        );
    }

    /// A refusing server's `ver` and its differing content keys reach the player's screen, so a
    /// mutant that loosens either check lets a server of another build write what it likes there.
    /// The version is a semantic version within its cap, a key `[a-z_]` within its own.
    #[test]
    fn a_record_states_only_text_the_screen_may_show() {
        let parse = |fields: &str| {
            Compatibility::parse(&format!(
                "overmatch-compat rev=7 wire=00000000000000ff {fields}"
            ))
        };
        let longest = format!("1.2.3-{}", "a".repeat(MAX_VERSION_BYTES - "1.2.3-".len()));
        for version in [
            "1.2.3",
            "10.20.30-rc.1+build-5.x",
            "1.2.3+sha",
            longest.as_str(),
        ] {
            assert!(parse(&format!("ver={version}")).is_ok(), "{version}");
        }
        let too_long = format!("{longest}a");
        for version in [
            "",
            "1.2",
            "1.2.3.4",
            "v1.2.3",
            "1.2.3-",
            "1.2.3+",
            "1.2.3-rc..1",
            "1.2.3+a+b",
            "1.2.3/x",
            too_long.as_str(),
        ] {
            assert_eq!(
                parse(&format!("ver={version}")),
                Err(RecordError::Version(version.into())),
            );
        }
        let longest = "k".repeat(MAX_KEY_BYTES);
        assert!(parse(&format!("ver=1.2.3 {longest}=0000000000000001")).is_ok());
        for key in [format!("{longest}k"), "Map".into(), "map-2".into()] {
            assert_eq!(
                parse(&format!("ver=1.2.3 {key}=0000000000000001")),
                Err(RecordError::Key(key)),
            );
        }
    }

    /// The tag is the record's: a mutant that leaves a field out of the text lets builds that
    /// differ only there connect. Each field and an added content key move the tag.
    #[test]
    fn every_stated_field_reaches_the_tag() {
        assert_eq!(
            super::super::protocol::protocol_id(),
            Compatibility::local().tag()
        );
        let base = fixture();
        assert_eq!(base.tag(), fixture().tag());
        let mut added = fixture();
        added.content.insert("ground".into(), 1);
        for changed in [
            base.at_revision(8),
            Compatibility {
                version: "1.2.4".into(),
                ..fixture()
            },
            Compatibility {
                wire: 0xfe,
                ..fixture()
            },
            base.with_content_changed("map"),
            added,
        ] {
            assert_ne!(changed.tag(), base.tag(), "{changed:?}");
        }
    }

    /// This process states every content source it derives play from, read from that source: a
    /// mutant that drops a term or hashes another file connects peers that disagree there.
    #[test]
    fn this_build_states_each_content_source() {
        let root = crate::assets::asset_root();
        let ground = std::fs::read(crate::ground_types::GroundTypes::shipped_path())
            .expect("the shipped ground-type table");
        let stated = |key: &str| {
            *Compatibility::local()
                .content
                .get(key)
                .unwrap_or_else(|| panic!("the record states `{key}`"))
        };
        assert_eq!(stated("map"), crate::map::content_digest(&root));
        assert_eq!(stated("tank"), crate::bake::construction_digest(&root));
        assert_eq!(stated("ground"), fnv1a_64(FNV_OFFSET_BASIS, &ground));
        assert_eq!(
            stated("materials"),
            bytes_digest(crate::substances::MATERIALS_RON)
        );
        assert_eq!(stated("aim"), bytes_digest(crate::aim::INPUT_RON));
        assert_eq!(
            stated("armaments"),
            armaments_digest(
                crate::rounds::ROUND_TYPES_RON,
                crate::armaments_index::ROUND_FILES,
                crate::armaments_index::GUN_FILES,
            )
        );
    }

    /// The game's damage model: a registry-only fuze or gun edit must refuse at connect even when
    /// tank construction and wire types match, so each registry input moves the `armaments`
    /// digest. Receiver fire bindings rely on this guarantee.
    #[test]
    fn every_armament_registry_input_moves_its_digest() {
        // Fixture bytes test the generic source fingerprint, not any asset's physical values.
        let rounds = [("round", "round source")];
        let guns = [("gun", "gun source")];
        let base = armaments_digest("round types", &rounds, &guns);
        for changed in [
            armaments_digest("changed types", &rounds, &guns),
            armaments_digest("round types", &[("round", "changed source")], &guns),
            armaments_digest("round types", &[("renamed round", "round source")], &guns),
            armaments_digest("round types", &rounds, &[("gun", "changed source")]),
            armaments_digest("round types", &rounds, &[("renamed gun", "gun source")]),
        ] {
            assert_ne!(base, changed);
        }
    }

    /// The player acts on the first difference: the revision decides who updates even when content
    /// differs too, and content names each key either side lacks. A mutant that reverses the sides
    /// tells the wrong party to update.
    #[test]
    fn the_first_difference_names_who_acts() {
        let base = fixture();
        assert_eq!(Incompatibility::between(&base, &base), None);
        assert_eq!(
            Incompatibility::between(&base, &base.at_revision(8)),
            Some(Incompatibility::Revision {
                client: 7,
                server: 8
            }),
        );
        assert!(
            Incompatibility::between(&base, &base.at_revision(8))
                .expect("revisions differ")
                .message()
                .contains("Update the client"),
        );
        assert!(
            Incompatibility::between(&base.at_revision(8), &base)
                .expect("revisions differ")
                .message()
                .contains("The server needs an update"),
        );
        assert_eq!(
            Incompatibility::between(&base.with_content_changed("map").at_revision(8), &base),
            Some(Incompatibility::Revision {
                client: 8,
                server: 7
            }),
        );
        let release = Compatibility {
            version: "1.2.4".into(),
            wire: 0xfe,
            ..fixture()
        };
        assert_eq!(
            Incompatibility::between(&base, &release),
            Some(Incompatibility::Version {
                client: "1.2.3-alpha.4".into(),
                server: "1.2.4".into()
            }),
        );
        assert_eq!(
            Incompatibility::between(
                &base,
                &Compatibility {
                    wire: 0xfe,
                    ..fixture()
                }
            ),
            Some(Incompatibility::Wire),
        );
        let mut grounded = fixture();
        grounded.content.insert("ground".into(), 1);
        assert_eq!(
            Incompatibility::between(&base.with_content_changed("map"), &grounded),
            Some(Incompatibility::Content(vec![
                "ground".into(),
                "map".into()
            ])),
        );
    }
}
