#!/usr/bin/env python3
# SPDX-License-Identifier: MIT
"""Holds the rendered managed settings to the two promises the module makes.

The module renders a TOML file for a managed home. It may name a value only where it owns the
decision: the engine and the weights of the catalog, the language, the transcribe.cpp program
and backend, the version of the format and the interface language. Every other value belongs
to the core defaults, and a managed home changes one through `programs.molvai.settings`.

The check reads the file itself and not the option tree, because the file is what a managed
home runs. Two failures are reported, and each one names the case and the path:

- a key in the file that the module neither owns nor received through `settings`: a policy
  hidden in the module, which makes a managed home behave unlike any other install
- a path in `settings` that is absent from the file or holds another value: a setting that
  the option promises and the file never receives
"""

import json
import sys
import tomllib

# The keys the module decides. A new entry here is a decision about every managed home, so
# this list is the place to read it, not the `generated` block of the module
OWNED = {
    "version",
    "ui_language",
    "stt.engine",
    "stt.model",
    "stt.model_path",
    "stt.language",
    "stt.transcribe_cli",
    "stt.transcribe_backend",
}


def leaves(tree, prefix=""):
    """Every path to a value, with tables opened into dotted names."""
    for key, value in tree.items():
        path = f"{prefix}{key}"
        if isinstance(value, dict) and value:
            yield from leaves(value, path + ".")
        else:
            yield path, value


def main(dump_path):
    with open(dump_path, encoding="utf-8") as handle:
        cases = json.load(handle)

    failures = []
    checked = 0
    for name, case in sorted(cases.items()):
        config = case.get("configFile")
        if not config:
            continue
        checked += 1
        with open(config, "rb") as handle:
            rendered = dict(leaves(tomllib.load(handle)))
        wanted = dict(leaves(case.get("settings") or {}))

        for path, value in wanted.items():
            if path not in rendered:
                failures.append(f"{name}: settings.{path} never reaches the file")
            elif rendered[path] != value:
                failures.append(
                    f"{name}: settings.{path} is {rendered[path]!r} in the file, not {value!r}"
                )

        for path, value in rendered.items():
            if path not in OWNED and path not in wanted:
                failures.append(f"{name}: the module forces {path} = {value!r}")

    if failures:
        print("module wiring:")
        for failure in failures:
            print(f"  {failure}")
        return 1
    print(f"module wiring: {checked} rendered file(s), every key owned or passed through")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1]))
