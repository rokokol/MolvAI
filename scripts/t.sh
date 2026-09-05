#!/usr/bin/env bash
# t.sh — the local test harness: one subcommand per question a test run raises.
#
#   t.sh run [-l DIR] [-m SET] [-p PATTERN] [-t N] -- CMD...
#                             run CMD once. The status reported is CMD's own, the whole
#                             output is kept in a log file, and the log is read even when
#                             CMD exited 0 — because that is not always a success.
#                             -m adds a marker set from markers/ (a name) or a file path;
#                             markers/default.txt always applies
#   t.sh flaky N [-l DIR] [-p PATTERN] -- CMD...
#                             run CMD N times and report how many runs disagreed with the
#                             first. Evidence that a test is unstable, never a way to
#                             tolerate one
#   t.sh bisect GOOD [-b BUILD] [-p PATTERN] -- CMD...
#                             git bisect run between GOOD and HEAD, judging each commit
#                             with run. A commit that cannot be built is skipped rather
#                             than blamed
#   t.sh bisect-probe [-b BUILD] [-p PATTERN] -- CMD...
#                             internal: the single-commit verdict `git bisect run` calls
#   t.sh falsify [-d FILE] [-b BUILD] [FILTER] -- CMD...
#                             break one guard at a time, as written by hand in FILE
#                             (default tests/defects.sh), and require the suite to notice.
#                             A defect the suite survives names something nobody checks
#
# The command is always explicit, after `--`. Nothing here guesses what your suite is:
# a harness that guesses runs the wrong thing on the day it matters.
#
# A repository may keep its POLICY — marker sets, excused lines, log directory — in
# ./tests/t.conf, which is read from the current directory only. It never carries the
# command. See the config section below, or `allow`/`markers`/`pattern`/`logdir`.
#
# Exit status: CMD's own, passed through unchanged, except
#   4  the runs disagreed with each other (flaky)
#   3  CMD exited 0 but its log says it did not do what a pass claims
#   2  a usage or harness error, before CMD ever ran
set -uo pipefail

# vendored from rokokol/tests-skill @ 6edbc0fe48d04ede242bec5052b0b371911546ee, 2026-09-05 (MIT);
# markers/ рядом — из той же ревизии. Обновлять копированием, не правкой на месте:
# `git log <sha>..master` в tests-skill показывает, чего не хватает этой копии.

usage() { sed -n '2,35p' "${BASH_SOURCE[0]}" | sed 's/^# \{0,1\}//'; }

die() {
  printf 't.sh: %s\n' "$1" >&2
  exit 2
}

HERE=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)
MARKER_DIR="$HERE/markers"
MARKER_FILES=()

