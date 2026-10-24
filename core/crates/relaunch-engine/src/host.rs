//! The ONE place the engine reads process state (INV-1).
//!
//! Every argv read, every namespace check and every parent hop goes through
//! [`Proc`]. A process outside our mount, user or PID namespace is inside a
//! sandbox or container: its argv is not ours to read or trust. [`Proc::walk`]
//! therefore stops at the first non-host pid — the start pid included — and
//! fails closed on any unreadable namespace link. No other module touches
//! process state (a test enforces it).

use std::path::{Path, PathBuf};

use relaunch_sys::fs::{read_regular_capped, MAX_FILE_BYTES};

/// Most pids a walk visits (the window's process and seven ancestors).
const MAX_HOPS: usize = 8;

/// Access to process state: the live `/proc`, or (fixtures build only) a
/// fixture directory of `<pid>` argv files, `<pid>.ppid` and `<pid>.ns`.
#[derive(Clone, Debug)]
pub struct Proc {
    fixture: Option<PathBuf>,
}

impl Proc {
    /// The live system's `/proc`.
    #[must_use]
    pub fn live() -> Self {
        Self { fixture: None }
    }

    /// A fixture tree (`RELAUNCH_CMDLINE_DIR`); only constructible in a
    /// `fixtures` build, so the release binary always reads `/proc`.
    #[cfg(feature = "fixtures")]
    #[must_use]
    pub fn fixture(dir: PathBuf) -> Self {
        Self { fixture: Some(dir) }
    }

    /// Whether a fixture tree is in use (it relaxes the missing-`xwayland`
    /// rule for fixtures that predate that field).
    #[must_use]
    pub fn is_fixture(&self) -> bool {
        self.fixture.is_some()
    }

    /// `pid` shares our mount, user and PID namespaces. Any unreadable link is
    /// `false` (fail closed). In a fixture tree, `<pid>.ns` holding `host`
    /// decides, and an absent file means host.
    #[must_use]
    pub fn is_host(&self, pid: u32) -> bool {
        if pid == 0 {
            return false;
        }
        if let Some(dir) = &self.fixture {
            return read_text(&dir.join(format!("{pid}.ns")))
                .is_none_or(|s| s.split_whitespace().collect::<String>() == "host");
        }
        ["mnt", "user", "pid"].iter().all(|ns| {
            let theirs = std::fs::read_link(format!("/proc/{pid}/ns/{ns}"));
            let ours = std::fs::read_link(format!("/proc/self/ns/{ns}"));
            matches!((theirs, ours), (Ok(a), Ok(b)) if !a.as_os_str().is_empty() && a == b)
        })
    }

    /// The argv of `pid`: NUL-separated words, an unterminated final chunk
    /// dropped (bash `read -d ''`). `None` if unreadable or empty.
    fn argv(&self, pid: u32) -> Option<Vec<String>> {
        let file = match &self.fixture {
            Some(dir) => dir.join(pid.to_string()),
            None => PathBuf::from(format!("/proc/{pid}/cmdline")),
        };
        let bytes = read_regular_capped(&file, MAX_FILE_BYTES).ok()?;
        let text = String::from_utf8_lossy(&bytes);
        let mut words: Vec<String> = text.split('\0').map(String::from).collect();
        words.pop();
        (!words.is_empty()).then_some(words)
    }

    /// The parent pid, or 0 when unknown.
    fn ppid(&self, pid: u32) -> u32 {
        match &self.fixture {
            Some(dir) => read_text(&dir.join(format!("{pid}.ppid")))
                .and_then(|s| s.split_whitespace().collect::<String>().parse().ok())
                .unwrap_or(0),
            None => status_ppid(&PathBuf::from(format!("/proc/{pid}/status"))),
        }
    }

    /// Visit `pid` and its ancestors, at most [`MAX_HOPS`], calling `visit` with
    /// each readable argv until it returns `Some`. The walk ends at the first
    /// pid that is not on the host — before reading that pid's argv — and at a
    /// pid with no (or a self-referencing) parent.
    pub fn walk<T>(&self, pid: u32, mut visit: impl FnMut(&[String]) -> Option<T>) -> Option<T> {
        self.walk_pid(pid, |_, argv| visit(argv))
    }

