//! `relaunch-anchor boot`, as a function of its host (spec §8): honour the
//! flags, launch every enabled entry of the verified document, and move each
//! one's window to its workspace as it appears (launch-and-move).
//!
//! Entries that could race each other — one app's instances, or apps sharing
//! one process, like a browser and its web apps — form a group and launch one
//! at a time, with the signed delays between them. Groups launch side by side:
//! round `n` starts the `n`-th entry of every group. A window is recognised by
//! being new (its address was not there before) and of the entry's
//! `windowClass`; one appearing later is caught by a bounded sweep at the end.
//! The binary supplies the real host (spawning, sleeping, `/usr/bin/hyprctl`);
//! tests supply their own.

use std::collections::BTreeSet;
use std::path::Path;
use std::time::Duration;

use relaunch_core::{Entry, SignedConfig};
use relaunch_shell::argv::{basename, is_relaunch_exec};
use relaunch_sys::paths;

use crate::report::{Launch, Placement, Record, Window};

/// The reason [`crate::load_verified`] gives for a document of another signed
/// schema: it must be saved, and so re-authorized, again.
pub const SCHEMA_UPGRADE_NEEDED: &str = "schema-upgrade-needed";

/// The shortest wait between two launches of one group, whatever was signed:
/// time enough for the first one's window to be told apart from the second's.
pub const MIN_POST_LAUNCH_MS: u32 = 250;

/// What boot needs from the system.
pub trait Host {
    /// The compositor's windows, or `None` when they cannot be read.
    fn windows(&mut self) -> Option<Vec<Window>>;
    /// Wait.
    fn sleep(&mut self, d: Duration);
    /// Verify and launch one entry, reporting what happened.
    fn launch(&mut self, entry: &Entry) -> Launch;
    /// Place the window at `address`: move it to `workspace` and float or
    /// tile it, each only when given.
    ///
    /// # Errors
    ///
    /// Why the window could not be placed.
    fn place(
        &mut self,
        address: &str,
        workspace: Option<u32>,
        float: Option<bool>,
    ) -> Result<(), String>;
}

/// How long boot watches for windows.
#[derive(Clone, Copy, Debug)]
pub struct Timing {
    /// How often windows are read while waiting for one.
    pub poll: Duration,
    /// How long a round waits for its launches' windows.
    pub correlate: Duration,
    /// How long the final sweep waits for windows still missing.
    pub sweep: Duration,
    /// How often the final sweep reads windows.
    pub sweep_interval: Duration,
}

impl Default for Timing {
    fn default() -> Self {
        Self {
            poll: Duration::from_millis(50),
            correlate: Duration::from_secs(5),
            sweep: Duration::from_secs(20),
            sweep_interval: Duration::from_secs(2),
        }
    }
}

/// The first word of a canonical `exec` (`\ ` and `\\` are its escapes).
fn program(exec: &str) -> String {
    let mut out = String::new();
    let mut chars = exec.chars();
    while let Some(c) = chars.next() {
        match c {
            '\\' => out.extend(chars.next()),
            ' ' => break,
            c => out.push(c),
        }
    }
    out
}

/// What makes two entries one group: the same program, or window classes that
/// start with the same word (`brave-browser` and `brave-…-Default`).
fn group_keys(e: &Entry) -> [String; 2] {
    let class = e.window_class.to_ascii_lowercase();
    let word = class.split(['-', '.']).next().unwrap_or_default();
    [
        format!("p:{}", basename(&program(&e.exec))),
        format!("c:{word}"),
    ]
}

/// Partition `entries` (already in launch order) into groups: entries sharing
/// either key are one group, transitively. Each group keeps launch order.
#[must_use]
pub fn groups<'a>(entries: &[&'a Entry]) -> Vec<Vec<&'a Entry>> {
    let keys: Vec<[String; 2]> = entries.iter().map(|e| group_keys(e)).collect();
    // group_of[i] is the group entry i is in, named by its first member.
    let mut group_of: Vec<usize> = (0..entries.len()).collect();
    for (i, a) in keys.iter().enumerate() {
        for (j, b) in keys.iter().enumerate().take(i) {
            if !a.iter().any(|k| b.contains(k)) {
                continue;
            }
            let (Some(&from), Some(&to)) = (group_of.get(i), group_of.get(j)) else {
                continue;
            };
            for g in &mut group_of {
                if *g == from {
                    *g = to;
                }
            }
        }
    }
    let mut out: Vec<(usize, Vec<&Entry>)> = Vec::new();
    for (e, g) in entries.iter().zip(&group_of) {
        match out.iter_mut().find(|(id, _)| id == g) {
            Some((_, members)) => members.push(e),
            None => out.push((*g, vec![e])),
        }
    }
    out.into_iter().map(|(_, members)| members).collect()
}

