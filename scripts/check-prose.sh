#!/usr/bin/env bash
# Needs bash 3.2 and POSIX tools only
set -euo pipefail

usage() {
  cat <<'EOF'
The mechanical half of the create-readme skill's rules, checked on any markdown

  check-prose.sh DOC...

  -h, --help   print this and exit

The rules are the readme's, and they hold over every document a repository ships — a
SKILL.md and a changelog are read on the same page by the same reader. Only what a
script can decide is here: whether a paragraph ends bare, whether it occupies one line,
whether an admonition is shaped the way GitHub wants it, whether a quotation mark is the
typographic one, and whether a section duplicates a file that already exists. Tone,
structure and honesty about versions stay a reading job. A finding is one line on
stderr, DOC:LINE: what, so an editor can jump to it

A YAML frontmatter block is data rather than prose, and is skipped: its keys are not
paragraphs, and two of them in a row are not a hard wrap

Before it reads the documents it was given, it proves each rule on two documents of its
own: one written to break every rule, where each finding is demanded by name, and one
written to break none, which must be silent. A check that has never been red is a
decoration, and this file travels to repositories that keep no fixture for it
Environment: CHECK_PROSE_NESTED=1 skips that proof, and is how the proof runs this copy

Nothing here reaches the network
Exit 0 when every document keeps the rules, 1 with one line per finding, 2 on a usage
error or a document that does not exist
EOF
}

die() { # the request itself is wrong
  printf 'check-prose: %s\n' "$1" >&2
  exit 2
}

while (($#)); do
  case "$1" in
    -h | --help)
      usage
      exit 0
      ;;
    -*)
      usage >&2
      exit 2
      ;;
    *) break ;;
  esac
