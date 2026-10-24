// The one lifecycle of a change to the signed list (spec §3/§5; INV-4, INV-10):
// the engine builds it -> relaunchctl -> install. An EXPANDING change (capture,
// import, set-exec; `kind: "commit"`) is prepared, reviewed in the panel and
// authorized; a REDUCING one (drop; `kind: "reduce"`) only stops launches and
// is applied without a prompt (§5.4).
//
// A pure state machine. Panel.qml forwards process signals in and performs the
// effects requested through `fx`; nothing here touches a process directly.
//
// - Every input names the attempt it belongs to. live(attempt) is the single
//   gate: input from an abandoned or already-resolved attempt is a no-op.
// - resolve() is the single terminal transition (committed, nothing, error,
//   cancel). Only it clears the review, and it terminates the attempt's child.
//   Clicking Authorize does NOT clear the review: it stays up while the system
//   dialog asks for the credential AFTER the click (INV-10).
// - Ownership (`committing`) is released only once the attempt is resolved AND
//   its child has ended, so a new attempt never overlaps a draining one.
// - The proposal arrives from the engine as JSON text and is forwarded as-is;
//   the panel never parses or rebuilds it. The engine's `kind` alone decides
//   which relaunchctl mode runs; a reply without one is refused.

function create(fx) {
  var s = {
    attempt: 0,
    resolved: true,
    committing: false,
    authorizing: false,
    review: null, // { summary, diff } from Prepare until the attempt resolves
    // [{ label, workspace }]: pins a proposal would remove, held for the
    // user's answer before anything is sent (null when there is no question).
    confirmDrop: null,
    // What a capture left out because each window was an identical duplicate
    // of one it proposes (same app, workspace and command), as one sentence;
    // "" when it left out none. Shown with the review, and with a no-op.
    mergedText: "",
    hint: "", // the review's instruction for this attempt
    child: null, // { attempt, kind: "propose" | "commit", running }
    mode: "", // "commit" | "reduce": the relaunchctl mode this attempt runs
    okText: "",
    what: "", // the action, as the user would name it ("Change the launch command for X")
    nothingText: "", // what to say when the change turns out to change nothing
    outcome: "", // "committed" | "nothing" | "error" | "cancelled", once resolved
    // A result the user must see and act on, named for its action: shown
    // until the next action. Every outcome but a commit or a cancel leaves one.
    notice: "",
    noticeIsError: false,
    frame: "",
    status: "",
    statusIsError: false
  }

  function live(attempt) {
    return attempt === s.attempt && !s.resolved
  }

  function render() {
    fx.render(s)
  }

  function resolve(text, isError, outcome) {
    if (s.resolved) return
    s.resolved = true
    s.authorizing = false
    s.review = null
    s.confirmDrop = null
    s.status = text
    s.statusIsError = isError
    s.outcome = outcome || (isError ? "error" : "committed")
    if (s.outcome === "error") {
      s.notice = (s.what ? s.what + " failed: " : "") + text
      s.noticeIsError = true
    } else if (s.outcome === "nothing") {
      s.notice = (s.what ? s.what + ": " : "") + (s.nothingText || text) +
        (s.mergedText ? " " + s.mergedText : "")
      s.noticeIsError = false
    }
    // Terminating the helper drops its bus connection, which the service
    // observes as a disconnect and invalidates the transaction.
    if (s.child && s.child.running) fx.terminate(s.child.attempt, s.child.kind)
    // With no helper left to drain (an attempt waiting on the user's answer),
    // ownership is released here: nothing else will.
    else {
      s.committing = false
      s.child = null
    }
  }

  function spawn(kind, args, stdinText) {
    s.child = { attempt: s.attempt, kind: kind, running: true }
    if (kind === "propose") fx.spawnPropose(s.attempt, args, stdinText)
    else fx.spawnCommit(s.attempt, s.mode)
  }

  function childEnded(attempt, kind) {
    if (s.child && s.child.attempt === attempt && s.child.kind === kind) s.child.running = false
    if (attempt === s.attempt && s.resolved && !(s.child && s.child.running)) {
      s.committing = false
      s.child = null
    }
  }

  // An engine reply is usable only if the engine exited 0 AND said ok:true
  // with a known kind and its text (an {"ok":false} body must not start
  // relaunchctl, and its error must reach the user). The frame is built from
  // the engine's text without parsing it: a commit wraps the proposal with its
  // base; a reduce request is relayed as-is, and must be one line.
  function engineResult(r) {
    var p = null
    try { p = JSON.parse(String(r.stdout || "")) } catch (e) { p = null }
    if (r.exitCode === 0 && p && p.ok === true && typeof p.base === "string") {
      if (p.kind === "commit" && typeof p.proposalJson === "string") {
        var named = function (list) {
          return Array.isArray(list) ? list.filter(function (d) {
            return d && typeof d.label === "string"
          }) : []
        }
        return { ok: true, mode: "commit", dropping: named(p.dropping), merged: named(p.merged),
          frame: '{"base":' + JSON.stringify(p.base) + ',"proposal":' + p.proposalJson + '}' }
      }
      if (p.kind === "reduce" && typeof p.reduceJson === "string" && p.reduceJson.indexOf("\n") === -1)
        return { ok: true, mode: "reduce", frame: p.reduceJson }
      return { ok: false, message: "Bad engine output." }
    }
    var msg = p && p.ok === false && p.error ? String(p.error) : String(r.stderr || "").trim()
    return { ok: false, message: msg !== "" ? msg : "The engine failed." }
  }

  // "2 open windows were identical duplicates and merged: foot (workspace 2),
  // foot (workspace 2)."
  function mergedSentence(merged) {
    if (!merged || merged.length === 0) return ""
    var names = merged.map(function (d) { return d.label + " (workspace " + d.workspace + ")" })
    return (merged.length === 1 ? "1 open window was an identical duplicate" :
      merged.length + " open windows were identical duplicates") + " and merged: " + names.join(", ") + "."
  }

  return {
    state: s,
    live: live,

    // Run one engine edit (`args`, with `stdinText` on its stdin) and carry the
    // change it builds through relaunchctl. `okText` is shown once applied;
    // `hint` is the review's instruction, should the change need one; `opts`
    // names the action (`what`) and what a no-op means (`nothingText`).
    // Returns whether it started: one change runs at a time, and a refused one
    // says so.
    start: function (args, stdinText, busyText, okText, hint, opts) {
      var o = opts || {}
      if (s.committing) {
        s.notice = (o.what ? o.what + ": " : "") +
          "another change is still being made. Finish or cancel it, then try again."
        s.noticeIsError = true
        render()
        return false
      }
      s.attempt += 1
      s.what = o.what || ""
      s.nothingText = o.nothingText || ""
      s.mergedText = ""
      s.outcome = ""
      s.notice = ""
      s.noticeIsError = false
      s.resolved = false
      s.committing = true
      s.authorizing = false
      s.review = null
      s.mode = ""
      s.okText = okText || "Saved and authorized."
      s.hint = hint || "Approve only if you just made this change in Relaunch."
      s.status = busyText || "Working…"
      s.statusIsError = false
      spawn("propose", args, stdinText || "")
      render()
      return true
    },

    // A result the panel found before any change started (an empty command).
    notify: function (text, isError) {
      s.notice = text
      s.noticeIsError = isError
      render()
    },

    dismissNotice: function () {
      s.notice = ""
      s.noticeIsError = false
      render()
    },

    // The answer to a confirmDrop question: go on and replace...
    confirmReplace: function () {
      if (s.confirmDrop === null || s.resolved) return
      s.confirmDrop = null
      s.status = "Preparing…"
      spawn("commit")
      render()
    },

    // ...or keep the boot list as it is: the attempt ends, nothing was sent.
    keepPins: function () {
      if (s.confirmDrop === null || s.resolved) return
      resolve("Kept the boot list as it is.", false, "cancelled")
      render()
    },

    // "Add open windows": the open windows added to the boot list, every
    // other pin kept.
    startMerge: function () {
      return this.start(["propose", "--merge", "--json"], "", "Capturing windows…", "Added and authorized.",
        "Approve only if you just clicked Add open windows.", {
          what: "Add open windows",
          nothingText: "nothing to add. Every open window is already in the boot list."
        })
    },

    startSave: function () {
      return this.start(["propose", "--json"], "", "Capturing windows…", "Saved and authorized.",
        "Approve only if you just clicked Capture open windows.", {
          what: "Capture open windows",
          nothingText: "nothing to change. The boot list already matches the open windows."
        })
    },

    // r = { started, exitCode, stdout, stderr }
    proposeEnded: function (attempt, r) {
      var wasLive = live(attempt)
      if (wasLive) {
        if (!r.started) {
          resolve("Could not run the engine.", true)
        } else {
          var c = engineResult(r)
          if (!c.ok) {
            resolve(c.message, true)
          } else {
            s.mode = c.mode
            s.frame = c.frame
            s.mergedText = mergedSentence(c.merged)
            s.statusIsError = false
            childEnded(attempt, "propose")
            if (c.dropping && c.dropping.length > 0) {
              // A change that would remove pins asks first; nothing is sent
              // until the user answers (confirmReplace / keepPins).
              s.confirmDrop = c.dropping
              s.status = "Waiting for your answer…"
              render()
              return
            }
            s.status = c.mode === "commit" ? "Preparing…" : "Applying…"
            spawn("commit")
            render()
            return
          }
        }
      }
      childEnded(attempt, "propose")
      render()
    },

    commitStarted: function (attempt) {
      if (!live(attempt)) {
        fx.terminate(attempt, "commit")
        return
      }
      // stdin stays open after this: the decision travels the same pipe.
      fx.writeCommit(attempt, s.frame + "\n")
    },

    commitLine: function (attempt, line) {
      if (!live(attempt)) return
      var ev
      try { ev = JSON.parse(String(line || "").trim()) } catch (e) { return }
      if (!ev || typeof ev !== "object") return
      if (ev.event === "prepared") {
        // Only an expanding change is reviewed; a reduce is never authorized.
        if (s.mode !== "commit" || s.review !== null) return
        s.review = { summary: String(ev.summary || ""), diff: String(ev.diff || "") }
        s.status = "Review the apps below, then Authorize."
        s.statusIsError = false
      } else if (ev.event === "committed") {
        var config = String(ev.config || "")
        resolve(s.okText, false)
        if (config !== "") fx.install(config, s.okText, s.mode)
      } else if (ev.event === "nothing") {
        resolve("Nothing to authorize — the list already matches.", false, "nothing")
      } else if (ev.event === "error") {
        resolve(String(ev.message || "Authorization failed."), true)
      } else {
        return
      }
      render()
    },

    // r = { started, stderr }
    commitEnded: function (attempt, r) {
      if (live(attempt)) {
        var err = String(r.stderr || "").trim()
        resolve(!r.started ? "Could not run relaunchctl." : (err !== "" ? err : "Authorization ended without a result."), true)
      }
      childEnded(attempt, "commit")
      render()
    },

    authorize: function () {
      var c = s.child
      // A reduce never has a review (its `prepared` is ignored), so this one
      // check also rules out authorizing a reduce.
      if (!c || c.kind !== "commit" || !live(c.attempt) || s.review === null || s.authorizing) return
      fx.writeCommit(c.attempt, '{"decision":"authorize"}\n')
      s.authorizing = true
      s.status = "Confirm in the system dialog to finish."
      s.statusIsError = false
      render()
    },

    // The one cancel: Cancel button, Escape, or the panel closing.
    cancel: function () {
      if (!s.committing) return
      resolve("Cancelled.", false, "cancelled")
      render()
    }
  }
}

if (typeof module !== "undefined") module.exports = { create: create }
