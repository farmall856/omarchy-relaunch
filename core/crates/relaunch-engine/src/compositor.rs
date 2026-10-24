//! The compositor's window list (`hyprctl clients -j`), typed (INV-9): every
//! field is decoded once by serde, so a class keeps its exact bytes from the
//! compositor to the proposal.

use serde::Deserialize;

use crate::env::Env;

/// One mapped window, as the compositor reports it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Window {
    /// The compositor address, or `pid:<pid>` when it has none.
    pub address: String,
    /// The owning process, as reported (absent for some clients).
    pub pid: Option<i64>,
    /// `initialClass`, or `class` when that is empty; may be empty.
    pub class: String,
    /// The workspace id; special workspaces are below 1.
    pub workspace: i64,
    /// Whether the window floats, when reported.
    pub floating: Option<bool>,
    /// Whether the window is an `XWayland` client, when reported.
    pub xwayland: Option<bool>,
    /// The monitor id, when reported.
    pub monitor: Option<i64>,
    /// The window title.
    pub title: String,
}

impl Window {
    /// The pid as a process to inspect: 0 (never host) when absent or not a
    /// valid pid.
    #[must_use]
    pub fn host_pid(&self) -> u32 {
        self.pid.and_then(|p| u32::try_from(p).ok()).unwrap_or(0)
    }

    /// A window Relaunch can resolve: it has a pid and a class.
    #[must_use]
    pub fn is_resolvable(&self) -> bool {
        self.pid.is_some() && !self.class.is_empty()
    }

    /// Whether its process also owns a window of another class, so the
    /// process's command line describes at most one of them.
    #[must_use]
    pub fn shares_process(&self, windows: &[Window]) -> bool {
        self.pid.is_some()
            && windows
                .iter()
                .any(|w| w.pid == self.pid && w.class != self.class)
    }
}

#[derive(Deserialize)]
struct RawWorkspace {
    #[serde(default)]
    id: Option<i64>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawClient {
    #[serde(default)]
    address: Option<String>,
    #[serde(default)]
    pid: Option<i64>,
    #[serde(default)]
    class: Option<String>,
    #[serde(default)]
    initial_class: Option<String>,
    #[serde(default)]
    workspace: Option<RawWorkspace>,
    #[serde(default)]
    floating: Option<bool>,
    #[serde(default)]
    xwayland: Option<bool>,
    #[serde(default)]
    monitor: Option<i64>,
    #[serde(default)]
    title: Option<String>,
}

/// Decode `hyprctl clients -j` output, every client kept in compositor order.
///
/// # Errors
///
/// A message if the output is not a JSON array of client objects.
pub fn parse_clients(bytes: &[u8]) -> Result<Vec<Window>, String> {
    let raw: Vec<RawClient> = serde_json::from_slice(bytes)
        .map_err(|e| format!("hyprctl clients: unexpected output: {e}"))?;
    Ok(raw
        .into_iter()
        .map(|c| Window {
            address: c.address.filter(|a| !a.is_empty()).unwrap_or_else(|| {
                format!(
                    "pid:{}",
                    c.pid.map_or_else(|| "null".to_owned(), |p| p.to_string())
                )
            }),
            pid: c.pid,
            class: c
                .initial_class
                .filter(|s| !s.is_empty())
                .or(c.class)
                .unwrap_or_default(),
            workspace: c.workspace.and_then(|w| w.id).unwrap_or(0),
            floating: c.floating,
            xwayland: c.xwayland,
            monitor: c.monitor,
            title: c.title.unwrap_or_default(),
        })
        .collect())
}

/// The compositor's current windows, read through the shared bounded reader.
///
/// # Errors
///
/// A message if `hyprctl` cannot run, fails, prints too much, or prints
/// something else.
pub fn clients(env: &Env) -> Result<Vec<Window>, String> {
    let out = relaunch_sys::cmd::output_capped(&env.hyprctl, &["clients", "-j"])
        .map_err(|e| format!("hyprctl clients: {e}"))?;
    parse_clients(&out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classes_keep_their_exact_bytes() {
        let w = parse_clients(
            br#"[{"address":"0x1","pid":5,"class":"c","initialClass":"Back\\slash","workspace":{"id":2},"floating":true,"xwayland":false},
                {"address":"0x2","pid":6,"class":"__proto__","initialClass":""}]"#,
        )
        .unwrap();
        assert_eq!(w[0].class, "Back\\slash");
        assert_eq!(
            (w[0].workspace, w[0].floating, w[0].xwayland),
            (2, Some(true), Some(false))
        );
        assert_eq!(w[1].class, "__proto__");
        assert_eq!(
            (w[1].workspace, w[1].floating, w[1].xwayland),
            (0, None, None)
        );
    }

    #[test]
    fn every_client_is_kept_and_odd_pids_are_never_host() {
        let w = parse_clients(
            br#"[{"pid":null,"class":"a"},{"class":"b"},{"pid":7,"class":""},{"pid":-1,"class":"d","monitor":1,"title":"t"}]"#,
        )
        .unwrap();
        assert_eq!(w.len(), 4);
        assert_eq!(
            w.iter().map(Window::is_resolvable).collect::<Vec<_>>(),
            [false, false, false, true]
        );
        assert_eq!(
            (w[0].address.as_str(), w[3].address.as_str()),
            ("pid:null", "pid:-1")
        );
        assert_eq!(
            (w[3].host_pid(), w[3].monitor, w[3].title.as_str()),
            (0, Some(1), "t")
        );
    }

    #[test]
    fn a_non_array_is_refused() {
        assert!(parse_clients(b"{}").is_err());
        assert!(parse_clients(b"not json").is_err());
    }
}
