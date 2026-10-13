# How to contribute

## Commits

A commit is small and complete: one thought, one reason to revert. You can name such a commit in one line, and `just bisect` finds the guilty commit in logarithmic time, not in an evening

Each commit passes the whole gate:

```sh
nix develop --command just check
```

The gate is `fmt`, `clippy -D warnings`, the tests through `scripts/t.sh`, the SPDX headers, and no `todo!`/`unimplemented!` in product code. CI runs the same list and calls the same `just` recipes: a copy of the commands in the workflow would drift from the original within a week

The history is linear and never rewritten: no `--amend`, no `rebase` of a pushed commit, no `push --force`, and no squash on merge. A published commit stays in place, even if it is a bad one. The fix goes in the next commit and explains what was wrong

Messages are in English, in the conventional commits style: `feat(audio): …`, `fix(inject): …`, `ci: …`, `docs: …`. The commit body answers "why" and does not retell the diff

## Tests

Write the test before the code and watch it fail at least once: a test that never failed protects nothing. Add the guarantee that the test protects to `tests/defects.sh` as one entry. The entry breaks this guarantee and does not break the compilation

```sh
just falsify            # break each guarantee in turn and require the suite to notice
just falsify audio      # only the entries whose name contains "audio"
just flaky 10           # ten runs in a row: do the results differ on the same code
just bisect <good>      # which commit broke the suite
```

A defect that the suite survived points to a place where nobody checks anything. This is a finding, not a minor nuisance

## AI assistance

AI helped to write parts of this repository, and each commit discloses it. A commit whose diff a machine wrote in a substantial part carries a trailer with the name of the tool and the model:

    Generated-by: Claude Code:claude-opus-5
    Assisted-by: Claude Code:claude-opus-5 (mostly)

`Generated-by:` means the tool did the whole task: a person set it, checked it and accepted it as it is. `Assisted-by:` means a person led the work by hand. Use `(mostly)` when the tool wrote most of the final diff, and `(partly)` when it wrote a substantial part. A commit without a trailer was written by hand, dictated line by line, or is mechanical, for example a formatter run or a rename through grep

We expect the same from changes by other people. Name the tool and the model in a trailer or in the description of the pull request. Do not use `Co-authored-by:` for a tool: it is reserved for human co-authors, and [nixpkgs](https://github.com/NixOS/nixpkgs/blob/master/CONTRIBUTING.md), [Mesa](https://gitlab.freedesktop.org/mesa/mesa/-/blob/main/docs/submittingpatches.rst) and the [Linux kernel](https://docs.kernel.org/process/coding-assistants.html) all refuse to count it as a disclosure. And never sign off for a tool: only a human can certify the Developer Certificate of Origin

The person who opens a pull request is responsible for it. Read what the tool wrote, understand it, and be ready to discuss it without passing the questions back to the tool. Undisclosed generated code is the one reason to close a pull request without reading it

Where the split is honestly unclear, the right answer is a bare `Assisted-by:` with no qualifier: it stays true for everything worth arguing about. The convention and the project policies behind it are collected in [rokokol/ai-commit-trailers-skill](https://github.com/rokokol/ai-commit-trailers-skill)
