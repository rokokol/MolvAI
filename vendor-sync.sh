#!/usr/bin/env bash
# Keeps verbatim copies of files that other repositories own, one line per copy in
# .github/vendor.lock: where the copy lives here, which repository and path it comes from,
# the commit it was taken at, and the git blob it must still be. A copy is never edited in
# place — the change goes to its source and comes back through the cascade, which runs
# `update` weekly and lands the result only on green. The procedure is described in
# references/bump-cascade.md of https://github.com/rokokol/ci-skill
#
#   vendor-sync.sh check
#   vendor-sync.sh update [--manual] [-u BASE]
#   vendor-sync.sh add [--manual] [-u BASE] LOCAL OWNER/REPO PATH
#
# check is offline and belongs in the repository's gate: every copy must still be the blob
# its line records, and a copy that is not was edited in place and is named. update refuses
# to run over such a copy, then takes each source's current HEAD and replaces every copy
# whose content moved, with its line; a copy whose content did not move keeps its line, so
# a source's unrelated commits never touch the lock. add takes a new copy and writes its
# line. A PATH ending in / is a directory kept whole: a file deleted or added in the source
# is deleted or added here; one holding a symlink is refused, since a copy cannot keep it.
#
# A copy that lands under .github/workflows/ is taken only with --manual: the token a
# workflow runs with cannot push a workflow file, so such copies are refreshed by a person.
# update takes the ordinary lines, and names each manual line whose source has moved
# rather than skipping it in silence; update --manual takes the manual lines and only
# those. BASE is where OWNER/REPO is fetched from (default https://github.com, file://DIR
# for a local source), and a source has to be reachable there without credentials. Run it
# from the repository root.
#
# Exit 1 with `vendor-sync: <what>` on a finding or a failed fetch, 2 on a usage error.
# Needs bash 3.2, git and POSIX tools only.
set -euo pipefail

# The whole header, however long it grows: up to the first line that is not a comment
usage() { sed -n '2,/^[^#]/p' "${BASH_SOURCE[0]}" | sed '$d; s/^# \{0,1\}//'; }

lock=.github/vendor.lock

fail() {
  echo "vendor-sync: $1" >&2
  exit 1
}

usage_error() {
  echo "vendor-sync: $1" >&2
  usage >&2
  exit 2
}

cmd="${1:-}"
case "$cmd" in
  -h | --help | help)
    usage
    exit 0
    ;;
  check | update | add) shift ;;
  "") usage_error "no command given" ;;
  *) usage_error "unknown command $cmd" ;;
esac

manual=0
base=https://github.com
while (($#)); do
  case "$1" in
    --manual)
      manual=1
      shift
      ;;
    -u)
      (($# >= 2)) || usage_error "-u needs a base URL"
      base="${2%/}"
      shift 2
      ;;
    -h | --help)
      usage
      exit 0
      ;;
    -*) usage_error "unknown option $1" ;;
    *) break ;;
  esac
done

# ---- what a copy is -------------------------------------------------------------------
# A file is its git blob. A directory is the blob of its listing — one "BLOB PATH" line per
# file, sorted by path — computed the same way on either side, so a file added, deleted or
# changed anywhere under it changes the digest. Names are taken raw on both sides: git's
# -z keeps a name it would otherwise quote, a Cyrillic one say, as the bytes find prints.

local_digest() { # local_digest LOCAL -> the digest of the copy here, empty when it is missing
  local p="$1" f
  case "$p" in
    */)
      [[ -d "$p" ]] || return 0
      (cd "$p" && find . -type f | sed 's|^\./||' | LC_ALL=C sort |
        while IFS= read -r f; do printf '%s %s\n' "$(git hash-object -- "$f")" "$f"; done) |
        git hash-object --stdin
      ;;
    *)
      if [[ -f "$p" ]]; then git hash-object -- "$p"; fi
      ;;
  esac
}

tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT
src="$tmp/src.git"
git init -q --bare "$src"
: >"$tmp/fetched"

# fetch OWNER/REPO -> sets fetched_sha to the commit of its HEAD, fetching each repository
# once per run. It sets a variable rather than printing, because a failure inside $(...)
# would exit only the subshell and the run would carry on without the source
fetched_sha=""
fetch() {
  local repo="$1" known
  known=$(awk -v r="$repo" '$1 == r { print $2; exit }' "$tmp/fetched")
  if [[ -n "$known" ]]; then
    fetched_sha="$known"
    return 0
  fi
  git -C "$src" fetch -q --depth=1 "$base/$repo" HEAD 2>"$tmp/err" ||
    fail "could not fetch $base/$repo: $(cat "$tmp/err")"
  fetched_sha=$(git -C "$src" rev-parse FETCH_HEAD)
  # A shallow fetch of the next repository moves FETCH_HEAD, so each commit is kept by a ref
  git -C "$src" update-ref "refs/vendor/$(printf '%s' "$repo" | tr '/' '_')" "$fetched_sha"
  printf '%s %s\n' "$repo" "$fetched_sha" >>"$tmp/fetched"
}

