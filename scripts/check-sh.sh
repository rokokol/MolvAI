#!/usr/bin/env bash
# Other repositories take this file through the vendoring cascade (references/bump-cascade.md
# in https://github.com/rokokol/ci-skill): a copy is never edited in place, a change is made
# here and reaches them from here
# Needs bash 3.2 and POSIX tools only for its own code, so it runs on a macOS runner
# unchanged; the script it is given is read as a tree, by shfmt and jq, and without them
# only the checks that ask this bash a question run (--bash-only)
set -euo pipefail

usage() {
  cat <<'EOF'
check-sh.sh — holds a shell script's help, documents and completions to its code

The help is the single source of truth for what a script accepts, so every subcommand its
dispatcher has, every flag its parsers take, every variable it reads and every code it
exits with must be in the help — and every document and completion that restates a list
is held to the same code, in both directions. Each check is proven able to fail on every
run, on a canonical script with one defect planted, so a copy falsifies itself wherever
it runs. It has no repo-specific part, and belongs in a repository's own gate

  check-sh.sh [-n NAME] [-e PREFIX] [-d DOC]... [-m DOC]... [-c BASH ZSH] SCRIPT
  check-sh.sh --template [script|bash|zsh]

  -n NAME      what the help, the docs and the completions call the script (default:
               the script's basename)
  -e PREFIX    the script reads environment variables with this prefix; each must be
               in the help
  -d DOC       a document that lists the script's subcommands, checked both ways: every
               subcommand named, and every `NAME word` it spells real; repeatable
  -m DOC       a document that mentions only some of them and sends the reader to the
               help for the rest: every `NAME word` it spells must be real; repeatable
  -c BASH ZSH  the two completion files, checked both ways
  -b, --bash-only
               run only the checks that ask this bash a question, and none that read the
               script as a tree; it needs no shfmt and no jq. It exists for a runner that
               has neither, says in the summary that the tree half was skipped, and is
               never chosen on its own — a missing tool is a refusal, not a quiet pass
  --template   print the canonical script, or its bash or zsh completion, and exit

The shapes it reads are the standard's own: a `case "$cmd"` dispatcher at the top level
with `-h | --help | help)` and a `*)` arm that sends usage to stderr, flag arms such as
`-n | --dry-run)` inside cmd_<sub>() functions or at the top level, literal `exit N`, a
help printed from a heredoc or by a `help [SUB]` subcommand, and a header comment that
lists nothing. They are spelled out in references/shape.md and help.md of
https://github.com/rokokol/bash-best-practices-skill. A header line claiming "Needs bash
X.Y" turns on a grep for constructs newer than that floor, and "POSIX tools only" one for
flags a BSD userland lacks or reads another way; a grep is a proxy, and the proof is a run
under the bash the claim names

CHECK_SH_NESTED=1 runs the checks and skips the self-test. The self-test runs itself that
way, and so should a gate that calls this script more than once in one run: the copy and
its tools are the same for every call, so proving it again proves nothing new. The first
call of every run keeps it: it is what notices a copy that stopped catching defects, and
the bash and tools under a copy change without the copy changing. A call with a finding
exits 1 before the self-test, so the call that keeps it is one that passes, and the
variable changes nothing on a call meant to go red

Nothing here reaches the network
Exit 0 when everything agrees, 1 with one `check-sh: <what>` line per finding, 2 on a
usage error, an unreadable file, a --help that fails, or a script with nothing to check
EOF
}

self=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)/$(basename -- "${BASH_SOURCE[0]}")

die() { # a usage error, never a finding
  printf 'check-sh: %s\n' "$1" >&2
  exit 2
}

# ---- the canonical script, and the completions that mirror it -------------------------
# The self-test below plants defects into this script, and `--template` prints it, so the
# shape the checker proves itself on and the shape it hands out are one text.

template_script() {
  cat <<'TEMPLATE'
#!/usr/bin/env bash
# Needs bash 3.2 and POSIX tools only
set -euo pipefail

usage() {
  cat <<'EOF'
script.sh — one line saying what it is and what it is for

  script.sh run [-n|--dry-run] [-l DIR]    do the thing, in DIR
  script.sh stop                           stop doing it

  -n, --dry-run   say what would be done and do nothing
  -l DIR          the log directory (default: $SCRIPT_LOGDIR, else the current one)

Environment: SCRIPT_LOGDIR is the log directory when -l is not given
Nothing here reaches the network
Exit 0 done, 1 when the thing asked about is wrong, 2 on a usage error
EOF
}

fail() { # the thing asked about is wrong
  printf 'script.sh: %s\n' "$1" >&2
  exit 1
}

die() { # the request itself is wrong
  printf 'script.sh: %s\n' "$1" >&2
  exit 2
}

HERE=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)

# >>> EXAMPLE: the subcommands, one cmd_<name>() each, with their parser inside
cmd_run() {
  local dry=0 logdir="${SCRIPT_LOGDIR:-.}"
  while (($#)); do
    case "$1" in
      -n | --dry-run)
        dry=1
        shift
        ;;
      -l)
        # Not ${2:?}: that exits 1 with bash's own message, and a usage error is 2
        (($# >= 2)) || die "-l needs a directory"
        logdir="$2"
        shift 2
        ;;
      -*) die "no such flag: $1" ;;
      *) break ;;
    esac
  done
  [[ -d "$logdir" ]] || fail "$logdir is not a directory"
  if ((dry)); then
    printf 'would run in %s\n' "$logdir"
  else
    printf 'ran in %s from %s\n' "$logdir" "$HERE"
  fi
}

cmd_stop() {
  printf 'stopped\n'
}
# <<< EXAMPLE

cmd="${1:-}"
(($# == 0)) || shift
case "$cmd" in
  run) cmd_run "$@" ;;
  stop) cmd_stop "$@" ;;
  -h | --help | help) usage ;;
  '')
    usage >&2
    exit 2
    ;;
  *)
    printf 'script.sh: no such subcommand: %s\n\n' "$cmd" >&2
    usage >&2
    exit 2
    ;;
esac
TEMPLATE
}

template_bash() {
  cat <<'EOF'
# shellcheck shell=bash
# Tab completion for script.sh in bash. Hand-written on purpose and drift-checked by
# machine: check-sh.sh -c holds every word here to the script's dispatcher and parsers.
# Builtins only, so it works without the bash-completion package and under the bash 3.2
# a stock macOS sources it with
_script_sh() {
  local cur prev words
  cur="${COMP_WORDS[COMP_CWORD]}"
  prev="${COMP_WORDS[COMP_CWORD - 1]}"
  if ((COMP_CWORD == 1)); then
    words="run stop help"
  else
    case "${COMP_WORDS[1]}" in
      run)
        case "$prev" in
          -l)
            compopt -o dirnames 2>/dev/null || true
            return
            ;;
        esac
        words="-n --dry-run -l"
        ;;
      *) words="" ;;
    esac
  fi
  COMPREPLY=()
  while IFS= read -r word; do
    [[ -n "$word" ]] && COMPREPLY+=("$word")
  done < <(compgen -W "$words" -- "$cur")
}
complete -F _script_sh script.sh
EOF
}

template_zsh() {
  cat <<'EOF'
#compdef script.sh
# Tab completion for script.sh in zsh. Hand-written on purpose and drift-checked by
# machine: check-sh.sh -c holds every word here to the script's dispatcher and parsers.
# The #compdef line binds it when the file sits on $fpath as _script.sh, and the last
# line calls the function, which is the autoload convention
_script_sh() {
  local -a subcommands
  subcommands=(
    'run:do the thing'
    'stop:stop doing it'
    'help:show the help'
  )
  _arguments -C \
    '1:subcommand:->subcommand' \
    '*::arguments:->arguments'
  case "$state" in
    subcommand) _describe 'subcommand' subcommands ;;
    arguments)
      case "${words[1]}" in
        run)
          _arguments \
            '(-n --dry-run)'{-n,--dry-run}'[say what would be done]' \
            '-l[the log directory]:directory:_directories'
          ;;
      esac
      ;;
  esac
}
_script_sh "$@"
EOF
}

# ---- arguments --------------------------------------------------------------------
name=""
prefix=""
docs=()
mentions=()
comp_bash=""
comp_zsh=""
script=""
bash_only=0
while (($#)); do
  case "$1" in
    -b | --bash-only)
      bash_only=1
      shift
      ;;
    -n)
      (($# >= 2)) || die "-n needs a name"
      name="$2"
      shift 2
      ;;
    -e)
      (($# >= 2)) || die "-e needs a prefix"
      prefix="$2"
      shift 2
      ;;
    -d)
      (($# >= 2)) || die "-d needs a document"
      docs+=("$2")
      shift 2
      ;;
    -m)
      (($# >= 2)) || die "-m needs a document"
      mentions+=("$2")
      shift 2
      ;;
    -c)
      (($# >= 3)) || die "-c needs two files, the bash and the zsh completion"
      comp_bash="$2"
      comp_zsh="$3"
      shift 3
      ;;
    --template)
      case "${2:-script}" in
        script) template_script ;;
        bash) template_bash ;;
        zsh) template_zsh ;;
        *) die "no such template: $2 — script, bash or zsh" ;;
      esac
      exit 0
      ;;
    -h | --help)
      usage
      exit 0
      ;;
    -*)
      usage >&2
      exit 2
      ;;
    *)
      [[ -z "$script" ]] || die "one script at a time"
      script="$1"
      shift
      ;;
  esac
done
[[ -n "$script" ]] || {
  usage >&2
  exit 2
}
[[ -r "$script" ]] || die "cannot read $script"
[[ -n "$name" ]] || name=$(basename -- "$script")
for f in "${docs[@]+"${docs[@]}"}" "${mentions[@]+"${mentions[@]}"}" "$comp_bash" "$comp_zsh"; do
  [[ -z "$f" || -r "$f" ]] || die "cannot read $f"
done

findings=0
finding() {
  printf 'check-sh: %s\n' "$1" >&2
  findings=$((findings + 1))
}

# With a template, because the BSD mktemp on macOS wants one
work=$(mktemp -d "${TMPDIR:-/tmp}/check-sh.XXXXXX")
trap 'rm -rf "$work"' EXIT

