import QtQuick
import Quickshell
import Quickshell.Io
import qs.Commons
import qs.Commons as Commons
import qs.Ui
import "TxnLifecycle.js" as TxnLifecycle
import "CoreInstall.js" as CoreInstall

// Relaunch panel. Shells out to `relaunch` for inventory, list edits, and
// boot-policy changes. Existing Hyprland startup apps are shown so they can
// be imported. Startup apps are listed and nothing more: Relaunch restores
// windows, it is not an autostart manager, and it never writes autostart.lua.
Panel {
  id: root
  moduleName: "io.github.laytonf.relaunch"
  manageIpc: false

  property var anchorItem: null
  // The theme's text colour, as the stock panels use it. Panel's barForeground
  // follows the bar, which turns dark over a light wallpaper while the popup
  // stays dark.
  readonly property color ink: bar ? bar.foreground : Commons.Color.foreground
  property var hostWidget: null

  property var entries: []
  property var rows: []
  property var boot: ({ disabled: false, skipOnce: false, active: true })
  property var lastBoot: null
  // The signed waits around each launch of a same-app group, and the
  // panel's edit of them (applied as a reviewed change).
  property int preLaunchMs: 0
  property int postLaunchMs: 500
  property int draftPreMs: 0
  property int draftPostMs: 500
  readonly property bool delaysEdited: draftPreMs !== preLaunchMs || draftPostMs !== postLaunchMs
  property string statusText: ""
  property string pendingStatus: ""
  // A brief acknowledgement of a promptless change, shown over the panel for
  // toastTimer.interval and then cleared. See run().
  property string toastText: ""
  property string pendingToast: ""
  property bool busy: false
  property bool confirmRemove: false
  // One Process serves every action, so requests that arrive while it is
  // running have to wait their turn rather than vanish. See run().
  property var requestQueue: []
  // Set when applyResult parsed a real error out of the engine's JSON, so the
  // fallback knows there is already a better message than "Command failed".
  property bool engineReportedError: false
  property bool outputUnparseable: false
  property string stderrText: ""
  // Omarchy's own plugins put it plainly: the exit and stream-finished
  // signals have NO guaranteed order. Deciding the status message in
  // whichever handler happens to run last is how the engine's real error got
  // replaced by a generic one. Both are recorded, and settle() decides once
  // both have arrived.
  property int lastExitCode: -1
  property bool sawOutput: false
  property bool sawStderr: false
  // The command could not be executed at all -- engine missing, or not
  // executable. Process emits no error signal to QML, so this is inferred.
  property bool startFailed: false
  // settle() can run more than once for one request -- a stream that arrives
  // late still gets to improve the message -- but the queue must only advance
  // once, and busy must only be released once.
  property bool settled: false
  // True from the moment settle() hands the queue to Qt.callLater until that
  // callback has started the request. In that window actionProc.running is
  // already false, so `running` alone would let a request through and the
  // pending callback would then write its command onto a live Process: the
  // dropped action again, one tick later.
  property bool startingNext: false
  // Failures get their own bordered box under the Save button; ordinary
  // results stay in the quiet line at the bottom.
  property bool statusIsError: false
  property string execEditClass: ""
  property var warnings: []

  // Plugin-manager clones land the script next to this file, not on PATH.
  readonly property string enginePath: {
    var s = Qt.resolvedUrl("relaunch").toString()
    if (s.indexOf("file://") === 0)
      return decodeURIComponent(s.slice(7))
    return "relaunch"
  }

  // The client that holds one bus connection across Prepare -> human pause ->
  // Authorize (spec §5.1/§12). Invoked by ABSOLUTE packaged path, never PATH;
  // the transaction id never crosses this boundary (it lives inside the running
  // `commit` process). A property so a dev checkout can point it elsewhere; the
  // installed plugin uses the packaged path.
  property string relaunchctlPath: "/usr/lib/omarchy-relaunch/relaunchctl"

  // The boot tools (the system package) and the two commands that install
  // them: FIXED literals, nothing from the user or the environment goes into
  // either. The plugin never elevates; pacman (through omarchy pkg aur add)
  // asks for the password in the terminal. See CoreInstall.js.
  readonly property string corePackage: "omarchy-relaunch"
  readonly property string relaunchdPath: "/usr/lib/omarchy-relaunch/relaunchd"
  readonly property string coreEnginePath: "/usr/lib/omarchy-relaunch/relaunch"
  // Prints "core" when the three components are installed, and "tools" when
  // Omarchy's package command, its AUR helper (yay) and a terminal launcher
  // are on PATH; then the package's version ("version 1.0.7-1", only when
  // pacman owns it) and relaunchd's stamp (inode and change time, new on every
  // reinstall). The paths arrive as arguments, never in the script.
  readonly property string coreProbeScript: 'if test -x "$1" && test -x "$2" && test -x "$3"; then echo core; fi; '
    + 'if command -v omarchy >/dev/null && command -v yay >/dev/null && command -v xdg-terminal-exec >/dev/null; then echo tools; fi; '
    + 'if v=$(pacman -Q omarchy-relaunch 2>/dev/null); then echo "version ${v#* }"; fi; '
    + 'if t=$(stat -c %i.%Z "$1" 2>/dev/null); then echo "stamp $t"; fi'
  readonly property var coreInstallCommand: ["xdg-terminal-exec", "--app-id=org.omarchy.terminal",
    "--title=Relaunch: install boot tools", "-e", "bash", "-c",
    'omarchy pkg aur add omarchy-relaunch; s=$?; echo; '
    + 'if [ "$s" -eq 0 ]; then echo "Done. Go back to the Relaunch panel."; else echo "The install did not finish (exit $s)."; fi; '
    + 'read -r -p "Press Enter to close this window. " _']
  readonly property var coreWireCommand: [root.coreEnginePath, "ensure-hooks", "--anchor"]
  // The lifecycle (boot-tools-lifecycle-buttons-design.md), each a FIXED
  // literal like the install. The check is headless: no password, no
  // terminal; it prints "installed V", "latest V" and one verdict line.
  // A failed fetch (fetchfailed) and an unreadable reply (badreply) are told
  // apart from a reply that has no such package (noaur); the transfer is
  // capped so a stalled reply cannot hold the check open.
  readonly property string coreUpdateCheckScript: 'command -v yay >/dev/null || { echo nohelper; exit 0; }; '
    + 'command -v curl >/dev/null 2>&1 || { echo nocurl; exit 0; }; '
    + 'command -v jq >/dev/null 2>&1 || { echo nojq; exit 0; }; '
    + 'v=$(pacman -Q omarchy-relaunch 2>/dev/null) || { echo notpkg; exit 0; }; v=${v#* }; echo "installed $v"; '
    + 'omarchy pkg aur accessible || { echo unreachable; exit 0; }; '
    + "r=$(curl -sfg --connect-timeout 15 --max-time 30 'https://aur.archlinux.org/rpc/v5/info?arg[]=omarchy-relaunch') || { echo fetchfailed; exit 0; }; "
    + "a=$(printf '%s' \"$r\" | jq -r '.results[0].Version // empty') || { echo badreply; exit 0; }; "
    + '[ -n "$a" ] || { echo noaur; exit 0; }; echo "latest $a"; '
    + 'if [ "$(vercmp "$a" "$v")" -gt 0 ]; then echo newer; else echo current; fi'
  // `omarchy pkg aur add` does nothing for a package already installed, so
  // an update and a reinstall go to yay (the AUR helper Omarchy uses) directly.
  readonly property var coreManageCommands: ({
    update: root.coreTerminal("Relaunch: update boot tools",
      'yay -S --noconfirm --needed omarchy-relaunch'),
    reinstall: root.coreTerminal("Relaunch: reinstall boot tools",
      'if omarchy pkg aur accessible; then yay -S --noconfirm --rebuild omarchy-relaunch; else echo "Could not reach the AUR."; false; fi'),
    uninstall: root.coreTerminal("Relaunch: remove boot tools",
      'omarchy pkg drop omarchy-relaunch')
  })
  // A visible terminal running `script` (a constant above), then its result
  // and a pause so the output can be read.
  function coreTerminal(title, script) {
    return ["xdg-terminal-exec", "--app-id=org.omarchy.terminal", "--title=" + title, "-e", "bash", "-c",
      script + '; s=$?; echo; if [ "$s" -eq 0 ]; then echo "Done. Go back to the Relaunch panel."; '
      + 'else echo "It did not finish (exit $s)."; fi; read -r -p "Press Enter to close this window. " _']
  }
  property string coreVersion: ""
  property string coreConfirm: ""
  property var core: null
  property string corePhase: "checking"
  property string coreMessage: ""
  property bool coreMessageIsError: false
  property bool coreBusy: false
  property bool coreChecking: false
  // The normal panel shows while the first probe is out, so an installed
  // system never flashes the install card.
  readonly property bool coreShown: ["missing", "installing", "wiring", "unwired"].indexOf(corePhase) < 0

  // The expanding-change lifecycle (TxnLifecycle.js) owns all transaction
  // state; these properties are its rendering. The review surface is THIS
  // panel (the polkit dialog is not trusted, §1/§3): the service's
  // reviewSummary is the headline and its reviewDiff sits under Details, both
  // verbatim PlainText, and both stay up through Authorize until the attempt
  // resolves (INV-10).
  property var txn: null
  property bool committing: false
  property bool authorizing: false
  property bool reviewVisible: false
  // A result the user must see, named for its action (TxnLifecycle `notice`).
  property string noticeText: ""
  property bool noticeIsError: false
  // Bumped whenever a change resolves: an edited field that was not
  // authorized goes back to the signed value.
  property int execRevision: 0
  // Pins a capture would remove, awaiting Replace or Keep (TxnLifecycle).
  property var confirmDrop: []
  property int lastResolvedAttempt: 0
  property string reviewSummary: ""
  property string reviewDiff: ""
  property string reviewHint: ""
  // Open windows the capture left out as identical duplicates (TxnLifecycle).
  property string reviewMerged: ""
  property bool reviewDetailsOpen: false
  // The live attempt's processes, created per attempt so a straggling callback
  // carries the attempt it belongs to.
  property var proposeChild: null
  property var commitChild: null

  readonly property var runningRows: rows.filter(function(r) { return r.kind === "running" })
  readonly property var relaunchRows: rows.filter(function(r) { return r.inRelaunch }).sort(function(a, b) {
    if (a.workspace !== b.workspace) return a.workspace - b.workspace
    return String(a.label).localeCompare(String(b.label))
  })
  // Running windows are deliberately not here: they do not start at login,
  // and one box holding both is what issue #30 was about.
  readonly property var startupRows: rows.filter(function(r) { return r.kind === "startup" })

  // One bordered box per workspace. relaunchRows is already sorted by
  // workspace, so a run of equal workspace numbers is one group.
  readonly property var relaunchGroups: {
    var groups = []
    var current = null
    for (var i = 0; i < root.relaunchRows.length; i++) {
      var row = root.relaunchRows[i]
      if (current === null || current.workspace !== row.workspace) {
        current = { workspace: row.workspace, apps: [] }
        groups.push(current)
      }
      current.apps.push(row)
    }
    return groups
  }

  function open() {
    root.confirmRemove = false
    root.controller.show()
    refresh()
  }
  function close() {
    root.confirmRemove = false
    root.controller.hide()
  }
  function toggle() {
    if (root.opened) root.close()
    else root.open()
  }
  function switchPanel(direction) {
    if (root.bar && typeof root.bar.switchPanelFrom === "function")
      return root.bar.switchPanelFrom(root.hostWidget || root, direction)
    return false
  }

  function refresh() {
    root.busy = true
    run(["list", "--json"])
  }

  // Setting `running = true` on a Process that is ALREADY running does
  // nothing: the command property changes but no process starts. Every
  // request that arrived while one was in flight was therefore dropped --
  // silently, and after pendingStatus had already been replaced, so the panel
  // could show the previous operation's success message for an action that
  // never ran. Reopening the panel calls refresh(), and the launch-command
  // field submits on Enter, so this was reachable by ordinary clicking.
  //
  // Queue instead of refusing: a request either runs now or runs next, and
  // nothing is discarded. FIFO with no coalescing, because deciding which
  // requests are equivalent is how actions get dropped again.
  // stdinText, when given, goes to the engine on stdin, never argv: another
  // local user can read any process's argv in /proc/<pid>/cmdline.
  // toast, when given, is flashed once the request succeeds.
  function run(args, okText, stdinText, toast) {
    root.confirmRemove = false
    // Not `busy`: refresh() sets that BEFORE calling run(), so testing it
    // would queue the ordinary idle open path behind nothing.
    if (actionProc.running || root.startingNext) {
      root.requestQueue = root.requestQueue.concat([{ args: args, okText: okText || "", stdinText: stdinText || "", toast: toast || "" }])
      root.busy = true
      return
    }
    root.startRequest(args, okText || "", false, stdinText || "", toast || "")
  }

  function showToast(text) {
    root.toastText = text
    toastTimer.restart()
  }

  function startRequest(args, okText, keepStatus, stdinText, toast) {
    root.pendingStatus = okText
    root.pendingToast = toast || ""
    if (!keepStatus) root.statusText = ""
    root.engineReportedError = false
    root.outputUnparseable = false
    if (!keepStatus) root.statusIsError = false
    root.stderrText = ""
    root.lastExitCode = -1
    root.sawOutput = false
    root.sawStderr = false
    root.startFailed = false
    root.settled = false
    root.startingNext = false
    root.busy = true
    actionProc.stdinText = stdinText || ""
    actionProc.stdinEnabled = actionProc.stdinText !== ""
    actionProc.command = [root.enginePath].concat(args)
    actionProc.running = true
  }

  // Every signal calls this; it does nothing until all three have reported.
  // stderr counts: a nonzero run with no JSON settled as "Command failed"
  // while the stderr callback was still in flight, and that callback only
  // assigned the text -- it never asked for the decision to be made again.
  // That is the same ordering bug as the stdout one, on the other stream.
  function settle() {
    if (root.lastExitCode === -1 || !root.sawOutput || !root.sawStderr) return
    root.decideStatus()
    if (root.settled) return
    root.settled = true
    root.pendingStatus = ""
    if (root.pendingToast !== "" && !root.statusIsError) root.showToast(root.pendingToast)
    root.pendingToast = ""
    if (root.requestQueue.length > 0) {
      var next = root.requestQueue[0]
      root.requestQueue = root.requestQueue.slice(1)
      // Keep the finished operation's message: a queued refresh should not
      // wipe the result the user is reading. callLater so any straggling
      // callback from the run that just ended lands first; startingNext keeps
      // the gap closed meanwhile.
      root.startingNext = true
      Qt.callLater(function() {
        root.startingNext = false
        root.startRequest(next.args, next.okText, true, next.stdinText, next.toast)
      })
      return
    }
    root.busy = false
  }

  // Last resort, from runningChanged. A command that cannot be executed never
  // emits `exited` at all, so waiting for signals that will never come left
  // the panel on "Working…" with every chip dead until the shell restarted.
  function recoverIfUnsettled() {
    if (root.settled || actionProc.running) return
    if (root.lastExitCode === -1) {
      // No exit code ever arrived: the process never started.
      root.startFailed = true
      root.lastExitCode = 127
    }
    root.sawOutput = true
    root.sawStderr = true
    root.settle()
  }

  // Idempotent on purpose: a stream that arrives after a forced settle can
  // call this again and upgrade the message.
  function decideStatus() {
    if (root.engineReportedError) {
      // applyResult already put the engine's own message up ("parse
      // overrides: …", "no relaunch entry for class: …"). That names the
      // actual problem; never replace it with a generic one.
      root.statusIsError = true
    } else if (root.startFailed && root.stderrText === "") {
      // Name the thing that could not be run: "Command failed" for a command
      // that never ran is the least useful sentence available.
      root.statusText = "Could not run the engine: " + root.enginePath
      root.statusIsError = true
      if (root.core) root.core.engineMissing()
    } else if (root.lastExitCode !== 0) {
      root.statusText = root.stderrText !== "" ? root.stderrText : "Command failed"
      root.statusIsError = true
      // The shim's "engine is not installed" (exit 127).
      if (root.lastExitCode === 127 && root.core) root.core.engineMissing()
    } else if (root.outputUnparseable) {
      root.statusText = "Bad engine output"
      root.statusIsError = true
    } else if (root.statusText === "" && root.pendingStatus !== "") {
      root.statusText = root.pendingStatus
      root.statusIsError = false
    }
  }

  Component.onCompleted: {
    root.txn = TxnLifecycle.create({
      render: function(st) {
        root.committing = st.committing
        root.authorizing = st.authorizing
        if (!root.reviewVisible && st.review !== null) root.reviewDetailsOpen = false
        root.reviewVisible = st.review !== null
        root.reviewSummary = st.review ? st.review.summary : ""
        root.reviewDiff = st.review ? st.review.diff : ""
        root.reviewHint = st.hint
        root.reviewMerged = st.mergedText
        if (st.notice !== root.noticeText && st.notice !== "")
          console.warn("relaunch: " + st.notice)
        root.confirmDrop = st.confirmDrop || []
        root.noticeText = st.notice
        root.noticeIsError = st.noticeIsError
        if (st.resolved && st.attempt !== root.lastResolvedAttempt) {
          root.lastResolvedAttempt = st.attempt
          root.execRevision += 1
          root.draftPreMs = root.preLaunchMs
          root.draftPostMs = root.postLaunchMs
        }
        root.statusText = st.status
        root.statusIsError = st.statusIsError
      },
      spawnPropose: function(attempt, args, stdinText) {
        root.proposeChild = proposeComponent.createObject(root, { attempt: attempt, args: args, stdinText: stdinText })
        root.proposeChild.running = true
      },
      spawnCommit: function(attempt, mode) {
        root.commitChild = commitComponent.createObject(root, { attempt: attempt, mode: mode })
        root.commitChild.running = true
      },
      writeCommit: function(attempt, text) {
        var c = root.commitChild
        if (c && c.attempt === attempt && c.running) c.write(text)
      },
      terminate: function(attempt, kind) {
        var c = kind === "propose" ? root.proposeChild : root.commitChild
        // running = false sends SIGTERM; for the commit helper that drops its
        // bus connection, which the service treats as a cancel.
        if (c && c.attempt === attempt && c.running) c.running = false
      },
      // Persist the signed list, then re-read it so the rows show what was
      // just authorized. A promptless reduce also gets a brief acknowledgement.
      install: function(config, okText, mode) {
        root.run(["install-signed", "--json"], okText, config, mode === "reduce" ? okText : "")
        root.run(["list", "--json"])
      }
    })
    root.core = CoreInstall.create({
      render: function(st) {
        root.corePhase = st.phase
        root.coreMessage = st.message
        root.coreMessageIsError = st.isError
        root.coreBusy = st.probing || ["installing", "updating", "reinstalling", "uninstalling",
          "wiring", "checking-updates"].indexOf(st.phase) >= 0
        root.coreChecking = st.probing
        root.coreVersion = st.version || ""
        root.coreConfirm = st.confirm
      },
      probe: function(op) { root.startCoreProc(coreProbeProc, op) },
      spawnInstall: function(op) {
        if (coreInstallProc.running) root.core.installEnded(op, { started: false, stillOpen: true })
        else root.startCoreProc(coreInstallProc, op)
      },
      spawnManage: function(op, kind) {
        if (coreManageProc.running || coreInstallProc.running) {
          root.core.installEnded(op, { started: false, stillOpen: true })
          return
        }
        coreManageProc.command = root.coreManageCommands[kind]
        root.startCoreProc(coreManageProc, op)
      },
      checkUpdates: function(op) { root.startCoreProc(coreUpdatesProc, op) },
      spawnWire: function(op) { root.startCoreProc(coreWireProc, op) },
      ready: function() {
        root.showToast("Boot tools installed.")
        root.run(["list", "--json"])
      }
    })
    root.core.check()
    root.run(["list", "--json"])
  }

  // Closing the panel (Escape, outside click) withdraws an open transaction.
  // The polkit dialog does not close it: that overlay takes its own clicks.
  // Opening it looks for the boot tools again (installed or removed since).
  onOpenedChanged: {
    if (!root.opened && root.txn) root.txn.cancel()
    if (root.opened && root.core) root.core.check()
  }

  // --- Boot tools processes (CoreInstall.js decides; these only run) ---

  // One process of each kind; CoreInstall never asks for a second while one
  // runs, and a running one is never restarted.
  function startCoreProc(proc, op) {
    if (proc.running) return
    proc.op = op
    proc.didStart = false
    proc.reported = false
    proc.exited = false
    proc.outDone = false
    proc.errDone = false
    proc.outText = ""
    proc.errText = ""
    proc.exitCode = -1
    proc.running = true
  }

  // Reported once: on exit, or when `running` drops without a start.
  function coreProcEnded(proc) {
    if (proc.reported) return
    proc.reported = true
    var lines = proc.outText.split("\n")
    // The value of the line "<key> <value>", or null.
    var field = function(key) {
      for (var i = 0; i < lines.length; i++)
        if (lines[i].indexOf(key + " ") === 0) return lines[i].slice(key.length + 1)
      return null
    }
    if (proc === coreProbeProc) {
      root.core.probeEnded(proc.op, { core: lines.indexOf("core") >= 0, canInstall: lines.indexOf("tools") >= 0,
        version: field("version"), stamp: field("stamp") })
    } else if (proc === coreUpdatesProc) {
      var verdicts = ["newer", "current", "unreachable", "notpkg", "noaur", "nohelper", "nocurl", "nojq",
        "fetchfailed", "badreply"]
      var result = lines.filter(function(l) { return verdicts.indexOf(l) >= 0 })[0] || ""
      root.core.updatesChecked(proc.op, { result: result, installed: field("installed"), latest: field("latest") })
    } else if (proc === coreInstallProc || proc === coreManageProc) {
      root.core.installEnded(proc.op, { started: proc.didStart })
    } else {
      root.core.wireEnded(proc.op, { started: proc.didStart, exitCode: proc.exitCode, stderr: proc.errText })
    }
  }

  component CoreProc: Process {
    id: cproc
    // Report only once both streams are in (the probe's answer is its stdout,
    // the wiring's error its stderr). The install terminal reports on exit
    // alone: a terminal that daemonizes keeps the pipes open.
    property bool waitStreams: true
    property bool errDone: false
    property int op: 0
    property bool didStart: false
    property bool reported: false
    property bool exited: false
    property bool outDone: false
    property string outText: ""
    property string errText: ""
    property int exitCode: -1
    stdout: StdioCollector {
      waitForEnd: true
      onStreamFinished: { cproc.outText = String(text || ""); cproc.outDone = true; cproc.progress() }
    }
    stderr: StdioCollector {
      waitForEnd: true
      onStreamFinished: {
        var raw = String(text || "").trim()
        var nl = raw.indexOf("\n")
        if (nl >= 0) raw = raw.slice(0, nl)
        cproc.errText = raw.length > 300 ? raw.slice(0, 300) + "…" : raw
        cproc.errDone = true
        cproc.progress()
      }
    }
    function progress() {
      if (cproc.exited && (!cproc.waitStreams || (cproc.outDone && cproc.errDone))) root.coreProcEnded(cproc)
    }
    onStarted: { cproc.didStart = true }
    onExited: function(code) { cproc.exitCode = code; cproc.exited = true; cproc.progress() }
    // The exit and stream signals have no guaranteed order, and a command that
    // never started emits neither: one turn after `running` drops, a process
    // that never started is reported.
    onRunningChanged: if (!cproc.running) Qt.callLater(function() { if (!cproc.didStart) root.coreProcEnded(cproc) })
  }

  CoreProc {
    id: coreProbeProc
    command: ["sh", "-c", root.coreProbeScript, "sh", root.relaunchdPath, root.relaunchctlPath, root.coreEnginePath]
  }
  CoreProc {
    id: coreInstallProc
    waitStreams: false
    command: root.coreInstallCommand
  }
  CoreProc {
    id: coreWireProc
    command: root.coreWireCommand
  }
  CoreProc {
    id: coreUpdatesProc
    command: ["sh", "-c", root.coreUpdateCheckScript]
  }
  CoreProc {
    id: coreManageProc
    waitStreams: false
    command: []
  }

  // While the install terminal is open: notices the install even when the
  // terminal handed off to a running instance and its process ended early.
  Timer {
    interval: 3000
    repeat: true
    running: ["installing", "updating", "reinstalling", "uninstalling"].indexOf(root.corePhase) >= 0
    onTriggered: root.core.poll()
  }

  Timer {
    id: toastTimer
    interval: 1000
    onTriggered: root.toastText = ""
  }

  Timer {
    id: disarmRemove
    interval: 4000
    onTriggered: root.confirmRemove = false
  }

  function applyResult(text) {
    try {
      const r = JSON.parse(text)
      if (r.ok === false) {
        root.statusText = r.error || "unknown"
        root.engineReportedError = true
        return
      }
      if (r.uninstalled === true) {
        root.statusText = "Relaunch removed."
        root.statusIsError = false
        root.rows = []
        root.entries = []
        root.close()
        Quickshell.execDetached(["omarchy-notification-send", "Relaunch", "Removed from this system."])
        return
      }
      if (r.entries !== undefined) root.entries = r.entries
      if (r.rows !== undefined) root.rows = r.rows
      if (r.boot !== undefined) root.boot = r.boot
      if (r.lastBoot !== undefined) root.lastBoot = r.lastBoot
      if (r.preLaunchMs !== undefined) root.preLaunchMs = root.draftPreMs = r.preLaunchMs
      if (r.postLaunchMs !== undefined) root.postLaunchMs = root.draftPostMs = r.postLaunchMs
      if (r.warnings !== undefined) root.warnings = r.warnings
      if (r.added !== undefined) {
        root.statusIsError = false
        root.statusText = "Saved: " + r.added + " new, " + r.updated + " updated."
        if (r.warnings && r.warnings.length > 0)
          root.statusText += " Warning: " + r.warnings.map(function(w) {
            return w.class + " — " + (w.message || "command not found")
          }).join(" ")
      } else if (root.statusText === "" && r.warnings && r.warnings.length > 0) {
        root.statusIsError = false
        root.statusText = r.warnings.map(function(w) {
          return w.class + " — " + (w.message || "command not found")
        }).join(" ")
      }
    } catch (e) {
      // Do not caption it here: an unparseable body on a FAILED run means the
      // real message is on stderr, and settle() is the only place that knows
      // the exit code.
      root.outputUnparseable = true
    }
  }

  function bootLabel() {
    if (root.boot.disabled) return "Disabled until you re-enable it."
    if (root.boot.skipOnce) return "Will skip the next boot only."
    return "Runs on boot."
  }

  function lastBootLink() {
    if (!root.lastBoot) return ""
    var when = String(root.lastBoot.startedAt || "").replace("T", " ")
    var n = (root.lastBoot.launches || []).length
    var extra = ""
    var placed = (root.lastBoot.launches || []).filter(function(l) { return l.placed }).length
    if (root.lastBoot.outcome === "landed") extra = " · " + n + " app" + (n === 1 ? "" : "s")
    else if (root.lastBoot.outcome === "launched") extra = " · " + placed + " of " + n + " placed"
    else if (root.lastBoot.outcome === "schema-upgrade-needed") extra = " · save again to launch"
    else if (root.lastBoot.outcome) extra = " · " + root.lastBoot.outcome
    return "Last boot log" + extra + (when ? " · " + when : "")
  }

  Process {
    id: actionProc
    property string stdinText: ""
    onStarted: {
      if (stdinText !== "") write(stdinText + "\n")
      stdinText = ""
      // Close the write channel (Qt flushes pending data first) so a reader
      // that consumes stdin to EOF, like install-signed, can finish.
      stdinEnabled = false
    }
    command: [root.enginePath, "list", "--json"]
    // waitForEnd, because without it the collector can report a stream that
    // has not been fully read: on a failing run the engine's JSON error was
    // still in flight, applyResult saw an empty body, and the panel fell all
    // the way through to "Command failed".
    stdout: StdioCollector {
      waitForEnd: true
      onStreamFinished: {
        root.applyResult(String(text || ""))
        root.sawOutput = true
        root.settle()
      }
    }
    // Failures that produce no JSON at all -- a die before --json is parsed,
    // a missing engine -- say why on stderr. Without this the panel had
    // nothing to show but "Command failed".
    stderr: StdioCollector {
      waitForEnd: true
      onStreamFinished: {
        // First line, and bounded: this ends up in the ERROR box, and a stuck
        // engine should not be able to grow it without limit.
        var raw = String(text || "").trim()
        var nl = raw.indexOf("\n")
        if (nl >= 0) raw = raw.slice(0, nl)
        root.stderrText = raw.length > 500 ? raw.slice(0, 500) + "…" : raw
        root.sawStderr = true
        root.settle()
      }
    }
    onExited: function(exitCode) {
      // A real exit code retracts the inference: recovery may have already
      // decided the process never started, and decideStatus checks that
      // before the exit code, so the caption would otherwise stay
      // "Could not run the engine" after a successful run reported 0.
      root.startFailed = false
      root.lastExitCode = exitCode
      root.settle()
    }
    // Covers BOTH failure shapes, because `running` drops to false for both:
    // a process that exited without its collectors finishing, and a command
    // that could not be executed and therefore never emitted `exited` at all.
    // Process exposes no error signal to QML, so this property change is the
    // only observation available. One event-loop turn later, so an ordinary
    // exit settles through its own signals first -- and settle() stays
    // re-runnable, so a stream arriving afterwards can still improve the
    // message. Not a timeout and not a retry.
    onRunningChanged: {
      if (actionProc.running) return
      Qt.callLater(root.recoverIfUnsettled)
    }
  }

  // --- Expanding change processes (one pair per attempt; see TxnLifecycle.js) ---

  // Report the capture once its exit and both streams are in (Omarchy: those
  // signals have no guaranteed order), or once `running` dropped without a
  // `started`, which is the only sign of a failed start Process gives QML.
  function proposeProgress(p, forced) {
    if (p.reported) return
    var failedStart = forced && !p.didStart && !p.running
    if (!(p.exited && p.outDone && p.errDone) && !failedStart) return
    p.reported = true
    root.txn.proposeEnded(p.attempt, { started: p.didStart, exitCode: p.exitCode, stdout: p.outText, stderr: p.errText })
    if (root.proposeChild === p) root.proposeChild = null
    Qt.callLater(function() { p.destroy() })
  }

  function commitProgress(c, forced) {
    if (c.reported) return
    var failedStart = forced && !c.didStart && !c.running
    if (!(c.exited && c.errDone) && !failedStart) return
    c.reported = true
    // One more turn, so a final stdout line delivered alongside the exit is
    // handled before the attempt is told its helper ended.
    Qt.callLater(function() {
      root.txn.commitEnded(c.attempt, { started: c.didStart, stderr: c.errText })
      if (root.commitChild === c) root.commitChild = null
      c.destroy()
    })
  }

  Component {
    id: proposeComponent
    Process {
      id: pp
      property int attempt: 0
      property bool didStart: false
      property bool exited: false
      property int exitCode: -1
      property bool outDone: false
      property bool errDone: false
      property string outText: ""
      property string errText: ""
      property bool reported: false
      // The engine edit to run (propose, import, set-exec, drop) and its stdin
      // (set-exec's command: never on argv, which other users can read).
      property var args: []
      property string stdinText: ""
      stdinEnabled: stdinText !== ""
      command: [root.enginePath].concat(args)
      stdout: StdioCollector {
        waitForEnd: true
        onStreamFinished: { pp.outText = String(text || ""); pp.outDone = true; root.proposeProgress(pp, false) }
      }
      stderr: StdioCollector {
        waitForEnd: true
        onStreamFinished: {
          var raw = String(text || "").trim()
          pp.errText = raw.length > 500 ? raw.slice(0, 500) + "…" : raw
          pp.errDone = true
          root.proposeProgress(pp, false)
        }
      }
      onStarted: {
        pp.didStart = true
        if (pp.stdinText !== "") pp.write(pp.stdinText + "\n")
        // Close stdin so the engine reads the command to EOF.
        pp.stdinEnabled = false
      }
      onExited: function(code) { pp.exitCode = code; pp.exited = true; root.proposeProgress(pp, false) }
      onRunningChanged: if (!pp.running) Qt.callLater(function() { root.proposeProgress(pp, true) })
    }
  }

  Component {
    id: commitComponent
    Process {
      id: cp
      property int attempt: 0
      property bool didStart: false
      property bool exited: false
      property bool errDone: false
      property string errText: ""
      property bool reported: false
      // "commit" (reviewed, authorized) or "reduce" (promptless): the engine's
      // reply decides, never the panel.
      property string mode: "commit"
      // Enabled BEFORE start and kept open: the request goes in on start, and a
      // commit's decision travels the same pipe after the human pause (§5.1).
      stdinEnabled: true
      command: [root.relaunchctlPath, mode]
      stdout: SplitParser {
        splitMarker: "\n"
        onRead: function(data) { root.txn.commitLine(cp.attempt, data) }
      }
      stderr: StdioCollector {
        waitForEnd: true
        onStreamFinished: {
          var raw = String(text || "").trim()
          cp.errText = raw.length > 500 ? raw.slice(0, 500) + "…" : raw
          cp.errDone = true
          root.commitProgress(cp, false)
        }
      }
      onStarted: { cp.didStart = true; root.txn.commitStarted(cp.attempt) }
      onExited: function(code) { cp.exited = true; root.commitProgress(cp, false) }
      onRunningChanged: if (!cp.running) Qt.callLater(function() { root.commitProgress(cp, true) })
    }
  }

  function openLog() {
    var payload = "{}"
    if (root.lastBoot && root.lastBoot.logPath)
      payload = JSON.stringify({ logPath: root.lastBoot.logPath })
    if (root.bar)
      root.bar.run("omarchy-shell shell summon io.github.laytonf.relaunch " + Util.shellQuote(payload))
    else
      Quickshell.execDetached(["omarchy-shell", "shell", "summon", "io.github.laytonf.relaunch", payload])
  }

  readonly property string helpUrl: "https://github.com/farmall856/omarchy-relaunch#readme"

  function openHelp() {
    if (root.bar)
      root.bar.run("xdg-open " + Util.shellQuote(root.helpUrl))
    else
      Quickshell.execDetached(["xdg-open", root.helpUrl])
  }

  // `saved` is the command the field started from (the pin's execText).
  function saveExec(className, label, exec, saved) {
    var what = "Change the launch command for " + (label || className)
    var cmd = String(exec || "").replace(/^\s+|\s+$/g, "")
    if (!className) return
    if (!cmd) {
      root.txn.notify(what + ": type a command first.", true)
      return
    }
    if (cmd === String(saved || "").replace(/^\s+|\s+$/g, "")) {
      root.txn.notify(what + ": no change. That is already the launch command.", false)
      return
    }
    root.execEditClass = ""
    root.txn.start(["set-exec", "--class", className, "--exec-stdin", "--json"], cmd,
      "Preparing the new command…", "Launch command authorized.", "",
      { what: what, nothingText: "no change. That is already the launch command." })
  }

  // The command came from the window's own process, not a packaged launcher.
  // A normal state for a terminal or a script, not a warning.
  function rowFromCommandLine(row) {
    return row && (row.execSource === "guess" || row.execSource === "cmdline" || row.unverified === true)
  }

  function rowBroken(row) {
    return row && row.execOk === false
  }

  // A full-width capture action: what it does, and in a second line what it
  // does to the pins already saved.
  component CaptureButton: Rectangle {
    id: capture
    property string label: ""
    property string subline: ""
    signal activated()
    width: parent ? parent.width : 0
    height: captureText.implicitHeight + Style.space(14)
    radius: Style.space(6)
    color: captureMouse.containsMouse ? Style.hoverFillFor(root.ink, Commons.Color.accent) : "transparent"
    border.color: root.ink
    border.width: 1
    opacity: root.busy || root.committing ? 0.5 : 1.0
    Column {
      id: captureText
      anchors.centerIn: parent
      spacing: Style.space(2)
      Text {
        textFormat: Text.PlainText
        anchors.horizontalCenter: parent.horizontalCenter
        text: root.busy || root.committing ? "Working…" : capture.label
        color: root.ink
        font.family: root.bar ? root.bar.fontFamily : Style.font.family
        font.pixelSize: Style.font.body
        font.bold: true
      }
      Text {
        textFormat: Text.PlainText
        anchors.horizontalCenter: parent.horizontalCenter
        text: capture.subline
        color: root.ink
        opacity: 0.65
        font.family: root.bar ? root.bar.fontFamily : Style.font.family
        font.pixelSize: Style.font.caption
      }
    }
    MouseArea {
      id: captureMouse
      anchors.fill: parent
      hoverEnabled: true
      enabled: !root.busy && !root.committing
      onClicked: capture.activated()
    }
  }

  component Chip: Rectangle {
    id: chip
    property string label: ""
    property bool danger: false
    // Shown but not clickable (e.g. Authorize while approval is pending).
    property bool inactive: false
    signal clicked
    height: Style.space(22)
    width: chipLabel.implicitWidth + Style.space(12)
    radius: Style.space(4)
    color: chipMouse.containsMouse ? Style.hoverFillFor(chip.danger ? Commons.Color.urgent : root.ink, chip.danger ? Commons.Color.urgent : Commons.Color.accent) : "transparent"
    border.color: chip.danger ? Commons.Color.urgent : root.ink
    border.width: 1
    opacity: root.busy || chip.inactive ? 0.45 : 1
    Text {
      textFormat: Text.PlainText
      id: chipLabel
      anchors.centerIn: parent
      text: chip.label
      color: chip.danger ? Commons.Color.urgent : root.ink
      font.family: root.bar ? root.bar.fontFamily : Style.font.family
      font.pixelSize: Style.font.caption
    }
    MouseArea {
      id: chipMouse
      anchors.fill: parent
      hoverEnabled: true
      enabled: !root.busy && !chip.inactive
      onClicked: chip.clicked()
    }
  }

  // Square, glyph-only variant of Chip. An icon-only control needs a name,
  // so `hint` is required reading for anyone adding one.
  component IconChip: Rectangle {
    id: iconChip
    property string glyph: ""
    property string hint: ""
    property bool danger: false
    property bool active: false
    property real glyphSize: Style.font.bodySmall
    signal clicked
    readonly property color tint: iconChip.danger ? Commons.Color.urgent : root.ink
    height: Style.space(22)
    width: Style.space(26)
    radius: Style.space(4)
    color: iconChipMouse.containsMouse || iconChip.active
      ? Style.hoverFillFor(iconChip.tint, iconChip.danger ? Commons.Color.urgent : Commons.Color.accent)
      : "transparent"
    border.color: iconChip.tint
    border.width: 1
    opacity: root.busy ? 0.45 : 1
    Text {
      textFormat: Text.PlainText
      anchors.centerIn: parent
      text: iconChip.glyph
      color: iconChip.tint
      font.family: root.bar ? root.bar.fontFamily : Style.font.family
      font.pixelSize: iconChip.glyphSize
    }
    MouseArea {
      id: iconChipMouse
      anchors.fill: parent
      hoverEnabled: true
      enabled: !root.busy
      onClicked: iconChip.clicked()
      PanelToolTip {
        visible: iconChipMouse.containsMouse && iconChip.hint !== ""
        text: iconChip.hint
      }
    }
  }

  // Title chip that straddles a box's top border, fieldset style. It paints
  // the panel background behind itself to break the border line, so the
  // heading costs no vertical space inside the box.
  component BorderLegend: Rectangle {
    id: legendChip
    property string title: ""
    property color tint: root.ink
    x: Style.space(10)
    width: legendLabel.implicitWidth + Style.space(8)
    height: legendLabel.implicitHeight
    color: root.bar ? root.bar.background : Commons.Color.background
    Text {
      textFormat: Text.PlainText
      id: legendLabel
      anchors.centerIn: parent
      text: legendChip.title
      color: legendChip.tint
      opacity: 0.75
      font.family: root.bar ? root.bar.fontFamily : Style.font.family
      font.pixelSize: Style.font.caption
      font.bold: true
    }
  }

  KeyboardPanel {
    id: panel
    anchorItem: root.anchorItem
    owner: root.hostWidget || root
    bar: root.bar
    open: root.opened
    focusTarget: keyCatcher
    contentWidth: panel.fittedContentWidth(Style.space(400))
    contentHeight: panel.fittedContentHeight(Math.min(content.implicitHeight, Style.space(520)))

    PanelKeyCatcher {
      id: keyCatcher
      anchors.fill: parent
      blocked: root.execEditClass !== ""
      onCloseRequested: root.close()
      onTabRequested: function(direction) { root.switchPanel(direction) }

      Flickable {
        id: scroller
        anchors.fill: parent
        contentWidth: width
        contentHeight: content.implicitHeight
        clip: true
        boundsBehavior: Flickable.StopAtBounds
        interactive: contentHeight > height

        Column {
          id: content
          width: scroller.width
          spacing: Style.space(10)

          // Lowercase to match how the plugin is named everywhere else.
          Item {
            width: parent.width
            height: Math.max(titleRow.implicitHeight, helpChip.height)

            Row {
              id: titleRow
              anchors.left: parent.left
              anchors.verticalCenter: parent.verticalCenter
              spacing: Style.space(8)

              Text {
                textFormat: Text.PlainText
                anchors.verticalCenter: parent.verticalCenter
                // Same glyph as the bar button (BarWidget.qml), so the popup
                // names the button that opened it. Keep the two in step.
                // Identity, not a control: plain Text, no chip border and no
                // hover state.
                text: "\uf1da"
                color: root.ink
                font.family: root.bar ? root.bar.fontFamily : Style.font.family
                font.pixelSize: Style.font.subtitle
              }

              Text {
                textFormat: Text.PlainText
                id: titleText
                anchors.verticalCenter: parent.verticalCenter
                text: "relaunch"
                color: root.ink
                font.family: root.bar ? root.bar.fontFamily : Style.font.family
                font.pixelSize: Style.font.subtitle
                font.bold: true
              }
            }

            IconChip {
              id: helpChip
              anchors.right: parent.right
              anchors.verticalCenter: parent.verticalCenter
              // Plain question mark, not the solid circled variant: the
              // chip already draws the surrounding border.
              glyph: "\uf128"  // nf-fa-question
              hint: "Open the Relaunch README on GitHub"
              onClicked: root.openHelp()
            }
          }

          // The boot tools are not installed (CoreInstall.js): what they are,
          // said before the button, and the button. The disclosure is the
          // consent for the package install and for the hyprland.lua edit.
          Rectangle {
            visible: !root.coreShown
            width: parent.width
            height: visible ? coreBody.implicitHeight + Style.space(20) : 0
            radius: Style.space(6)
            color: "transparent"
            border.color: root.ink
            border.width: 1
            Column {
              id: coreBody
              x: Style.space(10)
              y: Style.space(10)
              width: parent.width - Style.space(20)
              spacing: Style.space(8)
              Text {
                textFormat: Text.PlainText
                width: parent.width
                text: "Relaunch needs its boot tools"
                color: root.ink
                font.family: root.bar ? root.bar.fontFamily : Style.font.family
                font.pixelSize: Style.font.body
                font.bold: true
              }
              Text {
                textFormat: Text.PlainText
                width: parent.width
                text: "A small root-owned system package (" + root.corePackage + "): a polkit-gated signing authority, "
                  + "the login verifier that runs only a signature-verified launch list, and a systemd unit. "
                  + "Installing it uses pacman in a terminal window and will ask for your password (sudo). "
                  + "Relaunch will then wire its login step into your Hyprland config (hyprland.lua). "
                  + "Nothing launches until you Save and Authorize a layout."
                color: root.ink
                opacity: 0.85
                wrapMode: Text.Wrap
                font.family: root.bar ? root.bar.fontFamily : Style.font.family
                font.pixelSize: Style.font.bodySmall
              }
              Text {
                visible: root.coreMessage !== ""
                textFormat: Text.PlainText
                width: parent.width
                text: root.coreMessage
                color: root.coreMessageIsError ? Commons.Color.urgent : root.ink
                wrapMode: Text.Wrap
                font.family: root.bar ? root.bar.fontFamily : Style.font.family
                font.pixelSize: Style.font.bodySmall
              }
              Row {
                spacing: Style.space(10)
                Chip {
                  label: root.corePhase === "installing" ? "Installing…"
                    : root.corePhase === "wiring" ? "Wiring…"
                    : root.corePhase === "unwired" ? "Finish setup" : "Install boot tools"
                  inactive: root.coreBusy
                  onClicked: if (!root.coreBusy) root.core.install()
                }
                // Also the way out of "installing" when the terminal window
                // outlives its process: the user says it is done.
                Chip {
                  label: root.corePhase === "installing" ? "I'm done" : "Check again"
                  inactive: root.coreChecking || root.corePhase === "wiring"
                  onClicked: if (!root.coreChecking && root.corePhase !== "wiring") root.core.check()
                }
              }
            }
          }

          // The normal panel, once the boot tools are in.
          Column {
            visible: root.coreShown
            width: parent.width
            spacing: content.spacing
            // The two captures. Both read the open windows and are reviewed and
            // authorized; they differ in what happens to a pin whose app is not
            // open now. "Capture" replaces the boot list (and asks before it
            // drops anything); "Add" merges into it and drops nothing.
            CaptureButton {
              label: "Capture open windows"
              subline: "Replaces the boot list with the apps open now."
              onActivated: root.txn.startSave()
            }
            CaptureButton {
              label: "Add open windows"
              subline: "Adds the apps open now. Keeps every other pin."
              onActivated: root.txn.startMerge()
            }

            // Asked before a capture removes pins whose apps are not open. Nothing
            // is sent until it is answered; closing the panel answers Keep.
            Rectangle {
              visible: root.confirmDrop.length > 0
              width: parent.width
              height: visible ? dropBody.implicitHeight + Style.space(20) : 0
              radius: Style.space(6)
              color: "transparent"
              border.color: Commons.Color.urgent
              border.width: 1
              Column {
                id: dropBody
                x: Style.space(10)
                y: Style.space(10)
                width: parent.width - Style.space(20)
                spacing: Style.space(8)
                Text {
                  textFormat: Text.PlainText
                  width: parent.width
                  wrapMode: Text.WordWrap
                  color: root.ink
                  font.family: root.bar ? root.bar.fontFamily : Style.font.family
                  font.pixelSize: Style.font.bodySmall
                  text: {
                    var n = root.confirmDrop.length
                    var names = root.confirmDrop.map(function(d) { return d.label + " (workspace " + d.workspace + ")" })
                    return "This will remove " + n + " pinned app" + (n === 1 ? "" : "s")
                      + " that " + (n === 1 ? "isn't" : "aren't") + " open: " + names.join(", ")
                      + ". Replace anyway? To keep them, use Add open windows instead."
                  }
                }
                Row {
                  spacing: Style.space(10)
                  Chip {
                    label: "Replace"
                    danger: true
                    onClicked: root.txn.confirmReplace()
                  }
                  Chip {
                    label: "Keep them"
                    onClicked: root.txn.keepPins()
                  }
                }
              }
            }

            // What the last action came to when the user has to see it — a
            // failure, or a change that turned out to change nothing — named for
            // the action, here under the button that most often causes them, and
            // kept until the next action or a dismiss. An engine error from a
            // plain request shows here too.
            Item {
              id: noticeGroup
              readonly property bool isError: root.noticeText !== "" ? root.noticeIsError : true
              readonly property string body: root.noticeText !== ""
                ? root.noticeText
                : (root.statusIsError ? root.statusText : "")
              width: parent.width
              visible: body.length > 0
              height: visible ? errBox.height + errLegend.height / 2 : 0

              Rectangle {
                id: errBox
                y: errLegend.height / 2
                width: parent.width
                height: errBody.implicitHeight + Style.space(18)
                radius: Style.space(6)
                color: "transparent"
                border.color: noticeGroup.isError ? Commons.Color.urgent : root.ink
                border.width: 1
              }

              BorderLegend {
                id: errLegend
                title: noticeGroup.isError ? "ERROR" : "NOTHING CHANGED"
                tint: noticeGroup.isError ? Commons.Color.urgent : root.ink
              }

              Text {
                textFormat: Text.PlainText
                id: errBody
                x: Style.space(10)
                y: errBox.y + Style.space(9)
                width: parent.width - Style.space(20) - dismissNotice.width
                text: noticeGroup.body
                color: noticeGroup.isError ? Commons.Color.urgent : root.ink
                font.family: root.bar ? root.bar.fontFamily : Style.font.family
                font.pixelSize: Style.font.bodySmall
                wrapMode: Text.WordWrap
              }

              Text {
                id: dismissNotice
                textFormat: Text.PlainText
                visible: root.noticeText !== ""
                anchors.right: errBox.right
                anchors.rightMargin: Style.space(8)
                y: errBox.y + Style.space(9)
                text: "×"
                color: root.ink
                font.pixelSize: Style.font.body
                MouseArea {
                  anchors.fill: parent
                  anchors.margins: -Style.space(4)
                  cursorShape: Qt.PointingHandCursor
                  onClicked: root.txn.dismissNotice()
                }
              }
            }

            Rectangle { width: parent.width; height: 1; color: root.ink; opacity: 0.2 }

            Text {
              textFormat: Text.PlainText
              width: parent.width
              text: root.relaunchRows.length > 0
                ? "Boot list (" + root.relaunchRows.length + ")"
                : "No apps on the boot list yet"
              color: root.ink
              font.family: root.bar ? root.bar.fontFamily : Style.font.family
              font.pixelSize: Style.font.bodySmall
              font.bold: true
            }

            Text {
              textFormat: Text.PlainText
              visible: root.relaunchRows.length > 0
              width: parent.width
              text: "Changes to the saved boot list keep everything else."
              color: root.ink
              opacity: 0.6
              wrapMode: Text.WordWrap
              font.family: root.bar ? root.bar.fontFamily : Style.font.family
              font.pixelSize: Style.font.caption
            }

            Repeater {
              model: root.relaunchGroups
              delegate: Item {
                id: wsGroup
                required property var modelData
                width: content.width
                // The legend straddles the top border, so half of it sits
                // above the box and has to be paid for here.
                height: wsBox.height + wsLegend.height / 2

                Rectangle {
                  id: wsBox
                  y: wsLegend.height / 2
                  width: parent.width
                  height: wsBody.implicitHeight + Style.space(18)
                  radius: Style.space(6)
                  color: "transparent"
                  border.color: root.ink
                  border.width: 1
                }

                BorderLegend {
                  id: wsLegend
                  title: "Workspace " + wsGroup.modelData.workspace
                }

                Column {
                  id: wsBody
                  x: Style.space(10)
                  y: wsBox.y + Style.space(9)
                  width: parent.width - Style.space(20)
                  spacing: Style.space(6)

                  Repeater {
                    model: wsGroup.modelData.apps
                    delegate: Column {
                      id: relaunchRow
                      required property var modelData
                      // A broken row opens its editor unprompted: there is
                      // nothing on the collapsed line to fix it with.
                      property bool expanded: root.rowBroken(modelData)
                      width: wsBody.width
                      spacing: Style.space(4)

                      // One app is one line: the name, then the two actions.
                      Item {
                        width: parent.width
                        height: rowActions.height

                        Text {
                          textFormat: Text.PlainText
                          anchors.left: parent.left
                          anchors.right: rowActions.left
                          anchors.rightMargin: Style.space(6)
                          anchors.verticalCenter: parent.verticalCenter
                          text: relaunchRow.modelData.label
                            + (root.rowFromCommandLine(relaunchRow.modelData) ? "  (command line)" : "")
                          elide: Text.ElideRight
                          color: root.rowBroken(relaunchRow.modelData) ? Commons.Color.urgent : root.ink
                          opacity: relaunchRow.modelData.enabled ? 1.0 : 0.4
                          font.family: root.bar ? root.bar.fontFamily : Style.font.family
                          font.pixelSize: Style.font.bodySmall
                        }

                        Row {
                          id: rowActions
                          anchors.right: parent.right
                          anchors.verticalCenter: parent.verticalCenter
                          spacing: Style.space(6)
                          IconChip {
                            glyph: "\uf135"  // nf-fa-rocket
                            hint: relaunchRow.expanded ? "Hide launch command" : "View or edit launch command"
                            active: relaunchRow.expanded
                            danger: root.rowBroken(relaunchRow.modelData)
                            onClicked: relaunchRow.expanded = !relaunchRow.expanded
                          }
                          IconChip {
                            glyph: "\uf1f8"  // nf-fa-trash
                            // No app name here. The row shows it immediately to
                            // the left, and `hint` is rendered by the shell's
                            // PanelToolTip -- a Text with no textFormat, so
                            // AutoText -- which we do not own and must not edit.
                            // A window class or .desktop Name= is chosen by
                            // other software; the way to keep it from becoming
                            // markup there is not to send it.
                            hint: "Remove"
                            onClicked: root.txn.start(["drop", "--class", relaunchRow.modelData.class, "--json"], "",
                              "Removing…", "Removed.", "", { what: "Remove " + relaunchRow.modelData.label })
                          }
                        }
                      }

                      // The row shows a friendly label now, but the pin matches
                      // on class, so class has to stay reachable somewhere.
                      Text {
                        textFormat: Text.PlainText
                        visible: relaunchRow.expanded
                        width: parent.width
                        text: "class " + relaunchRow.modelData.class + " — what the workspace pin matches"
                        wrapMode: Text.WordWrap
                        color: root.ink
                        opacity: 0.6
                        font.family: root.bar ? root.bar.fontFamily : Style.font.family
                        font.pixelSize: Style.font.caption
                      }

                      Text {
                        textFormat: Text.PlainText
                        visible: relaunchRow.expanded && root.rowBroken(relaunchRow.modelData)
                        width: parent.width
                        text: "Launch command not found. Type the command that starts this app."
                        wrapMode: Text.WordWrap
                        color: Commons.Color.urgent
                        font.family: root.bar ? root.bar.fontFamily : Style.font.family
                        font.pixelSize: Style.font.caption
                      }

                      Row {
                        visible: relaunchRow.expanded
                        width: parent.width
                        spacing: Style.space(6)
                        TextField {
                          id: execField
                          // The signed command, as a command line set-exec reads
                          // back to it. An edit is only text until authorized:
                          // Escape, collapsing the row, and any outcome but a
                          // commit put the signed value back.
                          readonly property string saved: relaunchRow.modelData.execText
                            || relaunchRow.modelData.exec || ""
                          readonly property int revision: root.execRevision
                          readonly property bool rowOpen: relaunchRow.expanded
                          function revert() { text = saved }
                          onSavedChanged: revert()
                          onRevisionChanged: revert()
                          onRowOpenChanged: if (!rowOpen) revert()
                          Component.onCompleted: revert()
                          width: parent.width - Style.space(70)
                          placeholderText: "launch command"
                          font.family: Style.font.family
                          font.pixelSize: Style.font.bodySmall
                          foreground: root.ink
                          horizontalPadding: Style.spacing.controlGap
                          verticalPadding: Style.spacing.controlPaddingY
                          onActiveFocusChanged: {
                            if (activeFocus) root.execEditClass = relaunchRow.modelData.class
                            else if (root.execEditClass === relaunchRow.modelData.class) root.execEditClass = ""
                          }
                          onAccepted: root.saveExec(relaunchRow.modelData.class,
                            relaunchRow.modelData.label, text, saved)
                          Keys.onEscapePressed: {
                            root.execEditClass = ""
                            revert()
                            focus = false
                            relaunchRow.expanded = false
                          }
                        }
                        Chip {
                          label: "Save"
                          onClicked: root.saveExec(relaunchRow.modelData.class,
                            relaunchRow.modelData.label, execField.text, execField.saved)
                        }
                      }
                    }
                  }
                }
              }
            }

            // Open windows Relaunch does not manage. Ephemeral: close the
            // window and the row goes with it, and nothing here starts at
            // login. Split from the startup apps below because one box holding
            // both made "removed from relaunch" read as "moved to autostart".
            Item {
              id: winGroup
              visible: root.runningRows.length > 0
              width: content.width
              height: visible ? winBox.height + winLegend.height / 2 : 0

              Rectangle {
                id: winBox
                y: winLegend.height / 2
                width: parent.width
                height: winBody.implicitHeight + Style.space(18)
                radius: Style.space(6)
                color: "transparent"
                border.color: root.ink
                border.width: 1
              }

              BorderLegend {
                id: winLegend
                title: "WINDOWS NOT IN RELAUNCH"
              }

              Column {
                id: winBody
                x: Style.space(10)
                y: winBox.y + Style.space(9)
                width: parent.width - Style.space(20)
                spacing: Style.space(6)

                Repeater {
                  model: root.runningRows
                  delegate: Item {
                    id: winRow
                    required property var modelData
                    width: winBody.width
                    height: winActions.height

                    Text {
                      textFormat: Text.PlainText
                      anchors.left: parent.left
                      anchors.right: winActions.left
                      anchors.rightMargin: Style.space(6)
                      anchors.verticalCenter: parent.verticalCenter
                      text: "ws " + winRow.modelData.workspace + "  ·  " + winRow.modelData.label
                      elide: Text.ElideRight
                      color: root.ink
                      font.family: root.bar ? root.bar.fontFamily : Style.font.family
                      font.pixelSize: Style.font.bodySmall
                    }

                    Row {
                      id: winActions
                      anchors.right: parent.right
                      anchors.verticalCenter: parent.verticalCenter
                      spacing: Style.space(6)

                      IconChip {
                        glyph: "+"
                        glyphSize: Style.font.body
                        // Added where it already is. Workspace is an integer
                        // from our own JSON and cannot carry markup; the app
                        // name is text chosen by other software, so it stays
                        // out of a tooltip we do not render. See the Remove
                        // chip on the relaunch rows.
                        hint: "Add to relaunch on workspace " + winRow.modelData.workspace
                        onClicked: root.txn.start([
                          "import", "--class", winRow.modelData.class,
                          "--workspace", String(winRow.modelData.workspace), "--json"
                        ], "", "Preparing…", "Added and authorized.", "", {
                          what: "Add " + winRow.modelData.label,
                          nothingText: "nothing to add. It is already pinned there."
                        })
                      }
                    }
                  }
                }
              }
            }

            // Entries in the user's autostart.lua. These do start at login,
            // which is the whole reason they are not in the box above.
            //
            // The list is READ-ONLY, and that is the point: Relaunch restores
            // app windows to their workspaces, it is not an autostart manager.
            // The box exists so a user can recognise where an app they did not
            // expect at login comes from. The rows have no actions because
            // every action one could carry would write autostart.lua, and
            // Relaunch never does. An app on both lists appears on both.
            Item {
              id: startGroup
              visible: root.startupRows.length > 0
              width: content.width
              height: visible ? startBox.height + startLegend.height / 2 : 0

              Rectangle {
                id: startBox
                y: startLegend.height / 2
                width: parent.width
                height: startBody.implicitHeight + Style.space(18)
                radius: Style.space(6)
                color: "transparent"
                border.color: root.ink
                border.width: 1
              }

              BorderLegend {
                id: startLegend
                title: "STARTUP APPS"
              }

              Column {
                id: startBody
                x: Style.space(10)
                y: startBox.y + Style.space(9)
                width: parent.width - Style.space(20)
                spacing: Style.space(6)

                Repeater {
                  model: root.startupRows
                  delegate: Text {
                    required property var modelData
                    textFormat: Text.PlainText
                    width: startBody.width
                    text: modelData.label
                    elide: Text.ElideRight
                    color: root.ink
                    font.family: root.bar ? root.bar.fontFamily : Style.font.family
                    font.pixelSize: Style.font.bodySmall
                  }
                }
              }
            }
            // Boot policy and the last-boot log are one section. No divider
            // above it: two bordered boxes already read as separate.
            Item {
              id: bootGroup
              width: content.width
              height: bootBox.height + bootLegend.height / 2

              Rectangle {
                id: bootBox
                y: bootLegend.height / 2
                width: parent.width
                height: bootBody.implicitHeight + Style.space(18)
                radius: Style.space(6)
                color: "transparent"
                border.color: root.ink
                border.width: 1
              }

              BorderLegend {
                id: bootLegend
                title: "RELAUNCH ON BOOT"
              }

              Column {
                id: bootBody
                x: Style.space(10)
                y: bootBox.y + Style.space(9)
                width: parent.width - Style.space(20)
                spacing: Style.space(6)

                Text {
                  textFormat: Text.PlainText
                  width: parent.width
                  text: root.bootLabel()
                  color: root.ink
                  opacity: 0.75
                  font.family: root.bar ? root.bar.fontFamily : Style.font.family
                  font.pixelSize: Style.font.bodySmall
                  wrapMode: Text.WordWrap
                }

                Flow {
                  width: parent.width
                  spacing: Style.space(6)
                  Chip {
                    visible: !root.boot.disabled && !root.boot.skipOnce
                    label: "Skip next boot"
                    onClicked: root.run(["boot-skip", "--json"], "Skipping the next boot.")
                  }
                  Chip {
                    visible: root.boot.disabled || root.boot.skipOnce
                    label: "Enable on boot"
                    onClicked: root.run(["boot-enable", "--json"], "Enabled on boot.")
                  }
                  Chip {
                    visible: !root.boot.disabled
                    label: "Disable until re-enabled"
                    onClicked: root.run(["boot-disable", "--json"], "Disabled on boot.")
                  }
                }

                // Waits around each launch of one app's windows (a browser and
                // its web apps, or two of one terminal): different apps start
                // side by side. Changing them is reviewed like any other change.
                Flow {
                  width: parent.width
                  spacing: Style.space(10)
                  NumberField {
                    label: "Wait before (ms)"
                    from: 0
                    to: 60000
                    stepSize: 50
                    value: root.draftPreMs
                    foreground: root.ink
                    fontFamily: root.bar ? root.bar.fontFamily : Style.font.family
                    onModified: function(v) { root.draftPreMs = v }
                  }
                  NumberField {
                    label: "Wait after (ms)"
                    from: 0
                    to: 60000
                    stepSize: 50
                    value: root.draftPostMs
                    foreground: root.ink
                    fontFamily: root.bar ? root.bar.fontFamily : Style.font.family
                    onModified: function(v) { root.draftPostMs = v }
                  }
                }

                Chip {
                  visible: root.delaysEdited
                  label: "Apply delays"
                  inactive: root.committing
                  onClicked: root.txn.start(["set-delays", "--pre", String(root.draftPreMs),
                    "--post", String(root.draftPostMs), "--json"], "", "Saving delays…", "Delays saved.", "", {
                      what: "Apply the launch delays",
                      nothingText: "no change. Those are already the delays."
                    })
                }

                Text {
                  textFormat: Text.PlainText
                  width: parent.width
                  visible: root.lastBoot !== null
                  text: root.lastBootLink()
                  color: root.ink
                  opacity: 0.7
                  font.family: root.bar ? root.bar.fontFamily : Style.font.family
                  font.pixelSize: Style.font.caption
                  wrapMode: Text.WordWrap
                }

                Chip {
                  label: "View last boot log"
                  onClicked: root.openLog()
                }
              }
            }

            // Boot tools: the installed package and its lifecycle (CoreInstall.js).
            Rectangle { width: parent.width; height: 1; color: root.ink; opacity: 0.2 }
            Column {
              width: parent.width
              spacing: Style.space(6)
              Text {
                textFormat: Text.PlainText
                width: parent.width
                text: "Boot tools · " + (root.coreVersion !== "" ? root.corePackage + " " + root.coreVersion : "development install")
                color: root.ink
                font.family: root.bar ? root.bar.fontFamily : Style.font.family
                font.pixelSize: Style.font.bodySmall
                font.bold: true
              }
              Text {
                visible: root.coreMessage !== "" && root.corePhase !== "checking"
                textFormat: Text.PlainText
                width: parent.width
                text: root.coreMessage
                color: root.coreMessageIsError ? Commons.Color.urgent : root.ink
                opacity: root.coreMessageIsError ? 1 : 0.8
                wrapMode: Text.Wrap
                font.family: root.bar ? root.bar.fontFamily : Style.font.family
                font.pixelSize: Style.font.caption
              }
              Flow {
                width: parent.width
                spacing: Style.space(8)
                Chip {
                  visible: root.corePhase === "update-available"
                  label: "Update"
                  inactive: root.coreBusy
                  onClicked: if (!root.coreBusy) root.core.update()
                }
                Chip {
                  label: root.corePhase === "checking-updates" ? "Checking…"
                    : root.corePhase === "updating" ? "Updating…" : "Check for updates"
                  inactive: root.coreBusy
                  onClicked: if (!root.coreBusy) root.core.checkUpdates()
                }
                Chip {
                  label: root.corePhase === "reinstalling" ? "Reinstalling…" : "Reinstall"
                  inactive: root.coreBusy
                  onClicked: if (!root.coreBusy) root.core.requestReinstall()
                }
                Chip {
                  label: root.corePhase === "uninstalling" ? "Removing…" : "Uninstall"
                  danger: true
                  inactive: root.coreBusy
                  onClicked: if (!root.coreBusy) root.core.requestUninstall()
                }
                // Out of a package change whose terminal outlived its process.
                Chip {
                  visible: root.corePhase === "updating" || root.corePhase === "reinstalling" || root.corePhase === "uninstalling"
                  label: "I'm done"
                  inactive: root.coreChecking
                  onClicked: if (!root.coreChecking) root.core.check()
                }
              }
              // Asked before a reinstall or a removal; nothing runs until answered.
              Rectangle {
                visible: root.coreConfirm !== ""
                width: parent.width
                height: visible ? coreConfirmBody.implicitHeight + Style.space(20) : 0
                radius: Style.space(6)
                color: "transparent"
                border.color: root.coreConfirm === "uninstall" ? Commons.Color.urgent : root.ink
                border.width: 1
                Column {
                  id: coreConfirmBody
                  x: Style.space(10)
                  y: Style.space(10)
                  width: parent.width - Style.space(20)
                  spacing: Style.space(8)
                  Text {
                    textFormat: Text.PlainText
                    width: parent.width
                    text: root.coreConfirm === "uninstall"
                      ? "Remove the boot tools? Your apps will no longer restore at login. Your saved layout and signing key are kept, so reinstalling brings them back. This does not remove the Relaunch widget."
                      : "Rebuild and reinstall the boot tools? This runs in a terminal window and asks for your password."
                    color: root.ink
                    wrapMode: Text.Wrap
                    font.family: root.bar ? root.bar.fontFamily : Style.font.family
                    font.pixelSize: Style.font.bodySmall
                  }
                  Row {
                    spacing: Style.space(10)
                    Chip {
                      label: root.coreConfirm === "uninstall" ? "Remove boot tools" : "Reinstall"
                      danger: root.coreConfirm === "uninstall"
                      onClicked: root.core.confirm()
                    }
                    Chip {
                      label: "Cancel"
                      onClicked: root.core.cancelConfirm()
                    }
                  }
                }
              }
            }

            Text {
              textFormat: Text.PlainText
              width: parent.width
              visible: root.statusText.length > 0 && !root.statusIsError
              text: root.statusText
              color: root.ink
              opacity: 0.75
              font.family: root.bar ? root.bar.fontFamily : Style.font.family
              font.pixelSize: Style.font.bodySmall
              wrapMode: Text.WordWrap
            }

            // Uninstall stands alone at the bottom, centred and in no section.
            Item {
              width: parent.width
              height: removeChip.height
              Chip {
                id: removeChip
                anchors.horizontalCenter: parent.horizontalCenter
                label: root.confirmRemove ? "Confirm removing relaunch" : "Remove relaunch permanently"
                danger: true
                onClicked: {
                  // The armed state lives in the button's own label. It used to
                  // also set statusText, but that renders in a wrapping Text
                  // above this chip, which pushed the button below the panel
                  // edge -- the one control the user then needed to reach.
                  if (!root.confirmRemove) {
                    root.confirmRemove = true
                    root.statusIsError = false
                    disarmRemove.restart()
                    return
                  }
                  disarmRemove.stop()
                  root.confirmRemove = false
                  root.run(["uninstall", "--yes", "--json"], "Relaunch removed.")
                }
              }
            }
          }
        }
      }
    }

    // The human review surface (§3). The polkit dialog is NOT trusted; this is.
    // Shown while a commit is prepared: the service's plain-language summary
    // (which apps will open at login), and under a collapsed Details the
    // service's escaped diff, both verbatim PlainText. The binding to this
    // panel's session is the service's (the txid on relaunchctl's connection);
    // nothing here asks the user to compare codes.
    // It owns the keyboard while shown: the command editor loses focus, and
    // Escape is the review's Cancel; no other key reaches anything behind it.
    Item {
      id: reviewOverlay
      anchors.fill: parent
      visible: root.reviewVisible
      z: 1000
      onVisibleChanged: {
        if (visible) {
          root.execEditClass = ""
          reviewOverlay.forceActiveFocus()
        } else {
          keyCatcher.forceActiveFocus()
        }
      }
      Keys.onPressed: function(event) {
        if (event.key === Qt.Key_Escape) root.txn.cancel()
        event.accepted = true
      }

      Rectangle {
        anchors.fill: parent
        color: "#000000"
        opacity: 0.55
        // Swallow clicks so nothing behind the review reacts.
        MouseArea { anchors.fill: parent; hoverEnabled: true }
      }

      Rectangle {
        id: reviewCard
        anchors.centerIn: parent
        width: Math.min(parent.width - Style.space(20), Style.space(380))
        height: Math.min(parent.height - Style.space(20), Style.space(470))
        radius: Style.space(8)
        color: root.bar ? root.bar.background : Commons.Color.background
        border.color: root.ink
        border.width: 1

        Column {
          id: reviewColumn
          anchors.fill: parent
          anchors.margins: Style.space(14)
          spacing: Style.space(10)

          Text {
            id: reviewTitle
            textFormat: Text.PlainText
            width: parent.width
            text: "Authorize launch changes"
            color: root.ink
            font.family: root.bar ? root.bar.fontFamily : Style.font.family
            font.pixelSize: Style.font.subtitle
            font.bold: true
          }

          // What the change means: the service's summary and, when opened, the
          // service's diff, verbatim. PlainText so no markup, control or bidi
          // character can hide or reorder a line (§3.1). Scrolls; never
          // re-rendered by the panel.
          Flickable {
            id: reviewFlick
            width: parent.width
            height: reviewColumn.height - reviewTitle.height - reviewHintText.height - reviewButtons.height - reviewColumn.spacing * 3
            contentWidth: width
            contentHeight: reviewBody.implicitHeight
            clip: true
            boundsBehavior: Flickable.StopAtBounds
            interactive: contentHeight > height

            Column {
              id: reviewBody
              width: reviewFlick.width
              spacing: Style.space(8)

              Text {
                textFormat: Text.PlainText
                width: parent.width
                text: root.reviewSummary
                color: root.ink
                wrapMode: Text.Wrap
                font.family: root.bar ? root.bar.fontFamily : Style.font.family
                font.pixelSize: Style.font.body
              }

              // Not part of the change: what the capture left out.
              Text {
                visible: root.reviewMerged !== ""
                textFormat: Text.PlainText
                width: parent.width
                text: root.reviewMerged
                color: root.ink
                opacity: 0.8
                wrapMode: Text.Wrap
                font.family: root.bar ? root.bar.fontFamily : Style.font.family
                font.pixelSize: Style.font.bodySmall
              }

              Text {
                textFormat: Text.PlainText
                text: (root.reviewDetailsOpen ? "▾ " : "▸ ") + "Details"
                color: root.ink
                opacity: 0.7
                font.family: root.bar ? root.bar.fontFamily : Style.font.family
                font.pixelSize: Style.font.bodySmall
                MouseArea {
                  anchors.fill: parent
                  cursorShape: Qt.PointingHandCursor
                  onClicked: root.reviewDetailsOpen = !root.reviewDetailsOpen
                }
              }

              Rectangle {
                visible: root.reviewDetailsOpen
                width: parent.width
                height: diffText.implicitHeight + Style.space(16)
                radius: Style.space(6)
                color: "transparent"
                border.color: root.ink
                border.width: 1
                opacity: 0.9
                Text {
                  textFormat: Text.PlainText
                  id: diffText
                  anchors.fill: parent
                  anchors.margins: Style.space(8)
                  text: root.reviewDiff
                  color: root.ink
                  wrapMode: Text.Wrap
                  font.family: root.bar ? root.bar.fontFamily : Style.font.family
                  font.pixelSize: Style.font.caption
                }
              }
            }
          }

          Text {
            id: reviewHintText
            textFormat: Text.PlainText
            width: parent.width
            text: root.authorizing ? "Confirm in the system dialog to finish." : root.reviewHint
            color: root.ink
            opacity: 0.8
            wrapMode: Text.WordWrap
            font.family: root.bar ? root.bar.fontFamily : Style.font.family
            font.pixelSize: Style.font.bodySmall
          }

          Row {
            id: reviewButtons
            width: parent.width
            spacing: Style.space(10)
            Chip {
              label: root.authorizing ? "Waiting for approval…" : "Authorize"
              inactive: root.authorizing
              onClicked: root.txn.authorize()
            }
            Chip {
              label: "Cancel"
              danger: true
              onClicked: root.txn.cancel()
            }
          }
        }
      }
    }

    // The acknowledgement of a promptless change (see showToast): over the
    // panel, so it is seen wherever the list is scrolled, and gone after
    // toastTimer.interval.
    Rectangle {
      id: toast
      visible: root.toastText !== ""
      z: 1001
      anchors.horizontalCenter: parent.horizontalCenter
      anchors.bottom: parent.bottom
      anchors.bottomMargin: Style.space(14)
      width: toastLabel.implicitWidth + Style.space(28)
      height: toastLabel.implicitHeight + Style.space(14)
      radius: height / 2
      color: root.bar ? root.bar.background : Commons.Color.background
      border.color: Commons.Color.accent
      border.width: 1
      Text {
        id: toastLabel
        textFormat: Text.PlainText
        anchors.centerIn: parent
        text: root.toastText
        color: root.ink
        font.family: root.bar ? root.bar.fontFamily : Style.font.family
        font.pixelSize: Style.font.body
        font.bold: true
      }
    }
  }
}
