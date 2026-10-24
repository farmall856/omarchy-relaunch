// Tests for TxnLifecycle.js (INV-4, INV-10; Sol #3/#4/#5/#10).
// Run: node --test tests/txn-lifecycle.test.js
"use strict"
const test = require("node:test")
const assert = require("node:assert/strict")
const path = require("node:path")
const { create } = require(path.join(__dirname, "..", "TxnLifecycle.js"))

function harness() {
  const log = []
  const fx = {
    render() {},
    spawnPropose: (a, args, stdinText) => log.push(["spawnPropose", a, args, stdinText]),
    spawnCommit: (a, mode) => log.push(["spawnCommit", a, mode]),
    writeCommit: (a, text) => log.push(["write", a, text]),
    terminate: (a, kind) => log.push(["terminate", a, kind]),
    install: (config, okText, mode) => log.push(["install", config, okText, mode])
  }
  const t = create(fx)
  const count = (op) => log.filter((e) => e[0] === op).length
  return { t, s: t.state, log, count }
}

const PROPOSAL = '{"staggerSeconds":0,"entries":[{"class":"__proto__","exec":"a\\\\b"}]}'
const okCapture = (extra) => ({
  started: true,
  exitCode: 0,
  stdout: JSON.stringify({ ok: true, kind: "commit", base: '{"v":2}', proposalJson: PROPOSAL }),
  stderr: "",
  ...extra
})
const line = (o) => JSON.stringify(o)

// Drive one attempt up to the review being shown.
function toReview(h) {
  h.t.startSave()
  const a = h.s.attempt
  h.t.proposeEnded(a, okCapture())
  h.t.commitStarted(a)
  h.t.commitLine(a, line({ event: "prepared", summary: "Relaunch will open this app at login:", diff: "+ added x" }))
  return a
}

test("happy path: frame forwards base + proposal bytes, commit installs once", () => {
  const h = harness()
  const a = toReview(h)
  const frame = h.log.find((e) => e[0] === "write")[2]
  assert.equal(frame, '{"base":"{\\"v\\":2}","proposal":' + PROPOSAL + "}\n")
  h.t.authorize()
  h.t.commitLine(a, line({ event: "committed", config: "SIGNED" }))
  assert.deepEqual(h.log.filter((e) => e[0] === "install"), [["install", "SIGNED", "Saved and authorized.", "commit"]])
  assert.equal(h.s.resolved, true)
  assert.equal(h.s.review, null)
  h.t.commitEnded(a, { started: true, stderr: "" })
  assert.equal(h.s.committing, false)
})

test("INV-10: Authorize keeps the summary and diff on screen", () => {
  const h = harness()
  toReview(h)
  h.t.authorize()
  assert.deepEqual(h.s.review, { summary: "Relaunch will open this app at login:", diff: "+ added x" })
  assert.equal(h.s.authorizing, true)
  assert.equal(h.s.status, "Confirm in the system dialog to finish.")
})

test("repeat Authorize is a no-op, not a second decision", () => {
  const h = harness()
  toReview(h)
  h.t.authorize()
  h.t.authorize()
  assert.equal(h.log.filter((e) => e[0] === "write" && e[2].includes("authorize")).length, 1)
})

test("Authorize before Prepare does nothing", () => {
  const h = harness()
  h.t.startSave()
  h.t.authorize()
  assert.equal(h.count("write"), 0)
})

test("Sol #4: cancel during authorization terminates the helper; a late committed installs nothing", () => {
  const h = harness()
  const a = toReview(h)
  h.t.authorize()
  h.t.cancel()
  assert.deepEqual(h.log.filter((e) => e[0] === "terminate"), [["terminate", a, "commit"]])
  h.t.commitLine(a, line({ event: "committed", config: "SIGNED" }))
  assert.equal(h.count("install"), 0)
  assert.equal(h.s.status, "Cancelled.")
})

test("cancel clears the review; a late prepared after cancel is ignored", () => {
  const h = harness()
  const a = toReview(h)
  h.t.cancel()
  assert.equal(h.s.review, null)
  h.t.commitLine(a, line({ event: "prepared", summary: "forged", diff: "x" }))
  assert.equal(h.s.review, null)
})

test("cancel during capture terminates propose and never starts a commit", () => {
  const h = harness()
  h.t.startSave()
  const a = h.s.attempt
  h.t.cancel()
  assert.deepEqual(h.log.filter((e) => e[0] === "terminate"), [["terminate", a, "propose"]])
  h.t.proposeEnded(a, okCapture())
  assert.equal(h.count("spawnCommit"), 0)
  assert.equal(h.s.committing, false)
})

test("Sol #10: {ok:false} capture never starts a commit and surfaces the engine error", () => {
  const h = harness()
  h.t.startSave()
  h.t.proposeEnded(h.s.attempt, {
    started: true,
    exitCode: 1,
    stdout: JSON.stringify({ ok: false, error: "parse overrides: bad" }),
    stderr: ""
  })
  assert.equal(h.count("spawnCommit"), 0)
  assert.equal(h.s.status, "parse overrides: bad")
  assert.equal(h.s.statusIsError, true)
  assert.equal(h.s.committing, false)
})