/// Run one boot and return its record (`started_at` is left to the caller).
/// `verified` is the signed document as verification left it: an error means
/// nothing is launched. A `disabled` flag suppresses the boot; a `skip-once`
/// flag suppresses this one and is consumed.
pub fn run(
    config_dir: &Path,
    verified: Result<SignedConfig, String>,
    host: &mut impl Host,
    timing: Timing,
) -> Record {
    let record = |outcome: &'static str, reason: &str| Record {
        started_at: String::new(),
        outcome,
        reason: reason.to_owned(),
        pre_launch_ms: 0,
        post_launch_ms: 0,
        launches: Vec::new(),
    };
    let cfg = match verified {
        Ok(c) => c,
        Err(reason) if reason == SCHEMA_UPGRADE_NEEDED => {
            return record(
                SCHEMA_UPGRADE_NEEDED,
                "this list was saved by an older Relaunch; save it again to launch it",
            )
        }
        Err(reason) => return record("unverified", &reason),
    };
    if config_dir.join(paths::DISABLED_FLAG).exists() {
        return record("disabled", "disabled flag present");
    }
    let skip = config_dir.join(paths::SKIP_ONCE_FLAG);
    if skip.exists() {
        let _ = std::fs::remove_file(&skip);
        return record(
            "skipped",
            "skip-once consumed; the next boot launches again",
        );
    }
    let pre = Duration::from_millis(u64::from(cfg.pre_launch_ms));
    let post = Duration::from_millis(u64::from(cfg.post_launch_ms.max(MIN_POST_LAUNCH_MS)));

    // Relaunch itself is never launched: that would recurse.
    let mut order: Vec<&Entry> = cfg
        .entries
        .iter()
        .filter(|e| e.enabled && !is_relaunch_exec(&e.exec))
        .collect();
    order.sort_by(|a, b| (a.workspace, &a.class).cmp(&(b.workspace, &b.class)));
    let groups = groups(&order);
    let rounds = groups.iter().map(Vec::len).max().unwrap_or(0);

    let mut known: BTreeSet<String> = host
        .windows()
        .unwrap_or_default()
        .into_iter()
        .map(|w| w.address)
        .collect();
    let mut launches: Vec<Launch> = Vec::new();
    for round in 0..rounds {
        if round > 0 {
            host.sleep(post);
        }
        if !pre.is_zero() {
            host.sleep(pre);
        }
        let mut batch = Vec::new();
        for entry in groups.iter().filter_map(|g| g.get(round)) {
            let mut l = host.launch(entry);
            if l.status == "started" {
                l.placement = Some(Placement {
                    address: None,
                    actual_workspace: None,
                    outcome: "no-show",
                    seen_class: None,
                });
                batch.push(launches.len());
            }
            launches.push(l);
        }
        watch(
            &mut launches,
            &batch,
            &mut known,
            timing.correlate,
            timing.poll,
            host,
        );
    }
    let missing: Vec<usize> = (0..launches.len())
        .filter(|&i| unclaimed(&launches, i))
        .collect();
    watch(
        &mut launches,
        &missing,
        &mut known,
        timing.sweep,
        timing.sweep_interval,
        host,
    );
    claim_by_pid(&mut launches, &mut known, host);
    settle(&mut launches, host);

    let landed = !launches.is_empty() && launches.iter().all(Launch::placed);
    Record {
        pre_launch_ms: cfg.pre_launch_ms,
        post_launch_ms: cfg.post_launch_ms.max(MIN_POST_LAUNCH_MS),
        launches,
        ..record(if landed { "landed" } else { "launched" }, "")
    }
}