done
# No readme is nothing to check, and nothing checked must not read as a clean readme
(($# > 0)) || {
  usage >&2
  exit 2
}
# Every path is looked at before any is read, so a typo is a refusal rather than a
# half-checked run whose findings hide it
for file in "$@"; do
  [[ -f "$file" ]] || die "$file: no such file"
done

# Every rule is proved able to fail before a real document is read. This file travels to
# repositories whose gate runs it and keeps no fixture of its own, so the proof travels
# with it: a document written to break each rule must be reported for each rule by name,
# and one written to break none must be silent. CHECK_PROSE_NESTED=1 is how the proof runs
# this same copy without the proof running itself
if [ -z "${CHECK_PROSE_NESTED:-}" ]; then
  # Through the bash already running this, rather than by the shebang: a copy written by a
  # tool — a gate planting a defect into one, an unpacked archive — may have no execute
  # bit, and a permission denied would read as the first rule catching nothing
  self=${BASH:-bash}
  script=$0
  case $script in */*) ;; *) script=./$script ;; esac
  probe=$(mktemp -d "${TMPDIR:-/tmp}/check-prose.XXXXXX")
  trap 'rm -rf "$probe"' EXIT
  cat >"$probe/bad.md" <<'BAD'
# A document breaking every rule this script knows

````markdown
```
A short fence inside a longer one is text, not the end of the block.
```
````

~~~
A tilde fence closes on tildes.
~~~

    ```
    Four spaces make this indented code, so the line above opens no fence

```a span``` that opens a line is a code span and not a fence, since the info string of a backtick fence holds no backtick

This paragraph ends with a full stop.

**A bold paragraph that still ends with one.**

A remark, then a parenthesis that ends with one. (Like this.)

Some `code at the end that ends with one.`

This paragraph is hard-wrapped across
two lines, which a one-word edit would reflow

> [!NOTE] the keyword does not have its line to itself

A paragraph carrying « on its own, since a pair on one line lets a live half cover a dead one

A paragraph carrying » on its own

A paragraph carrying “ on its own

A paragraph carrying ” on its own

A paragraph carrying ‘ on its own

A paragraph carrying ’ on its own

## License

MIT
BAD
  # Every shape here was a false positive at some point: an ordered list read as a
  # paragraph broken in two, a frontmatter read as two paragraphs in a row, a title
  # naming the contributing skill, a heading that contains one of the words rather than
  # being it, and a code span holding the characters the rules forbid
  cat >"$probe/quiet.md" <<'QUIET'
---
name: a-document
description: "A frontmatter is data. Its keys are not paragraphs, two in a row are not a hard wrap, and a sentence in a field ends the way a sentence does."
---

# A document breaking none of them, titled like the contributing skill

1. A numbered list item, which is a list item and not a paragraph
2. A second one right after it, which is not a hard wrap

## Before contributing to someone else's project

A heading that is the word is a second copy of a file that already exists; one that contains it is a section about the topic

## What goes to git and the changelog

A rule has to be able to show the character it forbids, so a code span holding `«one»`, `“another”` or `‘a third’` is naming a character rather than quoting with it

> [!NOTE]
> An admonition with the keyword alone on its line, the way GitHub wants it

```sh
# Inside a fence, a sentence may end with a full stop.
echo "and a hard wrap
across two lines is code, not prose."
```

````markdown
```sh
echo "a short fence inside a longer one is text, not its end."
```
A line after it is still inside the outer block.
Two of them in a row are not a hard wrap.
````

```
```text with an info string opens a block and closes none
A sentence on the next line is still code.
```

~~~
A tilde fence is a fence.
```
A backtick fence does not close it.
~~~

    An indented block is code too.
    Two lines of it.

Text after a fence, on one line, ending bare
QUIET
  probe_out=$(CHECK_PROSE_NESTED=1 "$self" "$script" "$probe/bad.md" 2>&1 || true)
  for probe_want in \
    'ends with a full stop' \
    'hard-wrapped paragraph' \
    'admonition keyword' \
    'typographic quotation mark' \
    'has its own file'; do
    case $probe_out in
      *"$probe_want"*) ;;
      *)
        printf 'check-prose: the rule reporting "%s" caught nothing on a document written to break it\n' \
          "$probe_want" >&2
        exit 1
        ;;
    esac
  done
  # probe_count WHAT GOT WANT MISSED — a count the document demands exactly. Fewer means a
  # planted case went unreported, for the reason MISSED; more means the rule read a line
  # the document does not mean as prose, such as one inside a fence
  probe_count() {
    [ "$2" -eq "$3" ] && return 0
    if [ "$2" -lt "$3" ]; then
      printf 'check-prose: %s of the %s %s were caught — %s\n' "$2" "$3" "$1" "$4" >&2
    else
      printf 'check-prose: %s %s were reported where the document plants %s — a line that is not prose was read as prose\n' \
        "$2" "$1" "$3" >&2
    fi
    exit 1
  }
  # One message covers six characters, so a live one would cover for a dead one. Each gets
  # a paragraph to itself — a pair sharing a line lets its second half answer for the
  # first — and all six are demanded
  probe_quotes=$(printf '%s\n' "$probe_out" | grep -c 'typographic quotation mark' || true)
  probe_count 'typographic marks' "$probe_quotes" 6 'one of them reports nothing'
  # The same shape again: reading the last character only would pass `.**`, `.)` and a
  # stop inside closing backticks, so the document hides one behind each and all four
  # full stops are demanded
  probe_stops=$(printf '%s\n' "$probe_out" | grep -c 'ends with a full stop' || true)
  probe_count 'full stops' "$probe_stops" 4 'one hidden behind closing markup was not'
  if ! probe_out=$(CHECK_PROSE_NESTED=1 "$self" "$script" "$probe/quiet.md" 2>&1); then
    printf 'check-prose: a document breaking no rule was reported on:\n%s\n' "$probe_out" >&2
    exit 1
  fi
  rm -rf "$probe"
  trap - EXIT
fi

fail=0
file=''
n=0
report() { # report MESSAGE — about $file, at line $n
  printf '%s:%s: %s\n' "$file" "$n" "$1" >&2
  fail=1
}

# Two patterns, held in variables rather than written where they are matched. A bracket class
# containing a `)` has to escape it inside a case pattern, and tree-sitter's bash grammar
# rejects the escape: the whole file then parses as one error, and a checker that reads
# comments out of the tree reports this one as unreadable. Expanded unquoted below, which is
# what makes them patterns rather than strings
# shellcheck disable=SC2016 # the backticks are markdown, not command substitution
TAIL_MARKUP='*[*_)`"]'
# shellcheck disable=SC2016 # the same, and this one is a pair of them
TWO_SPANS='*`*`*'

for file in "$@"; do
  fence='' # the run that opened the fenced block the line is in, empty outside one
  prev_prose=0
  front=0
  n=0
  while IFS= read -r line; do
    n=$((n + 1))
    # A frontmatter block is data, not prose: its keys are neither paragraphs nor a hard
    # wrap, and a description ending on a full stop is a sentence in a field. Only a block
    # opening on the first line is one — a --- further down is a horizontal rule
    if [ "$n" -eq 1 ] && [ "$line" = "---" ]; then
      front=1
      continue
    fi
    if [ "$front" -eq 1 ]; then
      if [ "$line" = "---" ]; then front=0; fi
      continue
    fi
    # A fence is three or more backticks or tildes, indented by at most three spaces. It
    # closes only on the same character, at least as long and with nothing after it, so a
    # block can show a shorter fence as text: a toggle on every ``` line read the lines
    # after the inner one as prose. A backtick fence carries no backtick in its info string,
    # or the line is a code span instead
    lead=${line%%[! ]*}
    body=${line#"$lead"}
    case $body in
      '```'*) mark='`' ;;
      '~~~'*) mark='~' ;;
      *) mark='' ;;
    esac
    [ "${#lead}" -le 3 ] || mark=''
    run=''
    rest=$body
    if [ -n "$mark" ]; then
      while [ "${rest#"$mark"}" != "$rest" ]; do
        run=$run$mark
        rest=${rest#"$mark"}
      done
    fi
    if [ -z "$fence" ]; then
      if [ -n "$mark" ] && { [ "$mark" = '~' ] || [ "${rest#*\`}" = "$rest" ]; }; then
        fence=$run
        prev_prose=0
        continue
      fi
    else
      if [ "$mark" = "${fence%"${fence#?}"}" ] && [ "${#run}" -ge "${#fence}" ] &&
        [ -z "${rest//[[:space:]]/}" ]; then
        fence=''
      fi
      continue
    fi

    # An indented block is code as much as a fenced one is, and the rules below are about
    # prose: a line of shell that ends in a full stop was being reported as a paragraph
    if [ "${line#    }" != "$line" ] || [ "${line#	}" != "$line" ]; then
      prev_prose=0
      continue
    fi

    # A rule has to be able to show the character it forbids, and a code span is how prose
    # says "this one, as a character" rather than using it. The quotation rule below reads
    # the line without its spans for that reason alone. The full stop rule does not: a
    # span closing a paragraph is still the last thing the reader sees, so a stop inside
    # it is a stop at the end of the paragraph
    nospan=$line
    # shellcheck disable=SC2053 # unquoted on purpose: quoted it would be a string, not a pattern
    while [[ $nospan == $TWO_SPANS ]]; do
      rest=${nospan#*\`}
      nospan=${nospan%%\`*}${rest#*\`}
    done

    # Rule 4: sections that have their own file at the root of a repository. The level-one
    # heading is the readme's own title, the project's name, which may well be a contributing
    # skill or a changelog tool; the sections below it are what the rule is about. The
    # heading has to be the word rather than contain it: "Before contributing to someone
    # else's project" is a section about contributing, not a copy of CONTRIBUTING.md
    case $line in
      '# '*) ;;
      '#'*)
        title=$line
        while [[ $title == '#'* ]]; do
          title=${title#\#}
        done
        title=${title# }
        case $title in
          [Ll]icense | LICENSE | [Cc]ontributing | CONTRIBUTING | [Cc]hangelog | CHANGELOG)
            report "a heading for something that has its own file: ${line}"
            ;;
        esac
        ;;
    esac

    # A quotation mark is the plain one. The pairs are written here as the bytes they are,
    # since a source file carrying the character it forbids teaches the wrong thing to
    # whoever copies a line out of it: U+00AB/BB, U+201C/201D and U+2018/2019. The single
    # pair catches an apostrophe and the eyes of a kaomoji as well as a quotation, and
    # that is the point — none of the three belongs in a document that is read as plain
    # text somewhere
    case $nospan in
      *$'\xc2\xab'* | *$'\xc2\xbb'* | *$'\xe2\x80\x9c'* | *$'\xe2\x80\x9d'* | *$'\xe2\x80\x98'* | *$'\xe2\x80\x99'*)
        report "a typographic quotation mark — quote with the plain \" instead"
        ;;
    esac

    # Rule 5: the admonition keyword takes its line alone, or GitHub renders a
    # plain quote instead of the box
    case $line in
      '> [!'*']'?*)
        report "text on the admonition keyword's line — it belongs below"
        ;;
    esac

    # Rule 6: one paragraph is one line. Badge rows, tables, lists, headings and
    # html are not paragraphs; two prose lines in a row are a hard wrap. A numbered
    # list is a list: `1.` and `2.` on consecutive lines were being read as one
    # paragraph broken in two, and any document with an ordered list was reddened
    if [[ "$line" =~ ^[0-9]+[.\)][[:space:]] ]]; then
      prev_prose=0
    else
      case $line in
        '' | '#'* | '-'* | '*'* | '|'* | '>'* | '<'* | '!['* | '['* | ' '*)
          prev_prose=0
          ;;
        *)
          [ "$prev_prose" -eq 1 ] && report "a hard-wrapped paragraph — one paragraph is one line"
          prev_prose=1
          ;;
      esac
    fi

    # Rule 7: a paragraph, a list item and a table cell all end bare — read through the
    # markup that can close after the stop, since `.**`, `.)` and `` .` `` end on one too
    bare=$line
    # shellcheck disable=SC2053 # unquoted on purpose: quoted it would be a string, not a pattern
    while [[ $bare == $TAIL_MARKUP ]]; do
      bare=${bare%?}
    done
    case $bare in
      *..) ;;
      *[!.].)
        report "ends with a full stop"
        ;;
    esac
  done <"$file"
done

((fail == 0)) || exit 1
printf 'check-prose: %s document(s) keep every rule a script can decide\n' "$#"