test("Sol #10: a non-zero exit with an ok-looking body still does not commit", () => {
  const h = harness()
  h.t.startSave()
  h.t.proposeEnded(h.s.attempt, okCapture({ exitCode: 2 }))
  assert.equal(h.count("spawnCommit"), 0)
})

test("capture without proposalJson (old engine contract) does not commit", () => {
  const h = harness()
  h.t.startSave()
  h.t.proposeEnded(h.s.attempt, {
    started: true, exitCode: 0, stdout: JSON.stringify({ ok: true, base: "", candidates: [] }), stderr: ""
  })
  assert.equal(h.count("spawnCommit"), 0)
  assert.equal(h.s.statusIsError, true)
})

test("Sol #10: failed starts are reported and release ownership", () => {
  const h = harness()
  h.t.startSave()
  h.t.proposeEnded(h.s.attempt, { started: false, exitCode: -1, stdout: "", stderr: "" })
  assert.equal(h.s.status, "Could not run the engine.")
  assert.equal(h.s.committing, false)

  const h2 = harness()
  h2.t.startSave()
  const a = h2.s.attempt
  h2.t.proposeEnded(a, okCapture())
  h2.t.commitEnded(a, { started: false, stderr: "" })
  assert.equal(h2.s.status, "Could not run relaunchctl.")
  assert.equal(h2.s.committing, false)
})

test("helper exiting without a terminal event is an error, not a silent success", () => {
  const h = harness()
  const a = toReview(h)
  h.t.authorize()
  h.t.commitEnded(a, { started: true, stderr: "relaunchctl: cannot reach relaunchd" })
  assert.equal(h.s.statusIsError, true)
  assert.match(h.s.status, /cannot reach relaunchd/)
  assert.equal(h.count("install"), 0)
})

test("Sol #10: ownership is held until the cancelled child ends; Save meanwhile is ignored", () => {
  const h = harness()
  const a = toReview(h)
  h.t.cancel()
  assert.equal(h.s.committing, true)
  h.t.startSave()
  assert.equal(h.s.attempt, a, "a new attempt must not start while the old child drains")
  h.t.commitEnded(a, { started: true, stderr: "" })
  assert.equal(h.s.committing, false)
  h.t.startSave()
  assert.equal(h.s.attempt, a + 1)
})

test("lines from an abandoned attempt never reach the next attempt", () => {
  const h = harness()
  const a = toReview(h)
  h.t.cancel()
  h.t.commitEnded(a, { started: true, stderr: "" })
  const b = toReview(h)
  h.t.commitLine(a, line({ event: "committed", config: "OLD" }))
  assert.equal(h.count("install"), 0)
  assert.equal(h.s.resolved, false)
  assert.equal(h.s.review.summary, "Relaunch will open this app at login:")
  h.t.commitLine(b, line({ event: "committed", config: "NEW" }))
  assert.deepEqual(h.log.filter((e) => e[0] === "install"), [["install", "NEW", "Saved and authorized.", "commit"]])
})

test("a commit helper that starts after its attempt was cancelled is terminated, not fed", () => {
  const h = harness()
  h.t.startSave()
  const a = h.s.attempt
  h.t.proposeEnded(a, okCapture())
  h.t.cancel()
  const writes = h.count("write")
  h.t.commitStarted(a)
  assert.equal(h.count("write"), writes)
  assert.ok(h.log.some((e) => e[0] === "terminate" && e[1] === a))
})

test("garbage or unknown stdout lines are ignored", () => {
  const h = harness()
  const a = toReview(h)
  h.t.commitLine(a, "not json")
  h.t.commitLine(a, line({ event: "surprise" }))
  h.t.commitLine(a, "null")
  assert.equal(h.s.resolved, false)
  assert.deepEqual(h.s.review, { summary: "Relaunch will open this app at login:", diff: "+ added x" })
})

// --- M7: the edits (import / set-exec as commits, drop as a reduce) ---

const REDUCE = '{"base":"{\\"v\\":2}","ops":[{"op":"drop","class":"Gone"}]}'
const okReduce = () => ({
  started: true,
  exitCode: 0,
  stdout: JSON.stringify({ ok: true, kind: "reduce", base: '{"v":2}', reduceJson: REDUCE }),
  stderr: ""
})

