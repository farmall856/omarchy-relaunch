// Tests for LastBoot.js: what the panel says about the last boot's failures.
// Run: node --test tests/last-boot.test.js
"use strict"
const test = require("node:test")
const assert = require("node:assert/strict")
const path = require("node:path")
const lb = require(path.join(__dirname, "..", "LastBoot.js"))

const placed = { class: "relauncher-a", label: "Signal", workspace: 5, exec: "signal-desktop --",
  status: "started", placed: true, outcome: "placed", address: "0x1", problem: null }
const dua = { class: "relauncher-b", label: "bash", windowClass: "TUI.float", workspace: 6,
  exec: "/usr/bin/foot --app-id=TUI.float -e bash -c dua\\ i\\ /", status: "started", placed: false,
  outcome: "no-show", address: "0x2", exit: { code: 127 },
  problem: "its window opened, then closed; the command exited with status 127 (a program it runs was not found)" }
const boot = (launches, outcome) => ({ outcome: outcome || "launched", launches })

test("a boot where every window landed has nothing to say", () => {
  assert.deepEqual(lb.failures(boot([placed], "landed")), [])
  assert.equal(lb.summary(boot([placed], "landed")), "")
  assert.equal(lb.rowProblem(boot([placed], "landed"), placed), "")
})

test("a failed launch is named with its workspace, its reason and its readable command", () => {
  assert.equal(lb.summary(boot([placed, dua])),
    "1 app didn't open at the last boot:\n• Workspace 6 · bash: " + dua.problem
    + ".\n  Command: /usr/bin/foot --app-id=TUI.float -e bash -c dua i /")
  const two = lb.summary(boot([dua, Object.assign({}, dua, { workspace: 7, label: "" })]))
  assert.ok(two.startsWith("2 apps didn't open at the last boot:"), two)
  assert.ok(two.includes("• Workspace 7 · TUI.float: "), two)
})

test("the row that failed is marked while it still has the command that failed", () => {
  const b = boot([placed, dua])
  assert.equal(lb.rowProblem(b, { class: dua.class, exec: dua.exec }), dua.problem)
  assert.equal(lb.rowProblem(b, { class: dua.class, exec: "/usr/bin/foot -e ncdu /" }), "")
  assert.equal(lb.rowProblem(b, { class: "relauncher-other", exec: dua.exec }), "")
  assert.equal(lb.rowProblem(b, { class: placed.class, exec: placed.exec }), "")
  assert.equal(lb.rowProblem(null, { class: dua.class, exec: dua.exec }), "")
})

test("a boot that launched nothing reports no failed apps", () => {
  for (const outcome of ["skipped", "disabled", "unverified", "schema-upgrade-needed"])
    assert.deepEqual(lb.failures(boot([dua], outcome)), [], outcome)
  assert.deepEqual(lb.failures(null), [])
})

test("a record from an older anchor, with no problem, still says why", () => {
  const old = (o) => Object.assign({}, dua, { problem: undefined, exit: undefined }, o)
  assert.equal(lb.why(old({})), "its window opened, then closed")
  assert.equal(lb.why(old({ address: null })), "no window appeared")
  assert.equal(lb.why(old({ outcome: "misplaced", actualWorkspace: 2 })), "its window is on workspace 2, not 6")
  assert.equal(lb.why(old({ outcome: "misplaced", actualWorkspace: null })), "its window could not be moved to its workspace")
  assert.equal(lb.why(old({ status: "not_found", error: "dua: not found", outcome: undefined })), "it did not start: dua: not found")
  assert.equal(lb.why(old({ status: "verify-failed", error: "", outcome: undefined })), "it did not start")
})

test("the readable command drops only the escapes", () => {
  assert.equal(lb.readable("a\\ b\\\\c d"), "a b\\c d")
  assert.equal(lb.readable(undefined), "")
})
