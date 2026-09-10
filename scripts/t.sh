#!/usr/bin/env bash
# t.sh — the local test harness: one subcommand per question a test run raises.
#
# `t.sh help [SUBCOMMAND]` is the reference: every subcommand, every flag, the T_ variables,
# the exit codes. It is the only list on purpose — this header used to carry a second one,
# and it fell three subcommands behind the dispatch before anyone noticed.
# The command is always explicit, after `--`; nothing here guesses what your suite is.
# A repository keeps its policy — marker sets, excused lines, log directory — in
# ./tests/t.conf, read from the current directory only, and never the command.
# Another repository takes this file and markers/ beside it through the ci skill's
# vendoring cascade (references/bump-cascade.md in https://github.com/rokokol/ci-skill)
# and never edits its copies in place: a fix belongs in rokokol/tests-skill.
set -uo pipefail

# 64 is EX_USAGE and 70 is EX_SOFTWARE in sysexits(3): the caller asked wrongly, or the
# harness broke. Neither is CMD's status, and a probe that skips on 64 must not skip on 70.
die() {
  printf 't.sh: %s\n' "$1" >&2
  exit 64
}

fatal() {
  printf 't.sh: %s\n' "$1" >&2
  exit 70
}

# Resolved through symlinks: `ln -s .../t.sh ~/.local/bin/t.sh` is how this gets onto a
# PATH, and dirname of the link would look for markers/ beside the link and refuse every
# run. A loop over readlink rather than `readlink -f`, which macOS only gained in 12.3.
self="${BASH_SOURCE[0]}"
while [[ -L "$self" ]]; do
  target=$(readlink "$self")
  case "$target" in
    /*) self="$target" ;;
    *) self="$(dirname -- "$self")/$target" ;;
  esac
done
HERE=$(cd -- "$(dirname -- "$self")" && pwd)
SELF="$HERE/$(basename -- "$self")"
unset self target
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

# The same set named twice — twice on the line, or once in the config and once on the
# line — would print every finding twice, and a doubled finding reads as two problems
add_marker_file() {
  local f
  for f in ${MARKER_FILES[@]+"${MARKER_FILES[@]}"}; do
    [[ "$f" != "$1" ]] || return 0
  done
  MARKER_FILES+=("$1")
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
    # `-z "${T_CONFIG-}"` rather than `-v T_CONFIG`: the second is bash 4.2+, and macOS
    # ships 3.2. It is also the same test — an unset T_CONFIG expands to the empty string
    # here, and an empty one returned above.
    [[ -z "${T_CONFIG-}" ]] || die "config: $conf does not exist"
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
        add_marker_file "$RESOLVED"
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

# What the last cmd_run in this shell concluded: pass, fail or lied. Empty until it ran.
RUN_VERDICT=""

# falsify's marker of a defect in flight, removed with the restore; global for the traps
FLIGHT=""

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
  local source="$log" rc=0
  if [[ -n "$allow" ]]; then
    source="$log.scanned"
    grep -Ev -- "$allow" "$log" >"$source" || rc=$?
    # 1 means every line was excused, which is odd but legitimate. Anything above it
    # means the filter did not run, and a log it did not run on must not read as clean.
    # cmd_run refuses such a regex before CMD starts; this is the guard behind that one,
    # for the day it is bypassed — a `die` here would only exit the $(...) around us.
    if ((rc > 1)); then
      printf 'allow\tthe allow regex could not be applied (grep exited %d), so the log was not scanned\n' "$rc"
      rm -f "$source"
      return 0
    fi
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
        add_marker_file "$RESOLVED"
        shift 2
        ;;
      -p)
        extra+=("${2:?-p needs a pattern}")
        shift 2
        ;;
      -t)
        tail_n="${2:?-t needs a number}"
        # (( )) reads a word as the variable of that name, which is zero, so `-t abc`
        # would silently mean "no tail" rather than refuse
        [[ "$tail_n" =~ ^[0-9]+$ ]] || die "run: -t needs a number of lines, got '$tail_n'"
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

  # The same for the allow regex. It used to be applied only inside scan_log, where grep
  # rejecting it left the filtered log empty, and an empty log has no markers — so a typo
  # in `allow` turned every run into a pass, silently. Judged by what grep says rather
  # than by its status, and against a line of input rather than /dev/null: GNU and BSD grep
  # exit 2 on a regex they cannot compile, busybox's does not compile it at all until there
  # is a line to match, and every one of them complains on stderr once it does.
  local allow="${T_ALLOW:-$POLICY_ALLOW}" complaint=""
  if [[ -n "$allow" ]]; then
    complaint=$(printf 'x\n' | grep -E -- "$allow" 2>&1 >/dev/null || :)
    [[ -z "$complaint" ]] || die "allow: '$allow' is not a regex grep -E accepts — $complaint"
  fi

  mkdir -p "$logdir" || fatal "run: cannot create $logdir"
  local log="${T_LOGFILE:-}"
  [[ -n "$log" ]] || log="$logdir/run-$(date +%Y%m%d-%H%M%S)-$$.log"
  # Refuse before running rather than discover it afterwards: a run whose log could not be
  # written cannot be read, and reading the log is half of what this harness is for
  : >"$log" || fatal "run: cannot write $log"

  # CMD's own status, never the pipeline's. `cmd | tee` reports tee and `cmd | tail`
  # reports tail — both are 0 for a suite that just failed, which is how a red run
  # gets committed as a green one.
  "$@" 2>&1 | tee "$log"
  # Copied whole, in the one command that still can: bash resets PIPESTATUS after every
  # simple command, and an assignment or a `local` is one. A second reference on the next
  # line would already read empty.
  local -a ps=("${PIPESTATUS[@]}")
  local status=${ps[0]}

  local hits
  hits=$(scan_log "$log" ${extra[@]+"${extra[@]}"}) || :

  local verdict=$status
  if ((status == 0)) && [[ -n "$hits" ]]; then
    verdict=79
  fi

  # The KIND of verdict, out of band. A number cannot carry it: CMD's own 79 or 64 would
  # read as the harness's, and make's 2 once read as "the harness could not run it". So
  # the kind is set here for a caller in this shell, and written beside the log for a
  # caller that had to run this in a subshell — flaky, bisect-probe and falsify all do.
  RUN_VERDICT=fail
  ((verdict != 0)) || RUN_VERDICT=pass
  ((verdict != 79)) || RUN_VERDICT=lied
  printf '%s\n' "$RUN_VERDICT" >"$log.verdict"

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
    79) printf 't.sh: LIED — exit 0, but the log above says otherwise (%s)\n' "$log" >&2 ;;
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
      -m)
        # Resolved here as well as in run, so a set that does not exist is refused
        # before the first of twenty runs rather than inside it
        resolve_markers "${2:?-m needs a marker set or file}"
        pass+=("$1" "$2")
        shift 2
        ;;
      -p | -t)
        # Forwarded to run, which validates them. Named here rather than swept up by a
        # catch-all, so the gate can read from this parser which flags flaky accepts.
        pass+=("$1" "${2:?$1 needs a value}")
        shift 2
        ;;
      --) break ;;
      *) die "flaky: unexpected argument '$1' — the command goes after --" ;;
    esac
  done
  [[ "${1:-}" == "--" ]] || die "flaky: the command must follow -- (t.sh flaky 20 -- pytest -q)"

  local stamp
  stamp="$logdir/flaky-$(date +%Y%m%d-%H%M%S)-$$"
  mkdir -p "$stamp" || fatal "flaky: cannot create $stamp"

  local i status baseline="" differed=0 agreed=0 first_divergence=""
  for ((i = 1; i <= n; i++)); do
    status=0
    # In a subshell, so a `die` inside run ends that run and not this loop; and with its
    # stderr kept, because a refusal that went to /dev/null with the command's output
    # once left flaky exiting without a word
    (T_LOGFILE="$stamp/run-$i.log" cmd_run -t 0 -l "$stamp" "${pass[@]+"${pass[@]}"}" "$@") \
      >/dev/null 2>"$stamp/run-$i.err" || status=$?
    # No sidecar means run never reached a verdict — it refused, or it died. Neither is
    # a result to compare the other runs against.
    if [[ ! -e "$stamp/run-$i.log.verdict" ]]; then
      head -3 "$stamp/run-$i.err" >&2
      fatal "flaky: run $i produced no verdict (exit $status) — the harness could not run the command"
    fi
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
  return 86
}

# The verdict on ONE commit, in the vocabulary `git bisect run` speaks (git-bisect(1)):
#
#   0          good
#   1–124      bad
#   125        skip — this commit cannot answer the question
#   126, 127   bad as well: "command not found" is an ordinary error to git
#   128+       git ABORTS the whole session
#
# The kind of verdict comes from run's sidecar, never from the number: the command's own
# 2 is make failing, and once read here as "the harness could not run it", which skipped
# every commit where `make test` failed and named nobody. What the number still decides:
#   - 126 and 127 are a runner that is not there at this commit, which is no evidence
#     against it, so they are skipped rather than blamed;
#   - a run stopped by a person — HUP, INT, QUIT, TERM, that is 129/130/131/143 — is not
#     evidence either, and it is passed through so git aborts, which is what Ctrl-C means;
#   - any other signal (SEGV, ABRT, BUS, an OOM kill) is the code at this commit crashing,
#     and a crash is bad — clamped to 1 so git records it instead of aborting.
cmd_bisect_probe() {
  local build=""
  local -a pass=()
  while (($#)); do
    case "$1" in
      -b)
        build="${2:?-b needs a command}"
        shift 2
        ;;
      -l | -m | -p | -t)
        pass+=("$1" "${2:?$1 needs a value}")
        shift 2
        ;;
      --) break ;;
      *) die "bisect-probe: unexpected argument '$1' — the command goes after --" ;;
    esac
  done
  [[ "${1:-}" == "--" ]] || die "bisect-probe: the command must follow --"

  if [[ -n "$build" ]]; then
    if ! sh -c "$build" >&2; then
      echo "t.sh: this commit does not build — skipping it rather than blaming it" >&2
      return 125
    fi
  fi

  # The log path is chosen here so the sidecar can be found afterwards; bisect sets
  # T_LOGDIR to a directory outside the working tree it is checking commits out into
  local log="${T_LOGFILE:-}"
  if [[ -z "$log" ]]; then
    local dir="${T_LOGDIR:-.test-logs}"
    mkdir -p "$dir" || fatal "bisect-probe: cannot create $dir"
    log="$dir/probe-$(date +%Y%m%d-%H%M%S)-$$.log"
  fi

  # A short tail by default: a bisect prints one verdict per commit, and forty lines each
  # buries the answer. A -t the caller passed comes later in the list and wins. In a
  # subshell, so a refusal inside run reaches the case below as "no verdict" instead of
  # ending the probe with a status git would take for an answer.
  local status=0 kind=""
  (T_LOGFILE="$log" cmd_run -t 5 "${pass[@]+"${pass[@]}"}" "$@") || status=$?
  [[ ! -r "$log.verdict" ]] || kind=$(cat "$log.verdict")

  case "$kind" in
    pass) return 0 ;;
    lied)
      echo "t.sh: this commit's run exited 0 but its log says nothing ran — skipping it" >&2
      return 125
      ;;
    fail)
      case "$status" in
        126 | 127)
          echo "t.sh: the test runner is not there at this commit (exit $status) — skipping it" >&2
          return 125
          ;;
        129 | 130 | 131 | 143) return "$status" ;;
        *) return 1 ;;
      esac
      ;;
    *)
      echo "t.sh: no verdict from this commit (exit $status) — the harness could not run it, skipping" >&2
      return 125
      ;;
  esac
}

# A test that passes alone and fails in the suite was polluted by something that ran
# before it. references/debugging.md gives the method — halve the order the way git bisect
# halves commits — and this is that, mechanically. The candidates come in on stdin, one
# test per line in the order they run, which is what a collector prints:
#
#   pytest --collect-only -q | t.sh pollute tests/test_sync.py::test_retry -- pytest
#
# CMD is given the selection as trailing arguments, so it has to be a runner that takes a
# list of tests that way — pytest, vitest, jest, phpunit. `go test -run` does not, and a
# wrapper that turns a list into its own selector is the adopter's to write.
cmd_pollute() { # pollute [-l DIR] [-m SET] [-p PATTERN] VICTIM -- CMD...
  local victim=""
  local -a pass=() runner=()
  while (($#)); do
    case "$1" in
      -l | -m | -p | -t)
        pass+=("$1" "${2:?$1 needs a value}")
        shift 2
        ;;
      --)
        shift
        runner=("$@")
        break
        ;;
      -*) die "pollute: no such flag: $1" ;;
      *)
        [[ -z "$victim" ]] || die "pollute: only one victim may be given"
        victim="$1"
        shift
        ;;
    esac
  done
  [[ -n "$victim" ]] || die "pollute: needs the test that fails in the suite and passes alone"
  ((${#runner[@]} > 0)) || die "pollute: the command goes after -- (t.sh pollute VICTIM -- pytest)"

  local -a cands=()
  local line
  while IFS= read -r line; do
    [[ -n "$line" ]] || continue
    [[ "$line" != "$victim" ]] || continue
    cands+=("$line")
  done
  ((${#cands[@]} > 0)) ||
    die "pollute: no candidates on stdin — give the tests that run before the victim, one per line"

  # Reads as an ordinary run, so a suite that exits 0 while its log says otherwise counts
  # as a failure here too, which is the whole reason run exists
  fails_after() { # fails_after TEST... -> true when the victim fails with these before it
    local st=0
    (cmd_run "${pass[@]+"${pass[@]}"}" -- "${runner[@]}" "$@" "$victim") >/dev/null 2>&1 || st=$?
    ((st != 0))
  }

  # Both ends first, because a search whose premises do not hold finds a confident answer to
  # the wrong question: a victim that fails by itself is not polluted, and one that survives
  # the whole order has nothing to find
  local st=0
  (cmd_run "${pass[@]+"${pass[@]}"}" -- "${runner[@]}" "$victim") >/dev/null 2>&1 || st=$?
  ((st == 0)) || {
    printf 't.sh: pollute: %s fails on its own (exit %s) — that is a broken test, not a polluted one\n' \
      "$victim" "$st" >&2
    return 85
  }
  fails_after "${cands[@]}" || {
    printf 'pollute: %s passes with all %s candidate(s) before it — there is nothing to find in this order\n' \
      "$victim" "${#cands[@]}"
    return 0
  }

  # Halve, keep the half that still reproduces
  local -a narrowed=("${cands[@]}") left=() right=()
  local half
  while ((${#narrowed[@]} > 1)); do
    half=$((${#narrowed[@]} / 2))
    left=("${narrowed[@]:0:half}")
    right=("${narrowed[@]:half}")
    if fails_after "${left[@]}"; then
      narrowed=("${left[@]}")
    elif fails_after "${right[@]}"; then
      narrowed=("${right[@]}")
    else
      # Neither half on its own does it, so the pollution needs more than one of them
      # together. Saying so beats halving on and naming whichever test the split happened
      # to leave holding it
      printf 'POLLUTED  %s needs more than one of these together:\n' "$victim"
      printf '          %s\n' "${narrowed[@]}"
      echo
      printf 'pollute: narrowed to %s test(s), and no half of them reproduces it alone\n' "${#narrowed[@]}" >&2
      return 82
    fi
  done

  printf 'POLLUTED  %s fails when %s has run before it\n' "$victim" "${narrowed[0]}"
  echo
  printf 'pollute: %s of %s candidate(s) left after halving — run the two together to see it\n' \
    "${#narrowed[@]}" "${#cands[@]}"
}

# A test taken out of the gate with no date on it is not quarantined, it is deleted with
# extra steps. references/curation.md gives the file its shape — one row per test, with an
# owner and an expiry — and this is the part a gate can hold: a row past its expiry is a
# decision nobody made, and a date that is not a date is a row that can never expire, which
# is the same silence an unknown config key gives.
cmd_quarantine() { # quarantine [--on YYYY-MM-DD] [FILE]
  local on="" file=""
  while (($#)); do
    case "$1" in
      --on)
        on="${2:?--on needs a date}"
        [[ "$on" =~ ^[0-9][0-9][0-9][0-9]-[0-9][0-9]-[0-9][0-9]$ ]] ||
          die "quarantine: --on needs a date as YYYY-MM-DD, got '$on'"
        shift 2
        ;;
      --) shift ;;
      -*) die "quarantine: no such flag: $1" ;;
      *)
        [[ -z "$file" ]] || die "quarantine: only one file may be given"
        file="$1"
        shift
        ;;
    esac
  done
  file="${file:-tests/quarantine.md}"
  [[ -f "$file" ]] ||
    die "quarantine: $file does not exist — a test taken out of the gate needs a row somewhere a person rereads"
  on="${on:-$(date +%Y-%m-%d)}"

  # The columns are found by their headings rather than counted, so the file stays readable
  # and a column added in the middle does not silently shift what is being checked. ISO
  # dates compare correctly as text, which is the whole reason the format asks for them.
  local out
  out=$(awk -v today="$on" '
    /^[[:space:]]*\|/ {
      n = split($0, cell, "|")
      for (i = 1; i <= n; i++) { gsub(/^[[:space:]]+|[[:space:]]+$/, "", cell[i]) }
      if (!seen) {
        for (i = 1; i <= n; i++) {
          if (cell[i] == "expires") { ecol = i }
          if (cell[i] == "test") { tcol = i }
        }
        if (ecol && tcol) { seen = 1 }
        next
      }
      if (cell[ecol] ~ /^-+$/) { next }
      if (cell[tcol] == "") { next }
      rows++
      if (cell[ecol] !~ /^[0-9][0-9][0-9][0-9]-[0-9][0-9]-[0-9][0-9]$/) {
        printf "UNDATED   %s: expires is \"%s\", which is not a date, so this row can never come up for review\n", cell[tcol], cell[ecol]
        bad++
        next
      }
      if (cell[ecol] < today) {
        printf "OVERDUE   %s: expired %s\n", cell[tcol], cell[ecol]
        bad++
      }
    }
    END {
      if (!seen) { exit 1 }
      printf "COUNT %d %d\n", rows, bad
    }
  ' "$file") || die "quarantine: $file has no table with a 'test' and an 'expires' column — see references/curation.md"

  # The counts come back on the last line rather than through a second channel: a temporary
  # file for two numbers is a temporary file to clean up on every path out of here
  local tally rows bad
  tally=${out##*$'\n'}
  [[ "$tally" == COUNT\ * ]] || fatal "quarantine: the reader did not report a count"
  out=${out%$'\n'"$tally"}
  [[ "$out" != "$tally" ]] || out=""
  tally=${tally#COUNT }
  rows=${tally%% *}
  bad=${tally##* }
  [[ -n "$out" ]] || {
    printf 'quarantine: %s row(s) in %s, none past its expiry as of %s\n' "$rows" "$file" "$on"
    return 0
  }
  printf '%s\n' "$out"
  echo
  printf 'quarantine: %s of %s row(s) in %s are past review as of %s — a deadline that passed is a decision nobody made\n' \
    "$bad" "$rows" "$file" "$on" >&2
  return 81
}

focus_patterns() {
  # Two regexes rather than a list of fixed strings, for two reasons. A bare jasmine focus
  # name written as a literal, with its opening parenthesis,
  # matches curve_fit( and every other identifier ending in it, so a boundary is needed.
  # And a list of the literals would be found by this very scan when it is run on a
  # repository holding this file — a pattern that matches itself reports the harness as the
  # problem. Neither line below matches either line below.
  cat <<'FOCUS'
(^|[^A-Za-z0-9_.$])(describe|context|suite|it|test)\.only[[:space:]]*\(
^[[:space:]]*(fdescribe|fcontext|fit)[[:space:]]*[('"]
FOCUS
}

# The switch that runs one test and skips the rest of the file, left in the source. No
# runner reports it: jest and vitest print a skip count, which is what a suite legitimately
# skipping a platform test prints too, and both exit 0. So a marker cannot reach it and the
# log cannot show it — the source can, which is what this reads.
#
# It is one of a family. `.only`, `--pass-with-no-tests`, `-DskipTests`, `-x`: each was
# added for an honest local reason, each turns a run into a lie when it outlives the commit
# that needed it, and none of them is visible in a log. The ones that live in a config are
# forbidden there — vitest's allowOnly, playwright's forbidOnly. The ones that live in the
# source are found here.
cmd_focused() { # focused [--any-file] [PATH...]
  local any_file=""
  local -a paths=()
  while (($#)); do
    case "$1" in
      --any-file)
        any_file=1
        shift
        ;;
      --)
        shift
        ;;
      -*) die "focused: no such flag: $1" ;;
      *)
        paths+=("$1")
        shift
        ;;
    esac
  done
  ((${#paths[@]} > 0)) || paths=(.)
  local p
  for p in "${paths[@]}"; do
    [[ -e "$p" ]] || die "focused: $p does not exist"
  done

  local pat found=0
  pat=$(mktemp "${TMPDIR:-/tmp}/t.sh.XXXXXX") || fatal "focused: cannot write a pattern file"
  focus_patterns >"$pat" || fatal "focused: cannot write a pattern file"
  # Refused rather than run empty: a scan with no pattern reports nothing and reads exactly
  # like a clean tree, which is the shape of guard this whole harness exists to refuse
  [[ -s "$pat" ]] || fatal "focused: the pattern list came out empty"

  # The paths are filtered out of the result rather than kept out of the search:
  # --exclude-dir is a GNU extension, and the busybox grep a bash-3.2 container brings along
  # rejects it outright. With it, this found nothing there and said so cheerfully — a scan
  # that failed and reported nothing reads exactly like a clean tree, which is the shape of
  # guard this harness exists to refuse.
  local hits scan
  scan=0
  hits=$(grep -rnIE -f "$pat" -- "${paths[@]}" 2>/dev/null) || scan=$?
  rm -f "$pat"
  # grep says 1 for "nothing matched" and 2 or more for "I could not look", and only the
  # first of those is an answer
  ((scan <= 1)) || fatal "focused: the scan itself failed — grep exited $scan, so nothing below means anything"
  if [[ -z "$any_file" && -n "$hits" ]]; then
    # Somebody else's focused test is not this repository's problem, and a vendored tree is
    # large enough to bury the one line that is
    hits=$(printf '%s\n' "$hits" | grep -vE '(^|/)(node_modules|vendor|third_party|\.git|dist|build|target)/' || :)
  fi
  [[ -n "$hits" ]] || {
    echo "focused: no focus modifier in the source — every test the runner is given will run"
    return 0
  }
  printf '%s\n' "$hits" | sed 's/^/FOCUSED  /'
  found=$(printf '%s\n' "$hits" | grep -c . || :)
  echo
  printf 'focused: %s line(s) run one test and skip the rest of their file — the runner will not say so and will exit 0\n' "$found" >&2
  return 80
}

cmd_bisect() {
  local good="${1:-}"
  [[ -n "$good" ]] || die "bisect: needs a known-good ref (t.sh bisect v1.2.0 -- pytest -q)"
  shift
  local -a pass=() start_opts=()
  while (($#)); do
    case "$1" in
      -b | -m | -p | -t)
        # -b for the probe, the rest for run. No -l: the logs of a bisect go outside the
        # working tree on purpose, because bisect checks other commits out over it.
        pass+=("$1" "${2:?$1 needs a value}")
        shift 2
        ;;
      --first-parent | --no-checkout)
        # git's own. --first-parent follows only the first parent of a merge, which is
        # the bisect to run when a merged branch held commits that never built on their
        # own; --no-checkout only moves BISECT_HEAD, for a probe that reads history
        # rather than a tree.
        start_opts+=("$1")
        shift
        ;;
      --) break ;;
      *) die "bisect: unexpected argument '$1' — the command goes after --" ;;
    esac
  done
  [[ "${1:-}" == "--" ]] || die "bisect: the command must follow -- (t.sh bisect HEAD~20 -- pytest -q)"

  git rev-parse --git-dir >/dev/null 2>&1 || die "bisect: not inside a git repository"
  if ! git diff --quiet || ! git diff --cached --quiet; then
    die "bisect: the working tree has uncommitted changes — commit or stash them first, because bisect checks other commits out over them"
  fi
  git rev-parse --verify --quiet "$good^{commit}" >/dev/null ||
    die "bisect: '$good' is not a commit in this repository"
  # `git bisect start` over a bisect already in progress resets it without a word, and
  # whoever was in the middle of that one loses their place
  if git bisect log >/dev/null 2>&1; then
    die "bisect: a bisect is already in progress here — finish it, or run 'git bisect reset' first"
  fi

  # Logs go outside the working tree: bisect checks other commits out over it, and a
  # directory of logs sitting in the middle of that is noise at best. A global, because
  # the EXIT trap below runs after this function's locals are gone.
  BISECT_LOGDIR=$(mktemp -d "${TMPDIR:-/tmp}/t.sh.XXXXXX") || fatal "bisect: cannot create a log directory"
  local logdir="$BISECT_LOGDIR"
  echo "t.sh: logs for this bisect are in $logdir"

  # Leaving a repository in a detached bisect state is a nasty thing to do to whoever runs
  # this, including on an interrupt. git's own session log is saved first: it is the one
  # artifact from which a wrong answer can be corrected — edit it, `git bisect replay`.
  trap 'git bisect log >"$BISECT_LOGDIR/bisect.log" 2>/dev/null; git bisect reset >/dev/null 2>&1 || :' EXIT

  # A search whose premises do not hold answers the wrong question with confidence. Told
  # that HEAD is bad when it is not, git visits commits that all pass, marks each of them
  # good, converges on HEAD and names it: measured on a history where every commit passed,
  # this printed "first bad commit is <HEAD>" and exited 0. The probe git is about to run
  # is what asks, so the premise is measured by the same machinery as the search.
  #
  # The other end is deliberately not checked. Running the suite at GOOD needs a checkout,
  # which is the thing --no-checkout exists to avoid, and a worktree of that commit does
  # not carry the untracked build output the working tree keeps — a false "GOOD is not
  # good" would cost more than the check is worth.
  # Only a HEAD that passes is refused. A HEAD the command cannot be run at is a different
  # thing and not this one's business: git skips such a commit and searches on, and if every
  # commit between the ends is like that it says so — which is the 89 the stuck history is
  # for. Refusing there as well was tried and the gate caught it.
  local head_verdict=0
  T_LOGDIR="$logdir" "$SELF" bisect-probe "${pass[@]+"${pass[@]}"}" "$@" >/dev/null 2>&1 || head_verdict=$?
  ((head_verdict != 0)) || {
    printf 't.sh: bisect: HEAD passes, so there is no first bad commit between %s and here — %s\n' \
      "$good" "the search would visit commits that all pass and name HEAD" >&2
    return 85
  }

  git bisect start "${start_opts[@]+"${start_opts[@]}"}" >/dev/null || fatal "bisect: could not start"
  git bisect bad HEAD >/dev/null || fatal "bisect: could not mark HEAD bad"
  git bisect good "$good" >/dev/null || fatal "bisect: could not mark $good good"

  # git's own exit status cannot carry the answer: it is 0 on a culprit found, nonzero
  # both when only skipped commits are left and when the probe made it abort, and those
  # two mean different things to whoever asked. So what git said is read instead.
  local out="$logdir/bisect.out"
  T_LOGDIR="$logdir" git bisect run "$SELF" bisect-probe "${pass[@]+"${pass[@]}"}" "$@" 2>&1 | tee "$out"
  local -a ps=("${PIPESTATUS[@]}")
  local status=${ps[0]} culprit code
  git bisect log >"$logdir/bisect.log" 2>/dev/null || :

  # "bisect found first bad commit" in one git, "first 'bad' commit" in another, and the
  # term is whatever `git bisect terms` says: the quotes and the word are both optional
  if grep -qE "^bisect found first '?[a-z]*'? commit" "$out"; then
    culprit=$(sed -n "s/^\([0-9a-f]\{7,40\}\) is the first '\{0,1\}[a-z]*'\{0,1\} commit$/\1/p" "$out" | head -1)
    printf 't.sh: first bad commit is %s — the session is in %s/bisect.log, replayable with git bisect replay\n' \
      "$culprit" "$logdir"
    return 0
  fi
  if grep -q 'cannot continue any more' "$out"; then
    printf 't.sh: INCONCLUSIVE — only commits that could not answer are left between good and bad (%s/bisect.log)\n' \
      "$logdir" >&2
    return 89
  fi
  code=$(sed -n 's/^error: bisect run failed: exit code \([0-9]*\) from .*/\1/p' "$out" | head -1)
  if [[ -n "$code" ]] && ((code >= 128)); then
    # The probe passed a signal through so git would abort; end with the same one
    return "$code"
  fi
  fatal "bisect: git bisect run ended with exit $status and no verdict (see $out)"
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
DEF_EXPECT=()
DEF_EXPECT_KIND=()