    /// [`Proc::walk`], also telling `visit` which pid's argv it is reading,
    /// and answering with it.
    pub fn walk_pid<T>(
        &self,
        pid: u32,
        mut visit: impl FnMut(u32, &[String]) -> Option<T>,
    ) -> Option<T> {
        let mut pid = pid;
        for _ in 0..MAX_HOPS {
            if pid == 0 || !self.is_host(pid) {
                return None;
            }
            if let Some(found) = self.argv(pid).and_then(|argv| visit(pid, &argv)) {
                return Some(found);
            }
            let next = self.ppid(pid);
            if next == pid || next == 0 {
                return None;
            }
            pid = next;
        }
        None
    }

    /// What runs in the foreground of the terminal whose process is `pid`:
    /// the foreground process group of its child's controlling tty. `None`
    /// when no host child with a tty is found or anything along the way is
    /// not a host process; [`Foreground::Shell`] when the terminal's own child
    /// is itself the foreground (nothing was started in it).
    ///
    /// A leader whose namespaces cannot be read at all is still the command
    /// when [`Proc::hidden_leader`] proves the host started it.
    #[must_use]
    pub fn foreground(&self, pid: u32) -> Option<Foreground> {
        if pid == 0 || !self.is_host(pid) {
            return None;
        }
        let (child, tpgid) = self
            .children(pid)
            .into_iter()
            .filter(|&c| self.is_host(c))
            .find_map(|c| self.tpgid(c).map(|t| (c, t)))?;
        if tpgid == child {
            return Some(Foreground::Shell { pid: child });
        }
        let cwd = if self.is_host(tpgid) {
            self.cwd(tpgid)
        } else {
            self.hidden_leader(tpgid, child)?.cwd
        };
        let argv = self.argv(tpgid)?;
        Some(Foreground::Command {
            pid: tpgid,
            argv,
            cwd,
        })
    }

    /// A foreground leader the kernel will not show us (a non-dumpable
    /// process: setuid, file capabilities), accepted only when the host
    /// started it: every namespace link refused with `EACCES` (none gone,
    /// none readable), as deep in PID namespaces as we are, our real uid, and
    /// every ancestor up to the terminal's shell `shell` on the host. A
    /// sandboxed process can hide its links too, but not its PID-namespace
    /// depth or a non-host parent: an unprivileged sandbox's own processes
    /// stay readable to the user who owns it (and differ), and a privileged
    /// helper is itself unreadable. Its own directory is hidden too, so it
    /// runs in the nearest such ancestor's (the shell's, inherited at fork).
    fn hidden_leader(&self, leader: u32, shell: u32) -> Option<HiddenLeader> {
        if !self.ns_refused(leader)
            || self.nspid_depth(leader)? != self.nspid_depth(std::process::id())?
            || self.real_uid(leader)? != relaunch_sys::current_uid()
        {
            return None;
        }
        let mut cwd = None;
        let mut pid = self.ppid(leader);
        for _ in 0..MAX_HOPS {
            if pid == 0 || !self.is_host(pid) {
                return None;
            }
            cwd = cwd.or_else(|| self.cwd(pid));
            if pid == shell {
                return Some(HiddenLeader { cwd });
            }
            let next = self.ppid(pid);
            if next == pid {
                return None;
            }
            pid = next;
        }
        None
    }

    /// Every one of `pid`'s mount, user and PID namespace links refused with
    /// `EACCES` (the kernel's answer when `ptrace` read access is denied;
    /// `PermissionDenied` is how `std` names it). In a fixture tree,
    /// `<pid>.ns` holding `eacces`.
    fn ns_refused(&self, pid: u32) -> bool {
        if let Some(dir) = &self.fixture {
            return read_text(&dir.join(format!("{pid}.ns")))
                .is_some_and(|s| s.split_whitespace().collect::<String>() == "eacces");
        }
        ["mnt", "user", "pid"].iter().all(|ns| {
            std::fs::read_link(format!("/proc/{pid}/ns/{ns}"))
                .is_err_and(|e| e.kind() == std::io::ErrorKind::PermissionDenied)
        })
    }

