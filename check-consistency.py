#!/usr/bin/env python3
"""Preflight check for the desktop project's config wiring.

Catches the class of mistake that otherwise only surfaces several minutes into a
Windows Rust build (or worse, at runtime in the installed app):

  * a capability granting a permission for a plugin that isn't a dependency
    (this exact mistake failed a build: `process:default` was left behind after
    the tauri-plugin-process dependency was removed)
  * Rust code referencing a plugin that isn't in Cargo.toml
  * the frontend invoking a command the Rust side never registered
    (this exact mistake shipped: the UI called `save_file_as` against a binary
    built before that command existed -> "Command save_file_as not found")

Runs in about a second. Exits non-zero on any real problem.
"""
import json
import re
import sys
from pathlib import Path

root = Path(__file__).parent
cargo = (root / "src-tauri" / "Cargo.toml").read_text()
lib = (root / "src-tauri" / "src" / "lib.rs").read_text()
caps = json.loads((root / "src-tauri" / "capabilities" / "default.json").read_text())
ui = (root / "ui" / "index.html").read_text()

problems = []
warnings = []

cargo_plugins = set(re.findall(r"^tauri-plugin-([a-z-]+)\s*=", cargo, re.M))
lib_plugins = {p.replace("_", "-") for p in re.findall(r"tauri_plugin_([a-z_]+)", lib)}
cap_plugins = {p.split(":")[0] for p in caps["permissions"] if not p.startswith("core:")}

for p in sorted(lib_plugins - cargo_plugins):
    problems.append(f"lib.rs uses tauri_plugin_{p.replace('-', '_')} but Cargo.toml has no tauri-plugin-{p}")
for p in sorted(cap_plugins - cargo_plugins):
    problems.append(f"capabilities grants '{p}:*' but tauri-plugin-{p} is not a dependency")
for p in sorted(cargo_plugins - lib_plugins):
    warnings.append(f"tauri-plugin-{p} is a dependency but never referenced in lib.rs")

handler = re.search(r"generate_handler!\[(.*?)\]", lib, re.S)
registered = {c.strip() for c in handler.group(1).split(",") if c.strip()} if handler else set()
invoked = set(re.findall(r'invoke\(\s*"([a-z_]+)"', ui))

for c in sorted(invoked - registered):
    problems.append(f"the frontend invokes '{c}' but it is not in generate_handler![...]")
for c in sorted(registered - invoked):
    warnings.append(f"Rust command '{c}' is registered but never invoked from the frontend")

# versions must agree, or the updater won't recognise a new release as newer
vers = {
    "tauri.conf.json": json.loads((root / "src-tauri" / "tauri.conf.json").read_text())["version"],
    "Cargo.toml": re.search(r'^version\s*=\s*"([^"]+)"', cargo, re.M).group(1),
    "package.json": json.loads((root / "package.json").read_text())["version"],
}
if len(set(vers.values())) != 1:
    problems.append(f"version mismatch across files: {vers}")

print(f"plugins    cargo={sorted(cargo_plugins)} lib={sorted(lib_plugins)} caps={sorted(cap_plugins)}")
print(f"commands   registered={len(registered)} invoked={len(invoked)}")
print(f"version    {vers['tauri.conf.json']}")
for w in warnings:
    print(f"  warn  {w}")
for p in problems:
    print(f"  FAIL  {p}")
print("\n" + ("consistency checks passed" if not problems else f"{len(problems)} problem(s) found"))
sys.exit(1 if problems else 0)