test("an edit runs the engine with its arguments and stdin", () => {
  const h = harness()
  h.t.start(["set-exec", "--class", "Off", "--exec-stdin", "--json"], "off --x\n", "Saving…", "Saved.")
  assert.deepEqual(h.log[0], ["spawnPropose", 1, ["set-exec", "--class", "Off", "--exec-stdin", "--json"], "off --x\n"])
  h.t.proposeEnded(1, okCapture())
  assert.deepEqual(h.log.filter((e) => e[0] === "spawnCommit"), [["spawnCommit", 1, "commit"]])
  h.t.commitStarted(1)
  h.t.commitLine(1, line({ event: "prepared", summary: "Off", diff: "~ Off" }))
  h.t.authorize()
  h.t.commitLine(1, line({ event: "committed", config: "SIGNED" }))
  assert.equal(h.s.status, "Saved.")
})

test("a reduce is relayed as-is, never reviewed, never authorized", () => {
  const h = harness()
  h.t.start(["drop", "--class", "Gone", "--json"], "", "Removing…", "Removed.")
  h.t.proposeEnded(1, okReduce())
  assert.deepEqual(h.log.filter((e) => e[0] === "spawnCommit"), [["spawnCommit", 1, "reduce"]])
  h.t.commitStarted(1)
  assert.deepEqual(h.log.filter((e) => e[0] === "write"), [["write", 1, REDUCE + "\n"]])
  h.t.commitLine(1, line({ event: "prepared", summary: "forged", diff: "forged" }))
  assert.equal(h.s.review, null, "a reduce shows no review")
  h.t.authorize()
  assert.equal(h.log.filter((e) => e[0] === "write").length, 1, "nothing to authorize on a reduce")
  h.t.commitLine(1, line({ event: "committed", config: "SIGNED" }))
  assert.deepEqual(h.log.filter((e) => e[0] === "install"), [["install", "SIGNED", "Removed.", "reduce"]])
  assert.equal(h.s.status, "Removed.")
})

test("an engine reply without a known kind starts nothing", () => {
  for (const body of [
    { ok: true, base: "", proposalJson: PROPOSAL },
    { ok: true, kind: "launch", base: "", proposalJson: PROPOSAL },
    { ok: true, kind: "reduce", base: "" },
    { ok: true, kind: "reduce", base: "", reduceJson: '{"a":1}\n{"b":2}' }
  ]) {
    const h = harness()
    h.t.startSave()
    h.t.proposeEnded(1, { started: true, exitCode: 0, stdout: JSON.stringify(body), stderr: "" })
    assert.equal(h.count("spawnCommit"), 0, JSON.stringify(body))
    assert.equal(h.s.statusIsError, true)
    assert.equal(h.s.committing, false)
  }
})

test("an engine error from an edit is shown and starts nothing", () => {
  const h = harness()
  h.t.start(["import", "--class", "X", "--workspace", "2", "--json"], "", "Importing…", "Imported.")
  h.t.proposeEnded(1, { started: true, exitCode: 1, stdout: JSON.stringify({ ok: false, error: "import: no verifiable launch command for X" }), stderr: "" })
  assert.equal(h.count("spawnCommit"), 0)
  assert.equal(h.s.status, "import: no verifiable launch command for X")
})

test("the review's instruction names the action that started it", () => {
  const h = harness()
  h.t.startSave()
  assert.equal(h.s.hint, "Approve only if you just clicked Capture open windows.")
  h.t.cancel()
  h.t.proposeEnded(1, { started: true, exitCode: 1, stdout: "", stderr: "" })
  h.t.start(["set-exec", "--class", "x", "--exec-stdin", "--json"], "cmd", "Saving…", "Saved.")
  assert.equal(h.s.hint, "Approve only if you just made this change in Relaunch.")
})

// An edit through to its relaunchctl outcome, named `what`.
function edit(h, what, nothingText) {
  const started = h.t.start(["set-exec", "--class", "x", "--exec-stdin", "--json"], "cmd", "Saving…", "Saved.", "",
    { what, nothingText })
  const a = h.s.attempt
  if (started) {
    h.t.proposeEnded(a, okCapture())
    h.t.commitStarted(a)
  }
  return { started, a }
}

test("a change that changes nothing says so, named for its action", () => {
  const h = harness()
  const { a } = edit(h, "Change the launch command for Mail", "no change. That is already the launch command.")
  h.t.commitLine(a, line({ event: "nothing" }))
  assert.equal(h.s.outcome, "nothing")
  assert.equal(h.s.notice, "Change the launch command for Mail: no change. That is already the launch command.")
  assert.equal(h.s.noticeIsError, false)
})

test("a failure names its action and its reason", () => {
  const h = harness()
  const { a } = edit(h, "Change the launch command for Mail")
  h.t.commitLine(a, line({ event: "error", message: "authorization denied" }))
  assert.equal(h.s.notice, "Change the launch command for Mail failed: authorization denied")
  assert.equal(h.s.noticeIsError, true)
  const e = harness()
  e.t.start(["set-exec"], "x", "", "", "", { what: "Change the launch command for Mail" })
  e.t.proposeEnded(1, { started: true, exitCode: 1, stdout: '{"ok":false,"error":"no relaunch entry for class: x"}', stderr: "" })
  assert.equal(e.s.notice, "Change the launch command for Mail failed: no relaunch entry for class: x")
})