# The one call a defects file makes. Sourced, so the file is plain bash and needs no parser.
#
#   defect NAME FILE FIND REPLACE CONSEQUENCE
#   defect NAME FILE FIND REPLACE CONSEQUENCE expect survived REASON
#   defect NAME FILE FIND REPLACE CONSEQUENCE expect caught FRAGMENT
#
# The second form declares a defect nothing can catch — an edit that changes the code
# without changing what any caller can observe — with the reason written where the claim
# is. It is reported as expected rather than as a survivor, and the day the suite does
# catch it the expectation is stale and says so, so a declaration cannot outlive its truth.
#
# The third names what should do the catching: a test name, an assertion message, whatever
# the suite prints when that guard is the one that fails. "The suite went red" and "the
# suite noticed this" are different claims, and a run where something else is failing
# credits every defect to a suite that never saw them. Caught without FRAGMENT anywhere in
# the run's output is stale, like a renamed test, because the entry no longer describes
# what it is about.
#
# Both are declared on the line rather than in a separate list of exceptions, the way
# Stryker and cargo-mutants do it, because an exception kept elsewhere is one nobody
# rereads.
defect() {
  local expect="" expect_kind=""
  if (($# == 8)); then
    [[ "$6" == expect && -n "$8" ]] ||
      die "defects: after the consequence, the only words allowed are: expect survived REASON, or expect caught FRAGMENT"
    case "$7" in
      survived | caught) expect_kind="$7" ;;
      *) die "defects: after the consequence, the only words allowed are: expect survived REASON, or expect caught FRAGMENT" ;;
    esac
    expect="$8"
    set -- "$1" "$2" "$3" "$4" "$5"
  fi
  (($# == 5)) ||
    die "defects: defect takes 5 arguments (name file find replace consequence), or 8 with 'expect survived REASON' or 'expect caught FRAGMENT', got $#"
  DEF_NAME+=("$1")
  DEF_FILE+=("$2")
  DEF_FIND+=("$3")
  DEF_REPLACE+=("$4")
  DEF_WHY+=("$5")
  DEF_EXPECT+=("$expect")
  DEF_EXPECT_KIND+=("$expect_kind")
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

# A defect aimed at a test file proves nothing: the test file is executed, so the edit is
# "caught" by whatever it breaks, and the report reads as coverage the suite does not
# have. Vendored and generated code is nobody's guard either. The shapes are the usual
# ones; --any-file is for a list that knows better.
looks_like_test_file() {
  case "$1" in
    tests/* | test/* | spec/* | __tests__/* | */tests/* | */test/* | */spec/* | */__tests__/*) return 0 ;;
    *_test.* | *.test.* | *.spec.* | test_*.py | */test_*.py | *_spec.rb) return 0 ;;
    vendor/* | node_modules/* | third_party/* | */vendor/* | */node_modules/* | */third_party/*) return 0 ;;
    *_pb2.py | *.pb.go | *.generated.* | *.g.dart) return 0 ;;
  esac
  return 1
}

# One run of the suite with a verdict, shared by falsify and prove. Assigns VERDICT —
# caught, survived, unusable, timedout, or none — rather than printing it: a $(...) would
# swallow a die inside run, and the kind of verdict comes from run's sidecar, never from
# the number, because the suite's own exit 79 is not the harness's. run goes in a subshell
# so its own refusal ends that run and reaches the caller as "none".
#
# The run is watched against a deadline. A neutered guard is often a loop that no longer
# ends — the increment removed from a counter is the canonical one — and without a
# deadline that mutant would hang the whole falsification. The suite is started in its
# own process group, under job control, so the deadline can end the runner and everything
# it spawned, not just the shell around them; without GNU timeout(1), which macOS does not
# have, that is what a watchdog is.
#
# Reads three things from the calling subcommand's locals, which bash scopes dynamically:
# `build`, the command that must succeed before the suite is asked; `deadline`, in
# seconds, empty for none; and `pass`, the flags forwarded to run.
VERDICT=""
MUTANT_PGID=""
suite_verdict() { # suite_verdict LOG CMD...
  local log="$1" kind="" pid waited=0 grace=0
  shift
  VERDICT=none
  if [[ -n "$build" ]]; then
    if ! sh -c "$build" >"$log" 2>&1; then
      VERDICT=unusable
      return
    fi
  fi
  set -m
  (T_LOGFILE="$log" cmd_run -t 0 "${pass[@]+"${pass[@]}"}" "$@") >/dev/null 2>&1 &
  pid=$!
  set +m
  MUTANT_PGID="$pid"
  # Tenths of a second, so a fast suite is not held for a whole second per defect
  while kill -0 "$pid" 2>/dev/null; do
    if [[ -n "$deadline" ]] && ((waited >= deadline * 10)); then
      kill -TERM -- -"$pid" 2>/dev/null || :
      while kill -0 "$pid" 2>/dev/null && ((grace < 50)); do
        sleep 0.1
        grace=$((grace + 1))
      done
      kill -KILL -- -"$pid" 2>/dev/null || :
      wait "$pid" 2>/dev/null || :
      MUTANT_PGID=""
      VERDICT=timedout
      return
    fi
    sleep 0.1
    waited=$((waited + 1))
  done
  wait "$pid" 2>/dev/null || :
  MUTANT_PGID=""
  [[ ! -r "$log.verdict" ]] || kind=$(cat "$log.verdict")
  case "$kind" in
    pass) VERDICT=survived ;;
    lied) VERDICT=unusable ;;
    fail) VERDICT=caught ;;
  esac
}

# The suite's process group does not get the Ctrl-C the terminal sends, so a trap ends it
# shellcheck disable=SC2317  # reached through the traps
end_mutant() {
  [[ -z "$MUTANT_PGID" ]] || kill -TERM -- -"$MUTANT_PGID" 2>/dev/null || :
}

# A JSON string literal, for results.json: a consequence sentence may carry quotes
json_str() {
  local s="$1"
  s=${s//\\/\\\\}
  s=${s//\"/\\\"}
  s=${s//$'\n'/\\n}
  s=${s//$'\r'/\\r}
  s=${s//$'\t'/\\t}
  printf '"%s"' "$s"
}

cmd_falsify() {
  local defects="tests/defects.sh" build="" filter="" logdir="" deadline="" out="falsify.out" any_file="" since="" worktree="" shard="" shard_i=0 shard_n=0
  local -a pass=()
  while (($#)); do
    case "$1" in
      -d)
        defects="${2:?-d needs a file}"
        shift 2
        ;;
      --any-file)
        any_file=1
        shift
        ;;
      --worktree)
        worktree=1
        shift
        ;;
      --since)
        since="${2:?--since needs a git ref}"
        shift 2
        ;;
      --shard)
        shard="${2:?--shard needs I/N}"
        # Refused here rather than producing an empty or overlapping selection later: a
        # shard nobody runs, or one run twice, is a defect list that silently stops
        # covering what it names
        [[ "$shard" =~ ^[0-9]+/[0-9]+$ ]] || die "falsify: --shard needs I/N, got '$shard'"
        shard_i="${shard%/*}"
        shard_n="${shard#*/}"
        ((shard_n >= 1)) || die "falsify: --shard needs at least one shard, got '$shard'"
        ((shard_i >= 1 && shard_i <= shard_n)) ||
          die "falsify: --shard $shard asks for shard $shard_i of $shard_n"
        shift 2
        ;;
      --out)
        out="${2:?--out needs a directory}"
        shift 2
        ;;
      --timeout)
        deadline="${2:?--timeout needs a number of seconds}"
        [[ "$deadline" =~ ^[0-9]+$ ]] || die "falsify: --timeout needs a number of seconds, got '$deadline'"
        shift 2
        ;;
      -b)
        build="${2:?-b needs a command}"
        shift 2
        ;;
      -l)
        # Kept here as well as forwarded: the suite runs go through run, but the log
        # each run's verdict is read from has to be a path this function chose
        logdir="${2:?-l needs a directory}"
        pass+=("$1" "$2")
        shift 2
        ;;
      -m | -p | -t)
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
  # Everything a run found goes under one directory, in the shape cargo-mutants made
  # familiar: one file of names per verdict, which is what a diff or a grep wants; one
  # log per defect; and results.json for whatever reads machines. Written as the run
  # goes, so an interrupted run still leaves what it had found. Only the files this
  # writes are cleared first — the directory may be somebody's.
  rm -f "$out"/caught.txt "$out"/survived.txt "$out"/stale.txt "$out"/unusable.txt "$out"/timeout.txt \
    "$out"/expected.txt "$out"/results.json "$out"/in-flight
  rm -rf "$out/logs"
  mkdir -p "$out/logs" || fatal "falsify: cannot create $out"
  # Absolute, because the run may move into a worktree and the findings belong here
  out=$(cd -- "$out" && pwd)
  local -a RES_NAME=() RES_FILE=() RES_LINE=() RES_VERDICT=() RES_WHY=()
  write_results() {
    local i first=1
    {
      printf '{\n  "deadline": %s,\n  "defects": [' "${deadline:-null}"
      for i in "${!RES_NAME[@]}"; do
        if ((first)); then first=0; else printf ','; fi
        printf '\n    {"name": %s, "file": %s, "line": %s, "verdict": %s, "consequence": %s}' \
          "$(json_str "${RES_NAME[$i]}")" "$(json_str "${RES_FILE[$i]}")" "${RES_LINE[$i]:-null}" \
          "$(json_str "${RES_VERDICT[$i]}")" "$(json_str "${RES_WHY[$i]}")"
      done
      printf '\n  ]\n}\n'
    } >"$out/results.json.new" && mv "$out/results.json.new" "$out/results.json"
  }
  record() { # record VERDICT NAME FILE LINE CONSEQUENCE
    local list="$1"
    [[ "$list" != timedout ]] || list=timeout
    printf '%s\n' "$2" >>"$out/$list.txt"
    RES_NAME+=("$2")
    RES_FILE+=("$3")
    RES_LINE+=("$4")
    RES_VERDICT+=("$1")
    RES_WHY+=("$5")
    write_results
  }
  log_name() { # log_name DEFECT-NAME -> a file name: the slashes in a name become dashes
    local n="$1"
    printf '%s' "${n//\//-}"
  }
  # On a GitHub runner a finding also becomes an annotation on the file and line, in the
  # diff of the pull request. A finding in a log is read by whoever opens the log; one
  # next to the code is read by whoever is about to merge it — which is the only place a
  # survivor has ever changed what got written.
  annotate() { # annotate LEVEL FILE LINE TEXT
    [[ -n "${GITHUB_ACTIONS:-}" ]] || return 0
    if [[ -n "$3" ]]; then
      printf '::%s file=%s,line=%s,title=falsify::%s\n' "$1" "$2" "$3" "$4"
    else
      printf '::%s file=%s,title=falsify::%s\n' "$1" "$2" "$4"
    fi
  }

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
  local f i
  if [[ -z "$any_file" ]]; then
    for i in "${!DEF_NAME[@]}"; do
      looks_like_test_file "${DEF_FILE[$i]}" || continue
      die "falsify: ${DEF_NAME[$i]} edits ${DEF_FILE[$i]}, which looks like a test, vendored or generated file — a defect there proves nothing about the suite (--any-file if the list knows better)"
    done
  fi

  # --since narrows the list to the defects in files that changed since a ref: the run
  # for a pull request, with the full list kept for the default branch. A filter and not
  # a proof — a change in one file breaks the tests of another — which is why an empty
  # selection is said out loud rather than passed in silence.
  local changed=""
  if [[ -n "$since" ]]; then
    git rev-parse --verify --quiet "$since^{commit}" >/dev/null ||
      die "falsify: --since '$since' is not a commit in this repository"
    changed=$(git diff --name-only "$since" -- && printf x)
    changed="${changed%x}"
  fi
  changed_since() { # changed_since FILE -> true when FILE is in the diff, or --since is off
    [[ -n "$since" ]] || return 0
    [[ $'\n'"$changed" == *$'\n'"$1"$'\n'* ]]
  }
  local -a selected=()
  for i in "${!DEF_NAME[@]}"; do
    [[ -z "$filter" || "${DEF_NAME[$i]}" == *"$filter"* ]] || continue
    changed_since "${DEF_FILE[$i]}" || continue
    selected+=("$i")
  done
  if ((${#selected[@]} == 0)); then
    [[ -n "$since" ]] || die "falsify: no defect matched the filter '$filter'"
    printf 'nothing to falsify: no defect names a file changed since %s — this is a filter, not a proof; run the full list on the default branch\n' "$since"
    return 0
  fi

  # --shard I/N takes every Nth defect, so N checkouts on N machines cover the list between
  # them: the work is parallel and nothing inside this script is. Every Nth rather than a
  # contiguous block, because the defects of one file sit together and share a build, so
  # blocks would hand one shard all the slow ones and leave the rest waiting on it.
  if [[ -n "$shard" ]]; then
    local -a whole=("${selected[@]}")
    local s
    selected=()
    for ((s = shard_i - 1; s < ${#whole[@]}; s += shard_n)); do
      selected+=("${whole[$s]}")
    done
    # More shards than defects is a matrix wider than the list, not a mistake worth failing
    # a run over — but it is said out loud, because a shard that silently ran nothing looks
    # exactly like a shard where everything was caught
    if ((${#selected[@]} == 0)); then
      printf 'nothing to falsify: shard %s of a list holding %s defect(s) — the matrix is wider than the list\n' "$shard" "${#whole[@]}"
      return 0
    fi
    printf 'shard %s: %s of %s defect(s); the other shards carry the rest, and each has its own summary and exit code\n' \
      "$shard" "${#selected[@]}" "${#whole[@]}"
  fi

  # --worktree edits a checkout of HEAD in a git worktree rather than the files in front
  # of you. In place, a format-on-save, a file watcher, an editor's auto-fix or a commit
  # made mid-run all see the mutant, and the run has to be left alone for as long as it
  # takes. In a worktree none of that can happen, at the price of a build that starts
  # from nothing. Either way a marker names the defect in flight — FALSIFY-IN-PROGRESS at
  # the root of the worktree, in-flight under the findings directory in place — so a
  # mutant that somehow outlives the run is found by the name of what put it there.
  local root wt=""
  root=$(pwd)
  if [[ -n "$worktree" ]]; then
    wt=$(mktemp -d "${TMPDIR:-/tmp}/t.sh.XXXXXX")/wt || fatal "falsify: cannot create a directory for the worktree"
    git worktree add --detach "$wt" HEAD >/dev/null 2>&1 || fatal "falsify: git could not add a worktree at $wt"
    cd -- "$wt" || fatal "falsify: cannot enter the worktree at $wt"
    FLIGHT="$wt/FALSIFY-IN-PROGRESS"
  else
    FLIGHT="$out/in-flight"
  fi
  # shellcheck disable=SC2317  # reached through the traps
  cleanup_worktree() {
    [[ -n "$wt" ]] || return 0
    cd -- "$root" || :
    git -C "$root" worktree remove --force "$wt" >/dev/null 2>&1 || :
    rm -rf "$(dirname -- "$wt")"
    wt=""
  }
  for f in "${DEF_FILE[@]}"; do
    [[ " ${files[*]-} " == *" $f "* ]] || files+=("$f")
  done
  # Two parallel arrays rather than one associative array: `declare -A` is bash 4.0+, and
  # macOS ships 3.2. `files` holds at most a handful of paths, so a linear lookup costs
  # nothing and the harness stays runnable wherever bash is.
  local -a originals=()
  local __slurped=""
  for f in "${files[@]}"; do
    [[ -r "$f" ]] || die "falsify: $defects names $f, which cannot be read"
    slurp __slurped "$f" || fatal "falsify: cannot read $f"
    originals+=("$__slurped")
  done

  # Assigns to the variable NAMED by $1, for the same reason slurp does: a value fetched
  # through $(...) loses its trailing newlines, and a `die` inside one would exit only the
  # subshell — here that would mean silently restoring a file to nothing.
  original_of() { # original_of VARNAME FILE
    local wanted="$2" i
    for i in "${!files[@]}"; do
      [[ "${files[$i]}" == "$wanted" ]] || continue
      printf -v "$1" '%s' "${originals[$i]}"
      return 0
    done
    fatal "falsify: $wanted has no recorded original — the defect list and the file list disagree"
  }

  # Restoration happens here and not only at the end of the loop, so an interrupt, a
  # failure or a kill cannot leave the source edited. The contents come from memory rather
  # than from git, so this needs neither a clean checkout nor git to be working.
  # shellcheck disable=SC2317  # reached through the trap, which shellcheck does not follow
  restore_all() {
    local i
    for i in "${!files[@]}"; do
      printf '%s' "${originals[$i]}" >"${files[$i]}" 2>/dev/null || :
    done
    rm -f "$FLIGHT"
  }
  # EXIT covers a die or a fatal. INT and TERM restore and then die of the same signal,
  # because a handler that merely returns lets the script carry on: written that way,
  # Ctrl-C restored the file and the loop ran the next defect, with the interrupted one
  # gone from the report and still counted in the summary. `exit 130` would be wrong
  # too — the caller would see an exit rather than a signal, and a loop around this
  # would keep going.
  # The suite runs in its own process group (see suite_verdict), which Ctrl-C at the
  # terminal does not reach, so the group is ended first
  trap 'restore_all; cleanup_worktree' EXIT
  trap 'end_mutant; restore_all; cleanup_worktree; trap - INT; kill -INT $$' INT
  trap 'end_mutant; restore_all; cleanup_worktree; trap - TERM; kill -TERM $$' TERM

  echo "== the suite is green before anything is broken"
  # Falsification measures the distance between green and red. Starting red there is no
  # distance, and every "caught" below would be meaningless. The baseline is timed, and
  # the deadline for every mutant is five times that or twenty seconds, whichever is more
  # — five times leaves room for a slower path, twenty seconds for a fast suite whose
  # five times would be a fraction of a second. --timeout sets it outright.
  local started=$SECONDS
  suite_verdict "$out/logs/baseline.log" "$@"
  if [[ -z "$deadline" ]]; then
    deadline=$(((SECONDS - started) * 5))
    ((deadline >= 20)) || deadline=20
  fi
  case "$VERDICT" in
    survived) ;;
    caught)
      echo "t.sh: falsify: the suite is already failing — fix that first, or nothing measured here means anything" >&2
      return 85
      ;;
    unusable)
      echo "t.sh: falsify: the suite did not really run before any edit — check the build command and the log" >&2
      return 85
      ;;
    timedout)
      echo "t.sh: falsify: the suite did not finish within the --timeout before any edit was made" >&2
      return 85
      ;;
    *) fatal "falsify: the baseline run ended without a verdict — the harness could not run the suite (see $logdir)" ;;
  esac

  local name file find replace why expect expect_kind content mutated occurrences pristine line
  local -a caught=() survived=() stale=() unusable=() timedout=() expected=() ran=()
  for i in "${selected[@]}"; do
    name="${DEF_NAME[$i]}"
    file="${DEF_FILE[$i]}"
    find="${DEF_FIND[$i]}"
    replace="${DEF_REPLACE[$i]}"
    why="${DEF_WHY[$i]}"
    expect="${DEF_EXPECT[$i]}"
    expect_kind="${DEF_EXPECT_KIND[$i]}"
    ran+=("$name")

    original_of content "$file"
    occurrences=$(count_occurrences "$content" "$find")
    if ((occurrences != 1)); then
      # Not guessed at: a list that no longer describes the code has to say so, or it
      # quietly stops testing the thing it was written for
      printf 'stale     %s: its find text matches %s times in %s, not once\n' "$name" "$occurrences" "$file"
      annotate warning "$file" "" "stale $name: its find text matches $occurrences times, not once"
      stale+=("$name")
      record stale "$name" "$file" "" "$why"
      continue
    fi
    # The line the find text starts on, because a survivor is only actionable next to the
    # code it names: how many newlines come before it, plus one
    line=$(($(printf '%s' "${content%%"$find"*}" | wc -l) + 1))

    # Cut around the one occurrence rather than ${content//"$find"/"$replace"}: bash 3.2
    # keeps the quotes around the replacement as literal text, so every mutant on macOS
    # was `"if false"` and unusable, and unquoted, a `&` in the replacement is the match
    # itself from bash 5.2 on. Prefix and suffix have neither problem.
    mutated="${content%%"$find"*}$replace${content#*"$find"}"
    # Checked, because a write that fails leaves the pristine code in place, the suite
    # then passes against it, and that would be reported as a survivor: a read-only file
    # once made a guard the suite does cover read as one nobody checks
    printf '%s\n' "$name" >"$FLIGHT"
    printf '%s' "$mutated" >"$file" || fatal "falsify: cannot write $file — the tree is untouched, and nothing was measured"
    suite_verdict "$out/logs/$(log_name "$name").log" "$@"
    printf '%s' "$content" >"$file"
    rm -f "$FLIGHT"

    # A declared exception: surviving is the expected outcome and no finding; being caught
    # means the declaration has outlived its truth, which is the list's fault, like stale
    if [[ "$expect_kind" == survived ]]; then
      case "$VERDICT" in
        survived) VERDICT=expected ;;
        caught) VERDICT=disproved ;;
      esac
    elif [[ "$expect_kind" == caught && "$VERDICT" == caught ]]; then
      # "The suite went red" and "the suite noticed this" are different claims, and only
      # the second is worth anything: a run where something else is failing credits every
      # defect to a suite that never saw them. An entry that names what should do the
      # catching is held to it, and a name that no longer appears is the list drifting.
      grep -qF -- "$expect" "$out/logs/$(log_name "$name").log" || VERDICT=misattributed
    fi

    case "$VERDICT" in
      caught)
        printf 'caught    %s\n' "$name"
        caught+=("$name")
        ;;
      expected)
        printf 'expected  %s: %s\n' "$name" "$expect"
        expected+=("$name")
        ;;
      disproved)
        printf 'stale     %s: declared as one nothing can catch, and the suite caught it — drop the expectation\n' "$name"
        annotate warning "$file" "$line" "stale $name: declared as one nothing can catch, and the suite caught it"
        stale+=("$name")
        VERDICT=stale
        ;;
      misattributed)
        # Not caught: the suite went red without the named guard being anywhere in its
        # output, so this run says nothing about the guard the entry is written for. The
        # list's fault either way — a renamed test, or a claim that was never true
        printf 'stale     %s: the suite went red, but nothing in its output mentions %s — this run credits it to a guard that did not do the catching\n' "$name" "$expect"
        annotate warning "$file" "$line" "stale $name: red, but not by $expect"
        stale+=("$name")
        VERDICT=stale
        ;;
      survived)
        printf 'SURVIVED  %s: %s\n' "$name" "$why"
        # Where, and what the edit was — the first line of each, which is the whole edit
        # for the shapes worth writing
        printf '          %s:%s  - %s\n' "$file" "$line" "${find%%$'\n'*}"
        printf '          %s:%s  + %s\n' "$file" "$line" "${replace%%$'\n'*}"
        annotate error "$file" "$line" "SURVIVED $name: $why"
        survived+=("$name")
        ;;
      unusable)
        # The compiler noticed the syntax; the tests said nothing. Calling this "caught"
        # is how a suite gets credit for coverage it does not have — and calling it
        # SURVIVED, as this once did, blamed the suite for an edit that never reached it.
        printf 'unusable  %s: the edit stopped it building, so the tests were never asked\n' "$name"
        annotate warning "$file" "$line" "unusable $name: the edit stopped it building, so the tests were never asked"
        unusable+=("$name")
        ;;
      timedout)
        # Neither caught — that would credit the suite for a hang — nor SURVIVED, which
        # would blame it for an edit that never let it answer. Like stale, a fault of the
        # entry: the edit most likely made a loop that does not end.
        printf 'TIMEDOUT  %s: the suite did not finish within %ss, so it never gave a verdict\n' "$name" "$deadline"
        annotate warning "$file" "$line" "TIMEDOUT $name: the suite did not finish within ${deadline}s"
        timedout+=("$name")
        ;;
      # No verdict is a suite run that ended without one — killed, most likely. Silently
      # matching nothing here is how a defect once vanished from the report.
      *) fatal "falsify: no verdict for $name — the suite run ended without one" ;;
    esac
    record "$VERDICT" "$name" "$file" "$line" "$why"
  done

  restore_all
  trap - EXIT INT TERM
  for f in "${files[@]}"; do
    slurp __slurped "$f" || fatal "falsify: cannot re-read $f to confirm it was restored"
    original_of pristine "$f"
    [[ "$__slurped" == "$pristine" ]] ||
      fatal "falsify: $f was not restored to what it was — restore it from git before doing anything else"
  done
  cleanup_worktree

  # Three kinds of not-caught, three different problems, three exit codes. A survivor is
  # the suite's problem. A stale or unusable entry is the list's: it no longer describes
  # the code, or it breaks the build rather than the behaviour, and a report that folded
  # those into the survivors blamed the tests for a list nobody had maintained.
  echo
  printf '%d caught, %d SURVIVED, %d expected, %d stale, %d unusable, %d timed out, of %d defect(s) — %s/\n' \
    "${#caught[@]}" "${#survived[@]}" "${#expected[@]}" "${#stale[@]}" "${#unusable[@]}" "${#timedout[@]}" \
    "${#ran[@]}" "$out"
  if ((${#survived[@]} > 0)); then
    printf 'the suite did not notice %d of them — read the SURVIVED lines: each names what nobody checks\n' "${#survived[@]}" >&2
    return 83
  fi
  if ((${#timedout[@]} > 0)); then
    printf '%d edit(s) never let the suite finish — an infinite loop, most likely; neuter the guard another way\n' "${#timedout[@]}" >&2
    return 84
  fi
  if ((${#stale[@]} > 0)); then
    printf 'the defect list has drifted from the code: %d entry(ies) no longer describe it — see the stale lines\n' "${#stale[@]}" >&2
    return 87
  fi
  if ((${#unusable[@]} > 0)); then
    printf '%d edit(s) only stopped the build — neuter the guard instead, so the tests are asked\n' "${#unusable[@]}" >&2
    return 88
  fi
  printf 'all %d defect(s) were caught by the suite.\n' "${#ran[@]}"
}

# A commit that adds a test and the code it pins is proven by taking the code back: with
# the test kept and the fix gone, the suite has to go red. That is the revert-to-verify
# ritual — write, run green, revert the fix, run red, put it back — done by the harness
# rather than by hand, and it is what "a test and its fix are one commit" costs: the
# commit has to demonstrate itself. The files of the commit are split by the same rule
# falsify uses for a defect's file, test files stay, the rest is the fix.
cmd_prove() {
  local build="" deadline="" ref="HEAD" any_file="" worktree="" logdir="" seen_ref=""
  local -a pass=()
  while (($#)); do
    case "$1" in
      -b)
        build="${2:?-b needs a command}"
        shift 2
        ;;
      --timeout)
        deadline="${2:?--timeout needs a number of seconds}"
        [[ "$deadline" =~ ^[0-9]+$ ]] || die "prove: --timeout needs a number of seconds, got '$deadline'"
        shift 2
        ;;
      --any-file)
        any_file=1
        shift
        ;;
      --worktree)
        worktree=1
        shift
        ;;
      -l)
        logdir="${2:?-l needs a directory}"
        pass+=("$1" "$2")
        shift 2
        ;;
      -m | -p | -t)
        pass+=("$1" "${2:?$1 needs a value}")
        shift 2
        ;;
      --) break ;;
      -*) die "prove: unexpected argument '$1' — the command goes after --" ;;
      *)
        [[ -z "$seen_ref" ]] || die "prove: only one commit may be given"
        ref="$1"
        seen_ref=1
        shift
        ;;
    esac
  done
  [[ "${1:-}" == "--" ]] || die "prove: the suite command must follow -- (t.sh prove HEAD -- pytest -q)"

  git rev-parse --git-dir >/dev/null 2>&1 || die "prove: not inside a git repository"
  local commit parent
  commit=$(git rev-parse --verify --quiet "$ref^{commit}") || die "prove: '$ref' is not a commit in this repository"
  parent=$(git rev-parse --verify --quiet "$commit^") || die "prove: $ref has no parent to take its fix back to"
  if ! git diff --quiet || ! git diff --cached --quiet; then
    die "prove: the working tree has uncommitted changes — commit or stash them first, so an interrupted restore cannot be mistaken for your own edits"
  fi

  # What the commit changed, split the way falsify splits a defect's file: test files
  # stay, everything else is the fix. A commit that changed no source has nothing to
  # take away; one that changed no test is provable only by tests written before it,
  # which is worth saying.
  local f
  local -a src=() tests=()
  while IFS= read -r f; do
    [[ -n "$f" ]] || continue
    if [[ -z "$any_file" ]] && looks_like_test_file "$f"; then
      tests+=("$f")
    else
      src+=("$f")
    fi
  done < <(git diff --name-only "$parent" "$commit" --)
  ((${#src[@]} > 0)) ||
    die "prove: $ref changes no source file, only ${#tests[@]} test file(s) — there is no fix to take away (--any-file counts every file)"
  ((${#tests[@]} > 0)) ||
    echo "t.sh: prove: $ref changes no test file — whatever notices its fix going away was written before it" >&2

  POLICY_LOGDIR=""
  load_config
  [[ -n "$logdir" ]] || logdir="${T_LOGDIR:-${POLICY_LOGDIR:-.test-logs}}"
  mkdir -p "$logdir" || fatal "prove: cannot create $logdir"
  logdir=$(cd -- "$logdir" && pwd)

  # In place when the commit is what is checked out and nobody asked otherwise; in a
  # worktree at the commit when it is not, or on request — the same trade as falsify's
  local root wt=""
  root=$(pwd)
  if [[ -n "$worktree" || "$commit" != "$(git rev-parse HEAD)" ]]; then
    wt=$(mktemp -d "${TMPDIR:-/tmp}/t.sh.XXXXXX")/wt || fatal "prove: cannot create a directory for the worktree"
    git worktree add --detach "$wt" "$commit" >/dev/null 2>&1 || fatal "prove: git could not add a worktree at $wt"
    cd -- "$wt" || fatal "prove: cannot enter the worktree at $wt"
  fi
  # shellcheck disable=SC2317  # reached through the traps
  cleanup_worktree() {
    [[ -n "$wt" ]] || return 0
    cd -- "$root" || :
    git -C "$root" worktree remove --force "$wt" >/dev/null 2>&1 || :
    rm -rf "$(dirname -- "$wt")"
    wt=""
  }

  # The fix as it is, held in memory for the restore, and as it was before the commit,
  # for the taking away. A file the commit added has no "before" and is removed; a file
  # the commit deleted has no "now" and comes back.
  local i __c
  local -a originals=() present=() befores=() had_before=()
  for f in "${src[@]}"; do
    if [[ -e "$f" ]]; then
      slurp __c "$f" || fatal "prove: cannot read $f"
      originals+=("$__c")
      present+=(1)
    else
      originals+=("")
      present+=(0)
    fi
    if git cat-file -e "$parent:$f" 2>/dev/null; then
      __c=$(git show "$parent:$f" && printf x) || fatal "prove: cannot read $f as it was before $ref"
      befores+=("${__c%x}")
      had_before+=(1)
    else
      befores+=("")
      had_before+=(0)
    fi
  done

  # shellcheck disable=SC2317  # reached through the traps
  restore_all() {
    local i
    for i in "${!src[@]}"; do
      if ((present[i])); then
        printf '%s' "${originals[$i]}" >"${src[$i]}" 2>/dev/null || :
      else
        rm -f "${src[$i]}" 2>/dev/null || :
      fi
    done
  }
  trap 'restore_all; cleanup_worktree' EXIT
  trap 'end_mutant; restore_all; cleanup_worktree; trap - INT; kill -INT $$' INT
  trap 'end_mutant; restore_all; cleanup_worktree; trap - TERM; kill -TERM $$' TERM

  echo "== the suite is green with the fix in place"
  local started=$SECONDS
  suite_verdict "$logdir/prove-with-fix.log" "$@"
  if [[ -z "$deadline" ]]; then
    deadline=$(((SECONDS - started) * 5))
    ((deadline >= 20)) || deadline=20
  fi
  case "$VERDICT" in
    survived) ;;
    caught)
      echo "t.sh: prove: the suite is red at $ref with the fix in place — there is no green to take away" >&2
      return 85
      ;;
    unusable)
      echo "t.sh: prove: the suite did not really run at $ref — check the build command and $logdir/prove-with-fix.log" >&2
      return 85
      ;;
    timedout)
      echo "t.sh: prove: the suite did not finish within the --timeout with the fix in place" >&2
      return 85
      ;;
    *) fatal "prove: the run with the fix ended without a verdict — the harness could not run the suite (see $logdir)" ;;
  esac

  echo "== the fix is taken away, and the tests are kept"
  for i in "${!src[@]}"; do
    if ((had_before[i])); then
      printf '%s' "${befores[$i]}" >"${src[$i]}" || fatal "prove: cannot write ${src[$i]} — nothing was measured"
      printf '  - %s\n' "${src[$i]}"
    else
      rm -f "${src[$i]}" || fatal "prove: cannot remove ${src[$i]} — nothing was measured"
      printf '  - %s (added by the commit, removed)\n' "${src[$i]}"
    fi
  done
  suite_verdict "$logdir/prove-without-fix.log" "$@"
  restore_all
  trap - EXIT INT TERM
  git diff --quiet -- "${src[@]}" && [[ -z "$(git status --porcelain -- "${src[@]}")" ]] ||
    fatal "prove: the fix was not put back as it was — restore it from git before doing anything else"
  cleanup_worktree

  echo
  case "$VERDICT" in
    caught)
      printf 'proven: without its fix, the tests at %s go red\n' "$ref"
      return 0
      ;;
    survived)
      printf 'VACUOUS: the tests at %s pass without its fix — they pin nothing the commit did\n' "$ref" >&2
      if [[ -n "${GITHUB_ACTIONS:-}" ]]; then
        for f in "${src[@]}"; do
          printf '::error file=%s,title=prove::VACUOUS: the tests at %s pass without this change\n' "$f" "$ref"
        done
      fi
      return 83
      ;;
    unusable)
      printf 'unusable: without the fix nothing builds, so the tests were never asked — the build and the behaviour changed in one commit\n' >&2
      return 88
      ;;
    timedout)
      printf 'TIMEDOUT: without the fix the suite did not finish within %ss, so it never gave a verdict\n' "$deadline" >&2
      return 84
      ;;
    *) fatal "prove: the run without the fix ended without a verdict" ;;
  esac
}

