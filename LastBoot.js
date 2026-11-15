// What the panel says about the last boot's failures, from last-boot.json
// (relaunch-anchor's report.rs). Kept out of Panel.qml so it runs under
// node --test.
//
// A launch failed when its window did not end on its workspace. The anchor
// writes why as `problem`; a record from an older anchor has none, so the
// reason is rebuilt from the outcome it does have.

// Why `l` failed, or "" when it did not.
function why(l) {
  if (!l || l.placed === true) return ""
  if (typeof l.problem === "string" && l.problem !== "") return l.problem
  if (l.status !== "started") return "it did not start" + (l.error ? ": " + l.error : "")
  if (l.outcome === "misplaced")
    return l.actualWorkspace != null
      ? "its window is on workspace " + l.actualWorkspace + ", not " + l.workspace
      : "its window could not be moved to its workspace"
  return l.address ? "its window opened, then closed" : "no window appeared"
}

// The launches of `lastBoot` that failed. Only a boot that launched has any:
// a skipped, disabled or unverified boot launched nothing.
function failures(lastBoot) {
  if (!lastBoot || (lastBoot.outcome !== "launched" && lastBoot.outcome !== "landed")) return []
  return (lastBoot.launches || []).filter(function (l) { return why(l) !== "" })
}

// The signed command as a person reads it: `\ ` is a space inside a word.
function readable(exec) {
  return String(exec || "").replace(/\\(.)/g, "$1")
}

// Why `row` failed at the last boot, or "". A row matches only while it is
// still the entry that launched: same class and same command, so a fixed
// command clears it before the next boot.
function rowProblem(lastBoot, row) {
  if (!row) return ""
  var hit = failures(lastBoot).filter(function (l) {
    return l.class === row.class && l.exec === row.exec
  })[0]
  return hit ? why(hit) : ""
}

// The boot section's notice: one line per failure, or "".
function summary(lastBoot) {
  var f = failures(lastBoot)
  if (f.length === 0) return ""
  var head = f.length === 1 ? "1 app didn't open at the last boot:" : f.length + " apps didn't open at the last boot:"
  return head + f.map(function (l) {
    return "\n• Workspace " + l.workspace + " · " + (l.label || l.windowClass) + ": " + why(l)
      + ".\n  Command: " + readable(l.exec)
  }).join("")
}

if (typeof module !== "undefined") module.exports = { why: why, failures: failures, readable: readable, rowProblem: rowProblem, summary: summary }