/// Whether launch `i` started and no window has been recognised as its yet.
fn unclaimed(launches: &[Launch], i: usize) -> bool {
    launches
        .get(i)
        .and_then(|l| l.placement.as_ref())
        .is_some_and(|p| p.address.is_none())
}

/// Read windows every `interval` for up to `limit`, giving each launch in
/// `waiting` (in order) the first new window of its class, and placing that
/// window: moved to the launch's workspace and floated or tiled as captured,
/// each only if it is not already so. A recognised window joins `known`, so
/// it is never given to a second launch.
fn watch(
    launches: &mut [Launch],
    waiting: &[usize],
    known: &mut BTreeSet<String>,
    limit: Duration,
    interval: Duration,
    host: &mut impl Host,
) {
    let mut elapsed = Duration::ZERO;
    loop {
        if !waiting.iter().any(|&i| unclaimed(launches, i)) {
            return;
        }
        if let Some(windows) = host.windows() {
            for &i in waiting {
                if !unclaimed(launches, i) {
                    continue;
                }
                let Some(l) = launches.get_mut(i) else {
                    continue;
                };
                let Some(w) = windows
                    .iter()
                    .find(|w| !known.contains(&w.address) && w.is_of(&l.window_class))
                else {
                    continue;
                };
                known.insert(w.address.clone());
                claim(l, w, None, host);
            }
        }
        if elapsed >= limit || interval.is_zero() {
            return;
        }
        host.sleep(interval);
        elapsed += interval;
    }
}

/// Move `w`, now `l`'s window, to `l`'s workspace and float or tile it as
/// captured, each only if it is not already so.
fn claim(l: &mut Launch, w: &Window, seen_class: Option<String>, host: &mut impl Host) {
    let on_target = w.workspace == Some(i64::from(l.workspace));
    let float = l.float.filter(|&f| w.floating != Some(f));
    let moved = (on_target && float.is_none())
        || host
            .place(&w.address, (!on_target).then_some(l.workspace), float)
            .is_ok();
    l.placement = Some(Placement {
        address: Some(w.address.clone()),
        actual_workspace: if moved {
            Some(i64::from(l.workspace))
        } else {
            w.workspace
        },
        outcome: if moved { "placed" } else { "misplaced" },
        seen_class,
    });
}

/// Once the sweep is over, give each launch still without a window the new
/// window whose pid is exactly the one it spawned: an app whose window class
/// changed since it was pinned (an update renamed it). One read; a window
/// with no pid never matches.
fn claim_by_pid(launches: &mut [Launch], known: &mut BTreeSet<String>, host: &mut impl Host) {
    if !(0..launches.len()).any(|i| unclaimed(launches, i)) {
        return;
    }
    let Some(windows) = host.windows() else {
        return;
    };
    for l in launches.iter_mut() {
        let waiting = l.placement.as_ref().is_some_and(|p| p.address.is_none());
        let Some(pid) = l.pid.filter(|&p| p > 0 && waiting) else {
            continue;
        };
        let Some(w) = windows
            .iter()
            .find(|w| w.pid == Some(pid) && !known.contains(&w.address))
        else {
            continue;
        };
        known.insert(w.address.clone());
        let seen = if w.initial_class.is_empty() {
            &w.class
        } else {
            &w.initial_class
        };
        claim(l, w, Some(seen.clone()), host);
    }
}