# ---- the tree -------------------------------------------------------------------------
# `shfmt --to-json` emits mvdan.cc/sh's whole syntax tree, with a line number on every
# node, and the jq program below flattens it into one row per fact. The rules above read
# those rows with awk and grep, as they always have, so the non-POSIX dependency lives in
# this one stage: a change in shfmt's JSON is a change to this program and to nothing else
#
# The columns are LINE, KIND, FN, SUBST, OWNER, A, B. FN is the outermost enclosing
# function, which is what the awk lexer above tracks — it matches a function opening at
# column 0 — so both read a nested function's contents as its outermost one's. SUBST is 1
# inside `$( )` and `<( )` and 0 inside backticks and `$(( ))`, which is exactly the
# bash 3.2 heredoc rule
facts_jq() {
  cat <<'JQ'
def part_text:
  if .Type == "Lit" or .Type == "SglQuoted" then (.Value // "")
  elif .Type == "DblQuoted"
    then (if [(.Parts // [])[] | .Type] | all(. == "Lit")
          then [(.Parts // [])[] | .Value] | join("") else null end)
  else null end;

# A word's literal text when every part is literal, "$" when anything in it expands
def word_text:
  if . == null then "-"
  elif (.Parts // []) | length == 0 then ""
  else ([.Parts[] | part_text] | if any(. == null) then "$" else join("") end) end;

# A pattern as bash matches it: an unquoted part is a glob, and a quoted or expanded part
# is literal text, so it becomes `_`, which no bracket range can use as an end
def glob_text:
  [(.Parts // [])[]
   | if .Type == "Lit" then .Value
     elif .Type == "ExtGlob" then .Op + (.Pattern.Value // "") + ")"
     else "_" end]
  | join("") | if . == "" then "-" else . end;

# `"$cmd"` is a DblQuoted holding a ParamExp; `$cmd` is the ParamExp bare
def case_word:
  (.Parts[0] // {}) as $p
  | if $p.Type == "ParamExp" then "$" + $p.Param.Value
    elif $p.Type == "DblQuoted" and (($p.Parts[0] // {}).Type == "ParamExp")
      then "$" + $p.Parts[0].Param.Value
    else ($p.Value // "?") end;

# What an assignment is worth as text: the word, or an array's elements joined. A list
# wrapped onto a second line is one array either way, which a reader of the text has to
# work out and a reader of the tree does not
def assign_text:
  (if .Array then ([(.Array.Elems // [])[] | (.Value | word_text)] | join(" "))
   else ((.Value // null) | word_text) end)
  | if . == "" then "-" else . end;

def emit($fn; $subst):
  if type == "array" then (.[] | emit($fn; $subst))
  elif type != "object" then empty
  else
    if .Type == "FuncDecl" then
      [.Pos.Line, "func", $fn, $subst, "-", .Name.Value, (.End.Line | tostring)],
      # The name binds before the descent, so the outermost declaration keeps FN
      (.Name.Value as $n
       | .Body | emit((if $fn == "-" then $n else $fn end); $subst))

    elif .Type == "CaseClause" then
      (.Pos.Line as $c
       | [.Pos.Line, "case", $fn, $subst, "-", (.Word | case_word), (.End.Line | tostring)],
         (.Items[]?
          # B is the arm's last line, which is what scopes a row to an arm: a redirection
          # or a comment belongs to the arm whose range holds its line
          | [.Pos.Line, "arm", $fn, $subst, ($c | tostring),
             ([.Patterns[]? | word_text] | join("|")), (.End.Line | tostring)],
            # And a row of its own where the terminator is not the ordinary `;;`, since
            # `;&` and `;;&` fall through and the bash floor they need is 4.0
            (if (.Op // ";;") != ";;"
             then [.Pos.Line, "armop", $fn, $subst, ($c | tostring), .Op, "-"]
             else empty end),
            # One row per pattern, read as a glob: the arm row above keeps quoted text,
            # which bash matches literally, so a bracket range in it is not one
            (.Patterns[]? | [.Pos.Line, "glob", $fn, $subst, "-", glob_text, "-"]),
            (.Stmts | emit($fn; $subst))))

    # SUBST is 1 inside $( ) and < ( ) and stays 0 inside backticks, which is the bash 3.2
    # heredoc rule exactly: 3.2 scans a heredoc's body as code while looking for the end of
    # a $( ), and reads one inside backticks correctly. shfmt marks the second with a
    # Backquotes key that is absent on the first
    elif .Type == "CmdSubst" or .Type == "ProcSubst" then
      # Bound before the pipe: inside .Stmts the dot is the array and the key is gone
      ((if (.Backquotes // false) then $subst else 1 end) as $s
       | .Stmts | emit($fn; $s))

    # One row per `|`, naming the reader on its right: which reader it is decides whether
    # the producer on the left can be killed by SIGPIPE part-way through
    elif .Type == "BinaryCmd" and (.Op == "|" or .Op == "|&") then
      ((.Y.Cmd // {}) as $r
       | [.OpPos.Line, "pipeinto", $fn, $subst, "-",
          ((($r.Args // [])[0] // null) | word_text),
          ([($r.Args // [])[1:][]? | word_text] | join(" ") | if . == "" then "-" else . end)]),
      # `|&` is a 4.0 construct, so the operator gets a row of its own where it is not the
      # plain `|`, the way an arm's terminator does
      (if .Op != "|" then [.OpPos.Line, "pipeop", $fn, $subst, "-", .Op, "-"] else empty end),
      (to_entries[] | .value | emit($fn; $subst))

    elif .Type == "CallExpr" then
      [.Pos.Line, "call", $fn, $subst, "-",
       ((.Args[0] // null) | word_text),
       # "-" rather than the empty string, so no row ends in a tab: the golden table
       # below is a heredoc, and trailing whitespace there is what an editor eats
       ([.Args[1:][]? | word_text] | join(" ") | if . == "" then "-" else . end)],
      # Assigns as well as Args: a bare `v=$(…)` is a CallExpr with no Args at all, so a
      # descent into Args alone loses every assignment and the substitutions inside them
      ((.Assigns // [])[] | select(.Name != null)
       | [.Pos.Line, "assign", $fn, $subst, "-", .Name.Value, assign_text]),
      (.Args | emit($fn; $subst)), (.Assigns | emit($fn; $subst))

    # A test operator inside `[[ ]]`, one row per operator: `-v` is a 4.2 construct, and a
    # pattern over text has to tell it from the same two characters inside a string
    elif .Type == "UnaryTest" or .Type == "BinaryTest" then
      [.OpPos.Line, "test", $fn, $subst, "-", (.Op // "-"), "-"],
      # The right side of ==, = and != is a pattern, the same glob a case arm holds
      (if .Op == "==" or .Op == "=" or .Op == "!="
       then [.Y.Pos.Line, "glob", $fn, $subst, "-", (.Y | glob_text), "-"]
       else empty end),
      (to_entries[] | .value | emit($fn; $subst))

    # declare, local, typeset, export and readonly are a clause of their own and not a
    # call. A is the word written, B its flags joined, so `declare -rA` and `local -Ar`
    # answer the same question as `declare -A` — which a pattern over text does not
    elif .Type == "DeclClause" then
      [.Pos.Line, "decl", $fn, $subst, "-", (.Variant.Value // "-"),
       ([(.Args // [])[] | select(.Name == null) | (.Value | word_text)]
        | join(" ") | if . == "" then "-" else . end)],
      # A declared variable is an assignment too: `local -a w=(…)` holds a list the same
      # way `w=(…)` does, and a rule reading assignments should not have to know which
      ((.Args // [])[] | select(.Name != null)
       | [.Pos.Line, "assign", $fn, $subst, "-", .Name.Value, assign_text]),
      (.Args | emit($fn; $subst))

    elif .Type == "ParamExp" then
      # B is the operator where there is one, and otherwise says which of the two shapes
      # that carry none this is. A slice with a negative length and a replacement whose
      # text is quoted or holds & are each a bash floor of their own, and none of the three
      # is an Exp.Op: shfmt gives a slice its own Slice and a replacement its own Repl
      ([.Pos.Line, "param", $fn, $subst, "-", (.Param.Value // "-"),
        # `@Q` and its siblings are one operator and one letter, and only some letters are
        # 4.4; the letter is the Exp's own word, so B carries both
        (if .Exp.Op then (.Exp.Op + (if .Exp.Op == "@" then ((.Exp.Word.Parts // [])[0].Value // "") else "" end))
         elif .Slice then (if (.Slice.Length.Op // "") == "-" then "slice-neg" else "slice" end)
         elif .Repl then
           ([(.Repl.With.Parts // [])[]
             | if (.Type == "DblQuoted" or .Type == "SglQuoted") then "q"
               elif (.Value // "") | test("&") then "a"
               else "" end]
            | join("")
            | if test("q") then "repl-quoted" elif test("a") then "repl-amp" else "repl" end)
         else "-" end)]),
      (to_entries[] | .value | emit($fn; $subst))

    # A redirection carries no "Type" of its own, and Hdoc is absent rather than null
    # unless it opens a heredoc, so neither can identify one. OpPos it always has, and the
    # only other nodes carrying OpPos — BinaryCmd, BinaryArithm, UnaryTest — all have a Type
    elif has("OpPos") and (has("Type") | not) then
      [.Pos.Line, "redir", $fn, $subst, "-", (.Op // "-"), (.Word | word_text)],
      # A descriptor named by a variable, `exec {fd}<file`, is a 4.1 construct. shfmt puts
      # the `{fd}` in the redirection's N, where a plain `2>` leaves it unset
      (if ((.N.Value // "") | startswith("{"))
       then [.Pos.Line, "fdvar", $fn, $subst, "-", .N.Value, (.Op // "-")]
       else empty end),
      # A is the delimiter as written, quotes kept: `<<'EOF'` and `<<EOF` are the same
      # word to a reader of text and two different things to bash, since the second
      # expands the body. help.md asks for the first, and this is what lets it be checked
      (if (.Hdoc // null) != null
       then [.Pos.Line, "heredoc", $fn, $subst, "-",
             (if (((.Word.Parts // [])[0] // {}).Type == "SglQuoted")
              then "'" + (.Word | word_text) + "'" else (.Word | word_text) end),
             (.Hdoc.End.Line | tostring)],
            # And whether the body actually substitutes anything. The body is not walked —
            # a variable named in a help text is not one the script reads, which is the
            # whole reason the env check moved off a grep over the file — so the question
            # is answered here, from the parts, and nothing from inside becomes a row
            (if [(.Hdoc.Parts // [])[] | .Type] | any(. != "Lit")
             then [.Pos.Line, "heredocexp", $fn, $subst, "-",
                   (.Word | word_text), (.Hdoc.End.Line | tostring)]
             else empty end)
       else empty end),
      (.Word | emit($fn; $subst))

    else (to_entries[] | .value | emit($fn; $subst))
    end
  end;

# Comments hang off the statements they precede rather than off the file, and carry no
# "Type" either; Hash, the position of the `#`, is what marks one. They are gathered in
# their own pass so that the descent above stays about code
[.. | objects | select(has("Hash"))
 | [.Pos.Line, "comment", "-", 0, "-", .Text, "-"]] as $c
| ($c[], (.Stmts | emit("-"; 0)))
| @tsv
JQ
}

read_tree() { # read_tree FILE -> the facts table on stdout
  # --to-json reads stdin only, measured; --filename is what lets the dialect be guessed,
  # and it is taken before the pipeline so the file is named once inside it
  local f="$1" base
  base=$(basename -- "$f")
  shfmt --to-json --filename "$base" -ln auto <"$f" | jq -r "$(facts_jq)"
}

# A script exercising every kind of row the table has, and the table it must produce. The
# pair is the frontend's own regression test and, on every run that reads a tree, the
# check that shfmt's JSON still has the shape this program reads
#
# shfmt moved that shape once: 3.13.1 wrote `"Op": 71` where 3.14.1 writes `"Op": "<<"`.
# A version number is a proxy for the shape, so nothing here compares one — this compares
# the shape itself, which is what a rule downstream actually depends on. It runs before
# any rule reads a row, so a shape that moved is one refusal naming the tool rather than a
# scattering of findings, or worse a quiet under-report
tree_probe() {
  cat <<'PROBE'
#!/usr/bin/env bash
# probe
declare -A m
f() {
  case "$1" in
    -n | "[a-z]"[a-z]) echo "x" ;;&
  esac
  cat <<'HD'
body
HD
  g="${2:?need}"
  h=$(printf '%s' ok)
  echo a | grep -q b
  [[ -v g && $g != *[!a-z]* ]]
  i="${g:1:-2}${g//x/"y"}"
}
PROBE
}

# What the probe must flatten to. A run that disagrees prints the table it got, which is
# both the diagnosis and the replacement for this heredoc — read that diff like code, since
# pasting it unread turns whatever shfmt started doing into what this file expects
tree_golden() {
  cat <<'GOLDEN'
1	comment	-	0	-	!/usr/bin/env bash	-
2	comment	-	0	-	 probe	-
3	decl	-	0	-	declare	-A
3	assign	-	0	-	m	-
4	func	-	0	-	f	16
5	case	f	0	-	$1	7
6	arm	f	0	5	-n|[a-z][a-z]	6
6	armop	f	0	5	;;&	-
6	glob	f	0	-	-n	-
6	glob	f	0	-	_[a-z]	-
6	call	f	0	-	echo	x
8	call	f	0	-	cat	-
8	redir	f	0	-	<<	HD
8	heredoc	f	0	-	'HD'	10
11	call	f	0	-	-	-
11	assign	f	0	-	g	$
11	param	f	0	-	2	:?
12	call	f	0	-	-	-
12	assign	f	0	-	h	$
12	call	f	1	-	printf	%s ok
13	pipeinto	f	0	-	grep	-q b
13	call	f	0	-	echo	a
13	call	f	0	-	grep	-q b
14	test	f	0	-	&&	-
14	test	f	0	-	-v	-
14	test	f	0	-	!=	-
14	glob	f	0	-	*[!a-z]*	-
14	param	f	0	-	g	-
15	call	f	0	-	-	-
15	assign	f	0	-	i	$
15	param	f	0	-	g	slice-neg
15	param	f	0	-	g	repl-quoted
GOLDEN
}

# Both tools, then the shape, before a single rule reads a row
tree_preflight() {
  local missing=""
  command -v shfmt >/dev/null 2>&1 || missing="shfmt"
  command -v jq >/dev/null 2>&1 || missing="${missing:+$missing and }jq"
  [[ -z "$missing" ]] ||
    die "needs $missing to read SCRIPT as a tree, and nix develop -c is where the pinned one lives; on a machine with neither, --bash-only runs the checks that ask this bash and says so"

  tree_probe >"$work/probe.sh"
  read_tree "$work/probe.sh" >"$work/probe.tsv" 2>"$work/probe.err" ||
    die "shfmt or jq could not read the built-in probe: $(tr '\n' ' ' <"$work/probe.err")"
  tree_golden >"$work/probe.want"
  # The table it did produce goes with the refusal: it is what says how the shape moved,
  # and it is what replaces tree_golden's heredoc once a person has read the difference
  cmp -s "$work/probe.tsv" "$work/probe.want" || {
    printf 'check-sh: the tree from shfmt %s is not the one tree_golden describes — either the tool moved or the table did, and 3.14.0 is the oldest known to agree\n' \
      "$(shfmt --version 2>/dev/null || echo '(version unknown)')" >&2
    printf 'check-sh: the probe flattened to this instead, which is what tree_golden would become:\n' >&2
    sed 's/^/  /' "$work/probe.tsv" >&2
    exit 2
  }
}

# Run it here, as soon as the frontend exists and long before any rule reads a row, so a
# machine without the tools is refused rather than quietly checked with less. Degrading on
# a missing tool was rejected: an extractor that finds nothing must never read as "nothing
# drifted", or a bare machine exits 0 and believes the help agrees with the code
((bash_only)) || tree_preflight

# ERE-quoted, so a name with a dot matches itself and nothing else
name_re=$(printf '%s' "$name" | sed 's/[][\\.*^$/+?(){}|]/\\&/g')

# Whole tokens only. A substring match let `-f` pass on a completion that offered only
# `--force`, and `grep -w` treats the hyphen as a separator, accepting `--help` inside
# `--help-all`. A `]` is literal only first in a bracket expression, and `\]` is not an
# escape there — GNU grep 3.12 read the escaped form as the bracket's end.
has_token() { # has_token TOKEN <<<TEXT -> 0 when TEXT holds TOKEN as a whole token
  grep -qE -- "(^|[^[:alnum:]_-])$1([^[:alnum:]_-]|\$)"
}

# What a completion file offers, without the prose around it. A word in a comment is not
# offered, and neither is one in a zsh description: the text in `[...]` after an option,
# the part after the colon of a `'sub:description'` entry, the message between the first
# colons of an `'N:message:action'` spec — whose action, `(run stop)`, is kept. Nor is a
# case pattern opening a line inside `case … in … esac`, `-l)` or `--prefix | --destdir)`:
# that arm handles a flag's value and offers nothing, and a flag dropped from the list
# survived there. Only single words joined by `|` count as a pattern, so the last line of
# an array wrapped onto two, `    --no-configure --uninstall)`, is still read as offered.
# Quotes are walked a line at a time, so a `#` inside them is not a comment.
offered_words() { # offered_words FILE 0|1 -> FILE with comments dropped, and zsh prose when 1
  awk -v zsh="$2" '
    function prose(s, n, f, i, out) {
      if (!zsh) return s
      gsub(/\[[^]]*\]/, "", s)
      n = split(s, f, ":")
      if (n < 2) return s
      if (n == 2) return f[1]
      out = f[1]
      for (i = 2; i <= n && f[i] == ""; i++) out = out ":"
      out = out ":"
      for (i++; i <= n; i++) out = out ":" f[i]
      return out
    }
    {
      line = $0; out = ""; q = ""; body = ""
      if ($0 ~ /^[ \t]*case .* in[ \t]*$/) incase++
      else if (incase && $0 ~ /^[ \t]*esac([ \t;]|$)/) incase--
      else if (incase) sub(/^[ \t]*[^ \t|()]+([ \t]*[|][ \t]*[^ \t|()]+)*\)/, "", line)
      while (line != "") {
        c = substr(line, 1, 1)
        if (q == "") {
          if (c == "#" && (out == "" || out ~ /[ \t]$/)) break
          if (c == "\047" || c == "\"") { q = c; body = "" } else out = out c
          line = substr(line, 2)
        } else if (c == "\\" && q == "\"") {
          body = body substr(line, 1, 2); line = substr(line, 3)
        } else if (c == q) {
          out = out q prose(body) q; q = ""; line = substr(line, 2)
        } else {
          body = body c; line = substr(line, 2)
        }
      }
      if (q != "") out = out q body
      print out
    }
  ' "$1"
}

# ---- the truth: read out of the code ---------------------------------------------
# Subcommands, flags with the subcommand they belong to (`-` for a global one), the
# environment variables read, the exit codes returned. Extractors that find nothing are
# findings or refusals, because an empty list passes every loop.

subs=()
flags=() # "SUB<TAB>FLAG" per line, SUB is - for a global flag
open_set=0
proxy_only=0
{
  header=$(sed -n '2,/^[^#]/p' "$script" | sed '$d')
  # The floor the header declares, as one number: 3.2 is 302, 4.3 is 403, no claim is 0.
  # Any version is read rather than 3.2 alone, so a tool that needs 4.3 is still held to
  # what arrived after it and nothing earlier
  # Every text here reaches its reader through <<<, never through a pipe: a `grep -q` that
  # finds its match closes the pipe, and the producer's next write dies of SIGPIPE, which
  # `pipefail` then makes the pipeline's status (shape.md, pitfalls.md)
  # `|| :` because a header with no claim is the ordinary case, and a grep that finds
  # nothing exits 1, which pipefail would make the substitution's status and -e would act on
  claim=$(grep -oE 'Needs bash [0-9]+(\.[0-9]+)?' <<<"$header" | sed -n 1p || :)
  floor=0
  [[ -z "$claim" ]] ||
    floor=$(awk -v v="${claim#Needs bash }" 'BEGIN { n = split(v, p, "."); print p[1] * 100 + (n > 1 ? p[2] : 0) }')
  # The userland is a second claim and an independent one: bash 5 from brew or nix with a
  # BSD sed around it is an ordinary macOS machine, and its flags are the ones that differ
  posix_tools=0
  ! grep -q 'POSIX tools only' <<<"$header" || posix_tools=1

  # The table, read once and shared by every rule that has moved onto it. Under --bash-only
  # there is none, and the rules below that need one do not run; the summary says so
  tree="$work/tree.tsv"
  : >"$tree"
  ((bash_only)) || if ! read_tree "$script" >"$tree" 2>"$work/tree.err"; then
    # Whose problem it is, decided by the parser that matters: a script this bash rejects
    # is reported below in bash's own words, which is where the message belongs and what
    # the self-test expects. One bash parses and shfmt does not is the two disagreeing,
    # and then there is no tree to read and nothing here can stand in for it
    : >"$tree"
    if "$BASH" -n "$script" 2>/dev/null; then
      die "shfmt cannot read $name, which this bash parses: $(tr '\n' ' ' <"$work/tree.err")"
    fi
  fi

  # The dispatcher: the top-level `case "$cmd"`, one arm per subcommand, `a | b)` split
  # into two. The help arm and the refusal arms are not subcommands. Read from the tree,
  # so a `case "$cmd"` written inside a string or a heredoc is text and not a dispatcher,
  # and one indented or spelled `case $cmd` is still found
  # Every top-level `case "$cmd"`, not the first: a script may answer -h before the tools
  # its subcommands need are looked for, and that pre-dispatch is a `case "$cmd"` of its
  # own. The arms of all of them together are the dispatcher
  disp_line=$(awk -F'\t' '$2 == "case" && $3 == "-" && $6 == "$cmd" { print $1 }' "$tree")
  if [[ -n "$disp_line" ]]; then
    while IFS= read -r arm; do
      [[ -n "$arm" && "$arm" != help ]] || continue
      subs+=("$arm")
    done < <(awk -F'\t' '
      NR == FNR { if ($2 == "case" && $3 == "-" && $6 == "$cmd") d[$1] = 1; next }
      $2 == "arm" && ($5 in d) {
        n = split($6, p, "|")
        for (i = 1; i <= n; i++) if (p[i] ~ /^[a-z][a-z0-9-]*$/) print p[i]
      }' "$tree" "$tree")
    # The *) arm refuses, and a refusal is not output: a usage printed there goes to stderr.
    # A wrapper's *) arm passes the word through to another tool instead, and says so with
    # the comment `# pass-through` inside the arm; then the subcommand set is open — the
    # help and the docs may name commands the dispatcher never spells — and only the flags
    # stay closed. A declaration rather than a guess: whether an arm refuses is decided by
    # the helper it calls, which no grep can see
    # The refusal arm, as a line range: an arm row carries its own last line, so every
    # other row is inside it or is not, and no text has to be re-read to find out
    refusal_range=$(awk -F'\t' '
      NR == FNR { if ($2 == "case" && $3 == "-" && $6 == "$cmd") d[$1] = 1; next }
      $2 == "arm" && ($5 in d) {
        n = split($6, p, "|")
        for (i = 1; i <= n; i++) if (p[i] == "*") { print $1 "\t" $7; exit }
      }' "$tree" "$tree")
    [[ -n "$refusal_range" ]] || finding "$name's dispatcher has no *) arm to refuse an unknown subcommand"
    if [[ -n "$refusal_range" ]]; then
      refusal_from=${refusal_range%%$'\t'*}
      refusal_to=${refusal_range##*$'\t'}
      # A refusal is not output, so its usage goes to stderr. A call to usage inside the
      # arm and a `>&2` on the same line are both rows, and the line joins them
      usage_lines=$(awk -F'\t' -v a="$refusal_from" -v b="$refusal_to" '
        $2 == "call" && $1 >= a && $1 <= b && $6 == "usage" { print $1 }' "$tree")
      while IFS= read -r ul; do
        [[ -n "$ul" ]] || continue
        awk -F'\t' -v l="$ul" '$2 == "redir" && $1 == l && $6 == ">&" && $7 == "2" { found = 1 }
          END { exit !found }' "$tree" ||
          finding "$name's *) arm prints its usage to stdout rather than stderr"
      done <<<"$usage_lines"
      # A wrapper's *) arm passes the word through to another tool instead, and says so
      # with the comment `# pass-through` inside the arm; then the subcommand set is open —
      # the help and the docs may name commands the dispatcher never spells — and only the
      # flags stay closed. A declaration rather than a guess: whether an arm refuses is
      # decided by the helper it calls, which no reading of the code can see
      ! awk -F'\t' -v a="$refusal_from" -v b="$refusal_to" '
        $2 == "comment" && $1 >= a && $1 <= b && $6 ~ /pass-through/ { found = 1 }
        END { exit !found }' "$tree" || open_set=1
    fi
  fi

  # The flags: every arm pattern that is a flag, attributed to the cmd_<sub>() function it
  # sits in, or global when it sits in none. Arms in any other function are not a parser of
  # this script's own flags and are left alone. A pattern is read one at a time, so a flag
  # is a flag wherever it sits — `-v | --version)` on the dispatcher is a global flag the
  # help must list, beside `run)` which is a subcommand
  while IFS= read -r line; do
    [[ -n "$line" ]] || continue
    flags+=("$line")
  done < <(awk -F'\t' '
    $2 == "arm" && $6 ~ /^-/ {
      owner = "-"
      if ($3 != "-") {
        if (substr($3, 1, 4) != "cmd_") next
        owner = substr($3, 5); gsub(/_/, "-", owner)
      }
      n = split($6, p, "|")
      for (i = 1; i <= n; i++) if (p[i] ~ /^--?[a-zA-Z]/) print owner "\t" p[i]
    }' "$tree")

  # A plain script with no dispatcher and no flag has no help for anything to agree with.
  # If its header claims bash 3.2 the proxy below is still worth running, and it is all
  # that runs; with no claim either there is nothing to check, which is a refusal
  # Under --bash-only both lists are empty because no tree was read, which is a fact about
  # the run and not about the script, so the refusal below would be a lie and is not made
  if ((${#subs[@]} + ${#flags[@]} == 0 && bash_only == 0)); then
    ((floor || posix_tools)) ||
      die "nothing to check in $script: no case \"\$cmd\" dispatcher, no flag arms and no bash floor or POSIX userland claim — see references/shape.md"
    proxy_only=1
  fi
  # Under --bash-only the whole run is that proxy. No tree was read, so every check below
  # would hold the help against an empty list, and reaching the help at all means running
  # the script: a list of defects meant to be sourced answers 127 and prints a line per
  # call it could not resolve
  ((bash_only == 0)) || proxy_only=1
  # The help arm is spelled one way, so a reader and a completion can count on all three.
  # A wrapper passes `help` through to the tool behind it, whose help is the better one,
  # so it may answer -h and --help as flags before the dispatcher instead
  # The patterns come out of the table already split, so the three are compared as a set
  # and the spelling of the spaces around the bars is the formatter's business, not this
  # check's — which is what it was reading before
  help_arm=0
  [[ -z "$disp_line" ]] ||
    ! awk -F'\t' '
      NR == FNR { if ($2 == "case" && $3 == "-" && $6 == "$cmd") d[$1] = 1; next }
      $2 == "arm" && ($5 in d) && $6 == "-h|--help|help" { found = 1 }
      END { exit !found }' "$tree" "$tree" || help_arm=1
  if [[ -n "$disp_line" ]] && ((help_arm == 0)); then
    flag_rows=$(printf '%s\n' "${flags[@]+"${flags[@]}"}")
    if ! { ((open_set)) && grep -qx -- $'-\t--help' <<<"$flag_rows"; }; then
      finding "$name's dispatcher has no -h | --help | help arm"
    fi
  fi
}

known_sub() { # known_sub WORD -> 0 when the dispatcher has it, or it is help, or the set is open
  local s
  [[ "$1" != help ]] || return 0
  ((open_set == 0)) || return 0
  for s in "${subs[@]+"${subs[@]}"}"; do [[ "$s" == "$1" ]] && return 0; done
  return 1
}
known_flag() { # known_flag FLAG [SUB] -> 0 when SUB (or any parser) accepts it, or it is the help
  local f owner want="${2:-}"
  case "$1" in -h | --help) return 0 ;; esac
  for f in "${flags[@]+"${flags[@]}"}"; do
    owner="${f%%	*}"
    [[ "${f#*	}" == "$1" ]] || continue
    [[ -z "$want" || "$owner" == "$want" || "$owner" == - ]] && return 0
  done
  return 1
}

# ---- the bash 3.2 claim: a proxy, honestly labelled ------------------------------
# Every literal below is split by a bracket expression so the pattern cannot match its
# own line. A grep is a proxy: it once let nine constructs through that the real 3.2
# rejects, which is why the claim is proven by a run under /bin/bash on a macOS runner
# and this is only the cheap first look. It runs before the help is asked for, since
# under a real 3.2 a script holding such a construct may not parse at all
# Each row below is the bash a construct needs and the pattern that finds it, and a script
# is held to the rows *above* the floor it declares: `Needs bash 4.3` is checked for 4.4
# and 5.2 constructs and left alone about `mapfile`. The floors are bash's own NEWS, and
# references/portability.md carries the same table in prose, for a reader rather than a grep
# The middle column says which copy of the code the pattern is read in. `cmd` is a command,
# which a string only names — `fail "this bash accepts declare -A"` is prose, and a gate
# proving a bash is 3.2 has to write that sentence — so those are matched where quoted text
# is blanked. `exp` is an expansion, which double quotes do not suppress: `echo "${v,,}"`
# lowercases at runtime, so those keep reading inside them
# A letter range in a glob parses under every bash and matches different text: before 5.0
# it follows the locale's collation, where en_US.UTF-8 puts H inside [a-z].
# 5.0 turned globasciiranges on by default, so 5.0 is the floor of the ASCII reading
version_rows() {
  cat <<'ROWS'
400	call	mapfile	.
400	call	readarray	.
400	decl	declare|local|typeset	A
400	param	.	^,,$|^\^\^$
400	armop	;;&	.
400	pipeop	\|&	.
400	call	read	(^| )-t ?[0-9]*[.][0-9]
400	call	shopt	globstar
401	fdvar	.	.
402	test	-v	.
402	param	.	^slice-neg$
403	decl	declare|local|typeset	n
403	call	wait	(^| )-n( |$)
404	param	.	^@[QEPAaKk]$
500	glob	(^|[^\\])\[([^]]|\[:[a-z]+:\])*[A-Za-z]-[A-Za-z]	.
502	param	.	^repl-quoted$|^repl-amp$
bsd	call	^sort$	(^| )-[A-Za-z]*V
bsd	call	^grep$	(^| )-[A-Za-z]*P
bsd	call	^grep$	--exclude-dir
bsd	call	^readlink$	(^| )-f( |$)
bsd	call	^date$	(^| )-d( |$)
bsd	call	^mktemp$	(^| )(-[pt]|--tmpdir|--suffix)
bsd	call	^sed$	(^| )-[A-Za-z]*i( |$)|--in-place
bsd	call	^timeout$	^[0-9]
bsd	call	^tar$	--wildcards|--null
ROWS
}

# What `POSIX tools only` covers, so that anything else a script calls has to be named in
# the header beside the claim. It is what the claim promises rather than what POSIX.1
# tabulates: a utility both a GNU and a BSD userland ship, which is the portability the
# claim is about. `mktemp` is the case that decides between the two readings: POSIX.1-2017
# does not list it — its utility index goes from `mkfifo` straight to `more`
# (https://pubs.opengroup.org/onlinepubs/9699919799/idx/utilities.html) — while every
# userland this family targets has it, and portability.md already treats it as present by
# ruling on which of its flags differ. A name missing here is a finding
# asking for one word in a header, which is cheap; a name wrongly here is a claim nobody
# checks
posix_utilities() {
  cat <<'UTILS'
awk basename cat chgrp chmod chown cmp comm cp cut date diff dirname du echo env expand
expr false file find fold grep head id install join ln logname ls mkdir mkfifo mktemp more
mv nl od paste patch pr printf ps pwd rm rmdir sed sleep sort split stty tail tar tee test
time touch tr true tsort tty uname unexpand uniq wc who xargs
UTILS
}

# bash's own, which need no tool at all. `command`, `type` and `test` are here rather than
# in the list above because a script calling them is calling the builtin
bash_builtins() {
  cat <<'BUILTINS'
alias bg bind break builtin caller cd command compgen complete compopt continue declare
dirs disown enable eval exec exit export false fc fg getopts hash help history jobs kill
let local logout mapfile popd printf pushd pwd read readarray readonly return set shift
shopt source suspend test times trap true type typeset ulimit umask unalias unset wait
BUILTINS
}
# The rows that apply to this script: a numbered one when the floor it declares is below
# the bash the construct needs, and a `bsd` one when it claims POSIX tools. No claim, no
# proxy — a script that declares no floor has promised nothing about where it runs, and
# every construct below would be a finding against a promise nobody made
version_rows >"$work/rows.tsv"
active_rows="$work/active.tsv"
awk -F'\t' -v floor="$floor" -v posix="$posix_tools" '
  $1 == "bsd" { if (posix) print; next }
  floor && $1 + 0 > floor { print }' "$work/rows.tsv" >"$active_rows"
# Shown as the script has it, found in the table: a row carries the line, and the line is
# read back for the message alone
show_lines() { # show_lines < LINE NUMBERS -> `LINE has: text` from the script
  awk 'NR == FNR { want[$1]; next } FNR in want { sub(/^[[:space:]]*/, ""); print FNR " has: " $0 }' - "$script"
}
proxy_hits() { # proxy_hits -> `LINE has: text` for each active row a fact matches
  [[ -s "$active_rows" ]] || return 0
  # ++n and not n++: an awk variable starts as the empty string, so k[n] on the first row
  # lands in k[""] rather than k[0], and a loop from 0 then never sees that row at all
  awk -F'\t' '
    NR == FNR { ++n; k[n] = $2; a[n] = $3; b[n] = $4; next }
    { for (i = 1; i <= n; i++) if ($2 == k[i] && $6 ~ a[i] && $7 ~ b[i]) { print $1; break } }
  ' "$active_rows" "$tree" | sort -un | show_lines || :
}
# A heredoc opened inside $( ), <( ) or >( ) is bash 4.0: 3.2 finds the end of the
# substitution by scanning the heredoc's body as code, so an unpaired ' in it is a syntax
# error and an unpaired ) ends the substitution early, and the value is quietly wrong.
# One column of the table answers it: SUBST is 1 inside $( ) and <( ) and 0 inside
# backticks and $(( )), which is exactly this rule, and the thirty-eight lines of awk that
# tracked quotes, comments and shifts across lines to arrive at the same answer are gone
heredoc_in_subst() { # heredoc_in_subst -> `LINE has: text` for each such opener
  awk -F'\t' '$2 == "heredoc" && $4 == 1 { print $1 }' "$tree" | sort -un | show_lines || :
}
if [[ -s "$active_rows" ]]; then
  claimed="bash ${claim#Needs bash }"
  [[ -n "$claim" ]] || claimed="a POSIX userland"
  while IFS= read -r hit; do
    [[ -n "$hit" ]] || continue
    finding "$name claims $claimed but $script:$hit — a proxy grep; the proof is a run under it"
  done < <({
    proxy_hits
    if ((floor && floor < 400)); then heredoc_in_subst; fi
  } | sort -n -u)
fi

# ---- a positional parameter guarded by ${N:?} ------------------------------------
# It reads like an argument check and exits 1 with bash's own text, where a missing
# argument is a usage error: t.sh answered 23 of them with the code a failing command
# exits, and no literal `exit` betrayed it
while IFS= read -r hit; do
  [[ -n "$hit" ]] || continue
  finding "$script:$hit — \${N:?} exits 1 with bash's message, where a missing argument is a usage error; guard it with ((\$# >= N)) || die"
  # A parameter expansion whose name is a number and whose operator is :?. Being an
  # expansion is the whole question, so the tree answers it and the comment the grep had
  # to exclude by hand is not an expansion at all
done < <(awk -F'\t' '$2 == "param" && $6 ~ /^[0-9]+$/ && $7 == ":?" { print $1 }' "$tree" |
  sort -un | while IFS= read -r n; do printf '%s has: %s\n' "$n" "$(sed -n "${n}s/^[[:space:]]*//p" "$script")"; done)

# ---- a producer piped into a reader that stops early ------------------------------
# `grep -q` at its match, `head` at its line, `sed q` and `awk … exit` all close the pipe
# and the producer's next write dies of SIGPIPE, which pipefail makes the status of a
# pipeline that did its job. It is a race rather than a certainty — bash line-buffers
# stdout, so even a few hundred bytes leave in more than one write, and which write loses
# is a matter of scheduling — so it survives every local run and fails once in CI
# (pitfalls.md). The fix is to read the text with <<<, which has no producer to kill
while IFS= read -r hit; do
  [[ -n "$hit" ]] || continue
  finding "$script:$hit — a reader that stops early kills its producer with SIGPIPE, and pipefail makes that the pipeline's status; feed it with <<< instead"
  # One row per `|`, naming the reader on its right, so the shapes below are matched
  # against a command and its arguments rather than against whatever the line looks like.
  # The names no longer need splitting to keep this file from finding itself, since a
  # string is not a pipeline
done < <(awk -F'\t' '
  $2 == "pipeinto" &&
    (($6 == "grep" && $7 ~ /(^| )-[a-zA-Z]*q/) ||
      $6 == "head" ||
      ($6 == "sed" && $7 ~ /(^| )-n/ && $7 ~ /[0-9]q/) ||
      ($6 == "awk" && $7 ~ /exit/)) { print $1 }' "$tree" |
  sort -un | while IFS= read -r n; do printf '%s has: %s\n' "$n" "$(sed -n "${n}s/^[[:space:]]*//p" "$script")"; done)

# ---- the dispatcher and the cmd_ functions answer to each other ---------------------
# A subcommand whose function was renamed still dispatches, to nothing: bash reports
# `cmd_foo: command not found` at run time and the help agrees with the arm, so every
# check above passes. And a cmd_ function no arm calls is either a subcommand nobody can
# reach or a leftover. Both are a question about which functions the file defines and
# which names its arms call, and the table answers both
#
# It asks about calls and not about a naming convention: a dispatcher may validate the
# word and do the work further down, which is a shape of its own and not a defect
if [[ -n "$disp_line" ]] && ((! proxy_only)); then
  awk -F'\t' '$2 == "func" { print $6 }' "$tree" | sort -u >"$work/defined.fn"
  awk -F'\t' '
    NR == FNR { if ($2 == "case" && $3 == "-" && $6 == "$cmd") d[$1] = 1; next }
    $2 == "arm" && ($5 in d) { from = $1; to = $7 + 0; for (l = from; l <= to; l++) in_arm[l] = 1; next }
    $2 == "call" && $6 ~ /^cmd_/ && ($1 in in_arm) { print $6 }
  ' "$tree" "$tree" | sort -u >"$work/called.fn"
  while IFS= read -r fn; do
    [[ -n "$fn" ]] || continue
    grep -qx -- "$fn" "$work/defined.fn" ||
      finding "$name's dispatcher calls $fn(), which it does not define — bash says so only when that arm is reached"
  done <"$work/called.fn"
  while IFS= read -r fn; do
    [[ -n "$fn" ]] || continue
    grep -qx -- "$fn" "$work/called.fn" ||
      finding "$name defines $fn() and no dispatcher arm calls it"
  done < <(grep '^cmd_' "$work/defined.fn" || :)
fi

# ---- the tools the header claims are the tools the script calls ---------------------
# `POSIX tools only` is a promise about what has to be installed for the script to run, and
# it is the half of the header a macOS runner cannot prove: the bash is proven by running
# under it, while a missing tool is only missing on the machine that lacks it. Every name
# the script calls that is not a builtin, not one of its own functions and not a POSIX
# utility has to appear in the header beside the claim — one word, which is what makes the
# claim readable rather than aspirational
if ((posix_tools && ! proxy_only)); then
  {
    posix_utilities
    bash_builtins
  } | awk '{ for (i = 1; i <= NF; i++) print $i }' | sort -u >"$work/known.tools"
  awk -F'\t' '$2 == "func" { print $6 }' "$tree" | sort -u >>"$work/known.tools"
  while IFS= read -r tool; do
    [[ -n "$tool" ]] || continue
    has_token "$tool" <<<"$header" ||
      finding "$name calls $tool, which is neither a builtin nor a POSIX utility, and its header claims POSIX tools only without naming it"
  done < <(awk -F'\t' '$2 == "call" && $6 ~ /^[a-z][a-z0-9_.-]*$/ { print $6 }' "$tree" |
    sort -u | grep -vxF -f "$work/known.tools" || :)
fi

# ---- set -euo pipefail is the first thing the script does ---------------------------
# Anything above it runs without them, and what runs there is usually the part that reads
# the environment and decides where the script is — exactly where an unset variable or a
# failing command matters most. The tree says which statement is first; a grep for the
# line says only that it is somewhere. A script whose first statement is `set` with some
# other flags is a different claim and is left alone, since shape.md rules on the spelling
if ((! proxy_only)); then
  first=$(awk -F'\t' '$2 == "call" && $3 == "-" { print $1 "\t" $6 "\t" $7; exit }' "$tree")
  if [[ -n "$first" ]]; then
    first_name=$(printf '%s' "$first" | cut -f2)
    first_args=$(printf '%s' "$first" | cut -f3)
    first_line=$(printf '%s' "$first" | cut -f1)
    if [[ "$first_name" == set ]]; then
      case "$first_args" in
        *euo*pipefail*) ;;
        *u*pipefail*)
          # -e dropped: shape.md allows it where a non-zero status is the answer, and asks
          # for a comment above the line saying which of the two shapes this file is. The
          # comment is what makes the reason visible, since the line differs from the
          # ordinary one by a letter, so its absence is the finding rather than the flags
          # The whole comment block above, not the one line: a reason worth giving usually
          # takes two lines, and then the line directly above carries the second half
          awk -F'\t' -v l="$first_line" '
            $2 == "comment" { c[$1] = $6 }
            END {
              for (n = l - 1; n in c; n--) if (c[n] ~ /-e/) { found = 1; break }
              exit !found
            }' "$tree" ||
            finding "$name opens with \`set $first_args\` and the line above says nothing about the missing -e — shape.md asks for a comment naming which answer a non-zero status is"
          ;;
        *) finding "$name opens with \`set $first_args\` rather than \`set -euo pipefail\` — see references/shape.md" ;;
      esac
    else
      finding "$name runs \`$first_name\` before \`set -euo pipefail\`, on line $(printf '%s' "$first" | cut -f1) — what happens above that line happens without -e, -u or pipefail"
    fi
  fi
fi

# ---- the help's heredoc keeps its delimiter quoted ---------------------------------
# `<<EOF` expands the body, so a `$1` or a backtick in the help text is substituted on the
# way out and the help says something the script does not. That is the price of the one
# thing it buys, a value the script holds printed into the text — every installer in this
# family prints `$VERSION` that way — so the finding is an unquoted delimiter whose body
# substitutes nothing at all, which is the price paid for no purchase (help.md)
while IFS= read -r hit; do
  [[ -n "$hit" ]] || continue
  finding "$name prints its help from a heredoc whose delimiter is unquoted and whose text substitutes nothing — $script:$hit; write <<'EOF' and every \$ in it stays literal"
done < <(awk -F'\t' '
  $2 == "func" && $6 == "usage" { from = $1; to = $7 + 0; next }
  from && $2 == "heredoc" && $1 >= from && $1 <= to && $6 !~ /^'"'"'/ { hd[$1] = 1 }
  from && $2 == "heredocexp" && $1 >= from && $1 <= to { subst_in[$1] = 1 }
  END { for (l in hd) if (!(l in subst_in)) print l }' "$tree")

# ---- the header comment lists nothing ---------------------------------------------
# It says why the script exists and makes the claims; what the script accepts is the
# help's alone. A second list beside the help falls behind it — t.sh's header did, by
# three subcommands, before anyone noticed. A line the help's grammar would read as a
# usage, flag or code row, or an Exit or Environment line, is such a list
while IFS= read -r row; do
  [[ -n "$row" ]] || continue
  finding "$name's header comment carries a line that belongs to the help alone: $row"
done < <(printf '%s\n' "$header" | sed 's/^# \{0,1\}//' |
  grep -E "^ +(${name_re} |--?[a-zA-Z]|[0-9]+  )|^Exit[ :]+[0-9]|^Environment:" || :)

# ---- the script parses under the bash running this checker -------------------------
# Running the help would catch a syntax error too, but only for a script that has a
# dispatcher to run: a plain one, checked by the proxy alone, is never executed. And on a
# macOS runner this bash is the 3.2 a `Needs bash 3.2` claim is about, so the parse is the
# cheapest proof that claim has. Nothing below can be trusted about a file bash cannot
# read, so the help half is skipped once this fires
if ! parse=$("$BASH" -n "$script" 2>&1); then
  finding "$name does not parse under the bash running this checker: ${parse##*: }"
  proxy_only=1
fi

# Built once and printed by whichever exit is reached: an exit code does not say what was
# examined, and a run that examined nothing exits 0 too, so every run states its verdict
# and names what it did not do
summary=$(printf '%s — %d subcommands, %d flags agree with the help; %d document(s), %s completions checked' \
  "$name" "${#subs[@]}" "${#flags[@]}" "$((${#docs[@]} + ${#mentions[@]}))" \
  "$([[ -n "$comp_bash" ]] && echo 2 || echo 0)")
((bash_only == 0)) || summary="$summary; --bash-only, so nothing that reads the script as a tree ran"

# Everything from here on holds the help, a document or a completion to the subcommands
# and flags read out of the code. Under --bash-only those lists are empty because no tree
# was read, not because the script has none, so every row the help carries would be read
# as a flag no parser accepts: comparing against nothing finds everything wrong, the same
# way it finds nothing wrong
# ---- the help ---------------------------------------------------------------------
if ((! proxy_only)); then
  # Run under the bash running this checker, not the one the shebang finds: on a macOS
  # runner that is the 3.2 the claim is about
  help=$("$BASH" "$script" --help 2>&1) || die "$name --help exited $? rather than printing the help:"$'\n'"$help"
  [[ -n "$help" ]] || die "$name --help printed nothing"
  # The help again, through the pipe `bash <(curl …)` hands bash. bash reads a script from
  # a pipe no further than the command it runs, so a script that reads its own file finds
  # only what follows that command: nothing when the dispatcher is last, which prints an
  # empty help at exit 0, and the rest of its program otherwise (pitfalls.md); a heredoc
  # is code bash has already read. A run that fails outright is a script that needs the files beside
  # it, which says so and is no finding. The grep only names the line: `$0` is also awk's
  # record, so no grep can decide the question, and the pipe can. A whole single-quoted
  # word is skipped as one, since an awk program holds the `;` and `$` that end a match
  if piped=$("$BASH" <(cat "$script") --help 2>&1) && [[ "$piped" != "$help" ]]; then
    # Which line to name, best effort: a call to a reader whose arguments hold BASH_SOURCE.
    # The tree decides that it is a call and which words are its arguments — a comment and
    # a quoted awk program are neither — and the line itself is read only for the message.
    # `$` in B means a word that expands, which is what "${BASH_SOURCE[0]}" is, so the text
    # of that line is where the name can be looked for and nowhere else
    where=$(awk -F'\t' '
      $2 == "call" && $6 ~ /^(sed|awk|head|tail|cat|grep|cut)$/ && $7 ~ /(^| )\$( |$)/ { print $1 }' "$tree" |
      sort -un | while IFS= read -r n; do
      grep -q 'BASH_SOURCE' <<<"$(sed -n "${n}p" "$script")" || continue
      printf ' — line %s has: %s\n' "$n" "$(sed -n "${n}s/^[[:space:]]*//p" "$script")"
      break
    done)
    finding "$name --help prints other text through a pipe than from the file, at exit 0: under bash <(…) it reads its own source${where:-, by a path no grep here can name}; print the help from a heredoc"
  fi
  # The two runs above are the last thing --bash-only does: they ask this bash a question
  # and answer it. Everything past here holds the help, a document or a completion to the
  # subcommands and flags read out of the code, and in this mode those lists are empty
  # because no tree was read rather than because the script has none — so every row the
  # help carries would be read as a flag no parser accepts. Comparing against nothing
  # finds everything wrong, the same way it finds nothing wrong
  if ((bash_only)); then
    ((findings == 0)) || exit 1
    printf 'check-sh: %s\n' "$summary"
    exit 0
  fi

  # Per-subcommand help, where the script has it: `help SUB` for each help_<sub>() it
  # defines. Its flags are then looked for there rather than in the general help
  corpus="$help"
  sub_help() { # sub_help SUB -> that subcommand's help, or nothing
    local fn="help_${1//-/_}"
    grep -q "^${fn}() {" "$script" || return 0
    "$BASH" "$script" help "$1" 2>/dev/null || return 0
  }
  for s in "${subs[@]+"${subs[@]}"}"; do
    text=$(sub_help "$s")
    [[ -z "$text" ]] || corpus="$corpus"$'\n'"$text"
  done
  # A `help codes` topic, when the script has one, is where the exit codes live
  ! grep -q '^help_codes() {' "$script" || corpus="$corpus"$'\n'"$("$BASH" "$script" help codes 2>/dev/null || :)"

  # help ⇐ dispatcher, and back
  # `NAME sub`, with any bracketed global options between — `NAME [--vault V] sub`
  for s in "${subs[@]+"${subs[@]}"}"; do
    grep -qE -- "(^|[^[:alnum:]_./-])${name_re}( \[[^]]*\])* ${s}([^[:alnum:]_-]|\$)" <<<"$help" ||
      finding "$name dispatches '$s' but its help never mentions '$name $s'"
  done
  while IFS= read -r s; do
    [[ -n "$s" ]] || continue
    known_sub "$s" || finding "$name's help lists '$name $s', which the dispatcher does not have"
  done < <(printf '%s\n' "$help" | grep -oE "(^|[^[:alnum:]_./-])${name_re} [a-z][a-z0-9-]*" |
    sed "s/^[^a-z]*${name_re} //" | sort -u)

  # help ⇐ flags, and back
  for f in "${flags[@]+"${flags[@]}"}"; do
    owner="${f%%	*}"
    flag="${f#*	}"
    case "$flag" in -h | --help) continue ;; esac
    if [[ "$owner" == - ]]; then
      has_token "$flag" <<<"$help" || finding "$name accepts $flag but its help never mentions it"
    else
      text=$(sub_help "$owner")
      [[ -n "$text" ]] || text="$help"
      has_token "$flag" <<<"$text" || finding "$name $owner accepts $flag but its help never mentions it"
    fi
  done
  while IFS= read -r flag; do
    [[ -n "$flag" ]] || continue
    known_flag "$flag" || finding "$name's help has a row for $flag, which no parser accepts"
  done < <(printf '%s\n' "$corpus" | grep -oE '^ +--?[a-zA-Z][a-zA-Z0-9-]*(, *--?[a-zA-Z][a-zA-Z0-9-]*)*' |
    tr ',' '\n' | tr -d ' ' | sort -u)

  # help ⇐ environment variables
  if [[ -n "$prefix" ]]; then
    variables=0
    while IFS= read -r var; do
      [[ -n "$var" ]] || continue
      variables=$((variables + 1))
      has_token "$var" <<<"$corpus" || finding "$name reads $var but its help never mentions it"
      # A parameter expansion is where a variable is read. The grep this replaces ran over
      # the raw file, so a name written in a comment or inside the help's own heredoc
      # counted as a use, and a help could satisfy the check by mentioning a variable the
      # script never reads
    done < <(awk -F'\t' -v p="$prefix" '
      $2 == "param" && index($6, p) == 1 && $6 ~ /^[A-Z0-9_]+$/ { print $6 }' "$tree" | sort -u)
    ((variables > 0)) || finding "$name reads no $prefix variable at all — the prefix is wrong, or the extractor is"
  fi

  # help ⇐ exit codes: every literal `exit N` outside a comment must be on a line of the
  # help that starts with Exit, or in a `  N  text` row. Only the bash shape counts — the
  # statement ends there — so an awk program's `{ exit 1 }` inside a quoted string is not
  # read as this script's code
  # The Exit sentence and the line after it, since a header wraps it; a `  N  text` row
  codes_listed=$(printf '%s\n' "$corpus" | awk '/Exit[ :]/ { print; getline; print; next } /^  [0-9]+  / { print }' |
    grep -oE '[0-9]+' | sort -u || :)
  while IFS= read -r n; do
    [[ -n "$n" ]] || continue
    grep -qx -- "$n" <<<"$codes_listed" ||
      finding "$name exits $n but its help never lists $n on an Exit line"
    # A call to exit with one literal argument. Being a call is what the three greps this
    # replaces were spelling out by hand — outside a comment, ending its statement, not
    # inside a quoted awk program — and the tree answers all three by construction
  done < <(awk -F'\t' '$2 == "call" && $6 == "exit" && $7 ~ /^[1-9][0-9]*$/ { print $7 }' "$tree" | sort -u)
fi

# The same stop as inside the help section, for the path that skipped it: a script the
# proxy alone is checked by never reaches the exit there, and the documents below would
# still be held to an empty list
if ((bash_only)); then
  ((findings == 0)) || exit 1
  printf 'check-sh: %s\n' "$summary"
  exit 0
fi

# ---- the documents ----------------------------------------------------------------
# Back, for every document: each `NAME word` it spells is a subcommand, and each flag it
# attaches to one is parsed by it. This is the sharp direction — a document naming a
# subcommand that no longer exists is what the check exists to catch. Forward, only for
# a document given with -d, one that sets out to list them: every subcommand is named
# beside the script's name somewhere in it. A document given with -m mentions a few and
# sends the reader to the help for the rest, which is the help doing its job
doc_mentions_are_real() { # doc_mentions_are_real DOC -> a finding per mention that is not
  local doc="$1" span sub flag
  while IFS= read -r span; do
    [[ -n "$span" ]] || continue
    # shellcheck disable=SC2086 # the span is split into its words on purpose
    set -- $span
    shift # the name
    sub=""
    if [[ $# -gt 0 && "$1" =~ ^[a-z][a-z0-9-]*$ ]]; then
      sub="$1"
      known_sub "$sub" || finding "$doc names \`$name $sub\`, which $name does not have"
      shift
    fi
    while (($#)); do
      case "$1" in
        --) break ;;
        -[[:alpha:]]* | --[[:alpha:]]*)
          flag="${1%%=*}"
          known_flag "$flag" "$sub" ||
            finding "$doc gives \`$name${sub:+ $sub}\` the flag $flag, which it does not parse"
          ;;
      esac
      shift
    done
  done < <(grep -oE "\`${name_re}( [^\`]*)?\`" "$doc" | tr -d '`' | sort -u)
}
for doc in "${docs[@]+"${docs[@]}"}"; do
  for s in "${subs[@]+"${subs[@]}"}"; do
    grep -F -- "$name" "$doc" | has_token "$s" || finding "$doc never names $name $s"
  done
  doc_mentions_are_real "$doc"
done
for doc in "${mentions[@]+"${mentions[@]}"}"; do
  doc_mentions_are_real "$doc"
done

# ---- the completions --------------------------------------------------------------
if [[ -n "$comp_bash" ]]; then
  # A file that is only ever sourced has no shebang, so its first line names the dialect
  [[ "$(head -n 1 "$comp_bash")" == "# shellcheck shell=bash" ]] ||
    finding "$comp_bash does not open with \`# shellcheck shell=bash\`, the dialect line a sourced file needs"
  # The bash half is read as a tree: the words it offers live in assignments and in the
  # arguments of the calls that build the reply, and a case arm's pattern — `run)`, `-l)` —
  # is a pattern and not an offered word, which the tree says by putting it somewhere else.
  # The awk being replaced had to strip those arms by regular expression first
  #
  # The zsh half keeps that awk on purpose: an `_arguments` spec is a grammar of its own
  # living inside string literals, so a parse of the shell around it says nothing about
  # what the spec offers, and shfmt would hand back the string whole
  if read_tree "$comp_bash" >"$work/comp.tsv" 2>/dev/null; then
    awk -F'\t' '$2 == "assign" || $2 == "call" { print $6; print $7 }' "$work/comp.tsv" |
      tr ' ' '\n' | sed '/^-$/d;/^$/d' >"$work/offered.bash"
  else
    finding "$comp_bash cannot be read as a tree, so the words it offers cannot be checked"
    : >"$work/offered.bash"
  fi
  offered_words "$comp_zsh" 1 >"$work/offered.zsh"
  for f in "$comp_bash" "$comp_zsh"; do
    words="$work/offered.bash"
    [[ "$f" == "$comp_bash" ]] || words="$work/offered.zsh"
    for s in "${subs[@]+"${subs[@]}"}"; do
      has_token "$s" <"$words" || finding "$s is dispatched by $name but absent from $f"
    done
    for e in "${flags[@]+"${flags[@]}"}"; do
      flag="${e#*	}"
      case "$flag" in -h | --help) continue ;; esac
      has_token "$flag" <"$words" || finding "$flag is parsed by $name but absent from $f"
    done
  done
  offered=0
  while IFS= read -r flag; do
    [[ -n "$flag" ]] || continue
    offered=$((offered + 1))
    known_flag "$flag" || finding "$flag is offered by a completion but not parsed by $name"
  done < <(grep -ohE -- '--[a-z][a-z0-9-]+' "$work/offered.bash" "$work/offered.zsh" | sort -u)
  ((offered > 0)) || finding "neither completion offers a single --flag — the extractor is broken, or the files are"
fi

((findings == 0)) || exit 1

# ---- every check above is able to fail --------------------------------------------
# The canonical script above, its document and its completions pass as they are; then
# one copy per check gets one defect and this same script must go red for that defect's
# own reason, since a gate whose findings all come from one over-broad branch reads as
# thorough while testing one thing. A nested run skips this section.

if [[ -n "${CHECK_SH_NESTED:-}" ]]; then
  printf 'check-sh: %s; self-test skipped\n' "$summary"
  exit 0
fi

# Under --bash-only the checks that read a tree did not run, so most of what the plants
# below falsify was never asked. A falsification pass that cannot fail proves nothing and
# would read as though it had, so it is skipped whole and said out loud
if ((bash_only)); then
  printf 'check-sh: %s; self-test skipped, since it falsifies checks this mode does not run\n' "$summary"
  exit 0
fi

canon="$work/canon"
mkdir -p "$canon"
template_script >"$canon/script.sh"
template_bash >"$canon/script.sh.bash"
template_zsh >"$canon/_script.sh"
cat >"$canon/README.md" <<'EOF'
# script.sh

| Command | What it does |
|---|---|
| `script.sh run -n` | says what would be done |
| `script.sh stop` | stops it |

```
script.sh   the tool: run / stop
```
EOF

nested() { # nested DIR [ARGS...] -> this script on DIR's copy, falsification skipped
  local d="$1"
  shift
  # The mode travels into every nested run: under --bash-only the tools are absent, and a
  # nested call that asked for the tree would refuse and read as the checker being broken
  local mode=()
  ((bash_only == 0)) || mode=(--bash-only)
  CHECK_SH_NESTED=1 "$BASH" "$self" ${mode[@]+"${mode[@]}"} "$@"
}
copy() { # copy NAME -> a fresh copy of the canon
  local c="$work/$1"
  mkdir -p "$c"
  cp "$canon"/* "$c/"
  printf '%s\n' "$c"
}
full() { # full DIR -> the arguments that check everything in DIR
  printf -- '-n script.sh -e SCRIPT_ -d %s/README.md -c %s/script.sh.bash %s/_script.sh %s/script.sh\n' "$1" "$1" "$1" "$1"
}
planted=0
# The count lives here rather than beside each call, so a new planted case cannot be left
# out of the number the summary line reports
expect_red() { # expect_red DIR FRAGMENT WHAT [ARGS...]
  local d="$1" want="$2" what="$3" out
  shift 3
  if out=$(nested "$d" "$@" 2>&1); then
    die "self-test: a copy with $what passed — the check cannot catch it"
  fi
  case "$out" in
    *"$want"*) ;;
    *) die "self-test: a copy with $what was rejected for the wrong reason: $out" ;;
  esac
  planted=$((planted + 1))
}
# A copy that must pass is run once, and that run's own output and status are the report.
# A second run for the message would describe itself: a failure that does not repeat left
# an empty reason on a macOS runner, and nothing to find the cause by
expect_green() { # expect_green DIR WHAT [ARGS...]
  local d="$1" what="$2" out status=0
  shift 2
  out=$(nested "$d" "$@" 2>&1) || status=$?
  ((status == 0)) ||
    die "self-test: $what was rejected with exit $status — the checker is broken, not the script:"$'\n'"$out"
}
# The constructs planted below are spelled in two halves, so this file's own claim of
# bash 3.2 is not contradicted by its own self-test
# Through the environment rather than -v: awk reads escape sequences in a -v value, so a
# planted line holding a backslash would arrive changed
plant() { # plant DIR AFTER-PATTERN LINE -> the line inserted after the first match
  PAT="$2" LINE="$3" awk '{ print } !done && index($0, ENVIRON["PAT"]) == 1 { print ENVIRON["LINE"]; done = 1 }' "$1/script.sh" >"$1/script.sh.new"
  mv "$1/script.sh.new" "$1/script.sh"
}
swap() { # swap DIR PATTERN LINE -> the first line starting with PATTERN replaced
  PAT="$2" LINE="$3" awk '!done && index($0, ENVIRON["PAT"]) == 1 { print ENVIRON["LINE"]; done = 1; next } { print }' "$1/script.sh" >"$1/script.sh.new"
  mv "$1/script.sh.new" "$1/script.sh"
}
replace_usage() { # replace_usage DIR LINE -> usage() and its heredoc replaced by LINE
  LINE="$2" awk '/^usage\(\) \{$/ { print ENVIRON["LINE"]; skip = 1; next } skip && /^}$/ { skip = 0; next } skip { next } { print }' "$1/script.sh" >"$1/script.sh.new"
  mv "$1/script.sh.new" "$1/script.sh"
}

c=$(copy faithful)
# shellcheck disable=SC2046 # full() prints the arguments, split on purpose
expect_green "$c" "the canonical script" $(full "$c")

# The two below put the defect in the tools rather than in the text, which is the one
# thing a planted line cannot express: a copy that quietly lost shfmt must refuse, and
# --bash-only must be the only way past it. A PATH with the tool removed is how that is
# asked, since a machine cannot be asked to forget it
#
# Both are about the preflight, which --bash-only turns off, so a run already in that mode
# skips them: there is no tree-reading path for a missing tool to stop, and nested() would
# hand the plant the very flag it exists to do without. The summary says that half of the
# run did not happen, which is where a reader learns these did not either
if ((bash_only == 0)); then
  no_tree_path=""
  # shellcheck disable=SC2086 # splitting PATH on : is the point
  oldifs=$IFS
  IFS=:
  for p in $PATH; do
    [ -x "$p/shfmt" ] || no_tree_path="${no_tree_path:+$no_tree_path:}$p"
  done
  IFS=$oldifs

  c=$(copy no-shfmt)
  status=0
  out=$(PATH="$no_tree_path" nested "$c" "$c/script.sh" 2>&1) || status=$?
  ((status == 2)) || die "self-test: a run that cannot read a tree was not refused (got $status): $out"
  case "$out" in *"to read SCRIPT as a tree"*) ;; *) die "self-test: a run without shfmt was refused for the wrong reason: $out" ;; esac
  planted=$((planted + 1))

  c=$(copy bash-only)
  status=0
  out=$(PATH="$no_tree_path" nested "$c" --bash-only "$c/script.sh" 2>&1) || status=$?
  ((status == 0)) || die "self-test: --bash-only did not pass on a machine without shfmt (got $status): $out"
  case "$out" in
    *"nothing that reads the script as a tree ran"*) ;;
    *) die "self-test: --bash-only passed without saying the tree half was skipped: $out" ;;
  esac
fi

c=$(copy nothing)
printf '#!/usr/bin/env bash\necho hi\n' >"$c/script.sh"
status=0
out=$(nested "$c" "$c/script.sh" 2>&1) || status=$?
((status == 2)) || die "self-test: a script with nothing to check was not refused (got $status): $out"
case "$out" in *"nothing to check"*) ;; *) die "self-test: a script with nothing to check was refused for the wrong reason: $out" ;; esac
planted=$((planted + 1))

c=$(copy plain-claimed)
# A plain script with no dispatcher and no flag, whose header claims bash 3.2, is checked
# by the proxy alone: clean it passes, with a bash 4 construct it goes red
printf '#!/usr/bin/env bash\n# Needs bash 3.2 and POSIX tools only.\necho hi\n' >"$c/plain.sh"
nested "$c" "$c/plain.sh" >/dev/null 2>&1 || die "self-test: a plain script claiming bash 3.2 was refused rather than checked by the proxy"
printf 'false && declare -A m\n' >>"$c/plain.sh"
expect_red "$c" "claims bash 3.2 but $c/plain.sh:" "a bash 4 construct in a plain script claiming 3.2" "$c/plain.sh"

c=$(copy helper-case)
# A case inside a helper function is not a parser of this script's flags
# shellcheck disable=SC2016 # the $1 belongs to the helper being written out
plant "$c" 'HERE=' 'helper() { case "$1" in --inner) : ;; esac; }'
# shellcheck disable=SC2046
nested "$c" $(full "$c") >/dev/null 2>&1 || die "self-test: a case in a helper function was read as a parser"

c=$(copy wrapper)
# A dispatcher whose *) arm passes the word through is a wrapper, and a wrapper's help may
# name the commands of the tool behind it
awk '/^  \*\)$/ { print "  *) printf '"'"'passing %s through\\n'"'"' \"$cmd\" ;; # pass-through"; skip = 1; next } skip && /^    ;;$/ { skip = 0; next } skip { next } { print }' "$c/script.sh" >"$c/s" && mv "$c/s" "$c/script.sh"
plant "$c" '  script.sh stop' '  script.sh anything                       passed through to the tool behind'
# shellcheck disable=SC2016 # the backticks are markdown, not a command substitution
printf '\nAlso `script.sh anything` goes through\n' >>"$c/README.md"
# shellcheck disable=SC2046
expect_green "$c" "a wrapper's help naming a passed-through command" $(full "$c")

c=$(copy bracketed)
# A global option in brackets between the name and the subcommand is still `NAME sub`
swap "$c" '  script.sh stop' '  script.sh [--quiet] stop                 stop doing it'
# shellcheck disable=SC2046
nested "$c" $(full "$c") >/dev/null 2>&1 || die "self-test: a help spelling 'script.sh [--quiet] stop' was read as not naming stop"

c=$(copy unclaimed)
# The proxy is gated on the claim: a script that does not claim 3.2 may use bash 4
# The claim as the check finds it, not the template's whole line, which is then free to change
sed 's/^\(# .*\)Needs bash 3\.2.*$/\1Needs bash 4./' "$c/script.sh" >"$c/s" && mv "$c/s" "$c/script.sh"
plant "$c" 'HERE=' 'false && declar'"e -A m"
# shellcheck disable=SC2046
nested "$c" $(full "$c") >/dev/null 2>&1 || die "self-test: a bash 4 construct was flagged in a script that claims no bash 3.2"

c=$(copy new-arm)
# shellcheck disable=SC2016 # the $cmd is the dispatcher's, matched literally
plant "$c" 'case "$cmd" in' '  planted) : ;;'
expect_red "$c" "dispatches 'planted' but its help never mentions 'script.sh planted'" "a subcommand missing from the help" -n script.sh "$c/script.sh"

c=$(copy ghost-sub)
plant "$c" '  script.sh stop' '  script.sh ghost                          a subcommand that is not there'
expect_red "$c" "help lists 'script.sh ghost', which the dispatcher does not have" "a subcommand the help invents" -n script.sh "$c/script.sh"

c=$(copy new-flag)
# shellcheck disable=SC2016 # the $1 is the parser's, matched literally
plant "$c" '    case "$1" in' '      --planted) shift ;;'
expect_red "$c" "script.sh run accepts --planted but its help never mentions it" "a flag missing from the help" -n script.sh "$c/script.sh"

c=$(copy ghost-flag)
plant "$c" '  -l DIR' '  --ghost         a flag no parser accepts'
expect_red "$c" "help has a row for --ghost, which no parser accepts" "a flag the help invents" -n script.sh "$c/script.sh"

c=$(copy new-variable)
# shellcheck disable=SC2016 # the expansion belongs to the script being written out
plant "$c" 'HERE=' ': "${SCRIPT_PLANTED:-}"'
expect_red "$c" "reads SCRIPT_PLANTED but its help never mentions it" "a variable missing from the help" -n script.sh -e SCRIPT_ "$c/script.sh"

c=$(copy no-variable)
expect_red "$c" "reads no NOPE_ variable at all" "a prefix nothing is read with" -n script.sh -e NOPE_ "$c/script.sh"

c=$(copy new-code)
plant "$c" 'HERE=' 'false && exi'"t 97"
expect_red "$c" "exits 97 but its help never lists 97" "an exit code missing from the help" -n script.sh "$c/script.sh"

c=$(copy self-read-sed)
# A usage() printing its help back out of its own file, as the family's did. The text is
# the canon's own help as `#>` lines under the shebang, so from the file every other
# check passes, and `#>` is no row the header check reads. Through a pipe bash has read
# those lines long before the dispatcher runs, so the reader finds them gone and prints
# nothing at exit 0. Spelled in two halves so this file's own source reads nothing of
# itself
canon_help=$("$BASH" "$canon/script.sh" --help | sed 's/^/#> /')
# shellcheck disable=SC2016 # the expansion belongs to the usage() being written out
replace_usage "$c" 'usage() { sed -n '"'"'s/^#> \{0,1\}//p'"'"' "${BASH_SOUR''CE[0]}"; }'
plant "$c" '#!/usr/bin/env bash' "$canon_help"
expect_red "$c" "has: usage() { sed -n" "a usage() printing its help back with sed" -n script.sh "$c/script.sh"

c=$(copy self-read-dollar0)
# $0 names the file too, and in awk it is also the record, so no grep can tell the two
# apart; the pipe can. The finding carries no line then, and the fragment says so
# shellcheck disable=SC2016 # the expansion belongs to the usage() being written out
replace_usage "$c" 'usage() { awk '"'"'sub(/^#> ?/, "")'"'"' "$0"; }'
plant "$c" '#!/usr/bin/env bash' "$canon_help"
expect_red "$c" "it reads its own source, by a path no grep here can name" "a usage() printing its help back with awk on \$0" -n script.sh "$c/script.sh"

c=$(copy needs-its-directory)
# A script that needs a file beside it fails outright through a pipe: that is no finding,
# since it cannot run that way at all and says so
# shellcheck disable=SC2016 # the expansion belongs to the script being written out
plant "$c" 'HERE=' 'cat "$HERE/script.sh.bash" >/dev/null'
# shellcheck disable=SC2046
expect_green "$c" "a script that fails outright through a pipe" $(full "$c")

c=$(copy header-usage)
plant "$c" '#!/usr/bin/env bash' '#   script.sh stop                           stop doing it'
expect_red "$c" "belongs to the help alone:   script.sh stop" "a usage line in the header comment" -n script.sh "$c/script.sh"

c=$(copy header-flag)
plant "$c" '#!/usr/bin/env bash' '#   -l DIR          the log directory'
expect_red "$c" "belongs to the help alone:   -l DIR" "a flag row in the header comment" -n script.sh "$c/script.sh"

c=$(copy header-exit)
plant "$c" '#!/usr/bin/env bash' '# Exit 0 done, 2 on a usage error'
expect_red "$c" "belongs to the help alone: Exit 0" "an Exit line in the header comment" -n script.sh "$c/script.sh"

c=$(copy no-help-arm)
swap "$c" '  -h | --help | help) usage ;;' '  --help) usage ;;'
expect_red "$c" "has no -h | --help | help arm" "a dispatcher without a help arm" -n script.sh "$c/script.sh"

c=$(copy quiet-refusal)
# Every redirection inside the *) arm dropped, so the refusal goes to stdout
awk '/^  \*\)$/ { inarm = 1 } inarm { gsub(/ >&2/, "") } /;;/ { inarm = 0 } { print }' "$canon/script.sh" >"$c/script.sh"
expect_red "$c" "prints its usage to stdout rather than stderr" "a refusal that goes to stdout" -n script.sh "$c/script.sh"

c=$(copy doc-missing)
grep -v 'stop' "$canon/README.md" >"$c/README.md"
expect_red "$c" "README.md never names script.sh stop" "a document that lost a subcommand" -n script.sh -d "$c/README.md" "$c/script.sh"

c=$(copy doc-ghost)
# shellcheck disable=SC2016 # the backticks are markdown, not a command substitution
printf '\nAlso `script.sh ghost` for the thing that is not there\n' >>"$c/README.md"
expect_red "$c" "README.md names \`script.sh ghost\`, which script.sh does not have" "a document naming a subcommand that is not there" -n script.sh -d "$c/README.md" "$c/script.sh"

c=$(copy doc-ghost-flag)
# shellcheck disable=SC2016 # the backticks are markdown, not a command substitution
printf '\nAnd `script.sh run --ghost` for the flag that is not there\n' >>"$c/README.md"
expect_red "$c" "gives \`script.sh run\` the flag --ghost, which it does not parse" "a document attaching a flag that is not parsed" -n script.sh -d "$c/README.md" "$c/script.sh"

c=$(copy mention-partial)
# A document that mentions one subcommand and sends the reader to the help for the rest
# is not held to naming them all
# shellcheck disable=SC2016 # the backticks are markdown, not a command substitution
printf 'Run `script.sh stop` to stop it; `script.sh help` has the rest\n' >"$c/NOTE.md"
nested "$c" -n script.sh -m "$c/NOTE.md" "$c/script.sh" >/dev/null 2>&1 ||
  die "self-test: a document given with -m was held to naming every subcommand"

c=$(copy mention-ghost)
# shellcheck disable=SC2016 # the backticks are markdown, not a command substitution
printf 'Run `script.sh ghost` to stop it\n' >"$c/NOTE.md"
expect_red "$c" "NOTE.md names \`script.sh ghost\`, which script.sh does not have" "a mention of a subcommand that is not there" -n script.sh -m "$c/NOTE.md" "$c/script.sh"

c=$(copy comp-bash-missing)
sed 's/--dry-run//' "$canon/script.sh.bash" >"$c/script.sh.bash"
expect_red "$c" "--dry-run is parsed by script.sh but absent from $c/script.sh.bash" "a bash completion missing a flag" -n script.sh -c "$c/script.sh.bash" "$c/_script.sh" "$c/script.sh"

c=$(copy comp-zsh-missing)
grep -v "'stop:" "$canon/_script.sh" >"$c/_script.sh"
expect_red "$c" "stop is dispatched by script.sh but absent from $c/_script.sh" "a zsh completion missing a subcommand" -n script.sh -c "$c/script.sh.bash" "$c/_script.sh" "$c/script.sh"

c=$(copy comp-ghost)
sed 's/words="-n --dry-run -l"/words="-n --dry-run -l --ghost"/' "$canon/script.sh.bash" >"$c/script.sh.bash"
expect_red "$c" "--ghost is offered by a completion but not parsed by script.sh" "a completion offering a flag that is not parsed" -n script.sh -c "$c/script.sh.bash" "$c/_script.sh" "$c/script.sh"

c=$(copy comp-comment)
# A word that survives only in a comment is not offered
sed 's/words="-n --dry-run -l"/words="-n -l" # --dry-run is left out/' "$canon/script.sh.bash" >"$c/script.sh.bash"
expect_red "$c" "--dry-run is parsed by script.sh but absent from $c/script.sh.bash" "a flag named only in a comment of the bash completion" -n script.sh -c "$c/script.sh.bash" "$c/_script.sh" "$c/script.sh"

c=$(copy comp-description)
# A subcommand that survives only in another entry's description is not offered
grep -v "'stop:" "$canon/_script.sh" | sed "s/'run:do the thing'/'run:do the thing, or stop it'/" >"$c/_script.sh"
expect_red "$c" "stop is dispatched by script.sh but absent from $c/_script.sh" "a subcommand named only in a zsh description" -n script.sh -c "$c/script.sh.bash" "$c/_script.sh" "$c/script.sh"

c=$(copy comp-bracket)
# A flag that survives only in the [...] text of another option is not offered
sed "s/'(-n --dry-run)'{-n,--dry-run}'\[say what would be done\]'/'-n[say what would be done, as --dry-run does]'/" "$canon/_script.sh" >"$c/_script.sh"
expect_red "$c" "--dry-run is parsed by script.sh but absent from $c/_script.sh" "a flag named only in a zsh option description" -n script.sh -c "$c/script.sh.bash" "$c/_script.sh" "$c/script.sh"

c=$(copy comp-case-arm)
# A flag dropped from the offered list survives in the case arm that handles its value
sed 's/words="-n --dry-run -l"/words="-n --dry-run"/' "$canon/script.sh.bash" >"$c/script.sh.bash"
expect_red "$c" "-l is parsed by script.sh but absent from $c/script.sh.bash" "a flag left only as a case pattern of the bash completion" -n script.sh -c "$c/script.sh.bash" "$c/_script.sh" "$c/script.sh"

c=$(copy comp-wrapped-list)
# The last line of an offered list wrapped onto two ends in `)` inside a case arm, and is
# still an offer rather than a pattern: two consumers wrap their flag arrays this way
awk '/words="-n --dry-run -l"/ { sub(/words="-n --dry-run -l"/, "local -a w=(-n"); print; print "          --dry-run -l)"; next } { print }' "$canon/script.sh.bash" >"$c/script.sh.bash"
expect_green "$c" "a flag list wrapped onto a second line ending in )" -n script.sh -c "$c/script.sh.bash" "$c/_script.sh" "$c/script.sh"

c=$(copy comp-dialect)
tail -n +2 "$canon/script.sh.bash" >"$c/script.sh.bash"
expect_red "$c" "does not open with" "a bash completion without its dialect line" -n script.sh -c "$c/script.sh.bash" "$c/_script.sh" "$c/script.sh"

c=$(copy param-guard)
# Spelled in two halves, so this file's own source holds no such guard
# shellcheck disable=SC2016 # the expansion belongs to the script being written out
plant "$c" 'HERE=' 'x="${1'':?a value}"'
expect_red "$c" "exits 1 with bash's message" "a positional parameter guarded by \${N:?}" -n script.sh "$c/script.sh"

c=$(copy comp-action)
# The action of an `N:message:action` spec is what zsh offers, and it counts
awk '/^  local -a subcommands$/ { skip = 1 } skip && /^  \)$/ { skip = 0; next } skip { next } { print }' "$canon/_script.sh" |
  sed "s/'1:subcommand:->subcommand'/'1:subcommand:(run stop help)'/; s/subcommand) _describe 'subcommand' subcommands ;;/subcommand) ;;/" >"$c/_script.sh"
expect_green "$c" "a zsh completion offering its subcommands as an action list" -n script.sh -c "$c/script.sh.bash" "$c/_script.sh" "$c/script.sh"

c=$(copy heredoc-in-quotes)
# A `<<WORD` inside quotes is text, not a heredoc: read as one, it blanked the rest of the
# file, and every subcommand and flag after it vanished from what the checker saw, so
# the readme named subcommands the script no longer had
plant "$c" 'HERE=' "note=\"cat <<'NOPE' is text\""
plant "$c" 'HERE=' "note='and so is <<NOPE'"
# shellcheck disable=SC2046
expect_green "$c" "a copy holding <<WORD inside quotes" $(full "$c")

c=$(copy literal-bash4)
# A bash 4 construct inside single quotes is text, which a 3.2 parses happily: the proxy
# reads what a script would run, and a single-quoted string runs nothing
plant "$c" 'HERE=' "note='declare -A is bash 4'"
expect_green "$c" "a copy naming a bash 4 construct inside single quotes" -n script.sh "$c/script.sh"

c=$(copy literal-bash4-double)
# The same inside double quotes, which is where a message says it: a gate proving a bash
# is 3.2 has to print the construct's name, and the proxy read that sentence as a use
plant "$c" 'HERE=' "note=\"this bash accepts declare -A\""
expect_green "$c" "a copy naming a bash 4 construct inside double quotes" -n script.sh "$c/script.sh"

c=$(copy claimed-bash4)
plant "$c" 'HERE=' 'false && declar'"e -A m"
expect_red "$c" "claims bash 3.2 but $c/script.sh:" "a bash 4 construct under a 3.2 claim" -n script.sh "$c/script.sh"

c=$(copy heredoc-in-subst)
# The idiom opens the substitution on the line before the heredoc, where no one-line
# pattern sees both; bash -n under 3.2 passes it, having read the body as code
plant "$c" 'HERE=' "x=\"\$(
  cat <<'X'
a ) b
X
)\""
expect_red "$c" "claims bash 3.2 but $c/script.sh:$(($(grep -n '^HERE=' "$c/script.sh" | cut -d: -f1) + 2)) has: cat <<'X'" "a heredoc inside \$( ) under a 3.2 claim" -n script.sh "$c/script.sh"

c=$(copy heredoc-after-ansi-c)
# In $'...' a backslash escapes the quote, so \' leaves the text open: read as a plain
# single-quoted text it closes there, and every quote after it is read the other way round
plant "$c" 'HERE=' "s=\$'it\\'s'
x=\$(cat <<X
a
X
)"
expect_red "$c" "has: x=\$(cat <<X" "a heredoc inside \$( ) after a \$'...' holding \\' under a 3.2 claim" -n script.sh "$c/script.sh"

c=$(copy bash4-after-ansi-c)
# The masked copies the proxy reads track quotes the same way, and the construct after
# such a text was blanked as if it were quoted
plant "$c" 'HERE=' "s=\$'it\\'s'
false && declar"'e -A m'
expect_red "$c" "has: false && declar"'e -A m' "a bash 4 construct after a \$'...' holding \\' under a 3.2 claim" -n script.sh "$c/script.sh"

c=$(copy heredoc-in-procsubst)
plant "$c" 'HERE=' 'while read -r l; do :; done < <(cat <<X
a
X
)'
expect_red "$c" "has: while read -r l; do :; done < <(cat <<X" "a heredoc inside <( ) under a 3.2 claim" -n script.sh "$c/script.sh"

c=$(copy heredoc-beside-subst)
# What reads like one and is not: a shift inside $(( )), a here-string, a ( inside a
# string inside $( ), a heredoc once the substitution has closed, and one in backticks
plant "$c" 'HERE=' "n=\$(k=2; echo \$((1<<k)))
x=\$(tr a b <<<word)
y=\"\$(echo \"(\")\"; cat <<X >/dev/null
b
X
z=\`cat <<X
c
X
\`"
expect_green "$c" "a copy with a shift, a here-string and heredocs outside any \$( )" -n script.sh "$c/script.sh"

c=$(copy unparsable)
# A file bash cannot read at all: the help run would catch it only where there is a
# dispatcher to run, and the message would name the help rather than the syntax
printf 'if then\n' >>"$c/script.sh"
expect_red "$c" "does not parse under the bash running this checker" "a script with a syntax error" -n script.sh "$c/script.sh"

c=$(copy early-reader)
plant "$c" 'HERE=' 'printf "%s\n" here | grep -q x || :'
expect_red "$c" "a reader that stops early kills its producer" "a text piped into grep -q" -n script.sh "$c/script.sh"

c=$(copy claimed-gnu-mktemp)
# shellcheck disable=SC2016 # the substitution belongs to the script being written out
plant "$c" 'HERE=' 'x=$(mktemp -d -p /tmp)'
expect_red "$c" "has: x=\$(mktemp -d -p /tmp)" "a GNU mktemp flag under a 3.2 claim" -n script.sh "$c/script.sh"

c=$(copy claimed-letter-range)
# shellcheck disable=SC2016 # the $1 belongs to the script being written out
plant "$c" 'HERE=' 'case "${1-}" in [a-z]*) : ;; esac'
expect_red "$c" "has: case \"\${1-}\" in [a-z]*) : ;; esac" "a letter range in a case pattern under a 3.2 claim" -n script.sh "$c/script.sh"

c=$(copy letter-range-not-glob)
# A range bash does not match as a glob: quoted, escaped, a class, a regex after =~, and
# the argument of a tool, which follows that tool's rules and not bash's
# shellcheck disable=SC2016 # the $1 belongs to the script being written out
plant "$c" 'HERE=' 'case "${1-}" in "[a-z]" | \[a-z] | [[:lower:]]*) : ;; esac
[[ "${1-}" == "[A-Z]"* || "${1-}" =~ ^[a-z]+$ ]] || :
printf "%s\n" "${1-}" | tr a-z A-Z | grep "[A-Z]" >/dev/null || :'
expect_green "$c" "a copy whose letter ranges are quoted, escaped, a regex or a tool's argument" -n script.sh "$c/script.sh"

c=$(copy unnamed-tool)
# A tool that is neither a builtin nor a POSIX utility, called by a script whose header
# promises POSIX tools only and does not name it
plant "$c" 'HERE=' 'planted_never_called() { ripgrep --version; }'
expect_red "$c" "calls ripgrep, which is neither a builtin nor a POSIX utility" "a non-POSIX tool the header does not name" -n script.sh "$c/script.sh"

c=$(copy named-tool)
# And the same call with the header naming it, which is the whole fix the finding asks for
plant "$c" 'HERE=' 'planted_never_called() { ripgrep --version; }'
swap "$c" '# Needs bash 3.2 and POSIX tools only' '# Needs bash 3.2, ripgrep and POSIX tools only'
expect_green "$c" "a non-POSIX tool named in the header" -n script.sh "$c/script.sh"

c=$(copy code-before-set)
# A line above `set -euo pipefail` runs without any of the three, and this is where a
# script reads its environment
plant "$c" '#!/usr/bin/env bash' 'umask 022'
expect_red "$c" "before \`set -euo pipefail\`" "a command run before the shell options are set" -n script.sh "$c/script.sh"

c=$(copy dropped-e-unexplained)
# -e dropped with nothing above saying why: the line differs from the ordinary one by a
# letter, so the comment is the only thing that makes the reason visible
swap "$c" 'set -euo pipefail' 'set -uo pipefail'
expect_red "$c" "says nothing about the missing -e" "a dropped -e with no comment" -n script.sh "$c/script.sh"

c=$(copy dispatch-renamed-fn)
# The function is renamed and the arm is not: the help still agrees with the dispatcher,
# and bash says `cmd_run: command not found` only when someone runs that subcommand
swap "$c" 'cmd_run() {' 'cmd_execute() {'
expect_red "$c" "calls cmd_run(), which it does not define" "an arm calling a function that was renamed" -n script.sh "$c/script.sh"

c=$(copy orphan-cmd-fn)
# And the other direction: a cmd_ function nothing dispatches to
plant "$c" 'cmd_run() {' '  :'
plant "$c" 'HERE=' 'cmd_orphan() { :; }'
expect_red "$c" "defines cmd_orphan() and no dispatcher arm calls it" "a cmd_ function with no arm" -n script.sh "$c/script.sh"

c=$(copy help-heredoc-unquoted)
# A help with nothing to substitute, printed from a heredoc that would substitute: the
# price of auditing every $ in the text, paid for no purchase. The canonical help names
# $SCRIPT_LOGDIR, which is the deliberate form and not a finding, so the text goes too
replace_usage "$c" 'usage() { cat <<EOF
script.sh — one sentence

  script.sh [-n] run
  script.sh stop

  -n, --dry-run   say what would be done
  -l DIR          the log directory

Exit 0 done, 2 on a usage error
EOF
}'
expect_red "$c" "delimiter is unquoted" "a help printed from a heredoc that expands it" -n script.sh "$c/script.sh"

printf 'check-sh: %s; %d planted defects caught\n' "$summary" "$planted"