test("a change refused while another runs says so and does not start", () => {
  const h = harness()
  edit(h, "Capture open windows")
  const second = h.t.start(["drop"], "", "", "", "", { what: "Remove Mail" })
  assert.equal(second, false)
  assert.equal(h.count("spawnPropose"), 1)
  assert.match(h.s.notice, /^Remove Mail: another change is still being made/)
  assert.equal(h.s.noticeIsError, true)
})

test("a commit or a cancel leaves no notice, and a new action clears the last", () => {
  const h = harness()
  let { a } = edit(h, "Change the launch command for Mail")
  h.t.commitLine(a, line({ event: "error", message: "denied" }))
  h.t.commitEnded(a, { started: true, stderr: "" })
  assert.notEqual(h.s.notice, "")
  ;({ a } = edit(h, "Change the launch command for Mail"))
  assert.equal(h.s.notice, "", "the next action clears it")
  h.t.commitLine(a, line({ event: "prepared", summary: "s", diff: "d" }))
  h.t.authorize()
  h.t.commitLine(a, line({ event: "committed", config: "SIGNED" }))
  h.t.commitEnded(a, { started: true, stderr: "" })
  assert.equal(h.s.outcome, "committed")
  assert.equal(h.s.notice, "")
  ;({ a } = edit(h, "Apply the launch delays"))
  h.t.cancel()
  assert.equal(h.s.outcome, "cancelled")
  assert.equal(h.s.notice, "")
  h.t.notify("Change the launch command for Mail: type a command first.", true)
  assert.equal(h.s.noticeIsError, true)
  h.t.dismissNotice()
  assert.equal(h.s.notice, "")
})

const dropCapture = () => okCapture({
  stdout: JSON.stringify({
    ok: true, kind: "commit", base: '{"v":2}', proposalJson: PROPOSAL,
    dropping: [{ label: "Proton Mail", workspace: 3 }, { label: "Signal", workspace: 5 }]
  })
})

test("a capture that would drop pins asks first and sends nothing until answered", () => {
  const h = harness()
  h.t.startSave()
  h.t.proposeEnded(1, dropCapture())
  assert.deepEqual(h.s.confirmDrop, [{ label: "Proton Mail", workspace: 3 }, { label: "Signal", workspace: 5 }])
  assert.equal(h.count("spawnCommit"), 0, "nothing reaches relaunchctl while the question is open")
  assert.equal(h.s.resolved, false)
  h.t.authorize()
  assert.equal(h.count("write"), 0)
  h.t.confirmReplace()
  assert.equal(h.s.confirmDrop, null)
  assert.deepEqual(h.log.filter((e) => e[0] === "spawnCommit"), [["spawnCommit", 1, "commit"]])
  h.t.confirmReplace()
  assert.equal(h.count("spawnCommit"), 1, "answered once")
})

test("keeping the pins ends the capture with nothing sent", () => {
  const h = harness()
  h.t.startSave()
  h.t.proposeEnded(1, dropCapture())
  h.t.keepPins()
  assert.equal(h.s.resolved, true)
  assert.equal(h.s.outcome, "cancelled")
  assert.equal(h.s.confirmDrop, null)
  assert.equal(h.s.notice, "")
  assert.equal(h.count("spawnCommit"), 0)
  h.t.confirmReplace()
  assert.equal(h.count("spawnCommit"), 0, "a late Replace does nothing")
  assert.equal(h.s.committing, false, "a new change can start")
})

test("a capture that drops nothing, and a merge, go straight to review", () => {
  const h = harness()
  h.t.startSave()
  h.t.proposeEnded(1, okCapture())
  assert.equal(h.s.confirmDrop, null)
  assert.equal(h.count("spawnCommit"), 1)
  const m = harness()
  assert.equal(m.t.startMerge(), true)
  assert.deepEqual(m.log[0], ["spawnPropose", 1, ["propose", "--merge", "--json"], ""])
  assert.equal(m.s.what, "Add open windows")
})

test("closing the panel while the drop question is open ends it and frees the panel", () => {
  const h = harness()
  h.t.startSave()
  h.t.proposeEnded(1, dropCapture())
  h.t.cancel()
  assert.equal((h.s.resolved && !h.s.committing), true)
  assert.equal(h.count("spawnCommit"), 0)
  assert.equal(h.t.startMerge(), true, "the next change starts")
})

const mergedCapture = (merged) => okCapture({
  stdout: JSON.stringify({
    ok: true, kind: "commit", base: '{"v":2}', proposalJson: PROPOSAL, dropping: [], merged: merged
  })
})

