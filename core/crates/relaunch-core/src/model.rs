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
pub const SIGNED_SCHEMA_VERSION: u32 = 1;

/// Version of the on-disk `config.json` envelope.
pub const FILE_FORMAT_VERSION: u32 = 2;

/// Caps a filesystem-blind service can enforce (spec §6, §D7).
pub const MAX_ENTRIES: usize = 256;
pub const MAX_EXEC_LEN: usize = 4096;
pub const MAX_CLASS_LEN: usize = 256;
pub const MAX_LABEL_LEN: usize = 512;
pub const MAX_PATH_LEN: usize = 4096;
pub const MAX_USER_LEN: usize = 256;

/// `.desktop` launchers under these prefixes are trusted by ownership and need
/// no client-supplied hash. Fixed byte list; lexical membership is only an
/// eligibility *category* here — the anchor still proves ownership of the
/// resolved chain at boot (spec §8).
pub const ROOT_LAUNCHER_PREFIXES: [&str; 3] = [
    "/usr/share/applications/",
    "/usr/local/share/applications/",
    "/var/lib/flatpak/exports/share/applications/",
];

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ExecSource {
    DesktopFile,
    OverridesTable,
    Terminal,
    Cmdline,
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
pub enum Provenance {
    /// Window process shared our mnt/user/pid namespaces and was not XWayland.
    Host,
    /// Not established. The default for anything not captured from a host window.
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
pub struct Launcher {
    pub path: String,
    /// Lowercase hex SHA-256 of the launcher file the human approved. Absent
    /// only for a root-prefix launcher (trusted by ownership at the anchor).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sha256: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Entry {
    pub class: String,
    pub workspace: u32,
    /// Tri-state: `None` means "unknown" (legacy / import with no window) and
    /// emits neither a float nor a tile rule.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub float: Option<bool>,
    #[serde(default)]
    pub label: String,
    pub exec: String,
    #[serde(rename = "execSource")]
    pub exec_source: ExecSource,
    pub provenance: Provenance,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub launcher: Option<Launcher>,
    pub enabled: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SignedConfig {
    pub v: u32,
    pub uid: u32,
    pub user: String,
    pub gen: u64,
    #[serde(rename = "staggerSeconds", default)]
    pub stagger_seconds: u32,
    pub entries: Vec<Entry>,
}

/// On-disk `config.json`. `signed` is the authenticated document; `signature`
/// is hex(ed25519) over its canonical bytes; `pending` is display-only and is
/// never verified, emitted or launched.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConfigFile {
    pub v: u32,
    pub signed: SignedConfig,
    pub signature: String,
    #[serde(default)]
    pub pending: Vec<serde_json::Value>,
}

#[derive(Debug, PartialEq, Eq)]
pub enum ValidationError {
    SchemaVersion(u32),
    TooManyEntries(usize),
    EmptyClass,
    ClassTooLong(usize),
    ControlInClass(String),
    DuplicateClass(String),
    EntriesNotSorted,
    Workspace(String),
    EmptyExec(String),
    ExecTooLong(String),
    ControlInExec(String),
    LabelTooLong(String),
    ControlInLabel(String),
    UserTooLong,
    ControlInUser,
    DesktopFileNeedsLauncher(String),
    LauncherOnShellSource(String),
    ArgvSourceNeedsHost(String),
    NonArgvSourceHasHost(String),
    LauncherPath(String),
    LauncherNeedsHashOrRootPrefix(String),
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

fn is_root_prefix(path: &str) -> bool {
    ROOT_LAUNCHER_PREFIXES.iter().any(|p| path.starts_with(p))
}

impl Entry {
    fn validate(&self) -> Result<(), ValidationError> {
        if self.class.is_empty() {
            return Err(ValidationError::EmptyClass);
        }
        if self.class.len() > MAX_CLASS_LEN {
            return Err(ValidationError::ClassTooLong(self.class.len()));
        }
        if has_forbidden_control(&self.class, false) {
            return Err(ValidationError::ControlInClass(self.class.clone()));
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

        match self.exec_source {
            ExecSource::DesktopFile => {
                let launcher = self
                    .launcher
                    .as_ref()
                    .ok_or_else(|| ValidationError::DesktopFileNeedsLauncher(self.class.clone()))?;
                validate_launcher_path(&launcher.path, &self.class)?;
                match &launcher.sha256 {
                    Some(h) => validate_sha256(h, &self.class)?,
                    None => {
                        if !is_root_prefix(&launcher.path) {
                            return Err(ValidationError::LauncherNeedsHashOrRootPrefix(
                                self.class.clone(),
                            ));
                        }
                    }
                }
            }
            _ => {
                // The signed exec IS the shell string for these sources; a
                // launcher field would be a second, unreviewed command.
                if self.launcher.is_some() {
                    return Err(ValidationError::LauncherOnShellSource(self.class.clone()));
                }
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
    pub fn validate(&self) -> Result<(), ValidationError> {
        if self.v != SIGNED_SCHEMA_VERSION {
            return Err(ValidationError::SchemaVersion(self.v));
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
        for (i, e) in self.entries.iter().enumerate() {
            e.validate()?;
            if i > 0 {
                let prev = &self.entries[i - 1].class;
                if prev == &e.class {
                    return Err(ValidationError::DuplicateClass(e.class.clone()));
                }
                if prev.as_bytes() > e.class.as_bytes() {
                    return Err(ValidationError::EntriesNotSorted);
                }
            }
        }
        Ok(())
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