/// Re-read where each recognised window is now, so the report says where it
/// ended up, not where it was sent. Without a window list the moves' own
/// results stand.
fn settle(launches: &mut [Launch], host: &mut impl Host) {
    let Some(windows) = host.windows() else {
        return;
    };
    for l in launches.iter_mut() {
        let pinned = i64::from(l.workspace);
        let Some(p) = l.placement.as_mut() else {
            continue;
        };
        let Some(addr) = p.address.as_deref() else {
            continue;
        };
        let now = windows
            .iter()
            .find(|w| w.address == addr)
            .map(|w| w.workspace);
        // A window gone again by the end did not stay; report it as not shown.
        p.actual_workspace = now.flatten();
        p.outcome = match now {
            None => "no-show",
            Some(ws) if ws == Some(pinned) => "placed",
            Some(_) => "misplaced",
        };
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use relaunch_core::{ExecSource, Launcher, Provenance, SIGNED_SCHEMA_VERSION};

    /// A compositor: launching an entry maps a window of its class on
    /// `spawn_ws` (or never, for a class in `never`) after `delay` reads.
    #[derive(Default)]
    struct Fake {
        events: Vec<String>,
        windows: Vec<Window>,
        queued: Vec<(usize, Window)>,
        next: u64,
        never: Vec<String>,
        delay: usize,
        spawn_ws: i64,
        refuse_moves: bool,
        /// Report a placement as done without doing it (an app that puts
        /// its window back).
        ignore_moves: bool,
        /// A pinned class whose app now maps its window under another class.
        renamed: Vec<(String, String)>,
        /// Classes whose launch first maps a helper window of another class
        /// (`<class>-helper`) from the same process.
        helper_first: Vec<String>,
        /// Windows of a renamed app come from a process other than the one
        /// launched.
        stranger: bool,
        /// One pid for every launch (a single-instance app handing off).
        shared_pid: Option<u32>,
        /// Classes that launch but map no window of their own.
        silent: Vec<String>,
        next_pid: u32,
    }

    impl Fake {
        fn new() -> Self {
            Self {
                spawn_ws: 1,
                next: 0x100,
                next_pid: 500,
                ..Self::default()
            }
        }
        fn existing(mut self, class: &str, ws: i64) -> Self {
            self.windows
                .push(win(&format!("0x{:x}", self.next), class, ws));
            self.next += 1;
            self
        }
    }

    fn win(addr: &str, class: &str, ws: i64) -> Window {
        Window {
            address: addr.into(),
            class: class.into(),
            initial_class: class.into(),
            workspace: Some(ws),
            floating: Some(false),
            pid: None,
        }
    }

    impl Host for Fake {
        fn windows(&mut self) -> Option<Vec<Window>> {
            let (ready, later): (Vec<_>, Vec<_>) =
                self.queued.drain(..).partition(|(n, _)| *n == 0);
            self.queued = later.into_iter().map(|(n, w)| (n - 1, w)).collect();
            self.windows.extend(ready.into_iter().map(|(_, w)| w));
            Some(self.windows.clone())
        }
        fn sleep(&mut self, d: Duration) {
            self.events.push(format!("sleep {}", d.as_millis()));
        }
        fn launch(&mut self, e: &Entry) -> Launch {
            self.events.push(format!("launch {}", e.window_class));
            self.next_pid += 1;
            let pid = self.shared_pid.unwrap_or(self.next_pid);
            if self.helper_first.contains(&e.window_class) {
                let mut w = win(
                    &format!("0x{:x}", self.next),
                    &format!("{}-helper", e.window_class),
                    self.spawn_ws,
                );
                w.pid = Some(pid);
                self.next += 1;
                self.queued.push((0, w));
            }
            if !self.never.contains(&e.window_class) && !self.silent.contains(&e.window_class) {
                let renamed = self
                    .renamed
                    .iter()
                    .find(|(from, _)| *from == e.window_class);
                let class = renamed.map_or(e.window_class.as_str(), |(_, to)| to.as_str());
                let mut w = win(&format!("0x{:x}", self.next), class, self.spawn_ws);
                w.pid = Some(if renamed.is_some() && self.stranger {
                    pid + 1000
                } else {
                    pid
                });
                self.next += 1;
                self.queued.push((self.delay, w));
            }
            Launch {
                class: e.class.clone(),
                window_class: e.window_class.clone(),
                label: e.label.clone(),
                workspace: e.workspace,
                float: e.float,
                exec: e.exec.clone(),
                status: "started",
                pid: Some(pid),
                error: None,
                placement: None,
            }
        }
        fn place(
            &mut self,
            address: &str,
            workspace: Option<u32>,
            float: Option<bool>,
        ) -> Result<(), String> {
            if let Some(ws) = workspace {
                self.events.push(format!("move {address} {ws}"));
            }
            if let Some(f) = float {
                self.events.push(format!("float {address} {f}"));
            }
            if self.refuse_moves {
                return Err("no".into());
            }
            if self.ignore_moves {
                return Ok(());
            }
            for w in &mut self.windows {
                if w.address == address {
                    if let Some(ws) = workspace {
                        w.workspace = Some(i64::from(ws));
                    }
                    if float.is_some() {
                        w.floating = float;
                    }
                }
            }
            Ok(())
        }
    }

    fn entry(n: u8, window_class: &str, exec: &str, ws: u32) -> Entry {
        let class = format!("relauncher-{n:016x}");
        Entry {
            launcher: Launcher {
                path: format!("/h/omarchy-relaunch/launchers/{class}.desktop"),
                sha256: "ab".repeat(32),
            },
            class,
            window_class: window_class.into(),
            workspace: ws,
            float: None,
            label: String::new(),
            exec: exec.into(),
            exec_source: ExecSource::Cmdline,
            provenance: Provenance::Host,
            cwd: None,
            enabled: true,
        }
    }

    fn doc(pre: u32, post: u32, entries: Vec<Entry>) -> SignedConfig {
        SignedConfig {
            v: SIGNED_SCHEMA_VERSION,
            uid: 1,
            user: "u".into(),
            gen: 1,
            pre_launch_ms: pre,
            post_launch_ms: post,
            entries,
        }
    }

    fn dir(tag: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("relaunch-boot-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    const FAST: Timing = Timing {
        poll: Duration::from_millis(50),
        correlate: Duration::from_millis(200),
        sweep: Duration::from_millis(400),
        sweep_interval: Duration::from_millis(100),
    };

    fn go(tag: &str, cfg: SignedConfig, host: &mut Fake) -> Record {
        let d = dir(tag);
        let r = run(&d, Ok(cfg), host, FAST);
        let _ = std::fs::remove_dir_all(&d);
        r
    }

    fn outcomes(r: &Record) -> Vec<(&str, Option<i64>)> {
        r.launches
            .iter()
            .map(|l| {
                let p = l.placement.as_ref();
                (
                    p.map_or("-", |p| p.outcome),
                    p.and_then(|p| p.actual_workspace),
                )
            })
            .collect()
    }

    #[test]
    fn each_new_window_is_moved_to_its_workspace() {
        let mut host = Fake::new();
        let r = go(
            "move",
            doc(
                0,
                0,
                vec![
                    entry(1, "foot", "foot", 3),
                    entry(2, "Signal", "signal-desktop", 5),
                ],
            ),
            &mut host,
        );
        assert_eq!(outcomes(&r), [("placed", Some(3)), ("placed", Some(5))]);
        assert_eq!(r.outcome, "landed");
        assert!(
            host.events.contains(&"move 0x100 3".to_owned()),
            "{:?}",
            host.events
        );
        assert!(
            host.events.contains(&"move 0x101 5".to_owned()),
            "{:?}",
            host.events
        );
    }

    #[test]
    fn independent_apps_launch_together_and_one_apps_instances_one_at_a_time() {
        let mut host = Fake::new();
        go(
            "rounds",
            doc(
                0,
                400,
                vec![
                    entry(1, "foot", "foot", 1),
                    entry(2, "foot", "foot", 2),
                    entry(3, "Signal", "signal-desktop", 3),
                ],
            ),
            &mut host,
        );
        let launches: Vec<&str> = host
            .events
            .iter()
            .filter(|e| e.starts_with("launch") || e == &"sleep 400")
            .map(String::as_str)
            .collect();
        assert_eq!(
            launches,
            ["launch foot", "launch Signal", "sleep 400", "launch foot"]
        );
    }

    #[test]
    fn a_browser_and_its_web_apps_are_one_group() {
        let brave = entry(1, "brave-browser", "brave", 1);
        let maps = entry(
            2,
            "brave-maps.google.com__-Default",
            "omarchy-launch-webapp https://maps.google.com",
            8,
        );
        let pass = entry(
            3,
            "brave-pass.proton.me__-Default",
            "omarchy-launch-webapp https://pass.proton.me",
            4,
        );
        let other = entry(4, "Signal", "signal-desktop", 2);
        let order = [&brave, &other, &pass, &maps];
        let g = groups(&order);
        let classes: Vec<Vec<&str>> = g
            .iter()
            .map(|g| g.iter().map(|e| e.window_class.as_str()).collect())
            .collect();
        assert_eq!(
            classes,
            [
                vec![
                    "brave-browser",
                    "brave-pass.proton.me__-Default",
                    "brave-maps.google.com__-Default"
                ],
                vec!["Signal"]
            ]
        );
    }

    #[test]
    fn a_post_delay_below_the_floor_is_raised_and_pre_waits_before_every_round() {
        let mut host = Fake::new();
        let r = go(
            "floor",
            doc(
                30,
                0,
                vec![entry(1, "foot", "foot", 1), entry(2, "foot", "foot", 2)],
            ),
            &mut host,
        );
        let waits: Vec<&str> = host
            .events
            .iter()
            .filter(|e| e == &"sleep 30" || e == &"sleep 250")
            .map(String::as_str)
            .collect();
        assert_eq!(waits, ["sleep 30", "sleep 250", "sleep 30"]);
        assert_eq!(
            (r.pre_launch_ms, r.post_launch_ms),
            (30, MIN_POST_LAUNCH_MS)
        );
    }

    #[test]
    fn only_new_windows_of_the_entrys_class_are_taken() {
        // An old foot and a new unrelated window are both ignored.
        let mut host = Fake::new().existing("foot", 9);
        host.queued.push((1, win("0xbeef", "intruder", 1)));
        let r = go(
            "new-only",
            doc(0, 0, vec![entry(1, "foot", "foot", 2)]),
            &mut host,
        );
        assert_eq!(
            r.launches[0].placement.as_ref().unwrap().address.as_deref(),
            Some("0x101")
        );
        assert!(!host
            .events
            .iter()
            .any(|e| e.contains("0x100") || e.contains("0xbeef")));
    }

    #[test]
    fn a_slow_window_is_caught_by_the_sweep_and_a_missing_one_is_a_no_show() {
        let mut host = Fake::new();
        host.delay = 6; // past the round's wait, inside the sweep
        host.never = vec!["Ghost".into()];
        let r = go(
            "slow",
            doc(
                0,
                0,
                vec![
                    entry(1, "Calc", "libreoffice --calc", 4),
                    entry(2, "Ghost", "ghost", 5),
                ],
            ),
            &mut host,
        );
        assert_eq!(outcomes(&r), [("placed", Some(4)), ("no-show", None)]);
        assert_eq!(r.outcome, "launched", "landed needs every window placed");
    }

    #[test]
    fn a_renamed_apps_window_is_claimed_by_its_pid_once_the_sweep_is_over() {
        let mut host = Fake::new();
        host.renamed = vec![("Proton Pass".into(), "proton-pass".into())];
        let r = go(
            "renamed",
            doc(0, 0, vec![entry(1, "Proton Pass", "proton-pass", 4)]),
            &mut host,
        );
        assert_eq!(outcomes(&r), [("placed", Some(4))]);
        let p = r.launches[0].placement.as_ref().unwrap();
        assert_eq!(p.seen_class.as_deref(), Some("proton-pass"));
        assert!(
            r.log()
                .contains("placed  CLASS NOW proton-pass  proton-pass"),
            "{}",
            r.log()
        );
        assert!(r.json().contains("\"seenClass\": \"proton-pass\""));
        let moved = host
            .events
            .iter()
            .position(|e| e.starts_with("move "))
            .unwrap();
        let last_wait = host
            .events
            .iter()
            .rposition(|e| e.starts_with("sleep "))
            .unwrap();
        assert!(moved > last_wait, "only after the sweep: {:?}", host.events);
    }

    #[test]
    fn a_pid_claim_needs_the_launched_process_itself() {
        let mut host = Fake::new();
        host.renamed = vec![("Proton Pass".into(), "proton-pass".into())];
        host.stranger = true;
        let r = go(
            "stranger",
            doc(0, 0, vec![entry(1, "Proton Pass", "proton-pass", 4)]),
            &mut host,
        );
        assert_eq!(outcomes(&r), [("no-show", None)]);
        assert!(!host.events.iter().any(|e| e.starts_with("move ")));

        // A window that was there before boot is never taken, whatever its pid.
        let mut host = Fake::new().existing("proton-pass", 1);
        host.windows[0].pid = Some(501);
        host.never = vec!["Proton Pass".into()];
        let r = go(
            "before",
            doc(0, 0, vec![entry(1, "Proton Pass", "proton-pass", 4)]),
            &mut host,
        );
        assert_eq!(outcomes(&r), [("no-show", None)]);
    }

    #[test]
    fn a_class_window_mapped_late_wins_over_an_earlier_helper_of_its_process() {
        let mut host = Fake::new();
        host.helper_first = vec!["Calc".into()];
        host.delay = 6; // the real window: past the round, inside the sweep
        host.never = vec!["Ghost".into()]; // so the pid pass runs
        let r = go(
            "helper",
            doc(
                0,
                0,
                vec![
                    entry(1, "Calc", "libreoffice --calc", 4),
                    entry(2, "Ghost", "ghost", 5),
                ],
            ),
            &mut host,
        );
        assert_eq!(outcomes(&r), [("placed", Some(4)), ("no-show", None)]);
        assert_eq!(r.launches[0].placement.as_ref().unwrap().seen_class, None);
        assert!(!r.log().contains("CLASS NOW"));
        assert!(!r.json().contains("seenClass"));
        assert_eq!(
            host.events
                .iter()
                .filter(|e| e.starts_with("move "))
                .count(),
            1,
            "the helper is not moved: {:?}",
            host.events
        );
    }

    #[test]
    fn one_window_is_never_given_to_two_launches_by_pid() {
        let mut host = Fake::new();
        host.renamed = vec![
            ("Proton Pass".into(), "proton-pass".into()),
            ("Proton Mail".into(), "proton-mail".into()),
        ];
        let r = go(
            "two",
            doc(
                0,
                0,
                vec![
                    entry(1, "Proton Pass", "proton-pass", 4),
                    entry(2, "Proton Mail", "proton-mail", 3),
                ],
            ),
            &mut host,
        );
        let addrs: Vec<_> = r
            .launches
            .iter()
            .map(|l| l.placement.as_ref().unwrap().address.clone().unwrap())
            .collect();
        assert_ne!(addrs[0], addrs[1]);
        let seen: Vec<_> = r
            .launches
            .iter()
            .map(|l| {
                (
                    l.window_class.as_str(),
                    l.placement.as_ref().unwrap().seen_class.clone().unwrap(),
                )
            })
            .collect();
        assert_eq!(
            seen,
            [
                ("Proton Mail", "proton-mail".to_owned()),
                ("Proton Pass", "proton-pass".to_owned())
            ],
            "each launch its own process's window"
        );
    }

    #[test]
    fn one_window_is_claimed_by_pid_once_even_when_two_launches_share_its_pid() {
        let mut host = Fake::new();
        host.shared_pid = Some(777);
        host.renamed = vec![("Proton Pass".into(), "proton-pass".into())];
        host.silent = vec!["Proton Pass Two".into()];
        let r = go(
            "shared-pid",
            doc(
                0,
                0,
                vec![
                    entry(1, "Proton Pass", "proton-pass", 4),
                    entry(2, "Proton Pass Two", "proton-pass", 4),
                ],
            ),
            &mut host,
        );
        let claimed: Vec<_> = r
            .launches
            .iter()
            .filter_map(|l| l.placement.as_ref().and_then(|p| p.address.clone()))
            .collect();
        assert_eq!(claimed.len(), 1, "one window, one launch: {:?}", r.launches);
        assert_eq!(r.launches[0].pid, r.launches[1].pid);
    }

    #[test]
    fn a_window_already_on_its_workspace_is_not_moved_and_a_failed_move_is_misplaced() {
        let mut host = Fake::new();
        let r = go(
            "stay",
            doc(0, 0, vec![entry(1, "foot", "foot", 1)]),
            &mut host,
        );
        assert_eq!(outcomes(&r), [("placed", Some(1))]);
        assert!(!host.events.iter().any(|e| e.starts_with("move")));
        let mut host = Fake::new();
        host.refuse_moves = true;
        let r = go(
            "refused",
            doc(0, 0, vec![entry(1, "foot", "foot", 3)]),
            &mut host,
        );
        assert_eq!(outcomes(&r), [("misplaced", Some(1))]);
    }

    #[test]
    fn a_window_is_floated_or_tiled_as_captured_only_when_it_is_not_already() {
        let mut floats = entry(1, "Calc", "calc", 1);
        floats.float = Some(true);
        let mut tiles = entry(2, "foot", "foot", 3);
        tiles.float = Some(false);
        let unknown = entry(3, "Signal", "signal-desktop", 1);
        let mut host = Fake::new();
        let r = go("float", doc(0, 0, vec![floats, tiles, unknown]), &mut host);
        assert_eq!(r.outcome, "landed");
        let places: Vec<&String> = host
            .events
            .iter()
            .filter(|e| e.starts_with("move") || e.starts_with("float"))
            .collect();
        // Calc floats in place; foot (already tiled) is only moved; Signal's
        // float is unknown and it is already home, so it is left alone.
        assert_eq!(
            places,
            ["float 0x100 true", "move 0x102 3"],
            "{:?}",
            host.events
        );
    }

    #[test]
    fn each_instance_gets_its_own_window() {
        let mut host = Fake::new();
        let r = go(
            "instances",
            doc(
                0,
                0,
                vec![entry(1, "foot", "foot", 1), entry(2, "foot", "foot", 2)],
            ),
            &mut host,
        );
        let addrs: Vec<Option<&str>> = r
            .launches
            .iter()
            .map(|l| l.placement.as_ref().and_then(|p| p.address.as_deref()))
            .collect();
        assert_eq!(addrs, [Some("0x100"), Some("0x101")]);
        assert_eq!(outcomes(&r), [("placed", Some(1)), ("placed", Some(2))]);
    }

    #[test]
    fn the_report_says_where_a_window_ended_up_not_where_it_was_sent() {
        let mut host = Fake::new();
        host.ignore_moves = true;
        let r = go(
            "ended-up",
            doc(0, 0, vec![entry(1, "foot", "foot", 4)]),
            &mut host,
        );
        assert_eq!(outcomes(&r), [("misplaced", Some(1))]);
        assert_eq!(r.outcome, "launched");
    }

    #[test]
    fn only_enabled_entries_launch_and_relaunch_itself_never_does() {
        let mut off = entry(2, "Off", "off", 1);
        off.enabled = false;
        let mut host = Fake::new();
        go(
            "filter",
            doc(
                0,
                0,
                vec![
                    entry(1, "On", "on", 1),
                    off,
                    entry(3, "Self", "/home/u/.local/bin/relaunch boot", 1),
                ],
            ),
            &mut host,
        );
        let launched: Vec<&String> = host
            .events
            .iter()
            .filter(|e| e.starts_with("launch"))
            .collect();
        assert_eq!(launched, ["launch On"]);
    }

    #[test]
    fn the_flags_suppress_and_skip_once_is_consumed() {
        let d = dir("flags");
        let cfg = || Ok(doc(0, 0, vec![entry(1, "A", "a", 1)]));
        std::fs::write(d.join(paths::DISABLED_FLAG), "").unwrap();
        let mut host = Fake::new();
        assert_eq!(run(&d, cfg(), &mut host, FAST).outcome, "disabled");
        std::fs::remove_file(d.join(paths::DISABLED_FLAG)).unwrap();
        std::fs::write(d.join(paths::SKIP_ONCE_FLAG), "").unwrap();
        assert_eq!(run(&d, cfg(), &mut host, FAST).outcome, "skipped");
        assert!(
            !d.join(paths::SKIP_ONCE_FLAG).exists(),
            "skip-once is consumed"
        );
        assert!(host.events.is_empty());
        assert_eq!(
            run(&d, cfg(), &mut host, FAST).outcome,
            "landed",
            "the next boot launches"
        );
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn an_unverified_or_old_document_launches_nothing() {
        let d = dir("unverified");
        let mut host = Fake::new();
        let rec = run(&d, Err("signature mismatch".into()), &mut host, FAST);
        assert_eq!(
            (rec.outcome, rec.reason.as_str()),
            ("unverified", "signature mismatch")
        );
        let rec = run(&d, Err(SCHEMA_UPGRADE_NEEDED.into()), &mut host, FAST);
        assert_eq!(rec.outcome, SCHEMA_UPGRADE_NEEDED);
        assert!(host.events.is_empty());
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn the_program_is_read_through_the_canonical_escapes() {
        assert_eq!(program("/opt/My\\ App/run --x"), "/opt/My App/run");
        assert_eq!(program("a\\\\b c"), "a\\b");
        assert_eq!(program(""), "");
    }
}
