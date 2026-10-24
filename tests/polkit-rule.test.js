// The shipped polkit rule (packaging/polkit/00-io.github.laytonf.relaunch.rules),
// executed: for both Relaunch actions a local, active caller must authenticate as
// themselves and anyone else is refused; any other action is left to polkit.
"use strict";
const test = require("node:test");
const assert = require("node:assert");
const fs = require("node:fs");
const path = require("node:path");

const RULE = path.join(__dirname, "../packaging/polkit/00-io.github.laytonf.relaunch.rules");
const Result = { NO: "no", AUTH_SELF: "auth_self", NOT_HANDLED: undefined };

function loadRule() {
  const rules = [];
  const polkit = { Result, addRule: (f) => rules.push(f) };
  new Function("polkit", fs.readFileSync(RULE, "utf8"))(polkit);
  assert.strictEqual(rules.length, 1);
  return rules[0];
}

for (const id of ["io.github.laytonf.relaunch.authorize", "io.github.laytonf.relaunch.purge"]) {
  test(`${id}: a local, active caller authenticates as themselves`, () => {
    assert.strictEqual(loadRule()({ id }, { local: true, active: true }), Result.AUTH_SELF);
  });
  test(`${id}: a remote or inactive caller is refused`, () => {
    const rule = loadRule();
    for (const subject of [{ local: false, active: true }, { local: true, active: false }, { local: false, active: false }]) {
      assert.strictEqual(rule({ id }, subject), Result.NO, JSON.stringify(subject));
    }
  });
}

test("any other action is not ours to decide", () => {
  const rule = loadRule();
  for (const id of ["org.freedesktop.login1.reboot", "io.github.laytonf.relaunch.reduce", "io.github.laytonf.relaunch"]) {
    assert.strictEqual(rule({ id }, { local: true, active: true }), Result.NOT_HANDLED, id);
  }
});