# no_symlink OWNER/REPO SHA PATH -> refuses a source directory that holds a symlink: find
# sees none of it here and git lists it there, so the copy would read as edited forever
no_symlink() {
  local repo="$1" sha="$2" path="$3" found
  case "$path" in */) ;; *) return 0 ;; esac
  found=$(git -C "$src" ls-tree -r "$sha:${path%/}" | awk '$1 == "120000" { sub(/^[^\t]*\t/, ""); print; exit }')
  [[ -z "$found" ]] || fail "$repo's $path holds a symlink, $found, which a copy cannot keep byte for byte"
}

source_digest() { # source_digest SHA PATH -> the digest of PATH at SHA, as local_digest has it
  local sha="$1" path="$2" entry
  case "$path" in
    */)
      git -C "$src" ls-tree -r -z "$sha:${path%/}" |
        while IFS= read -r -d '' entry; do
          printf '%s\t%s\n' "${entry#*	}" "$(printf '%s' "${entry%%	*}" | awk '{ print $3 }')"
        done |
        LC_ALL=C sort | awk -F'\t' '{ print $2 " " $1 }' | git hash-object --stdin
      ;;
    *) git -C "$src" rev-parse "$sha:$path" ;;
  esac
}

take() { # take LOCAL OWNER/REPO PATH SHA -> writes the copy here from PATH at SHA
  local dest="$1" repo="$2" path="$3" sha="$4" mode
  case "$path" in
    */)
      [[ "$(git -C "$src" cat-file -t "$sha:${path%/}" 2>/dev/null)" == tree ]] ||
        fail "$repo has no directory $path at ${sha:0:12}"
      rm -rf "${dest%/}.vendor-sync.new"
      mkdir -p "${dest%/}.vendor-sync.new"
      git -C "$src" archive "$sha:${path%/}" | tar -xf - -C "${dest%/}.vendor-sync.new"
      rm -rf "$dest"
      mv "${dest%/}.vendor-sync.new" "${dest%/}"
      ;;
    *)
      mode=$(git -C "$src" ls-tree "$sha" -- "$path" | cut -d' ' -f1)
      [[ -n "$mode" ]] || fail "$repo has no file $path at ${sha:0:12}"
      [[ "$mode" != 040000 ]] || fail "$repo's $path is a directory — write it as $path/, here and there"
      mkdir -p "$(dirname -- "$dest")"
      git -C "$src" cat-file blob "$sha:$path" >"$dest.vendor-sync.new"
      if [[ "$mode" == 100755 ]]; then chmod +x "$dest.vendor-sync.new"; else chmod -x "$dest.vendor-sync.new"; fi
      # Replaced, never rewritten: bash reads a script while running it, and the copy being
      # replaced may be this very script, which would carry on into the new text
      mv -f "$dest.vendor-sync.new" "$dest"
      ;;
  esac
}

# ---- the commands ---------------------------------------------------------------------

