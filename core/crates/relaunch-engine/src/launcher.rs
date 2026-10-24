//! The all-private per-pin `.desktop` launchers (spec: the `.desktop` launch
//! model). Every pin is one `relauncher-<id>.desktop` under
//! `<config_dir>/launchers/`, holding the capture's resolved command verbatim
//! as its `Exec`. The signed entry binds the file's path and sha256; the
//! anchor reads it once at boot and runs the argv parsed from those bytes, so
//! a modified launcher is refused.
//!
//! A launcher's bytes are a function of its signed entry alone
//! ([`launcher_bytes`]: the label and the `exec`), so capture only computes
//! the hash, and [`install`] writes the files from a document `relaunchd`
//! signed. Nothing on disk changes until a change is authorized.
//!
//! The command is written UNMODIFIED — no `--app-id` is injected (Chromium's
//! `--app-id` selects an app, not a window class, so injecting one breaks web
//! apps; placement is handled separately by the boot launch orchestration).
//!
//! The one `.desktop` parser ([`relaunch_desktop`], INV-2) is what the anchor
//! verifies with, so [`serialize_exec`] here produces an `Exec` that parser —
//! and `gio`, which follows the same freedesktop spec — reads back to the exact
//! argv. The round-trip is proved in tests against `relaunch_desktop::parse_exec`.

use std::collections::BTreeSet;
use std::io;
use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
use std::path::{Path, PathBuf};

use relaunch_core::{Entry, SignedConfig};
use relaunch_desktop::split_canonical_exec;
use sha2::{Digest, Sha256};

/// The config-dir subdirectory holding generated launchers.
pub const LAUNCHERS_SUBDIR: &str = "launchers";
/// The prefix of every generated id, its file name, and its pin identity.
pub const ID_PREFIX: &str = "relauncher-";

/// The private launcher directory for a config dir.
#[must_use]
pub fn launchers_dir(config_dir: &Path) -> PathBuf {
    config_dir.join(LAUNCHERS_SUBDIR)
}

/// The file name for an id: `<id>.desktop`.
#[must_use]
pub fn file_name(id: &str) -> String {
    format!("{id}.desktop")
}

/// Whether a file name is a generated launcher (`relauncher-<id>.desktop`).
#[must_use]
pub fn is_launcher_file(name: &str) -> bool {
    name.starts_with(ID_PREFIX) && name.ends_with(".desktop")
}

/// The id a launcher file name carries (`relauncher-ab12.desktop` ->
/// `relauncher-ab12`), or `None` if it is not a launcher file.
#[must_use]
pub fn id_of_file(name: &str) -> Option<String> {
    is_launcher_file(name).then(|| name.trim_end_matches(".desktop").to_owned())
}

/// Mint a fresh unique id: [`ID_PREFIX`] + 16 lowercase hex chars. Filename-,
/// class- and `Exec`-safe (no control, path, quoting or bidi characters).
#[must_use]
pub fn mint_id() -> String {
    let mut b = [0u8; 8];
    // The OS RNG is the source. If it is somehow unavailable, fall back to a
    // time+pid seed so an id is still produced; uniqueness is then best effort,
    // which never happens on a running system.
    if getrandom::fill(&mut b).is_err() {
        let d = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default();
        let seed = d
            .as_secs()
            .wrapping_mul(1_000_000_000)
            .wrapping_add(u64::from(d.subsec_nanos()))
            ^ (u64::from(std::process::id()) << 32);
        b.copy_from_slice(&seed.to_le_bytes());
    }
    format!("{ID_PREFIX}{}", hex::encode(b))
}

/// The id a new pin of `window_class` on `workspace` gets: [`mint_id`]. A
/// fixtures build derives it from the two instead (and `nth`, for a further
/// new pin at that place), so recorded test output is reproducible.
#[must_use]
pub fn new_id(window_class: &str, workspace: u32, nth: u32) -> String {
    if cfg!(feature = "fixtures") {
        let seed = match nth {
            0 => format!("{window_class}\0{workspace}"),
            n => format!("{window_class}\0{workspace}\0{n}"),
        };
        let digest = Sha256::digest(seed);
        let hex = hex::encode(digest);
        format!("{ID_PREFIX}{}", hex.get(..16).unwrap_or_default())
    } else {
        mint_id()
    }
}