    /// How many PID namespaces `pid` is in (the fields of `NSpid:`). A
    /// fixture tree gives it as `<pid>.nspid`, default 1.
    fn nspid_depth(&self, pid: u32) -> Option<usize> {
        if let Some(dir) = &self.fixture {
            return match read_text(&dir.join(format!("{pid}.nspid"))) {
                Some(s) => s.trim().parse().ok(),
                None => Some(1),
            };
        }
        let status = read_text(&PathBuf::from(format!("/proc/{pid}/status")))?;
        let depth = status_field(&status, "NSpid:")?.split_whitespace().count();
        (depth > 0).then_some(depth)
    }

    /// `pid`'s real uid (the first field of `Uid:`). A fixture tree gives it
    /// as `<pid>.uid`, default ours.
    fn real_uid(&self, pid: u32) -> Option<u32> {
        if let Some(dir) = &self.fixture {
            return match read_text(&dir.join(format!("{pid}.uid"))) {
                Some(s) => s.trim().parse().ok(),
                None => Some(relaunch_sys::current_uid()),
            };
        }
        let status = read_text(&PathBuf::from(format!("/proc/{pid}/status")))?;
        status_field(&status, "Uid:")?
            .split_whitespace()
            .next()?
            .parse()
            .ok()
    }

    /// The direct children of `pid`.
    fn children(&self, pid: u32) -> Vec<u32> {
        match &self.fixture {
            Some(dir) => {
                let mut out: Vec<u32> = std::fs::read_dir(dir)
                    .into_iter()
                    .flatten()
                    .flatten()
                    .filter_map(|e| {
                        let name = e.file_name().into_string().ok()?;
                        let child = parse_pid(name.strip_suffix(".ppid")?)?;
                        (self.ppid(child) == pid).then_some(child)
                    })
                    .collect();
                out.sort_unstable();
                out
            }
            None => std::fs::read_dir(format!("/proc/{pid}/task"))
                .into_iter()
                .flatten()
                .flatten()
                .filter_map(|t| read_text(&t.path().join("children")))
                .flat_map(|text| {
                    text.split_whitespace()
                        .filter_map(parse_pid)
                        .collect::<Vec<_>>()
                })
                .collect(),
        }
    }

    /// The foreground process group of `pid`'s controlling tty; `None`
    /// without one. A fixture tree gives it as `<pid>.tpgid`.
    fn tpgid(&self, pid: u32) -> Option<u32> {
        let text = if let Some(dir) = &self.fixture {
            read_text(&dir.join(format!("{pid}.tpgid")))?
        } else {
            tpgid_field(&read_text(&PathBuf::from(format!("/proc/{pid}/stat")))?)?.to_owned()
        };
        let t: i64 = text.trim().parse().ok()?;
        u32::try_from(t).ok().filter(|&t| t > 0)
    }

    /// The working directory of a host process, when it can be read and is
    /// an absolute path. A fixture tree gives it as `<pid>.cwd`.
    #[must_use]
    pub fn cwd(&self, pid: u32) -> Option<String> {
        if pid == 0 || !self.is_host(pid) {
            return None;
        }
        let dir = match &self.fixture {
            Some(dir) => read_text(&dir.join(format!("{pid}.cwd")))?
                .trim_end_matches('\n')
                .to_owned(),
            None => std::fs::read_link(format!("/proc/{pid}/cwd"))
                .ok()?
                .to_str()?
                .to_owned(),
        };
        dir.starts_with('/').then_some(dir)
    }
}

/// A small text file, read bounded ([`read_regular_capped`]); `None` if
/// unreadable.
/// What a terminal is running in its foreground ([`Proc::foreground`]).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Foreground {
    /// The terminal's own child (its shell) is the foreground: nothing runs
    /// in it.
    Shell {
        /// The shell.
        pid: u32,
    },
    /// Another process group leads: the command started in the terminal.
    Command {
        /// Its leader.
        pid: u32,
        /// The leader's argv.
        argv: Vec<String>,
        /// Where it runs: the leader's directory, or for a leader the kernel
        /// hides, its nearest host ancestor's.
        cwd: Option<String>,
    },
}

/// A foreground leader [`Proc::hidden_leader`] accepted.
struct HiddenLeader {
    /// Its nearest host ancestor's directory.
    cwd: Option<String>,
}

