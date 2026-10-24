//! The signed launch document and its validation rules.
//!
//! `SignedConfig` is the object that is canonically serialized and signed
//! (`canonical`), and the same parsed value feeds the Lua emitter and the
//! spawner at boot — nothing is verified on one read and executed from
//! another. Everything here is spec §6 (validation and eligibility): the
//! allowlist, the bounds, and the ordering/uniqueness invariants.

use serde::{Deserialize, Serialize};

/// Version of the *signed* schema, carried inside the signed bytes. Distinct
/// from the outer file format version (`ConfigFile::v`), so an unsigned outer
/// version can never reinterpret identical signed bytes under a new policy.
pub const SIGNED_SCHEMA_VERSION: u32 = 2;

/// Version of the root-held `public.json` (the committed key, generation and
/// account), which the service writes and the anchor trusts. Independent of
/// [`SIGNED_SCHEMA_VERSION`]: a new signed schema must still find the key it
/// is to be re-authorized under.
pub const PUBLIC_STATE_VERSION: u32 = 1;

/// Version of the on-disk `config.json` envelope.
pub const FILE_FORMAT_VERSION: u32 = 2;

// Caps a filesystem-blind service can enforce (spec §6, §D7).
/// Most entries one signed document may hold.
pub const MAX_ENTRIES: usize = 256;
/// Longest `exec` in bytes.
pub const MAX_EXEC_LEN: usize = 4096;
/// Longest window class in bytes.
pub const MAX_CLASS_LEN: usize = 256;
/// Longest display label in bytes.
pub const MAX_LABEL_LEN: usize = 512;
/// Longest launcher path in bytes.
pub const MAX_PATH_LEN: usize = 4096;
/// Longest account name in bytes.
pub const MAX_USER_LEN: usize = 256;

/// Longest pre- or post-launch delay in milliseconds.
pub const MAX_DELAY_MS: u32 = 60_000;

/// The prefix of every pin id: an entry's `class` and its launcher's file stem.
pub const LAUNCHER_ID_PREFIX: &str = "relauncher-";
/// Hex digits after [`LAUNCHER_ID_PREFIX`] in a pin id.
pub const LAUNCHER_ID_HEX: usize = 16;
/// The directory every launcher lives directly in, as a path suffix. The
/// service cannot see the filesystem, so this is a lexical category; the
/// anchor requires the exact path in the user's own config dir and proves its
/// ownership at boot (spec §8).
pub const LAUNCHERS_DIR_SUFFIX: &str = "/launchers/";

