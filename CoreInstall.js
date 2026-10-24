// The boot-tools install flow (design: install-boot-tools-button-design.md)
// and its lifecycle once installed (boot-tools-lifecycle-buttons-design.md),
// kept out of Panel.qml so it runs under node --test. Panel.qml owns the
// processes; this owns what they mean.
//
// The panel works only once the root-owned system package (`omarchy-relaunch`)
// is installed and its login step is wired into hyprland.lua. Phases:
//   "checking"   the first probe is out; the normal panel shows meanwhile
//   "missing"    the core is not installed: the explainer and the button
//   "installing" the package install runs in a visible terminal
//   "wiring"     the core is in; `ensure-hooks --anchor` runs
//   "unwired"    the core is in but wiring failed: the button retries it
//   "ready"      the normal panel, with the Boot tools section
// From "ready", the lifecycle (the normal panel stays up throughout):
//   "checking-updates" the headless update check runs
//   "update-available" the AUR has a newer version: the Update action
//   "updating" / "reinstalling" / "uninstalling"
//                the package change runs in a visible terminal; an update or
//                reinstall then wires again, an uninstall ends in "missing"
//
// Only a probe of the installed files decides a terminal's attempt: a terminal
// exits 0 even when the user aborted the build, so its exit is never the
// result. The probe reports the files (core), the pacman version and a stamp
// of relaunchd's file (new on every reinstall): an update is done when the
// version changed, a reinstall when the stamp did, an uninstall when the core
// is gone.
//
// fx = { probe(op), spawnInstall(op), spawnManage(op, kind), checkUpdates(op),
//        spawnWire(op), ready(), render(s) }; kind is "update", "reinstall" or
// "uninstall".
// Every reply carries the op it was started under; one from an op that is no
// longer current (a straggling terminal, a probe overtaken by a newer one) is
// ignored, so nothing fires twice.