/// Reserved characters that force an `Exec` argument to be double-quoted (the
/// freedesktop spec's reserved set; whitespace plus the shell metacharacters).
fn needs_quoting(arg: &str) -> bool {
    arg.is_empty()
        || arg.chars().any(|c| {
            matches!(
                c,
                ' ' | '\t'
                    | '"'
                    | '`'
                    | '$'
                    | '\\'
                    | '\''
                    | '<'
                    | '>'
                    | '~'
                    | '|'
                    | '&'
                    | ';'
                    | '*'
                    | '?'
                    | '#'
                    | '('
                    | ')'
            )
        })
}

/// Escape a value for the `.desktop` file-format layer (the freedesktop crate
/// decodes this BEFORE our `Exec` tokenizer runs): a literal backslash is
/// doubled so it survives as one backslash. Control characters are handled by
/// the callers (rejected), so no `\n`/`\t`/`\r` escaping is needed here.
fn file_escape(s: &str) -> String {
    s.replace('\\', "\\\\")
}

/// Render an argv as a freedesktop `Exec` value that [`relaunch_desktop`] (and
/// `gio`) parse back to exactly this argv. Two escaping layers stack: the
/// `Exec` quoting (`"`-quote an argument with a reserved char; inside, escape
/// `"`, `` ` ``, `$`, `\`; `%` -> `%%`), then the file-format layer
/// ([`file_escape`], which doubles every backslash). The round-trip is proved
/// against `relaunch_desktop::parse_exec` in tests.
///
/// # Errors
///
/// A message if an argument contains a control character (which no valid
/// resolved command carries, and which the signed model already forbids), or if
/// the argv is empty.
pub fn serialize_exec(argv: &[String]) -> Result<String, String> {
    if argv.is_empty() {
        return Err("cannot serialize an empty Exec".into());
    }
    let mut out = Vec::with_capacity(argv.len());
    for arg in argv {
        if arg.chars().any(char::is_control) {
            return Err("Exec argument has a control character".into());
        }
        if needs_quoting(arg) {
            let mut q = String::with_capacity(arg.len() + 2);
            q.push('"');
            for c in arg.chars() {
                match c {
                    '"' | '`' | '$' | '\\' => {
                        q.push('\\');
                        q.push(c);
                    }
                    '%' => q.push_str("%%"),
                    other => q.push(other),
                }
            }
            q.push('"');
            out.push(q);
        } else {
            out.push(arg.replace('%', "%%"));
        }
    }
    Ok(file_escape(&out.join(" ")))
}

/// Build the `.desktop` text for a pin: the resolved command (as argv) verbatim
/// in `Exec`, the directory it runs in as `Path` when there is one, a display
/// `Name`, and `NoDisplay` so it can never surface in a menu even if it is ever
/// copied into an applications dir.
///
/// # Errors
///
/// A message if `label` has a control character (it would corrupt the file or
/// inject a key) or if [`serialize_exec`] refuses the argv.
pub fn desktop_contents(label: &str, argv: &[String], cwd: Option<&str>) -> Result<String, String> {
    if label.chars().any(char::is_control) {
        return Err("launcher label has a control character".into());
    }
    let exec = serialize_exec(argv)?;
    let name = string_value(label);
    let path = match cwd {
        Some(dir) if dir.chars().any(char::is_control) => {
            return Err("launcher directory has a control character".into())
        }
        Some(dir) => format!("Path={}\n", string_value(dir)),
        None => String::new(),
    };
    Ok(format!(
        "[Desktop Entry]\nType=Application\nName={name}\nExec={exec}\n{path}NoDisplay=true\nTerminal=false\n"
    ))
}

/// A `.desktop` string value: [`file_escape`], and a leading space as `\s`
/// (the format would otherwise drop it).
fn string_value(s: &str) -> String {
    let escaped = file_escape(s);
    match escaped.strip_prefix(' ') {
        Some(rest) => format!("\\s{rest}"),
        None => escaped,
    }
}

/// Ensure the private launcher dir exists at mode `0700`, owned by this user.
/// Any directory it has to create on the way — the config dir itself, on a
/// first install — is born `0700` too, whatever the umask.
///
/// # Errors
///
/// Any I/O error creating it or setting its mode.
pub fn ensure_launchers_dir(config_dir: &Path) -> io::Result<PathBuf> {
    let dir = launchers_dir(config_dir);
    std::fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(&dir)?;
    std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700))?;
    Ok(dir)
}