check() {
  local n=0 bad=0 l r p c b m got
  [[ -f "$lock" ]] || fail "there is no $lock — nothing is vendored here, so there is nothing to check"
  while read -r l r p c b m <&3; do
    [[ -z "$l" || "$l" == \#* ]] && continue
    n=$((n + 1))
    got=$(local_digest "$l")
    if [[ -z "$got" ]]; then
      echo "vendor-sync: $l is missing; its line says it comes from $r $p" >&2
      bad=1
    elif [[ "$got" != "$b" ]]; then
      echo "vendor-sync: $l was edited in place — it is no longer $p of $r at ${c:0:12}. Make the change there and let the cascade bring it back; restore this copy with git checkout" >&2
      bad=1
    fi
  done 3<"$lock"
  ((n > 0)) || fail "$lock lists no copy — a check over nothing is not a pass"
  ((bad == 0)) || return 1
  echo "vendor-sync: $n vendored cop$( ((n == 1)) && echo y || echo ies) match their lock"
}

update() {
  local n=0 moved=0 behind=0 l r p c b m blob is_manual
  check >/dev/null || fail "refusing to update over the copy named above"
  : >"$tmp/lock"
  while IFS= read -r line <&3; do
    read -r l r p c b m <<<"$line" || true
    if [[ -z "${l:-}" || "$l" == \#* ]]; then
      printf '%s\n' "$line" >>"$tmp/lock"
      continue
    fi
    is_manual=0
    [[ "${m:-}" != manual ]] || is_manual=1
    fetch "$r"
    no_symlink "$r" "$fetched_sha" "$p"
    blob=$(source_digest "$fetched_sha" "$p") || fail "$r has no $p at ${fetched_sha:0:12}"
    # A line this run does not take — a manual one without --manual, an ordinary one with it —
    # keeps its line. A manual one is still named when its source has moved: nothing else
    # would ever tell a person to refresh it
    if ((is_manual != manual)); then
      if ((is_manual == 1)) && [[ "$blob" != "$b" ]]; then
        behind=$((behind + 1))
        echo "vendor-sync: $l is behind $p of $r — refresh it with vendor-sync.sh update --manual" >&2
        [[ -z "${GITHUB_ACTIONS:-}" ]] || echo "::warning file=$l::$l is behind $p of $r; refresh it with vendor-sync.sh update --manual"
      fi
      printf '%s\n' "$line" >>"$tmp/lock"
      continue
    fi
    n=$((n + 1))
    if [[ "$blob" == "$b" ]]; then
      printf '%s\n' "$line" >>"$tmp/lock"
      continue
    fi
    take "$l" "$r" "$p" "$fetched_sha"
    moved=$((moved + 1))
    printf '%s %s %s %s %s%s\n' "$l" "$r" "$p" "$fetched_sha" "$blob" "${m:+ $m}" >>"$tmp/lock"
  done 3<"$lock"
  cmp -s "$tmp/lock" "$lock" || cp "$tmp/lock" "$lock"
  echo "vendor-sync: $n cop$( ((n == 1)) && echo y || echo ies) checked against their source, $moved taken anew$( ((behind == 0)) || echo ", $behind manual behind")"
}

header() {
  cat <<'EOF'
# Files vendored from other repositories and kept byte-equal to their source by
# vendor-sync.sh, one per line: LOCAL OWNER/REPO PATH COMMIT BLOB [manual]. Never edit a
# copy in place: change it at its source and let the weekly cascade bring it back. How it
# works: references/bump-cascade.md in https://github.com/rokokol/ci-skill
EOF
}

add() {
  (($# == 3)) || usage_error "add needs LOCAL OWNER/REPO PATH"
  local l="${1#./}" r="$2" p="$3" blob ldir=0 pdir=0 workflow=0
  [[ -n "$l" ]] || usage_error "LOCAL is empty"
  case "$l$r$p" in *[[:space:]]*) usage_error "a path with whitespace cannot be written to the lock" ;; esac
  [[ "$r" == */* ]] || usage_error "$r is not OWNER/REPO"
  [[ "$l" != */ ]] || ldir=1
  [[ "$p" != */ ]] || pdir=1
  ((ldir == pdir)) || usage_error "LOCAL and PATH both end in / for a directory, or neither does"
  # Where the copy lands decides it, however the path is spelled: a file under
  # .github/workflows/, or a directory that holds that directory
  case "$l" in .github/workflows/*) workflow=1 ;; esac
  if ((ldir == 1)) && [[ .github/workflows/ == "$l"* ]]; then workflow=1; fi
  ((workflow == 0 || manual == 1)) ||
    fail "$l lands a workflow file: the token the cascade runs with cannot push one, so take it with --manual and refresh it by hand"
  if [[ -f "$lock" ]] && awk -v l="$l" '$1 == l { found = 1 } END { exit !found }' "$lock"; then
    fail "$l is already vendored — update refreshes it"
  fi
  fetch "$r"
  no_symlink "$r" "$fetched_sha" "$p"
  blob=$(source_digest "$fetched_sha" "$p") || fail "$r has no $p at ${fetched_sha:0:12}"
  take "$l" "$r" "$p" "$fetched_sha"
  if [[ ! -f "$lock" ]]; then
    mkdir -p "$(dirname -- "$lock")"
    header >"$lock"
  fi
  printf '%s %s %s %s %s%s\n' "$l" "$r" "$p" "$fetched_sha" "$blob" "$( ((manual == 1)) && echo ' manual')" >>"$lock"
  echo "vendor-sync: took $l from $p of $r at ${fetched_sha:0:12}"
}

"$cmd" "$@"