# The reference, as text rather than as the file's header: a header has to read as a
# description of the file, a reference has to be complete, and one text cannot be both.
# The gate reads every flag out of every parser, every T_ variable out of this file and
# every exit code out of every return, and requires each to appear here.
help_general() {
  cat <<'EOF'
t.sh — the local test harness: one subcommand per question a test run raises

  t.sh run [FLAGS] -- CMD...           did it pass: CMD's own status, the whole log kept and read even at 0
  t.sh flaky N [FLAGS] -- CMD...       do N runs of the same code disagree
  t.sh focused [--any-file] [PATH...]  is a `.only` left in the source, so most of the suite is skipped
  t.sh quarantine [FILE]               is a test out of the gate past the date somebody promised to look
  t.sh pollute VICTIM -- CMD...        which earlier test makes this one fail, by halving the order
  t.sh bisect GOOD [FLAGS] -- CMD...   which commit between GOOD and HEAD broke it
  t.sh falsify [FLAGS] [FILTER] -- CMD...
                                       which guards the suite would not notice being broken
  t.sh prove [FLAGS] [REF] -- CMD...   does the commit's own test go red when its fix is taken away
  t.sh help [SUBCOMMAND | codes]       this, or one subcommand's flags, or the exit codes
  t.sh bisect-probe [FLAGS] -- CMD...  internal: the single-commit verdict git bisect run calls

The command is always explicit, after `--`: a harness that guesses what your suite is
runs the wrong thing on the day it matters. The flags every subcommand forwards to run:

  -l DIR       where the logs go (default .test-logs, or `logdir` in the policy)
  -m SET       add a marker set: a name from markers/ beside this script, or a file path;
               markers/default.txt always applies, and a set that resolves to nothing refuses
  -p PATTERN   add one marker for this run, matched case-insensitively as a fixed string
  -t N         how many lines of the log to show after a verdict that is not a pass (run: 40)

A repository keeps its policy in ./tests/t.conf, read from the current directory only and
never the command: `markers NAME`, `pattern TEXT`, `allow REGEX`, `logdir PATH`. An unknown
key, a key with no value or a set that does not exist stops the run and names the line.

The environment:

  T_ALLOW      an extended regex; matching log lines are excused before the scan. Overrides
               the policy's `allow`. A regex grep cannot compile is refused, never ignored
  T_LOGDIR     where the logs go; -l overrides it, the policy's `logdir` is under it
  T_LOGFILE    one log file for one run, instead of a name chosen under the log directory
  T_CONFIG     another policy file; T_CONFIG= (empty) reads none

Exit status: CMD's own, passed through unchanged, and the harness's own verdicts in a band
no test runner uses — `t.sh help codes`.
EOF
}