test("windows a capture merged as identical duplicates are named with the review", () => {
  const h = harness()
  h.t.startMerge()
  h.t.proposeEnded(1, mergedCapture([{ label: "foot", workspace: 2 }, { label: "foot", workspace: 6 }]))
  assert.equal(h.s.confirmDrop, null, "merged windows are not pins being dropped")
  assert.equal(h.s.mergedText,
    "2 open windows were identical duplicates and merged: foot (workspace 2), foot (workspace 6).")
  h.t.commitStarted(1)
  h.t.commitLine(1, line({ event: "prepared", summary: "s", diff: "d" }))
  assert.notEqual(h.s.review, null)
  assert.notEqual(h.s.mergedText, "", "still shown while the review is up")
  h.t.cancel()
  h.t.commitEnded(1, { started: true, stderr: "" })
  assert.equal(h.t.startSave(), true)
  assert.equal(h.s.mergedText, "", "a new capture starts with none")
  h.t.proposeEnded(2, mergedCapture([]))
  assert.equal(h.s.mergedText, "")
})

test("a no-op capture still says which windows it merged", () => {
  const h = harness()
  h.t.startMerge()
  h.t.proposeEnded(1, mergedCapture([{ label: "foot", workspace: 2 }]))
  h.t.commitStarted(1)
  h.t.commitLine(1, line({ event: "nothing" }))
  assert.equal(h.s.outcome, "nothing")
  assert.match(h.s.notice, /1 open window was an identical duplicate and merged: foot \(workspace 2\)\.$/)
})

// --- The boot-tools install flow (CoreInstall.js) ---
const CoreInstall = require(path.join(__dirname, "..", "CoreInstall.js"))

function coreHarness() {
  const log = []
  const fx = {
    render() {},
    probe: (op) => log.push(["probe", op]),
    spawnInstall: (op) => log.push(["install", op]),
    spawnWire: (op) => log.push(["wire", op]),
    spawnManage: (op, kind) => log.push(["manage", op, kind]),
    checkUpdates: (op) => log.push(["updates", op]),
    ready: () => log.push(["ready"])
  }
  const c = CoreInstall.create(fx)
  const count = (k) => log.filter((e) => e[0] === k).length
  const lastOp = (k) => log.filter((e) => e[0] === k).slice(-1)[0][1]
  return { c, s: c.state, log, count, lastOp }
}
const HERE = { core: true, canInstall: true }
const ABSENT = { core: false, canInstall: true }

test("installed on open: the normal panel, nothing run", () => {
  const h = coreHarness()
  h.c.check()
  h.c.probeEnded(h.lastOp("probe"), HERE)
  assert.equal(h.s.phase, "ready")
  assert.equal(h.count("install") + h.count("wire") + h.count("ready"), 0)
})

test("missing on open shows the install surface", () => {
  const h = coreHarness()
  h.c.check()
  h.c.probeEnded(h.lastOp("probe"), ABSENT)
  assert.equal(h.s.phase, "missing")
  assert.equal(h.s.isError, false)
})

test("a successful install is re-probed, then wired, then the normal panel", () => {
  const h = coreHarness()
  h.c.check(); h.c.probeEnded(h.lastOp("probe"), ABSENT)
  assert.equal(h.c.install(), true)
  assert.equal(h.s.phase, "installing")
  assert.equal(h.c.install(), false, "a second click does nothing while it runs")
  assert.equal(h.count("install"), 1)
  const op = h.lastOp("install")
  h.c.installEnded(op, { started: true })
  assert.equal(h.count("probe"), 2, "the terminal's exit is not the result: probe again")
  h.c.probeEnded(op, HERE)
  assert.equal(h.s.phase, "wiring")
  assert.equal(h.count("wire"), 1)
  h.c.wireEnded(h.lastOp("wire"), { started: true, exitCode: 0, stderr: "" })
  assert.equal(h.s.phase, "ready")
  assert.equal(h.count("ready"), 1)
  assert.match(h.s.message, /^Boot tools installed/)
})

test("a cancelled or failed install stays missing, says so, and can be retried", () => {
  const h = coreHarness()
  h.c.check(); h.c.probeEnded(h.lastOp("probe"), ABSENT)
  h.c.install()
  const op = h.lastOp("install")
  h.c.installEnded(op, { started: true })
  h.c.probeEnded(op, ABSENT)
  assert.equal(h.s.phase, "missing")
  assert.equal(h.s.isError, true)
  assert.match(h.s.message, /still not installed/)
  assert.equal(h.count("wire"), 0, "nothing is wired without the core")
  assert.equal(h.c.install(), true, "retry")
  assert.equal(h.count("install"), 2)
})

test("a terminal that never opened stays missing", () => {
  const h = coreHarness()
  h.c.check(); h.c.probeEnded(h.lastOp("probe"), ABSENT)
  h.c.install()
  h.c.installEnded(h.lastOp("install"), { started: false })
  assert.equal(h.s.phase, "missing")
  assert.match(h.s.message, /Could not open a terminal/)
})