function create(fx) {
  var s = {
    phase: "checking",
    // Whether Omarchy's package tools and a terminal launcher are here: the
    // button can install only through them.
    canInstall: false,
    message: "",
    isError: false,
    op: 0,
    probing: false,
    // The install terminal has exited (this op); a failed probe now means the
    // install did not happen.
    installEnded: false,
    // From the last probe: the installed package's version (null when pacman
    // does not own the files: a development install) and relaunchd's stamp.
    version: null,
    stamp: null,
    // { from, to } while "update-available".
    update: null,
    // "reinstall" or "uninstall" while the panel asks first; "" otherwise.
    confirm: "",
    // The version and stamp when a package change started.
    before: null,
    // What the wiring follows: "install", "update" or "reinstall".
    wiredFor: "install"
  }

  // The terminal phases, and what each runs.
  var TERMINAL = { installing: "install", updating: "update", reinstalling: "reinstall", uninstalling: "uninstall" }

  function render() {
    fx.render(s)
  }

  function busy() {
    return s.probing || TERMINAL.hasOwnProperty(s.phase) || s.phase === "wiring" ||
      s.phase === "checking-updates"
  }

  // The lifecycle acts on a package pacman installed, from the ready panel.
  function manageable() {
    return (s.phase === "ready" || s.phase === "update-available") && !busy()
  }

  function ready(text, isError) {
    s.phase = "ready"
    s.update = null
    s.message = text || ""
    s.isError = !!isError
  }

  var NOT_A_PACKAGE = "These boot tools were not installed by pacman (a development install), so Relaunch can't update, reinstall or remove them here."

  // Start a package change in the terminal.
  function manage(kind, phase, text) {
    s.op += 1
    s.phase = phase
    s.confirm = ""
    s.installEnded = false
    s.before = { version: s.version, stamp: s.stamp }
    s.message = text
    s.isError = false
    fx.spawnManage(s.op, kind)
  }

  // Whether this probe shows the package change done.
  function changed(kind, r) {
    if (kind === "uninstall") return !r.core
    if (!r.core) return false
    if (kind === "update") return !!r.version && r.version !== s.before.version
    return !!r.stamp && r.stamp !== s.before.stamp
  }

  function probe() {
    s.probing = true
    fx.probe(s.op)
  }

  function wire(why) {
    s.wiredFor = why || "install"
    s.op += 1
    s.phase = "wiring"
    s.message = "Wiring the login step into your Hyprland config…"
    s.isError = false
    fx.spawnWire(s.op)
  }

  function missing(text, isError) {
    s.phase = "missing"
    s.message = text || ""
    s.isError = !!isError
  }

  return {
    state: s,

    // Probe on panel open, and from "Check again". While installing it is the
    // user saying the terminal is done: a failed probe then ends the attempt.
    // Nothing while a probe is out or the wiring runs.
    check: function () {
      if (s.probing || s.phase === "wiring" || s.phase === "checking-updates") return false
      if (TERMINAL.hasOwnProperty(s.phase)) s.installEnded = true
      probe()
      render()
      return true
    },

    // r = { core: bool, canInstall: bool, version: string|null, stamp: string|null }
    probeEnded: function (op, r) {
      if (op !== s.op || !s.probing) return
      s.probing = false
      s.canInstall = !!(r && r.canInstall)
      var core = !!(r && r.core)
      s.version = core && r.version ? String(r.version) : null
      s.stamp = core && r.stamp ? String(r.stamp) : null
      var kind = TERMINAL[s.phase]
      if (kind && kind !== "install") {
        var rr = { core: core, version: s.version, stamp: s.stamp }
        if (changed(kind, rr)) {
          if (kind === "uninstall")
            missing("Boot tools removed. Your saved layout and signing key are kept: Install boot tools brings them back.", false)
          else wire(kind)
        } else if (s.installEnded) {
          if (!core && kind !== "uninstall")
            missing("The boot tools are no longer installed: the " + kind + " did not finish. Install them again.", true)
          else if (kind === "uninstall")
            ready("The boot tools are still installed. The removal did not finish; see the terminal window.", true)
          else ready("The " + kind + " did not finish; see the terminal window, then try again.", true)
        }
        render()
        return
      }
      if (s.phase === "installing") {
        if (core) wire()
        else if (s.installEnded)
          missing("The boot tools are still not installed. The install did not finish; see the terminal window, then try again.", true)
      } else if (core) {
        // Found already installed when the panel opened: nothing to do. Found
        // after the user asked for the install (Check again, once a detached
        // terminal finished): the wiring is still owed.
        if (s.phase === "missing") wire()
        else if (s.phase === "checking") s.phase = "ready"
      } else if (s.phase !== "missing") {
        missing("", false)
      }
      render()
    },

    // The button. In "missing" it installs; in "unwired" it retries the wiring.
    install: function () {
      if (busy()) return false
      if (s.phase === "unwired") {
        wire()
        render()
        return true
      }
      if (s.phase !== "missing") return false
      if (!s.canInstall) {
        missing("Relaunch can't install its boot tools here: it needs Omarchy's package tools (omarchy pkg aur add, with yay) and a terminal. See the README to install the omarchy-relaunch package yourself.", true)
        render()
        return false
      }
      s.op += 1
      s.phase = "installing"
      s.installEnded = false
      s.message = "Installing in the terminal window. Enter your password there when asked."
      s.isError = false
      fx.spawnInstall(s.op)
      render()
      return true
    },

    // While the terminal is open, a probe now and then: it notices the install
    // even if the terminal handed off to a running instance and exited early.
    poll: function () {
      if (!TERMINAL.hasOwnProperty(s.phase) || s.probing) return
      probe()
      render()
    },

    // r = { started: bool, stillOpen: bool }. The terminal closed, never
    // opened, or could not open because the last attempt's is still open.
    installEnded: function (op, r) {
      if (op !== s.op || !TERMINAL.hasOwnProperty(s.phase)) return
      var back = s.phase === "installing" ? missing : ready
      if (r && r.stillOpen) {
        back("The terminal window from the last attempt is still open. Finish or close it, then try again.", true)
        render()
        return
      }
      if (!r || !r.started) {
        back("Could not open a terminal to run the " + TERMINAL[s.phase] + ".", true)
        render()
        return
      }
      s.installEnded = true
      if (!s.probing) probe()
      render()
    },

    // r = { started, exitCode, stderr }
    wireEnded: function (op, r) {
      if (op !== s.op || s.phase !== "wiring") return
      if (r && r.started && r.exitCode === 0) {
        ready(s.wiredFor === "update" ? "Boot tools updated to " + s.version + "."
          : s.wiredFor === "reinstall" ? "Boot tools reinstalled."
          : "Boot tools installed. Save a layout and it will restore at login.", false)
        fx.ready()
      } else {
        s.phase = "unwired"
        var why = r && r.stderr ? String(r.stderr) : (r && r.started ? "exit " + r.exitCode : "could not run it")
        s.message = "The boot tools are installed, but wiring the login step failed (" + why + "). Try again."
        s.isError = true
      }
      render()
    },

    // Check for Updates: headless (no terminal, no password).
    checkUpdates: function () {
      if (!manageable()) return false
      if (!s.version) {
        ready(NOT_A_PACKAGE, false)
        render()
        return false
      }
      s.op += 1
      s.phase = "checking-updates"
      s.confirm = ""
      s.message = "Checking the AUR for a newer version…"
      s.isError = false
      fx.checkUpdates(s.op)
      render()
      return true
    },

    // r = { result: "newer"|"current"|"unreachable"|"notpkg"|"noaur"|"nohelper"|
    //       "nocurl"|"nojq"|"fetchfailed"|"badreply", installed, latest }
    updatesChecked: function (op, r) {
      if (op !== s.op || s.phase !== "checking-updates") return
      var res = r && r.result
      if (res === "newer") {
        s.phase = "update-available"
        s.update = { from: String(r.installed), to: String(r.latest) }
        s.message = "Update available: " + s.update.from + " → " + s.update.to
        s.isError = false
      } else if (res === "current") {
        ready("Up to date (" + String(r.installed) + ").", false)
      } else if (res === "unreachable") {
        ready("Couldn't reach the AUR. Try again later.", true)
      } else if (res === "notpkg") {
        ready(NOT_A_PACKAGE, false)
      } else if (res === "noaur") {
        ready("The AUR has no omarchy-relaunch package.", true)
      } else if (res === "nohelper") {
        ready("Updating needs yay, Omarchy's AUR helper.", true)
      } else if (res === "nocurl") {
        ready("The update check needs curl.", true)
      } else if (res === "nojq") {
        ready("The update check needs jq.", true)
      } else if (res === "fetchfailed") {
        ready("Couldn't check for updates: the request to the AUR failed. Try again later.", true)
      } else if (res === "badreply") {
        ready("Couldn't check for updates: the AUR's reply could not be read. Try again later.", true)
      } else {
        ready("The update check failed.", true)
      }
      render()
    },

    // Update: only once a check found a newer version.
    update: function () {
      if (s.phase !== "update-available" || busy()) return false
      manage("update", "updating", "Updating in the terminal window. Enter your password there when asked.")
      render()
      return true
    },

    // Reinstall and Uninstall ask first (confirm / cancelConfirm).
    requestReinstall: function () { return this.ask("reinstall") },
    requestUninstall: function () { return this.ask("uninstall") },
    ask: function (what) {
      if (!manageable()) return false
      if (!s.version) {
        ready(NOT_A_PACKAGE, false)
        render()
        return false
      }
      if (!s.canInstall) {
        ready("This needs Omarchy's package tools (omarchy, yay) and a terminal.", true)
        render()
        return false
      }
      s.confirm = what
      render()
      return true
    },
    confirm: function () {
      var what = s.confirm
      if (what === "" || !manageable()) return false
      if (what === "reinstall")
        manage("reinstall", "reinstalling", "Reinstalling in the terminal window. Enter your password there when asked.")
      else
        manage("uninstall", "uninstalling", "Removing the boot tools in the terminal window. Enter your password there when asked.")
      render()
      return true
    },
    cancelConfirm: function () {
      if (s.confirm === "") return
      s.confirm = ""
      render()
    },

    // A normal command found the engine missing: show the install surface.
    engineMissing: function () {
      if ((s.phase !== "ready" && s.phase !== "update-available") || busy()) return
      s.phase = "checking"
      probe()
      render()
    }
  }
}

if (typeof module !== "undefined") module.exports = { create: create }
