#!/usr/bin/env python3
"""Generate the Rust engine's unit goldens from the bash oracle.

Each golden line is {"in": <input>, "out": <the bash engine's output>}, produced
by the oracle's read-only probes (__quote, __split, __split-ifs, __nkey). The
Rust tests load these files and must reproduce every output byte for byte.

    python3 -I tests/differential/gen_unit_goldens.py

Re-run only when the bash oracle changes; the files are committed.
"""
import json
import pathlib
import subprocess
import sys

ROOT = pathlib.Path(__file__).resolve().parents[2]
ENGINE = ROOT / "tests" / "oracle" / "relaunch"
OUT = ROOT / "core" / "crates" / "relaunch-engine" / "tests" / "golden"

def probe(*args):
    r = subprocess.run([str(ENGINE), *args], capture_output=True, check=True)
    return json.loads(r.stdout)

PRINTABLE = [chr(c) for c in range(32, 127)]
CONTROLS = [chr(c) for c in list(range(1, 32)) + [127]]

def quote_cases():
    words = [[""], ["", ""], ["a", ""]]
    for c in PRINTABLE:
        words += [[c], [f"a{c}b"], [f"{c}x"], [f"x{c}"]]
    for w in ["~", "~a", "a~", "=~", "a=~b", ":~", "a:~b", "a=b~", "#a", "a#", "a #b",
              "==", "::", "--app-id=TUI.float", "https://x.y/z?a=1&b=2"]:
        words.append([w])
    for c in CONTROLS:
        words += [[c], [f"a{c}b"]]
    words += [["a b\tc"], ["it's\tx"], ["a\\\tb"], ["\x1b[0m"], ["$'x'"], ["a'b\tc\"d"]]
    words += [
        ["xdg-terminal-exec", "--app-id=TUI.float", "-e", "bash", "-c", "dua i /"],
        ["brave", "--app=https://x.y/z?a=1&b=2"],
        ["/opt/thing/bin/thing", "--profile", "a b"],
        ["env", "FOO=bar baz", "cmd", "--", "~/x", "#tag"],
    ]
    return [{"in": w, "out": probe("__quote", *w)["exec"]} for w in words]

# Inputs whose bash %q output is locale/locale-table dependent: the Rust engine
# uses a single-quote fallback (D-P2(a)), so only argv round-trip is checked.
ROUNDTRIP_ONLY = [["é"], ["a é"], ["日本"], ["a‎b"], [" "], ["ü\tx"], ["Ω", "a b"]]

SPLIT_INPUTS = [
    "", " ", "a", "a b", "  a   b  ", "a\tb", "a\nb", "a b\nc d", "\na b",
    "'a b'", '"a b"', "a\\ b", "a\\", "'unterminated", '"unterm', "a'b'c",
    'a"b\\"c"d', '"a\\nb"', "'a\\b'", "bash -c 'dua i /'", "x '' y", 'x "" y',
    "$'a\\tb'", "a\\\nb", "'a\nb'", "'a\tb'", '"\\\\"', "é ü", "\\'", "''",
    'gio launch "/usr/share/applications/a b.desktop"',
    "xdg-terminal-exec --app-id=TUI.float -e bash -c dua\\ i\\ /",
]

def split_cases(probe_name):
    inputs = list(SPLIT_INPUTS)
    # Everything the quoter emits must split back the way the engine expects.
    inputs += [c["out"] for c in quote_cases_cache]
    seen, cases = set(), []
    for s in inputs:
        if s in seen:
            continue
        seen.add(s)
        cases.append({"in": s, "out": probe(probe_name, s)})
    return cases

NKEY_INPUTS = [
    "A B..C", "Foo___Bar", "x", "", "A.B_C D", "___", "a..b__c  d", "brave-browser",
    "org.gnome.Nautilus", "LibreOffice  Calc", "a__b", "x.y_z-w", "ÉCOLE", "---",
    "Mixed.Case_Name", "a - b", " lead", "trail ", "UPPER", "tab\there", "dots...",
]

WEBAPP_EXECS = [
    "omarchy-launch-webapp https://maps.example.com/app",
    "omarchy-launch-webapp --flag https://x.example.com/ %U",
    "uwsm-app -- omarchy-launch-webapp https://y.example.com/p",
    "env FOO=bar omarchy-launch-webapp https://z.example.com/",
    "/usr/bin/omarchy-launch-webapp 'https://q.example.com/a b'",
    "setsid omarchy-launch-webapp %u",
    "omarchy-launch-webapp",
    "brave --app=https://not-a-webapp-line.example.com/",
    "omarchy-launch-webapp-extra https://n.example.com/",
    "nice -n 5 omarchy-launch-webapp https://w.example.com/",
    "VAR=1 --x omarchy-launch-webapp https://v.example.com/",
]
URLS = [
    "https://trail-a.example.com/a/b/..", "https://trail-b.example.com/a/b/../",
    "https://empty.example.com/a//b", "https://dot.example.com/a/.",
    "https://[2001:db8::1]:8443/x/../y", "https://user:pw@Host.Example.COM:8080/P/Q?x=1#f",
    "https://h.example.com", "https://h.example.com/", "https://h.example.com?q", "https://h.example.com#f",
    "no-scheme.example.com/x", "https://", "https:///path", "https://a@b@c.example.com/z",
    "http://h.example.com/a/b/c/./../../g", "http://h.example.com/mid/content=5/../6",
    "http://h.example.com/./a", "http://h.example.com/../a", "http://h.example.com/a/..",
    "http://h.example.com/.", "http://h.example.com/..", "http://h.example.com/a/b%2Fc",
    "ftp://ÜNI.example.com/x", "https://h.example.com/a/b/../../../c",
]
LEAF_EXECS = [
    'xdg-terminal-exec --app-id=TUI.float -e bash -c "dua i /"',
    "foot -e herdr", "foot --app-id herdr -e herdr", "alacritty --working-directory /tmp -e btop",
    "kitty --title X htop", "ghostty -- nvim file", "uwsm-app -- foot -e lazygit",
    "env A=1 B=2 kitty -e top", "bash -c 'dua i /'", "sh -c ' leading'", "zsh -c",
    "brave", "/usr/bin/brave --app=x", "omarchy-launch-terminal-x -e cmd", "setsid -f foot -e x",
    "foot", "foot -e", "", "wezterm-gui start -- vim", "foot -D /tmp mc",
]

def main():
    global quote_cases_cache
    OUT.mkdir(parents=True, exist_ok=True)
    quote_cases_cache = quote_cases()
    files = {
        "quote.jsonl": quote_cases_cache,
        "quote_roundtrip.jsonl": [{"in": w} for w in ROUNDTRIP_ONLY],
        "split.jsonl": split_cases("__split"),
        "split_ifs.jsonl": split_cases("__split-ifs"),
        "nkey.jsonl": [{"in": s, "out": probe("__nkey", s)["key"]} for s in NKEY_INPUTS],
        "webapp_url.jsonl": [{"in": s, "out": probe("__webapp-url", s)["url"]} for s in WEBAPP_EXECS],
        "webapp_ident.jsonl": [{"in": s, "out": probe("__webapp-ident", s)["ident"]} for s in URLS],
        "leaf.jsonl": [{"in": s, "out": probe("__leaf", s)["leaf"]} for s in LEAF_EXECS],
    }
    for name, cases in files.items():
        with open(OUT / name, "w", encoding="utf-8") as f:
            for c in cases:
                f.write(json.dumps(c, ensure_ascii=False) + "\n")
        print(f"{name}: {len(cases)} cases", file=sys.stderr)

quote_cases_cache = []
if __name__ == "__main__":
    main()