test("without Omarchy's package tools the button explains and installs nothing", () => {
  const h = coreHarness()
  h.c.check(); h.c.probeEnded(h.lastOp("probe"), { core: false, canInstall: false })
  assert.equal(h.c.install(), false)
  assert.equal(h.count("install"), 0)
  assert.equal(h.s.phase, "missing")
  assert.match(h.s.message, /can't install its boot tools here/)
})

test("a poll notices the install before the terminal closes; the terminal's late exit is ignored", () => {
  const h = coreHarness()
  h.c.check(); h.c.probeEnded(h.lastOp("probe"), ABSENT)
  h.c.install()
  const op = h.lastOp("install")
  h.c.poll()
  h.c.probeEnded(op, ABSENT)
  assert.equal(h.s.phase, "installing", "a poll before the exit decides nothing")
  h.c.poll()
  h.c.probeEnded(op, HERE)
  assert.equal(h.s.phase, "wiring")
  h.c.installEnded(op, { started: true })
  h.c.probeEnded(op, ABSENT)
  assert.equal(h.s.phase, "wiring", "stragglers from the install op change nothing")
  assert.equal(h.count("wire"), 1)
})

test("a probe already out when the terminal exits decides the attempt; no second probe", () => {
  const h = coreHarness()
  h.c.check(); h.c.probeEnded(h.lastOp("probe"), ABSENT)
  h.c.install()
  const op = h.lastOp("install")
  h.c.poll()
  h.c.installEnded(op, { started: true })
  assert.equal(h.count("probe"), 2)
  h.c.probeEnded(op, ABSENT)
  assert.equal(h.s.phase, "missing")
})

test("Check again after a detached terminal finished: wires, then the normal panel", () => {
  const h = coreHarness()
  h.c.check(); h.c.probeEnded(h.lastOp("probe"), ABSENT)
  assert.equal(h.c.check(), true)
  h.c.probeEnded(h.lastOp("probe"), HERE)
  assert.equal(h.s.phase, "wiring")
})

test("failed wiring keeps the core, says why, and the button retries only the wiring", () => {
  const h = coreHarness()
  h.c.check(); h.c.probeEnded(h.lastOp("probe"), ABSENT)
  h.c.install()
  const op = h.lastOp("install")
  h.c.installEnded(op, { started: true })
  h.c.probeEnded(op, HERE)
  h.c.wireEnded(h.lastOp("wire"), { started: true, exitCode: 1, stderr: "ensure-hooks: hyprland.lua: permission denied" })
  assert.equal(h.s.phase, "unwired")
  assert.match(h.s.message, /permission denied/)
  assert.equal(h.c.install(), true)
  assert.equal(h.count("install"), 1, "no second package install")
  assert.equal(h.count("wire"), 2)
  h.c.wireEnded(h.lastOp("wire"), { started: true, exitCode: 0, stderr: "" })
  assert.equal(h.s.phase, "ready")
})

test("a normal command finding the engine gone brings the install surface back", () => {
  const h = coreHarness()
  h.c.check(); h.c.probeEnded(h.lastOp("probe"), HERE)
  h.c.engineMissing()
  h.c.probeEnded(h.lastOp("probe"), ABSENT)
  assert.equal(h.s.phase, "missing")
})

test("I'm done while installing: a failed probe ends the attempt even if the terminal never reports", () => {
  const h = coreHarness()
  h.c.check(); h.c.probeEnded(h.lastOp("probe"), ABSENT)
  h.c.install()
  const op = h.lastOp("install")
  assert.equal(h.c.check(), true)
  assert.equal(h.c.check(), false, "one probe at a time")
  h.c.probeEnded(op, ABSENT)
  assert.equal(h.s.phase, "missing")
  assert.equal(h.s.isError, true)
  h.c.installEnded(op, { started: true })
  assert.equal(h.count("probe"), 2, "the terminal's late exit after the attempt ended changes nothing")
})

test("retrying while the last install window is still open says so", () => {
  const h = coreHarness()
  h.c.check(); h.c.probeEnded(h.lastOp("probe"), ABSENT)
  h.c.install()
  h.c.installEnded(h.lastOp("install"), { started: false, stillOpen: true })
  assert.equal(h.s.phase, "missing")
  assert.match(h.s.message, /still open/)
})

test("the button does nothing while a Check again probe is out", () => {
  const h = coreHarness()
  h.c.check(); h.c.probeEnded(h.lastOp("probe"), ABSENT)
  h.c.check()
  assert.equal(h.c.install(), false)
  assert.equal(h.count("install"), 0)
  h.c.probeEnded(h.lastOp("probe"), ABSENT)
  assert.equal(h.s.probing, false, "the probe still settles")
  assert.equal(h.c.install(), true)
})

test("an earlier attempt's terminal closing during a newer attempt is ignored", () => {
  const h = coreHarness()
  h.c.check(); h.c.probeEnded(h.lastOp("probe"), ABSENT)
  h.c.install()
  const first = h.lastOp("install")
  h.c.check(); h.c.probeEnded(first, ABSENT)
  h.c.install()
  assert.notEqual(h.lastOp("install"), first)
  const probes = h.count("probe")
  h.c.installEnded(first, { started: true })
  assert.equal(h.count("probe"), probes, "the old terminal's exit decides nothing")
  assert.equal(h.s.installEnded, false)
})

// --- The boot-tools lifecycle from the ready panel ---
const PKG = (version, stamp) => ({ core: true, canInstall: true, version: version, stamp: stamp })
const DEV = { core: true, canInstall: true, version: null, stamp: "1.100" }

function readyAt(version, stamp) {
  const h = coreHarness()
  h.c.check()
  h.c.probeEnded(h.lastOp("probe"), PKG(version, stamp))
  assert.equal(h.s.phase, "ready")
  return h
}
// Finish a terminal attempt: it exits, and the probe that follows sees `after`.
function terminalDone(h, after) {
  const op = h.lastOp("manage")
  h.c.installEnded(op, { started: true })
  h.c.probeEnded(op, after)
  return op
}

test("check for updates: a newer version, Update in the terminal, re-probe, wire, ready", () => {
  const h = readyAt("1.0.7-1", "11.100")
  assert.equal(h.c.checkUpdates(), true)
  assert.equal(h.s.phase, "checking-updates")
  assert.equal(h.count("manage"), 0, "the check runs no terminal")
  assert.equal(h.c.checkUpdates(), false, "one check at a time")
  h.c.updatesChecked(h.lastOp("updates"), { result: "newer", installed: "1.0.7-1", latest: "1.0.8-1" })
  assert.equal(h.s.phase, "update-available")
  assert.equal(h.s.message, "Update available: 1.0.7-1 → 1.0.8-1")
  assert.equal(h.c.update(), true)
  assert.equal(h.s.phase, "updating")
  assert.deepEqual(h.log.filter((e) => e[0] === "manage").map((e) => e[2]), ["update"])
  assert.equal(h.c.update(), false, "a second click does nothing while it runs")
  terminalDone(h, PKG("1.0.8-1", "12.200"))
  assert.equal(h.s.phase, "wiring")
  h.c.wireEnded(h.lastOp("wire"), { started: true, exitCode: 0, stderr: "" })
  assert.equal(h.s.phase, "ready")
  assert.equal(h.s.message, "Boot tools updated to 1.0.8-1.")
})

test("an update the user cancelled: same version after the terminal, back to ready with an error", () => {
  const h = readyAt("1.0.7-1", "11.100")
  h.c.checkUpdates()
  h.c.updatesChecked(h.lastOp("updates"), { result: "newer", installed: "1.0.7-1", latest: "1.0.8-1" })
  h.c.update()
  terminalDone(h, PKG("1.0.7-1", "11.100"))
  assert.equal(h.s.phase, "ready")
  assert.equal(h.s.isError, true)
  assert.match(h.s.message, /update did not finish/)
  assert.equal(h.count("wire"), 0)
})

test("up to date, and the AUR-unreachable and no-helper guards, run nothing", () => {
  for (const [r, phase, msg] of [
    [{ result: "current", installed: "1.0.7-1" }, "ready", /^Up to date \(1\.0\.7-1\)\.$/],
    [{ result: "unreachable", installed: "1.0.7-1" }, "ready", /Couldn't reach the AUR/],
    [{ result: "nohelper" }, "ready", /needs yay/],
    [{ result: "noaur", installed: "1.0.7-1" }, "ready", /has no omarchy-relaunch/]
  ]) {
    const h = readyAt("1.0.7-1", "11.100")
    h.c.checkUpdates()
    h.c.updatesChecked(h.lastOp("updates"), r)
    assert.equal(h.s.phase, phase, r.result)
    assert.match(h.s.message, msg, r.result)
    assert.equal(h.c.update(), false, r.result + ": no Update without a newer version")
    assert.equal(h.count("manage"), 0, r.result)
  }
})

test("reinstall asks first, then is decided by relaunchd's new stamp", () => {
  const h = readyAt("1.0.7-1", "11.100")
  assert.equal(h.c.confirm(), false, "nothing to confirm yet")
  assert.equal(h.c.requestReinstall(), true)
  assert.equal(h.s.confirm, "reinstall")
  assert.equal(h.count("manage"), 0, "nothing runs until confirmed")
  h.c.cancelConfirm()
  assert.equal(h.s.confirm, "")
  h.c.requestReinstall()
  assert.equal(h.c.confirm(), true)
  assert.equal(h.s.phase, "reinstalling")
  terminalDone(h, PKG("1.0.7-1", "13.300"))
  assert.equal(h.s.phase, "wiring")
  h.c.wireEnded(h.lastOp("wire"), { started: true, exitCode: 0, stderr: "" })
  assert.equal(h.s.message, "Boot tools reinstalled.")
})

test("a reinstall whose files did not change did not happen", () => {
  const h = readyAt("1.0.7-1", "11.100")
  h.c.requestReinstall(); h.c.confirm()
  terminalDone(h, PKG("1.0.7-1", "11.100"))
  assert.equal(h.s.phase, "ready")
  assert.match(h.s.message, /reinstall did not finish/)
})

test("uninstall asks first, drops the package, and the core gone returns the install card", () => {
  const h = readyAt("1.0.7-1", "11.100")
  h.c.requestUninstall()
  assert.equal(h.s.confirm, "uninstall")
  h.c.confirm()
  assert.equal(h.s.phase, "uninstalling")
  assert.deepEqual(h.log.filter((e) => e[0] === "manage").map((e) => e[2]), ["uninstall"])
  terminalDone(h, ABSENT)
  assert.equal(h.s.phase, "missing")
  assert.equal(h.s.isError, false)
  assert.match(h.s.message, /signing key are kept/)
  assert.equal(h.count("wire"), 0, "nothing is wired after a removal")
})

test("an uninstall that did not happen stays ready and says so", () => {
  const h = readyAt("1.0.7-1", "11.100")
  h.c.requestUninstall(); h.c.confirm()
  terminalDone(h, PKG("1.0.7-1", "11.100"))
  assert.equal(h.s.phase, "ready")
  assert.match(h.s.message, /still installed/)
})

test("a development install (not pacman's) is never updated, reinstalled or removed", () => {
  const h = coreHarness()
  h.c.check(); h.c.probeEnded(h.lastOp("probe"), DEV)
  assert.equal(h.s.phase, "ready")
  assert.equal(h.c.checkUpdates(), false)
  assert.equal(h.c.requestReinstall(), false)
  assert.equal(h.c.requestUninstall(), false)
  assert.equal(h.c.confirm(), false)
  assert.equal(h.count("manage") + h.count("updates"), 0)
  assert.match(h.s.message, /development install/)
})

test("the lifecycle is inert while any op is in flight", () => {
  const h = readyAt("1.0.7-1", "11.100")
  h.c.requestUninstall(); h.c.confirm()
  assert.equal(h.c.checkUpdates(), false)
  assert.equal(h.c.requestReinstall(), false)
  assert.equal(h.c.update(), false)
  assert.equal(h.c.install(), false)
  assert.equal(h.count("manage"), 1)
  const h2 = readyAt("1.0.7-1", "11.100")
  h2.c.checkUpdates()
  assert.equal(h2.c.requestUninstall(), false, "not while the update check runs")
  assert.equal(h2.c.check(), false)
})

test("stale replies are ignored: an old check, an old terminal", () => {
  const h = readyAt("1.0.7-1", "11.100")
  h.c.checkUpdates()
  const oldCheck = h.lastOp("updates")
  h.c.updatesChecked(oldCheck, { result: "newer", installed: "1.0.7-1", latest: "1.0.8-1" })
  h.c.update()
  h.c.updatesChecked(oldCheck, { result: "current", installed: "1.0.7-1" })
  assert.equal(h.s.phase, "updating", "the old check's reply changes nothing")
  const upd = h.lastOp("manage")
  h.c.installEnded(upd, { started: true })
  h.c.probeEnded(upd, PKG("1.0.7-1", "11.100"))
  h.c.requestUninstall(); h.c.confirm()
  const probes = h.count("probe")
  h.c.installEnded(upd, { started: true })
  assert.equal(h.count("probe"), probes, "the update's terminal closing late decides nothing")
  assert.equal(h.s.phase, "uninstalling")
})

test("a poll notices the update before the terminal closes", () => {
  const h = readyAt("1.0.7-1", "11.100")
  h.c.checkUpdates()
  h.c.updatesChecked(h.lastOp("updates"), { result: "newer", installed: "1.0.7-1", latest: "1.0.8-1" })
  h.c.update()
  const op = h.lastOp("manage")
  h.c.poll()
  h.c.probeEnded(op, PKG("1.0.7-1", "11.100"))
  assert.equal(h.s.phase, "updating", "mid-build: not yet, and not a failure")
  h.c.poll()
  h.c.probeEnded(op, PKG("1.0.8-1", "12.200"))
  assert.equal(h.s.phase, "wiring")
})

test("a missing tool or a failed request is said as such, never as no AUR package", () => {
  for (const [result, msg] of [
    ["nocurl", /^The update check needs curl\.$/],
    ["nojq", /^The update check needs jq\.$/],
    ["fetchfailed", /request to the AUR failed/],
    ["badreply", /reply could not be read/]
  ]) {
    const h = readyAt("1.0.7-1", "11.100")
    h.c.checkUpdates()
    h.c.updatesChecked(h.lastOp("updates"), { result: result, installed: "1.0.7-1" })
    assert.equal(h.s.phase, "ready", result)
    assert.equal(h.s.isError, true, result)
    assert.match(h.s.message, msg, result)
    assert.doesNotMatch(h.s.message, /has no omarchy-relaunch/, result)
    assert.equal(h.c.update(), false, result + ": no Update offered")
    assert.equal(h.count("manage"), 0, result + ": nothing spawned")
  }
})