help_run() {
  cat <<'EOF'
t.sh run [-l DIR] [-m SET] [-p PATTERN] [-t N] -- CMD...

Runs CMD once. The status reported is CMD's own — read from PIPESTATUS, never from the
tee that keeps the log — and the log is read even when CMD exited 0, because that is not
always a success: `collected 0 items`, `no tests ran`, a traceback in a passing run. The
markers of such a run live in markers/*.txt as data; markers/default.txt always applies,
-m adds a set, -p adds one line.

The kind of verdict — pass, fail or lied — is written beside the log as LOG.verdict, one
word, so a wrapper can read it without guessing from the number.

Exit: CMD's own; 79 when CMD exited 0 but its log says otherwise; 64 for a usage error;
70 when the log could not be written.
EOF
}

help_focused() {
  cat <<'EOF'
t.sh focused [--any-file] [PATH...]

Finds the modifier that runs one test and skips the rest of its file — `test.only`,
`it.only`, `describe.only`, and jasmine's `fit` and `fdescribe` — left in the source. PATH
defaults to the working directory.

  --any-file          look inside node_modules, vendor, third_party, dist, build and
                      target as well, which are skipped by default because somebody
                      else's focused test is not this repository's problem

No runner reports one. jest and vitest print a skip count, which is what a suite skipping
a platform test prints too, and both exit 0 — so this cannot be a marker, and a log cannot
show it. Where the switch lives in a config instead, forbid it there: vitest's
`allowOnly: false`, playwright's `forbidOnly: true`, eslint's `jest/no-focused-tests`.

Exit: 0 when the source holds none; 80 when it holds any, with every line named.
EOF
}

help_pollute() {
  cat <<'EOF'
t.sh pollute [-l DIR] [-m SET] [-p PATTERN] [-t N] VICTIM -- CMD...

A test that passes alone and fails in the suite was polluted by something that ran before
it. This halves the order the way bisect halves commits, and names the test that does it.

The candidates arrive on stdin, one per line, in the order they run — which is what a
collector prints:

  pytest --collect-only -q | t.sh pollute tests/test_sync.py::test_retry -- pytest

CMD is given the selection as trailing arguments, so it must be a runner that takes a list
of tests that way: pytest, vitest, jest, phpunit. `go test -run` takes a regex instead, and
turning a list into one is a wrapper the adopter writes.

Each probe goes through `run`, so a suite that exits 0 while its log says otherwise counts
as a failure here too. Both ends are checked before any halving: a victim that fails by
itself is a broken test rather than a polluted one, and a victim that survives the whole
order has nothing to find.

Exit: 0 when it names the test, or when the order holds nothing to find; 82 when no single
test explains it and the smallest reproducing set is printed instead; 85 when the victim
fails on its own.
EOF
}

help_quarantine() {
  cat <<'EOF'
t.sh quarantine [--on YYYY-MM-DD] [FILE]

Reads the quarantine table — `tests/quarantine.md` by default, the shape is in
references/curation.md — and refuses a row nobody came back to. The columns are found by
their headings, so a column added in the middle shifts nothing.

  --on YYYY-MM-DD     judge the rows as of this date rather than today, which is how a
                      gate checks the check without waiting for a deadline to pass

Two rows are refused. One whose `expires` is before the date being judged: the deadline
was a promise to look again, and it passed. And one whose `expires` is not a date at all,
because a row that cannot expire never comes up for review — the same silence an unknown
config key gives.

Exit: 0 when every row is still within its date; 81 when any is not, with each named.
EOF
}

help_flaky() {
  cat <<'EOF'
t.sh flaky N [-l DIR] [-m SET] [-p PATTERN] [-t N] -- CMD...

Runs CMD N times, N at least 2, and reports how many runs disagreed with the first, with
the first divergent log named. Evidence that a test is unstable, never a way to tolerate
one: nothing here retries, and the logs of every run are kept under one directory.

Exit: the runs' common status when they agree; 86 when they disagreed; 64 for a usage
error; 70 when a run produced no verdict at all.
EOF
}

help_bisect() {
  cat <<'EOF'
t.sh bisect GOOD [-b BUILD] [-m SET] [-p PATTERN] [-t N] [--first-parent] [--no-checkout] -- CMD...

git bisect run between GOOD and HEAD, judging each commit with run. -b BUILD runs first
at every commit, and a commit that does not build is skipped rather than blamed; so is one
whose run exited 0 while its log says nothing ran, and one where the runner is not there.
A crash of the suite is bad; a Ctrl-C is passed through so git aborts. --first-parent and
--no-checkout are git's own. The working tree must be clean and no bisect may already be
in progress; the tree is put back afterwards, interrupt included, and git's session log
is kept beside the run's logs as bisect.log for `git bisect replay`.

Exit: 0 with the first bad commit named on its own line; 89 when only commits that could
not answer are left between good and bad; 64 for a usage error; 70 when git failed.

t.sh bisect-probe [-b BUILD] [-l DIR] [-m SET] [-p PATTERN] [-t N] -- CMD...

Internal: the single-commit verdict git bisect run calls, in git's vocabulary — 0 good,
1 bad, 125 cannot answer.
EOF
}

help_bisect_probe() { help_bisect; }

help_falsify() {
  cat <<'EOF'
t.sh falsify [-d FILE] [-b BUILD] [--timeout SECONDS] [--out DIR] [--since REF] [--shard I/N] [--worktree] [--any-file] [-l DIR] [-m SET] [-p PATTERN] [-t N] [FILTER] -- CMD...

Breaks one guard at a time, as written by hand in FILE (default tests/defects.sh, see
templates/defects.sh), and requires the suite to notice. FILTER runs only the defects
whose name contains it. Nothing is generated, and the defect list is sourced: it is code.

  -d FILE             the defect list
  -b BUILD            a command that must succeed before the suite is asked; an edit that
                      stops it is `unusable`, never credited to the suite
  --timeout SECONDS   the deadline for one run; default five times the unbroken suite's own
                      time or twenty seconds, whichever is more
  --out DIR           where the findings go (default falsify.out): one file of names per
                      verdict, a log per defect, results.json, written as the run goes
  --since REF         only the defects in files changed since REF — a filter for a pull
                      request, not a proof; an empty selection is said out loud, exit 0
  --shard I/N         run every Nth defect, starting at the Ith: N checkouts cover the list
                      between them, each with its own summary and exit code, and the run is
                      parallel without anything here being concurrent. One checkout per
                      shard — two shards sharing a tree would meet each other's mutants
  --worktree          edit a checkout of HEAD in a git worktree instead of the files in
                      front of you, so an editor, a watcher or a commit cannot meet a mutant
  --any-file          allow a defect in a test, vendored or generated file, which is
                      otherwise refused because it proves nothing about the suite

Verdicts: caught, SURVIVED with the file, the line and the edit, expected (declared with
`expect survived REASON`), stale, unusable, TIMEDOUT. On a GitHub runner each finding is
also an annotation on its file and line.

A defect list entry may end with `expect survived REASON`, for an edit nothing can
observe, or `expect caught FRAGMENT`, naming what should do the catching — a test name or
an assertion message. Caught while FRAGMENT is nowhere in that run's output is stale: the
suite went red without the named guard being involved, so the run says nothing about it.

Exit: 0 all caught; 83 a survivor; 84 a timeout; 85 the suite red or never really run
before any edit; 87 the list drifted, or a declared exception was disproved; 88 an edit
only stopped the build; 64 for a usage error; 70 when a file could not be written back.
EOF
}

help_prove() {
  cat <<'EOF'
t.sh prove [-b BUILD] [--timeout SECONDS] [--worktree] [--any-file] [-l DIR] [-m SET] [-p PATTERN] [-t N] [REF] -- CMD...

Takes the fix out of one commit (default HEAD), keeps its tests, and requires the suite
to go red: a commit that adds a test and the code it pins has to demonstrate itself. The
commit's files are split the way falsify splits a defect's file — test files stay, the
rest is the fix, --any-file counts everything as the fix. The suite must be green with
the fix in first. A commit other than HEAD is proven in a worktree at that commit;
--worktree does the same for HEAD. -b and --timeout as in falsify.

Exit: 0 proven; 83 VACUOUS, the tests pass without the fix; 84 the suite did not finish;
85 the suite red or never really run with the fix in; 88 without the fix nothing builds;
64 when the commit changes no source file, or for any other usage error.
EOF
}

help_codes() {
  cat <<'EOF'
Exit status: CMD's own, passed through unchanged, and the harness's own verdicts in a band
no test runner uses. 2, 3 and 4 were tried first and collide: GNU make exits 2 on any
error, pytest uses 2 to 5, cargo-nextest exits 4 for "no tests ran".

  64  a usage error — a flag, the config, a missing --, an allow regex grep rejects
  70  the harness itself failed — a log it cannot write, a file it cannot put back
  79  CMD exited 0 but its log says it did not do what a pass claims (run)
  83  a defect SURVIVED (falsify); the tests pass without the fix, VACUOUS (prove)
  84  a defect, or the fix taken away, never let the suite finish (falsify, prove)
  85  the state the search assumes does not hold: the suite red or never really run
      before any edit (falsify, prove), or HEAD passing (bisect)
  86  the runs disagreed with each other (flaky)
  87  the defect list has drifted, or a declared exception was disproved (falsify)
  88  a defect, or the fix taken away, only stopped the build (falsify, prove)
  80  a focus modifier was left in the source, so most of the suite will not run (focused)
  81  a quarantined test is past the date somebody promised to look at it (quarantine)
  82  the order was halved and no single test explains the pollution (pollute)
  89  only commits that could not answer are left between good and bad (bisect)
EOF
}

cmd_help() {
  local topic="${1:-}"
  case "$topic" in
    '') help_general ;;
    run | flaky | focused | quarantine | pollute | bisect | bisect-probe | falsify | prove) "help_${topic//-/_}" ;;
    codes | exit | status) help_codes ;;
    *) die "help: no such topic '$topic' — run, flaky, focused, quarantine, pollute, bisect, falsify, prove, codes" ;;
  esac
}

cmd="${1:-}"
(($# == 0)) || shift
case "$cmd" in
  run) cmd_run "$@" ;;
  flaky) cmd_flaky "$@" ;;
  focused) cmd_focused "$@" ;;
  quarantine) cmd_quarantine "$@" ;;
  pollute) cmd_pollute "$@" ;;
  bisect) cmd_bisect "$@" ;;
  bisect-probe) cmd_bisect_probe "$@" ;;
  falsify) cmd_falsify "$@" ;;
  prove) cmd_prove "$@" ;;
  -h | --help | help) cmd_help "$@" ;;
  '')
    help_general >&2
    exit 64
    ;;
  *)
    printf 't.sh: no such subcommand: %s\n\n' "$cmd" >&2
    help_general >&2
    exit 64
    ;;
esac