/// The tpgid field of a `/proc/<pid>/stat` line. `comm` may hold spaces and
/// parentheses, so the fields are counted from its last `)`: state, ppid,
/// pgrp, session, `tty_nr`, then tpgid.
fn tpgid_field(stat: &str) -> Option<&str> {
    stat.rsplit_once(')')?.1.split_whitespace().nth(5)
}

fn read_text(path: &Path) -> Option<String> {
    read_regular_capped(path, MAX_FILE_BYTES)
        .ok()
        .map(|b| String::from_utf8_lossy(&b).into_owned())
}

/// `PPid:` from a status file, or 0.
fn status_ppid(path: &Path) -> u32 {
    read_text(path)
        .and_then(|s| status_field(&s, "PPid:")?.trim().parse().ok())
        .unwrap_or(0)
}

/// The value of a status file's `key` line (`key` includes its colon).
fn status_field<'a>(status: &'a str, key: &str) -> Option<&'a str> {
    status.lines().find_map(|l| l.strip_prefix(key))
}

/// A pid from text: decimal with no sign or leading zero (`^[1-9][0-9]*$`).
#[must_use]
pub fn parse_pid(s: &str) -> Option<u32> {
    if s.starts_with('0') || !s.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    s.parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn this_process_is_on_the_host_and_has_an_argv() {
        let p = Proc::live();
        assert!(p.is_host(std::process::id()));
        assert!(p.argv(std::process::id()).is_some());
    }

    #[test]
    fn only_refused_namespace_links_are_hidden_and_the_status_reads_are_ours() {
        let p = Proc::live();
        let me = std::process::id();
        assert!(!p.ns_refused(me), "readable links are not refused");
        assert!(
            !p.ns_refused(4_000_000_000),
            "a gone process is not refused"
        );
        assert_eq!(p.real_uid(me), Some(relaunch_sys::current_uid()));
        let depth = p.nspid_depth(me).unwrap();
        assert!(depth >= 1);
        assert_eq!(p.nspid_depth(4_000_000_000), None);
        let status = "Name:\tx\nUid:\t1000\t0\t0\t0\nNSpid:\t70\t3\t1\n";
        assert_eq!(status_field(status, "NSpid:"), Some("\t70\t3\t1"));
        assert_eq!(status_field(status, "Gid:"), None);
    }

    #[test]
    fn an_absent_or_zero_pid_is_not_host() {
        let p = Proc::live();
        assert!(!p.is_host(0));
        assert!(!p.is_host(4_000_000_000));
    }

    #[test]
    fn tpgid_is_read_past_a_comm_holding_spaces_and_parentheses() {
        let stat = "17792 (bash) x) (y) S 17785 17792 17792 34825 17797 4194560 1 2";
        assert_eq!(tpgid_field(stat), Some("17797"));
        assert_eq!(tpgid_field("1 (init) S 0 1 1 0 -1 4194560"), Some("-1"));
        assert_eq!(tpgid_field("no parenthesis"), None);
    }

    #[test]
    fn parse_pid_is_strict() {
        assert_eq!(parse_pid("123"), Some(123));
        for bad in ["", "0", "012", "-1", "+5", "1a", " 1", "99999999999"] {
            assert_eq!(parse_pid(bad), None, "{bad:?}");
        }
    }

    /// INV-1 grep guard: no module but this one touches process state, so a
    /// second (unguarded) walker cannot appear.
    #[test]
    fn only_host_rs_touches_process_state() {
        let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut offenders = Vec::new();
        for entry in std::fs::read_dir(&src).unwrap() {
            let path = entry.unwrap().path();
            if path.file_name().and_then(|n| n.to_str()) == Some("host.rs") {
                continue;
            }
            let text = std::fs::read_to_string(&path).unwrap();
            for (n, line) in text.lines().enumerate() {
                let code = line.trim_start();
                if code.starts_with("//") {
                    continue;
                }
                if ["\"/proc", "read_link(", "/cmdline", "/status", "PPid"]
                    .iter()
                    .any(|needle| code.contains(needle))
                {
                    offenders.push(format!("{}:{}: {line}", path.display(), n + 1));
                }
            }
        }
        assert!(
            offenders.is_empty(),
            "process state read outside host.rs:\n{}",
            offenders.join("\n")
        );
    }
}