/// The bytes `entry`'s launcher holds: its label as `Name` and its `exec`,
/// as argv, as `Exec`.
///
/// # Errors
///
/// As [`desktop_contents`].
pub fn launcher_bytes(entry: &Entry) -> Result<Vec<u8>, String> {
    desktop_contents(
        &entry.label,
        &split_canonical_exec(&entry.exec),
        entry.cwd.as_deref(),
    )
    .map(String::into_bytes)
}

/// The lowercase-hex sha256 of `bytes`: what a signed entry binds.
#[must_use]
pub fn sha256_hex(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

/// The `launcher` an entry with this id, label, `exec` and `cwd` signs: its
/// path in this config dir and the hash of the bytes [`launcher_bytes`] gives
/// it.
///
/// # Errors
///
/// As [`desktop_contents`].
pub fn binding(
    config_dir: &Path,
    id: &str,
    label: &str,
    exec: &str,
    cwd: Option<&str>,
) -> Result<relaunch_core::Launcher, String> {
    let bytes = desktop_contents(label, &split_canonical_exec(exec), cwd)?;
    Ok(relaunch_core::Launcher {
        path: launchers_dir(config_dir)
            .join(file_name(id))
            .to_string_lossy()
            .into_owned(),
        sha256: sha256_hex(bytes.as_bytes()),
    })
}

/// Write every launcher of a signed document. Each entry's bytes are rebuilt
/// from the entry and must hash to what was signed, at the path that was
/// signed, or nothing is written at all. Launchers no longer named are left
/// for [`collect`], once the document itself is in place.
///
/// # Errors
///
/// A message naming the first entry whose launcher does not match its
/// signature, or the write that failed.
pub fn install(config_dir: &Path, cfg: &SignedConfig) -> Result<(), String> {
    let dir = launchers_dir(config_dir);
    let mut files = Vec::with_capacity(cfg.entries.len());
    for e in &cfg.entries {
        let bytes = launcher_bytes(e)?;
        let path = dir.join(file_name(&e.class));
        if Path::new(&e.launcher.path) != path || sha256_hex(&bytes) != e.launcher.sha256 {
            return Err(format!(
                "the signed launcher for {} does not match its entry",
                e.class
            ));
        }
        files.push((file_name(&e.class), bytes));
    }
    if files.is_empty() {
        return Ok(());
    }
    let dir = ensure_launchers_dir(config_dir).map_err(|e| format!("launcher dir: {e}"))?;
    for (name, bytes) in &files {
        relaunch_sys::fs::write_private_artifact(&dir, name, bytes)
            .map_err(|e| format!("write {name}: {e}"))?;
    }
    Ok(())
}

/// Remove every generated launcher `cfg` does not name ([`reconcile`]).
///
/// # Errors
///
/// An I/O error reading the launcher directory.
pub fn collect(config_dir: &Path, cfg: &SignedConfig) -> Result<(), String> {
    let live: BTreeSet<String> = cfg.entries.iter().map(|e| e.class.clone()).collect();
    reconcile(config_dir, &live)
        .map(drop)
        .map_err(|e| format!("launcher dir: {e}"))
}

/// Remove every `relauncher-<id>.desktop` whose id is not in `live` (GC-
/// reconcile): dropping a pin orphans its file, which the next reconcile
/// collects. Returns the ids removed. A missing launcher dir is not an error.
///
/// # Errors
///
/// An I/O error reading the dir (a per-file remove error is skipped, not fatal,
/// so one unreadable entry cannot strand the rest).
pub fn reconcile(config_dir: &Path, live: &BTreeSet<String>) -> io::Result<Vec<String>> {
    let dir = launchers_dir(config_dir);
    let entries = match std::fs::read_dir(&dir) {
        Ok(e) => e,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(e),
    };
    let mut removed = Vec::new();
    for entry in entries.flatten() {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if let Some(id) = id_of_file(&name) {
            if !live.contains(&id) && std::fs::remove_file(entry.path()).is_ok() {
                removed.push(id);
            }
        }
    }
    removed.sort();
    Ok(removed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn id_is_prefixed_hex_and_distinct() {
        let a = mint_id();
        let b = mint_id();
        assert!(a.starts_with(ID_PREFIX));
        let hexpart = a.trim_start_matches(ID_PREFIX);
        assert_eq!(hexpart.len(), 16);
        assert!(hexpart.chars().all(|c| c.is_ascii_hexdigit()));
        assert_ne!(a, b, "two mints collide");
        assert!(is_launcher_file(&file_name(&a)));
        assert_eq!(id_of_file(&file_name(&a)).as_deref(), Some(a.as_str()));
    }

    // The serializer must round-trip through the ONE parser (INV-2): what we
    // write is exactly what the anchor (and gio) read back.
    fn round_trips(argv: &[&str]) {
        let argv: Vec<String> = argv.iter().map(|s| (*s).to_owned()).collect();
        let exec = serialize_exec(&argv).unwrap();
        let text = format!("[Desktop Entry]\nType=Application\nExec={exec}\n");
        let back = relaunch_desktop::parse_exec(&text)
            .unwrap_or_else(|e| panic!("exec {exec:?} did not parse: {e}"));
        assert_eq!(back, argv, "round-trip failed for exec {exec:?}");
    }

    #[test]
    fn exec_round_trips_through_the_parser() {
        round_trips(&["foot"]);
        round_trips(&["brave", "--profile-directory=Default", "--app-id=fnnddiok"]);
        round_trips(&["signal-desktop", "--"]);
        round_trips(&["omarchy-launch-webapp", "https://maps.google.com"]);
        round_trips(&["a b", "c d"]); // embedded spaces
        round_trips(&["say", "100%", "done"]); // literal percent
        round_trips(&["q\"u$o`te\\s"]); // every in-quote escape
        round_trips(&["weird;|&<>()*?#~'"]); // shell metachars
        round_trips(&["--msg=a b % \"x\""]); // mixed inside one arg
    }

    #[test]
    fn serialize_rejects_control_and_empty() {
        assert!(serialize_exec(&[]).is_err());
        assert!(serialize_exec(&["ok".into(), "bad\nline".into()]).is_err());
    }

    #[test]
    fn desktop_contents_parses_and_rejects_bad_label() {
        let argv = vec!["foot".to_string()];
        let text = desktop_contents("My Terminal", &argv, None).unwrap();
        assert_eq!(relaunch_desktop::parse_exec(&text).unwrap(), argv);
        assert!(text.contains("NoDisplay=true"));
        assert!(desktop_contents("bad\nName", &argv, None).is_err());
        assert!(desktop_contents("ok", &argv, Some("/a\nb")).is_err());
        for dir in ["/home/u", "/opt/My Dir", " /lead", "/back\\slash", "/x/"] {
            let text = desktop_contents("X", &argv, Some(dir)).unwrap();
            let path = relaunch_desktop::parse_entry(&text).unwrap().path;
            assert_eq!(path.as_deref(), Some(dir), "{dir:?}: {text}");
        }
    }

    fn signed(config_dir: &Path, pins: &[(&str, &str, &str)]) -> SignedConfig {
        let entries = pins
            .iter()
            .map(|(id, label, exec)| Entry {
                class: (*id).to_owned(),
                window_class: "foot".into(),
                workspace: 1,
                float: None,
                label: (*label).to_owned(),
                exec: (*exec).to_owned(),
                exec_source: relaunch_core::ExecSource::Cmdline,
                provenance: relaunch_core::Provenance::Host,
                launcher: binding(config_dir, id, label, exec, None).unwrap(),
                cwd: None,
                enabled: true,
            })
            .collect();
        SignedConfig {
            v: relaunch_core::SIGNED_SCHEMA_VERSION,
            uid: 1,
            user: "u".into(),
            gen: 1,
            pre_launch_ms: 0,
            post_launch_ms: 500,
            entries,
        }
    }

    #[test]
    fn install_writes_each_signed_launcher_and_the_anchor_reads_its_exec_back() {
        let tmp = std::env::temp_dir().join(format!("rl-install-{}", mint_id()));
        let (a, b) = (mint_id(), mint_id());
        let cfg = signed(
            &tmp,
            &[(&a, "Foot", "foot --title=X\\ Y"), (&b, "", "a\\\\b \\ ")],
        );
        install(&tmp, &cfg).unwrap();
        for e in &cfg.entries {
            let on_disk = std::fs::read(&e.launcher.path).unwrap();
            assert_eq!(sha256_hex(&on_disk), e.launcher.sha256);
            let argv = relaunch_desktop::parse_exec(&String::from_utf8(on_disk).unwrap()).unwrap();
            assert_eq!(relaunch_desktop::canonical_exec_string(&argv), e.exec);
        }
        let mode = std::fs::metadata(launchers_dir(&tmp))
            .unwrap()
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(mode, 0o700, "the dir is private");
        std::fs::remove_dir_all(&tmp).ok();
    }

    #[test]
    fn install_refuses_the_whole_document_if_any_launcher_does_not_match() {
        let tmp = std::env::temp_dir().join(format!("rl-refuse-{}", mint_id()));
        let (a, b) = (mint_id(), mint_id());
        let mut cfg = signed(&tmp, &[(&a, "A", "a"), (&b, "B", "b")]);
        cfg.entries[1].exec = "evil".into();
        assert!(install(&tmp, &cfg).is_err());
        assert!(!launchers_dir(&tmp).exists(), "nothing is written");
        let mut cfg = signed(&tmp, &[(&a, "A", "a")]);
        cfg.entries[0].launcher.path = tmp.join("elsewhere.desktop").to_string_lossy().into_owned();
        assert!(
            install(&tmp, &cfg).is_err(),
            "a signed path elsewhere is refused"
        );
        std::fs::remove_dir_all(&tmp).ok();
    }

    #[test]
    fn a_first_install_creates_the_config_dir_private_whatever_the_umask() {
        let tmp = std::env::temp_dir().join(format!("rl-umask-{}", mint_id()));
        let cfg = tmp.join("omarchy-relaunch");
        let id = mint_id();
        std::fs::create_dir_all(&tmp).unwrap();
        std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o755)).unwrap();
        install(&cfg, &signed(&cfg, &[(&id, "A", "a")])).unwrap();
        for d in [&cfg, &launchers_dir(&cfg)] {
            let mode = std::fs::metadata(d).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o700, "{}", d.display());
        }
        let empty = tmp.join("empty");
        install(&empty, &signed(&empty, &[])).unwrap();
        assert!(!empty.exists(), "a document with no pins creates nothing");
        std::fs::remove_dir_all(&tmp).ok();
    }

    #[test]
    fn collect_removes_the_launchers_a_document_no_longer_names() {
        let tmp = std::env::temp_dir().join(format!("rl-collect-{}", mint_id()));
        let (keep, gone) = (mint_id(), mint_id());
        install(&tmp, &signed(&tmp, &[(&keep, "K", "k"), (&gone, "G", "g")])).unwrap();
        let next = signed(&tmp, &[(&keep, "K", "k")]);
        install(&tmp, &next).unwrap();
        assert!(
            launchers_dir(&tmp).join(file_name(&gone)).exists(),
            "install alone deletes nothing"
        );
        collect(&tmp, &next).unwrap();
        assert!(launchers_dir(&tmp).join(file_name(&keep)).exists());
        assert!(!launchers_dir(&tmp).join(file_name(&gone)).exists());
        std::fs::remove_dir_all(&tmp).ok();
    }

    #[test]
    fn reconcile_removes_orphans_keeps_live() {
        let tmp = std::env::temp_dir().join(format!("rl-recon-{}", mint_id()));
        let keep = mint_id();
        let drop = mint_id();
        install(
            &tmp,
            &signed(&tmp, &[(&keep, "Keep", "foot"), (&drop, "Drop", "foot")]),
        )
        .unwrap();
        // A non-launcher file in the dir must be left alone.
        std::fs::write(launchers_dir(&tmp).join("notes.txt"), b"x").unwrap();
        let mut live = BTreeSet::new();
        live.insert(keep.clone());
        let removed = reconcile(&tmp, &live).unwrap();
        assert_eq!(removed, vec![drop.clone()]);
        assert!(launchers_dir(&tmp).join(file_name(&keep)).exists());
        assert!(!launchers_dir(&tmp).join(file_name(&drop)).exists());
        assert!(launchers_dir(&tmp).join("notes.txt").exists());
        // Reconcile on a missing dir is a no-op, not an error.
        std::fs::remove_dir_all(&tmp).ok();
        assert_eq!(reconcile(&tmp, &live).unwrap(), Vec::<String>::new());
    }
}