/// Whether `id` is a pin id: [`LAUNCHER_ID_PREFIX`] then exactly
/// [`LAUNCHER_ID_HEX`] lowercase hex digits.
#[must_use]
pub fn is_launcher_id(id: &str) -> bool {
    id.strip_prefix(LAUNCHER_ID_PREFIX).is_some_and(|hex| {
        hex.len() == LAUNCHER_ID_HEX && hex.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
    })
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
/// Which capture tier resolved an entry's command. Decides which eligibility
/// rule applies (spec §6); every source launches the same way, from its
/// private launcher (spec §8).
pub enum ExecSource {
    /// The app's own `.desktop` launcher's `Exec`.
    DesktopFile,
    /// A command the user typed and approved in a reviewed transaction.
    OverridesTable,
    /// The command hosted by a terminal wrapper, from the window's argv.
    Terminal,
    /// The window process's own argv.
    Cmdline,
    /// The lowercased class as one quoted word: nothing better was found.
    Guess,
}

impl ExecSource {
    /// Stable discriminant for the canonical encoding. Never reuse a number.
    fn tag(self) -> u32 {
        match self {
            ExecSource::DesktopFile => 1,
            ExecSource::OverridesTable => 2,
            ExecSource::Terminal => 3,
            ExecSource::Cmdline => 4,
            ExecSource::Guess => 5,
        }
    }
    /// Argv-derived sources carry no independent launcher; they run only with
    /// host provenance (spec §6).
    fn is_argv_derived(self) -> bool {
        matches!(
            self,
            ExecSource::Terminal | ExecSource::Cmdline | ExecSource::Guess
        )
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
/// Whether an argv-derived `exec` came from a host (non-sandboxed) window.
pub enum Provenance {
    /// Window process shared our mnt/user/pid namespaces and was not `XWayland`.
    Host,
    /// Not established; the value for anything not captured from a host window.
    None,
}

impl Provenance {
    fn tag(self) -> u32 {
        match self {
            Provenance::None => 0,
            Provenance::Host => 1,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
/// The private `.desktop` file an entry launches from.
pub struct Launcher {
    /// Absolute path of the launcher, as approved.
    pub path: String,
    /// Lowercase hex SHA-256 of the launcher file the human approved.
    pub sha256: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
/// One app to launch at login and pin to a workspace.
pub struct Entry {
    /// The pin's id ([`is_launcher_id`]): the sort and uniqueness key, and its
    /// launcher's file stem.
    pub class: String,
    /// The real window class the pin was captured from, for display and for
    /// recognising its window at boot. Never a lookup key for anything else.
    #[serde(rename = "windowClass")]
    pub window_class: String,
    /// The workspace to pin the window to (>= 1).
    pub workspace: u32,
    /// Tri-state: `None` means "unknown" (legacy / import with no window) and
    /// emits neither a float nor a tile rule.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub float: Option<bool>,
    /// Display name; cosmetic, never a lookup key.
    #[serde(default)]
    pub label: String,
    /// The launch command (shell string, or canonical `Exec` for desktop-file).
    pub exec: String,
    /// Where `exec` came from.
    #[serde(rename = "execSource")]
    pub exec_source: ExecSource,
    /// Host provenance of an argv-derived `exec`.
    pub provenance: Provenance,
    /// The private launcher the entry runs from.
    pub launcher: Launcher,
    /// The directory the command runs in (the launcher's `Path=`): the
    /// window process's own, when its command line was the one captured.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    /// Whether the entry launches at login.
    pub enabled: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
/// The authenticated launch document: exactly what was authorized.
pub struct SignedConfig {
    /// Signed schema version ([`SIGNED_SCHEMA_VERSION`]).
    pub v: u32,
    /// The uid the document is bound to.
    pub uid: u32,
    /// The account name the document is bound to.
    pub user: String,
    /// The generation committed in the root state for this document.
    pub gen: u64,
    /// Milliseconds to wait before each launch within a same-app group.
    #[serde(rename = "preLaunchMs", default)]
    pub pre_launch_ms: u32,
    /// Milliseconds to wait after each launch within a same-app group.
    #[serde(rename = "postLaunchMs", default)]
    pub post_launch_ms: u32,
    /// The entries, sorted by class, each class at most once.
    pub entries: Vec<Entry>,
}

/// A proposed set of entries, as the panel relays it to `relaunchd` and as the
/// engine's `propose` emits it (`proposalJson`). The identity fields of the
/// signed document (uid, user, generation) are never part of a proposal; the
/// service sets them from bus credentials.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Proposal {
    /// Milliseconds to wait before each launch within a same-app group.
    #[serde(rename = "preLaunchMs", default)]
    pub pre_launch_ms: u32,
    /// Milliseconds to wait after each launch within a same-app group.
    #[serde(rename = "postLaunchMs", default)]
    pub post_launch_ms: u32,
    /// The proposed entries.
    pub entries: Vec<Entry>,
}

/// On-disk `config.json`. `signed` is the authenticated document; `signature`
/// is hex(ed25519) over its canonical bytes; `pending` is display-only and is
/// never verified, emitted or launched.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConfigFile {
    /// Envelope version ([`FILE_FORMAT_VERSION`]).
    pub v: u32,
    /// The authenticated document.
    pub signed: SignedConfig,
    /// Lowercase hex Ed25519 signature over `signed`'s canonical bytes.
    pub signature: String,
    /// Display-only rows awaiting authorization; never verified or launched.
    #[serde(default)]
    pub pending: Vec<serde_json::Value>,
}

/// Verification view of `config.json`: `pending` is parsed as `IgnoredAny`, so
/// the verifier never allocates the (potentially large, attacker-sized) pending
/// array before it checks the signature (reviewer M2/#5). The schema checks on
/// the authenticated fields are unchanged.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct VerifyFile {
    pub v: u32,
    pub signed: SignedConfig,
    pub signature: String,
    /// A sequence whose elements are consumed as `IgnoredAny`: this still
    /// requires `pending` to be an array (matching `ConfigFile`), but allocates
    /// nothing (reviewer #5) — `Vec<IgnoredAny>` is a Vec of a zero-sized type,
    /// so it never touches the heap no matter how many elements are present.
    #[serde(default)]
    #[allow(
        dead_code,
        reason = "parsed only so `pending` must be an array; never read"
    )]
    pub pending: Vec<serde::de::IgnoredAny>,
}

/// One shared upper bound on a serialized `config.json`, used by the service
/// (it refuses to sign a resulting document larger than this) and by the anchor
/// reader (reviewer #1). Comfortably above the worst-case valid signed document
/// (256 entries at the field caps ~= 2.4 MiB), with headroom for the envelope
/// and client-carried `pending`.
pub const MAX_DOCUMENT_BYTES: usize = 4 * 1024 * 1024;

/// Transport cap on ONE request input (a Prepare/Reduce base, or a proposal),
/// shared by the service and the client so they cannot drift (INV-5). It is the
/// document limit plus envelope headroom, so a just-signed document always
/// round-trips as the next base.
pub const MAX_REQUEST_BYTES: usize = MAX_DOCUMENT_BYTES + 64 * 1024;
const _: () = assert!(MAX_REQUEST_BYTES >= MAX_DOCUMENT_BYTES);

/// Most reducing ops in one Reduce request: a disable and a drop for every
/// entry the document can hold.
pub const MAX_REDUCE_OPS: usize = 2 * MAX_ENTRIES;

/// Why a document or entry failed structural validation (spec §6). A `String`
/// payload is the offending entry's class.
#[derive(Debug, PartialEq, Eq)]
pub enum ValidationError {
    /// The signed schema version is not [`SIGNED_SCHEMA_VERSION`].
    SchemaVersion(u32),
    /// More than [`MAX_ENTRIES`] entries.
    TooManyEntries(usize),
    /// An entry has an empty class.
    EmptyClass,
    /// A class longer than [`MAX_CLASS_LEN`] (its length).
    ClassTooLong(usize),
    /// A class contains a control or bidi/format character.
    ControlInClass(String),
    /// Two entries share a class.
    DuplicateClass(String),
    /// Entries are not sorted by class.
    EntriesNotSorted,
    /// The workspace is below 1.
    Workspace(String),
    /// The `exec` is empty.
    EmptyExec(String),
    /// The `exec` is longer than [`MAX_EXEC_LEN`].
    ExecTooLong(String),
    /// The `exec` contains a control (other than tab) or bidi/format character.
    ControlInExec(String),
    /// The label is longer than [`MAX_LABEL_LEN`].
    LabelTooLong(String),
    /// The label contains a control or bidi/format character.
    ControlInLabel(String),
    /// The account name is longer than [`MAX_USER_LEN`].
    UserTooLong,
    /// The account name contains a control or bidi/format character.
    ControlInUser,
    /// The class is not a pin id (`relauncher-` + 16 lowercase hex).
    ClassNotLauncherId(String),
    /// The window class is empty, too long, or holds a control character.
    WindowClass(String),
    /// The launcher is not `<class>.desktop` directly in a launchers directory.
    LauncherName(String),
    /// A pre- or post-launch delay above [`MAX_DELAY_MS`].
    DelayTooLong,
    /// An argv-derived entry lacks host provenance.
    ArgvSourceNeedsHost(String),
    /// A non-argv entry claims host provenance.
    NonArgvSourceHasHost(String),
    /// The launcher path is empty, relative, too long, or has `.`/`..` segments.
    LauncherPath(String),
    /// The working directory is empty, relative, too long, or holds a control
    /// character.
    Cwd(String),
    /// The launcher sha256 is not 64 lowercase hex characters.
    BadSha256(String),
}

/// Reject a string carrying C0/C1 controls or a Unicode bidi/format control.
/// `allow_tab` keeps 0x09, which a shell-quoted hosted command can legitimately
/// contain. The bidi/format set (LRM/RLM, the embedding/override and isolate
/// ranges, ALM) is rejected outright so a class or command can never reorder or
/// disguise itself in the review diff, independent of the diff escaper.
fn has_forbidden_control(s: &str, allow_tab: bool) -> bool {
    s.chars().any(|c| {
        let cp = c as u32;
        let is_ctrl = cp < 0x20 || (0x7f..=0x9f).contains(&cp);
        let is_bidi = matches!(cp,
            0x200E | 0x200F | 0x061C
            | 0x202A..=0x202E
            | 0x2066..=0x2069);
        (is_ctrl && !(allow_tab && c == '\t')) || is_bidi
    })
}

impl Entry {
    /// Structural validation of one entry, including the eligibility allowlist
    /// (spec §6). [`SignedConfig::validate`] applies it to every entry.
    ///
    /// # Errors
    ///
    /// The first [`ValidationError`] found.
    pub fn validate(&self) -> Result<(), ValidationError> {
        if self.class.is_empty() {
            return Err(ValidationError::EmptyClass);
        }
        if self.class.len() > MAX_CLASS_LEN {
            return Err(ValidationError::ClassTooLong(self.class.len()));
        }
        if has_forbidden_control(&self.class, false) {
            return Err(ValidationError::ControlInClass(self.class.clone()));
        }
        if !is_launcher_id(&self.class) {
            return Err(ValidationError::ClassNotLauncherId(self.class.clone()));
        }
        if self.window_class.is_empty()
            || self.window_class.len() > MAX_CLASS_LEN
            || has_forbidden_control(&self.window_class, false)
        {
            return Err(ValidationError::WindowClass(self.class.clone()));
        }
        if self.workspace < 1 {
            return Err(ValidationError::Workspace(self.class.clone()));
        }
        if self.exec.is_empty() {
            return Err(ValidationError::EmptyExec(self.class.clone()));
        }
        if self.exec.len() > MAX_EXEC_LEN {
            return Err(ValidationError::ExecTooLong(self.class.clone()));
        }
        if has_forbidden_control(&self.exec, true) {
            return Err(ValidationError::ControlInExec(self.class.clone()));
        }
        if self.label.len() > MAX_LABEL_LEN {
            return Err(ValidationError::LabelTooLong(self.class.clone()));
        }
        if has_forbidden_control(&self.label, false) {
            return Err(ValidationError::ControlInLabel(self.class.clone()));
        }

        // Eligibility allowlist (spec §6).
        if self.exec_source.is_argv_derived() && self.provenance != Provenance::Host {
            return Err(ValidationError::ArgvSourceNeedsHost(self.class.clone()));
        }
        if !self.exec_source.is_argv_derived() && self.provenance == Provenance::Host {
            // Only argv-derived rows carry host provenance; a desktop-file or
            // overrides-table row claiming it is malformed.
            return Err(ValidationError::NonArgvSourceHasHost(self.class.clone()));
        }

        // Every entry launches from its own private launcher, named by its id.
        let launcher = &self.launcher;
        validate_launcher_path(&launcher.path, &self.class)?;
        let expected = format!("{}{}.desktop", LAUNCHERS_DIR_SUFFIX, self.class);
        if !launcher.path.ends_with(&expected) {
            return Err(ValidationError::LauncherName(self.class.clone()));
        }
        validate_sha256(&launcher.sha256, &self.class)?;
        if let Some(cwd) = &self.cwd {
            if cwd.is_empty()
                || !cwd.starts_with('/')
                || cwd.len() > MAX_PATH_LEN
                || has_forbidden_control(cwd, false)
            {
                return Err(ValidationError::Cwd(self.class.clone()));
            }
        }
        Ok(())
    }
}

fn validate_launcher_path(path: &str, class: &str) -> Result<(), ValidationError> {
    if path.is_empty() || path.len() > MAX_PATH_LEN || !path.starts_with('/') {
        return Err(ValidationError::LauncherPath(class.to_string()));
    }
    if has_forbidden_control(path, false) {
        return Err(ValidationError::LauncherPath(class.to_string()));
    }
    // Reject any "." or ".." segment before the anchor ever resolves it.
    if path.split('/').any(|seg| seg == ".." || seg == ".") {
        return Err(ValidationError::LauncherPath(class.to_string()));
    }
    Ok(())
}

fn validate_sha256(h: &str, class: &str) -> Result<(), ValidationError> {
    if h.len() == 64 && h.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f')) {
        Ok(())
    } else {
        Err(ValidationError::BadSha256(class.to_string()))
    }
}

impl SignedConfig {
    /// Full structural validation. Also enforces that entries are sorted by
    /// class and that no class repeats, so the canonical bytes are unique and a
    /// class-keyed map can never hide one of two duplicate rows from review.
    ///
    /// # Errors
    ///
    /// Returns the first [`ValidationError`] found, document-level checks first,
    /// then each entry in order.
    pub fn validate(&self) -> Result<(), ValidationError> {
        if self.v != SIGNED_SCHEMA_VERSION {
            return Err(ValidationError::SchemaVersion(self.v));
        }
        if self.pre_launch_ms > MAX_DELAY_MS || self.post_launch_ms > MAX_DELAY_MS {
            return Err(ValidationError::DelayTooLong);
        }
        if self.user.len() > MAX_USER_LEN {
            return Err(ValidationError::UserTooLong);
        }
        if has_forbidden_control(&self.user, false) {
            return Err(ValidationError::ControlInUser);
        }
        if self.entries.len() > MAX_ENTRIES {
            return Err(ValidationError::TooManyEntries(self.entries.len()));
        }
        let mut prev: Option<&Entry> = None;
        for e in &self.entries {
            e.validate()?;
            if let Some(p) = prev {
                if p.class == e.class {
                    return Err(ValidationError::DuplicateClass(e.class.clone()));
                }
                if p.class.as_bytes() > e.class.as_bytes() {
                    return Err(ValidationError::EntriesNotSorted);
                }
            }
            prev = Some(e);
        }
        Ok(())
    }

    /// The signed document a proposal becomes: bound to `uid`/`user`/`gen`,
    /// entries in canonical order, fully validated.
    ///
    /// # Errors
    ///
    /// The first [`ValidationError`] of the resulting document.
    pub fn from_proposal(
        uid: u32,
        user: &str,
        gen: u64,
        proposal: Proposal,
    ) -> Result<Self, ValidationError> {
        let mut cfg = SignedConfig {
            v: SIGNED_SCHEMA_VERSION,
            uid,
            user: user.to_string(),
            gen,
            pre_launch_ms: proposal.pre_launch_ms,
            post_launch_ms: proposal.post_launch_ms,
            entries: proposal.entries,
        };
        cfg.sort_entries();
        cfg.validate()?;
        Ok(cfg)
    }

    /// Sort entries by class, the canonical order. Does not detect duplicates;
    /// `validate` does that.
    pub fn sort_entries(&mut self) {
        self.entries
            .sort_by(|a, b| a.class.as_bytes().cmp(b.class.as_bytes()));
    }
}

// Expose the discriminants to the canonical encoder without widening the public
// surface of the enums.
pub(crate) fn exec_source_tag(s: ExecSource) -> u32 {
    s.tag()
}
pub(crate) fn provenance_tag(p: Provenance) -> u32 {
    p.tag()
}