# The markers live in markers/*.txt as data, not in this file as code, so the list can grow
# without touching the harness and so check.sh can read exactly what `run` reads. A NAME
# with no slash resolves to a set shipped beside this script; anything else is a path.
#
# It assigns to RESOLVED instead of printing, and everything below refuses instead of
# returning, for one reason: `die` inside a `$(...)` exits the SUBSHELL. The caller carries
# on with an empty string, so a refusal written that way does not refuse — and here it
# would leave the marker list empty, which is the one state that makes every run pass while
# the check still looks like it is working.
RESOLVED=""
resolve_markers() {
  local name="$1"
  if [[ "$name" != */* && -r "$MARKER_DIR/$name.txt" ]]; then
    RESOLVED="$MARKER_DIR/$name.txt"
    return
  fi
  [[ -r "$name" ]] ||
    die "no marker set called '$name' — expected $MARKER_DIR/$name.txt or a readable file"
  RESOLVED="$name"
}

# Blank lines and # comments out; everything else verbatim, spaces included.
#
# The trailing CR is stripped first, and that is not cosmetic: a file checked out with
# CRLF turns every blank line into a marker of a single carriage return, which `grep -F`
# then finds on every line of a CRLF log — so a healthy run is reported as a lie, with a
# random build line offered as the evidence. Found on a Windows runner, where git's
# autocrlf does the conversion on checkout.
read_markers() {
  local file="$1" line
  while IFS= read -r line || [[ -n "$line" ]]; do
    line="${line%$'\r'}"
    [[ -z "$line" || "$line" == \#* ]] && continue
    printf '%s\n' "$line"
  done <"$file"
}

# A repository's own policy: which marker sets apply, which lines are excused, where logs
# go. Read from ./tests/t.conf and nowhere else — no search up the tree, because a config
# found three directories away is a config nobody knew was in effect. T_CONFIG points
# somewhere else; T_CONFIG= (empty) turns it off.
#
# It carries policy and never the command. The command stays after `--`, in the line you
# typed, so what runs is always visible where it runs.
EXTRA_PATTERNS=()
POLICY_LOGDIR=""
POLICY_ALLOW=""
load_config() {
  local conf="${T_CONFIG-tests/t.conf}"
  [[ -n "$conf" ]] || return 0
  if [[ ! -e "$conf" ]]; then
    # A repository with no config is the normal case; only a config that exists and cannot
    # be used is an error
    [[ "${T_CONFIG-}" == "" || ! -v T_CONFIG ]] || die "config: $conf does not exist"
    return 0
  fi
  [[ -r "$conf" ]] || die "config: $conf exists but cannot be read"

  local line key value n=0 sets=0 pats=0
  while IFS= read -r line || [[ -n "$line" ]]; do
    n=$((n + 1))
    # Same CRLF stripping as read_markers, for the same reason: a config checked out with
    # CRLF would otherwise carry a carriage return into every value
    line="${line%$'\r'}"
    [[ -z "${line//[[:space:]]/}" || "$line" == \#* ]] && continue
    key=${line%%[[:space:]]*}
    value=${line#"$key"}
    value=${value#"${value%%[![:space:]]*}"}
    [[ -n "$value" ]] || die "config: $conf:$n — '$key' has no value"
    case "$key" in
      markers)
        resolve_markers "$value"
        MARKER_FILES+=("$RESOLVED")
        sets=$((sets + 1))
        ;;
      pattern)
        EXTRA_PATTERNS+=("$value")
        pats=$((pats + 1))
        ;;
      allow)
        [[ -z "$POLICY_ALLOW" ]] || die "config: $conf:$n — 'allow' is given more than once"
        POLICY_ALLOW="$value"
        ;;
      logdir) POLICY_LOGDIR="$value" ;;
      # An unknown key is a typo, and a typo that is ignored is a policy silently not in
      # effect — the failure this whole file exists to avoid
      *) die "config: $conf:$n — unknown key '$key' (markers, pattern, allow, logdir)" ;;
    esac
  done <"$conf"

  printf 't.sh: policy from %s (%d marker set(s), %d pattern(s)%s)\n' \
    "$conf" "$sets" "$pats" "$([[ -n "$POLICY_ALLOW" ]] && printf ', 1 allow')" >&2
}

# Fills MARKER_PATTERNS from MARKER_FILES. Called from the shell that can actually exit.
MARKER_PATTERNS=()
load_markers() {
  MARKER_PATTERNS=()
  local file line before
  for file in "${MARKER_FILES[@]}"; do
    before=${#MARKER_PATTERNS[@]}
    while IFS= read -r line; do MARKER_PATTERNS+=("$line"); done < <(read_markers "$file")
    ((${#MARKER_PATTERNS[@]} > before)) ||
      die "$file holds no markers — an empty set reads as a working check while checking nothing"
  done
  ((${#MARKER_PATTERNS[@]} > 0)) || die "no marker files were loaded"
}

# Prints "pattern<TAB>line" for every marker found; returns 0 when anything was found.
# T_ALLOW is an extended regex whose matching lines are dropped before the scan.
scan_log() {
  local log="$1"
  shift
  # Already loaded and validated by the caller, which is the shell that can still exit
  local -a pats=("${MARKER_PATTERNS[@]}")
  (($#)) && pats+=("$@")

  # T_ALLOW is the ad-hoc override and wins over the repository's own `allow` line
  local allow="${T_ALLOW:-$POLICY_ALLOW}"
  local source="$log"
  if [[ -n "$allow" ]]; then
    source="$log.scanned"
    grep -Ev -- "$allow" "$log" >"$source" || :
  fi

  local pat line found=1
  for pat in "${pats[@]}"; do
    while IFS= read -r line; do
      printf '%s\t%s\n' "$pat" "$line"
      found=0
    done < <(grep -i -F -m 3 -- "$pat" "$source" || :)
  done

  [[ -n "$allow" ]] && rm -f "$source"
  return "$found"
}

cmd_run() {
  local tail_n=40 saw_ddash="" logdir=""
  MARKER_FILES=("$MARKER_DIR/default.txt")
  EXTRA_PATTERNS=()
  POLICY_LOGDIR=""
  POLICY_ALLOW=""

  # Policy first, then the flags on top: what you type adds to the repository's own
  # settings rather than silently replacing them
  load_config
  logdir="${T_LOGDIR:-${POLICY_LOGDIR:-.test-logs}}"

  local -a extra=(${EXTRA_PATTERNS[@]+"${EXTRA_PATTERNS[@]}"})
  while (($#)); do
    case "$1" in
      -l)
        logdir="${2:?-l needs a directory}"
        shift 2
        ;;
      -m)
        # Additive: the default set always applies, and a repository opts into more
        resolve_markers "${2:?-m needs a marker set or file}"
        MARKER_FILES+=("$RESOLVED")
        shift 2
        ;;
      -p)
        extra+=("${2:?-p needs a pattern}")
        shift 2
        ;;
      -t)
        tail_n="${2:?-t needs a number}"
        shift 2
        ;;
      --)
        saw_ddash=1
        shift
        break
        ;;
      *) die "run: unexpected argument '$1' — the command goes after --" ;;
    esac
  done
  [[ -n "$saw_ddash" ]] || die "run: the command must follow -- (t.sh run -- pytest -q)"
  (($#)) || die "run: no command after --"

  # Before the command runs, and from this shell rather than a subshell, so a broken
  # marker set stops the run instead of quietly making every run a pass
  load_markers

  mkdir -p "$logdir" || die "run: cannot create $logdir"
  local log="${T_LOGFILE:-}"
  [[ -n "$log" ]] || log="$logdir/run-$(date +%Y%m%d-%H%M%S)-$$.log"
  # Refuse before running rather than discover it afterwards: a run whose log could not be
  # written cannot be read, and reading the log is half of what this harness is for
  : >"$log" || die "run: cannot write $log"

  # CMD's own status, never the pipeline's. `cmd | tee` reports tee and `cmd | tail`
  # reports tail — both are 0 for a suite that just failed, which is how a red run
  # gets committed as a green one.
  "$@" 2>&1 | tee "$log"
  local status=${PIPESTATUS[0]}

  local hits
  hits=$(scan_log "$log" ${extra[@]+"${extra[@]}"}) || :

  local verdict=$status
  if ((status == 0)) && [[ -n "$hits" ]]; then
    verdict=3
  fi

  # The verdict is decided above, before a single line of the log is shown. Whatever is
  # printed from here on is for a reader, and can no longer become the answer.
  echo
  if [[ -n "$hits" ]]; then
    printf 't.sh: the log carries markers of a run that did not do its job:\n' >&2
    printf '%s\n' "$hits" | while IFS=$'\t' read -r pat line; do
      printf '  [%s] %s\n' "$pat" "$line" >&2
    done
  fi

  case "$verdict" in
    0) printf 't.sh: pass — exit 0, log clean (%s)\n' "$log" ;;
    3) printf 't.sh: LIED — exit 0, but the log above says otherwise (%s)\n' "$log" >&2 ;;
    *) printf 't.sh: fail — exit %d (%s)\n' "$status" "$log" >&2 ;;
  esac

  if ((verdict != 0)) && ((tail_n > 0)); then
    printf '\n--- last %d lines ---\n' "$tail_n" >&2
    tail -n "$tail_n" "$log" >&2
  fi

  return "$verdict"
}

# Runs the same command N times and reports how many runs disagreed with the first.
# That is the whole mechanism: no history, no statistics, no quarantine list. It exists to
# turn "it failed once, probably nothing" into evidence, so the underlying race can be
# fixed — never to make an unstable test tolerable by rerunning it until it agrees.
cmd_flaky() {
  local n="${1:-}"
  [[ "$n" =~ ^[0-9]+$ ]] || die "flaky: first argument must be a run count (t.sh flaky 20 -- ...)"
  ((n >= 2)) || die "flaky: $n runs cannot show disagreement; use 2 or more"
  shift

  # The same policy `run` obeys: a repository that named its log directory once should not
  # find flaky writing somewhere else
  POLICY_LOGDIR=""
  load_config
  local logdir="${T_LOGDIR:-${POLICY_LOGDIR:-.test-logs}}"
  local -a pass=()
  while (($#)); do
    case "$1" in
      -l)
        logdir="${2:?-l needs a directory}"
        shift 2
        ;;
      --) break ;;
      *)
        pass+=("$1")
        shift
        ;;
    esac
  done
  [[ "${1:-}" == "--" ]] || die "flaky: the command must follow -- (t.sh flaky 20 -- pytest -q)"

  local stamp
  stamp="$logdir/flaky-$(date +%Y%m%d-%H%M%S)-$$"
  mkdir -p "$stamp" || die "flaky: cannot create $stamp"

  local i status baseline="" differed=0 agreed=0 first_divergence=""
  for ((i = 1; i <= n; i++)); do
    status=0
    T_LOGFILE="$stamp/run-$i.log" cmd_run -t 0 -l "$stamp" "${pass[@]+"${pass[@]}"}" "$@" \
      >/dev/null 2>&1 || status=$?
    if [[ -z "$baseline" ]]; then
      baseline="$status"
      agreed=1
      printf 'run %d/%d  baseline: exit %d\n' "$i" "$n" "$status"
      continue
    fi
    if ((status == baseline)); then
      agreed=$((agreed + 1))
      printf 'run %d/%d  same (exit %d)\n' "$i" "$n" "$status"
    else
      differed=$((differed + 1))
      [[ -n "$first_divergence" ]] || first_divergence="$stamp/run-$i.log"
      printf 'run %d/%d  DIFFERED: exit %d, not %d  -> %s\n' "$i" "$n" "$status" "$baseline" "$stamp/run-$i.log"
    fi
  done

  echo
  if ((differed == 0)); then
    printf 't.sh: stable across %d runs — every run exited %d (%s)\n' "$n" "$baseline" "$stamp"
    return "$baseline"
  fi
  printf 't.sh: UNSTABLE — %d of %d runs disagreed with the first (%s)\n' "$differed" "$n" "$stamp" >&2
  printf 'first divergence: %s\n' "$first_divergence" >&2
  printf '%d agreed, %d differed. Fix the race or quarantine the test; do not retry it.\n' \
    "$agreed" "$differed" >&2
  return 4
}

# The verdict on ONE commit, in the vocabulary `git bisect run` speaks:
#
#   0        good
#   1        bad
#   125      skip — this commit cannot answer the question
#   126+     git bisect ABORTS the whole session
#
# That last line is why nothing here passes a status through untouched. A test runner that
# is missing at an old commit exits 127, and a raw pass-through would end the bisect
# instead of stepping over that commit; a suite killed by a signal exits 128+n and would do
# the same. Both are clamped below, and the two states that mean "no answer" — a commit
# that will not build, and a run whose log says it never really ran — become skips rather
# than a confident, wrong accusation.
cmd_bisect_probe() {
  local build=""
  local -a pass=()
  while (($#)); do
    case "$1" in
      -b)
        build="${2:?-b needs a command}"
        shift 2
        ;;
      --) break ;;
      *)
        pass+=("$1")
        shift
        ;;
    esac
  done
  [[ "${1:-}" == "--" ]] || die "bisect-probe: the command must follow --"

  if [[ -n "$build" ]]; then
    if ! sh -c "$build" >&2; then
      echo "t.sh: this commit does not build — skipping it rather than blaming it" >&2
      return 125
    fi
  fi

  # A short tail by default: a bisect prints one verdict per commit, and forty lines each
  # buries the answer. A -t the caller passed comes later in the list and wins.
  local status=0
  cmd_run -t 5 "${pass[@]+"${pass[@]}"}" "$@" || status=$?
  case "$status" in
    0) return 0 ;;
    2 | 3 | 125 | 127)
      # 2 the harness could not run it, 3 the run did not really run, 127 the runner is
      # not there at this commit. None of them is evidence against the commit.
      echo "t.sh: no verdict from this commit (exit $status) — skipping" >&2
      return 125
      ;;
    *) return 1 ;;
  esac
}

cmd_bisect() {
  local good="${1:-}"
  [[ -n "$good" ]] || die "bisect: needs a known-good ref (t.sh bisect v1.2.0 -- pytest -q)"
  shift
  local -a pass=()
  while (($#)); do
    case "$1" in
      --) break ;;
      *)
        pass+=("$1")
        shift
        ;;
    esac
  done
  [[ "${1:-}" == "--" ]] || die "bisect: the command must follow -- (t.sh bisect HEAD~20 -- pytest -q)"

  git rev-parse --git-dir >/dev/null 2>&1 || die "bisect: not inside a git repository"
  if ! git diff --quiet || ! git diff --cached --quiet; then
    die "bisect: the working tree has uncommitted changes — commit or stash them first, because bisect checks other commits out over them"
  fi
  git rev-parse --verify --quiet "$good^{commit}" >/dev/null ||
    die "bisect: '$good' is not a commit in this repository"

  local self
  self=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)/$(basename -- "${BASH_SOURCE[0]}")

  # Logs go outside the working tree: bisect checks other commits out over it, and a
  # directory of logs sitting in the middle of that is noise at best
  local logdir
  logdir=$(mktemp -d) || die "bisect: cannot create a log directory"
  echo "t.sh: logs for this bisect are in $logdir"

  # Leaving a repository in a detached bisect state is a nasty thing to do to whoever runs
  # this, including on an interrupt
  trap 'git bisect reset >/dev/null 2>&1 || :' EXIT

  git bisect start >/dev/null || die "bisect: could not start"
  git bisect bad HEAD >/dev/null || die "bisect: could not mark HEAD bad"
  git bisect good "$good" >/dev/null || die "bisect: could not mark $good good"

  local status=0
  T_LOGDIR="$logdir" git bisect run "$self" bisect-probe "${pass[@]+"${pass[@]}"}" "$@" || status=$?
  return "$status"
}

# A passing suite says the code works. It does not say the suite would notice if the code
# stopped working, and that is the question worth asking of a green run. This answers it by
# applying, one at a time, edits a human wrote down — never edits it invented. Nothing here
# generates mutants: a tool that rewrites code on its own mostly produces code that will
# not compile, and a compiler error is not a test noticing anything.
DEF_NAME=()
DEF_FILE=()
DEF_FIND=()
DEF_REPLACE=()
DEF_WHY=()

# The one call a defects file makes. Sourced, so the file is plain bash and needs no parser.
defect() {
  (($# == 5)) ||
    die "defects: defect takes 5 arguments (name file find replace consequence), got $#"
  DEF_NAME+=("$1")
  DEF_FILE+=("$2")
  DEF_FIND+=("$3")
  DEF_REPLACE+=("$4")
  DEF_WHY+=("$5")
}

# Reads a file into the variable NAMED by $1, so putting it back is byte-for-byte rather
# than close enough. It assigns rather than prints for a reason: command substitution
# strips trailing newlines, so `var=$(slurp f)` would throw away the very thing the
# `printf x` dance exists to preserve — and a restore that drops a trailing newline leaves
# the working tree dirty in a way only a byte-wise diff notices.
slurp() { # slurp VARNAME FILE
  local __content
  __content=$(cat "$2" && printf x) || return 1
  printf -v "$1" '%s' "${__content%x}"
}

count_occurrences() {
  local haystack="$1" needle="$2" n=0
  while [[ "$haystack" == *"$needle"* ]]; do
    haystack="${haystack#*"$needle"}"
    n=$((n + 1))
  done
  printf '%d' "$n"
}

cmd_falsify() {
  local defects="tests/defects.sh" build="" filter=""
  local -a pass=()
  while (($#)); do
    case "$1" in
      -d)
        defects="${2:?-d needs a file}"
        shift 2
        ;;
      -b)
        build="${2:?-b needs a command}"
        shift 2
        ;;
      -l | -m | -p | -t)
        pass+=("$1" "${2:?$1 needs a value}")
        shift 2
        ;;
      --) break ;;
      *)
        [[ -z "$filter" ]] || die "falsify: only one filter may be given"
        filter="$1"
        shift
        ;;
    esac
  done
  [[ "${1:-}" == "--" ]] || die "falsify: the suite command must follow -- (t.sh falsify -- pytest -q)"
  [[ -r "$defects" ]] || die "falsify: cannot read $defects — write the defect list first (see templates/defects.sh)"

  # A dirty tree makes an interrupted restore indistinguishable from your own edits, and
  # this is a command that edits your source on purpose
  git rev-parse --git-dir >/dev/null 2>&1 || die "falsify: not inside a git repository"
  if ! git diff --quiet || ! git diff --cached --quiet; then
    die "falsify: the working tree has uncommitted changes — commit or stash them first, so an interrupted run cannot be mistaken for your own edits"
  fi

  # shellcheck source=/dev/null
  source "$defects"
  ((${#DEF_NAME[@]} > 0)) || die "falsify: $defects declared no defects — an empty list proves nothing"

  local -a files=()
  local f
  for f in "${DEF_FILE[@]}"; do
    [[ " ${files[*]-} " == *" $f "* ]] || files+=("$f")
  done
  local -A original=()
  local __slurped=""
  for f in "${files[@]}"; do
    [[ -r "$f" ]] || die "falsify: $defects names $f, which cannot be read"
    slurp __slurped "$f" || die "falsify: cannot read $f"
    original["$f"]="$__slurped"
  done

  # Restoration happens here and not only at the end of the loop, so an interrupt, a
  # failure or a kill cannot leave the source edited. The contents come from memory rather
  # than from git, so this needs neither a clean checkout nor git to be working.
  # shellcheck disable=SC2317  # reached through the trap, which shellcheck does not follow
  restore_all() {
    local file
    for file in "${files[@]}"; do
      printf '%s' "${original[$file]}" >"$file" 2>/dev/null || :
    done
  }
  trap 'restore_all' EXIT INT TERM

  suite_verdict() { # prints caught | survived | unusable
    if [[ -n "$build" ]]; then
      if ! sh -c "$build" >/dev/null 2>&1; then
        printf 'unusable'
        return
      fi
    fi
    local status=0
    cmd_run -t 0 "${pass[@]+"${pass[@]}"}" "$@" >/dev/null 2>&1 || status=$?
    case "$status" in
      0) printf 'survived' ;;
      3) printf 'unusable' ;;
      *) printf 'caught' ;;
    esac
  }

  echo "== the suite is green before anything is broken"
  # Falsification measures the distance between green and red. Starting red there is no
  # distance, and every "caught" below would be meaningless.
  local baseline
  baseline=$(suite_verdict "$@")
  case "$baseline" in
    caught) die "falsify: the suite is already failing — fix that first, or nothing measured here means anything" ;;
    unusable) die "falsify: the suite did not really run before any edit — check the build command and the log" ;;
  esac

  local i name file find replace why verdict content mutated occurrences
  local -a survived=() ran=()
  for i in "${!DEF_NAME[@]}"; do
    name="${DEF_NAME[$i]}"
    [[ -z "$filter" || "$name" == *"$filter"* ]] || continue
    file="${DEF_FILE[$i]}"
    find="${DEF_FIND[$i]}"
    replace="${DEF_REPLACE[$i]}"
    why="${DEF_WHY[$i]}"
    ran+=("$name")

    content="${original[$file]}"
    occurrences=$(count_occurrences "$content" "$find")
    if ((occurrences != 1)); then
      # Not guessed at: a list that no longer describes the code has to say so, or it
      # quietly stops testing the thing it was written for
      printf 'stale     %s: its find text matches %s times in %s, not once\n' "$name" "$occurrences" "$file"
      survived+=("$name")
      continue
    fi

    mutated="${content//"$find"/"$replace"}"
    printf '%s' "$mutated" >"$file"
    verdict=$(suite_verdict "$@")
    printf '%s' "$content" >"$file"

    case "$verdict" in
      caught) printf 'caught    %s\n' "$name" ;;
      survived)
        printf 'SURVIVED  %s: %s\n' "$name" "$why"
        survived+=("$name")
        ;;
      unusable)
        # The compiler noticed the syntax; the tests said nothing. Calling this "caught"
        # is how a suite gets credit for coverage it does not have.
        printf 'unusable  %s: the edit stopped it building, so the tests were never asked\n' "$name"
        survived+=("$name")
        ;;
    esac
  done

  restore_all
  trap - EXIT INT TERM
  for f in "${files[@]}"; do
    slurp __slurped "$f" || die "falsify: cannot re-read $f to confirm it was restored"
    [[ "$__slurped" == "${original[$f]}" ]] ||
      die "falsify: $f was not restored to what it was — restore it from git before doing anything else"
  done

  ((${#ran[@]} > 0)) || die "falsify: no defect matched the filter '$filter'"

  echo
  if ((${#survived[@]} > 0)); then
    printf '%d of %d defect(s) were not caught by the suite.\n' "${#survived[@]}" "${#ran[@]}" >&2
    return 1
  fi
  printf 'all %d defect(s) were caught by the suite.\n' "${#ran[@]}"
}

cmd="${1:-}"
(($# == 0)) || shift
case "$cmd" in
  run) cmd_run "$@" ;;
  flaky) cmd_flaky "$@" ;;
  bisect) cmd_bisect "$@" ;;
  bisect-probe) cmd_bisect_probe "$@" ;;
  falsify) cmd_falsify "$@" ;;
  -h | --help | help) usage ;;
  '')
    usage >&2
    exit 2
    ;;
  *)
    printf 't.sh: no such subcommand: %s\n\n' "$cmd" >&2
    usage >&2
    exit 2
    ;;
esac
