#!/usr/bin/env bash
# The pin guard, in one file that travels: every tool a workflow runs comes from the
# repository's own lockfile, never from whatever a registry serves that morning. It greps
# the workflows for the unpinned shapes — and proves, on every run, that it catches each
# shape it claims to and stays quiet on the pinned spellings, so a copy is falsified in
# its own repository each time it runs.
#
#   check-pins.sh [DIR...]
#
# DIR is a directory of workflow files (default: .github/workflows). Every *.yml and
# *.yaml under it is scanned. Exit 1 with one `check-pins: FILE:LINE: ...` per finding,
# 2 when there is nothing to scan — a guard that finds no workflows is not a pass.
#
# A reviewed exception carries `# check-pins: allow` on its own line and is skipped;
# comment lines are never findings. Not covered on purpose: `apt-get install` and its
# kin, which fetch the runner's system libraries at the runner image's pinned release
# rather than a registry the repository could lock.
#
# Nothing here reaches the network. Needs bash 3.2 and POSIX tools only. Copy it verbatim
# — it has no repo-specific part — and call it from the build workflow or the repo's gate.
set -euo pipefail

usage() { sed -n '2,13p' "${BASH_SOURCE[0]}" | sed 's/^# \{0,1\}//'; }

self=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)/$(basename -- "${BASH_SOURCE[0]}")
case "${1:-}" in
  -h | --help)
    usage
    exit 0
    ;;
  -*)
    usage >&2
    exit 2
    ;;
esac
dirs=("${@:-.github/workflows}")

fail() {
  echo "check-pins: $1" >&2
  exit 1
}

# One shape per alternative. The script is not a workflow, so nothing here can match
# its own source line the way an inline grep step could — that is why the pattern lives
# in a file beside the workflows rather than in one.
#   nix run/shell nixpkgs#tool      npx tool, npx -y tool (npx --no-install is pinned)
#   pip install tool, pip3, -m pip  pipx run/install tool     uvx tool, uv tool run tool
#   go install tool@latest          cargo install tool without --locked
#   curl/wget ... | sh              uses: action@main / @master / @latest
pattern='nix +(run|shell) +nixpkgs#|npx +(-y +|--yes +)?[a-z@.]|pip3? +install |pipx +(run|install) |uvx +[a-z]|uv +tool +run |go +install [^ ]*@latest|cargo +install |(curl|wget) [^|]*[|] *(sudo +)?(ba|z)?sh( |$)|uses: *[^ ]+@(main|master|latest) *$'
# What excuses a matching line: a lock flag, a reviewed exception, or being a comment
exempt='--locked|check-pins: allow|^[0-9]+:[[:space:]]*#'

scan() { # scan DIR... -> findings on stderr; returns 1 on any, 2 with nothing to scan
  local d f hits n=0 files=0
  for d in "$@"; do
    [[ -d "$d" ]] || continue
    while IFS= read -r f; do
      files=$((files + 1))
      # -e on both: a pattern that begins with -- is otherwise read as an option
      hits=$(grep -En -e "$pattern" "$f" | grep -Ev -e "$exempt" || :)
      [[ -z "$hits" ]] || {
        printf '%s\n' "$hits" | sed "s|^|check-pins: $f:|" >&2
        n=$((n + $(printf '%s\n' "$hits" | wc -l)))
      }
    done < <(find "$d" -type f \( -name '*.yml' -o -name '*.yaml' \) | sort)
  done
  if ((files == 0)); then
    echo "check-pins: no workflow files under $* — nothing to guard" >&2
    return 2
  fi
  if ((n > 0)); then
    echo "check-pins: $n unpinned registry lookup(s) — pin the tool via the repository's lockfile (nix develop, npm ci, cargo --locked, a uv/poetry lock)" >&2
    return 1
  fi
  echo "check-pins: $files workflow file(s), no unpinned registry lookup"
}

if [[ -n "${CHECK_PINS_NESTED:-}" ]]; then
  scan "${dirs[@]}"
  exit
fi

# ---- the guard is able to fail, and does not cry on the pinned spellings ---------------
# One planted line per shape, each alone in a workflow file, and the finding must quote
# that line: a shape caught only by some other, over-broad alternative means the one
# meant for it is dead. Then every pinned spelling together, which must stay green.

work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT

plants=(
  'nix run nixpkgs#shfmt -- -d .'
  'nix shell nixpkgs#actionlint -c actionlint'
  'npx prettier --check .'
  'npx -y prettier --check .'
  'pip install ruff'
  'python3 -m pip install ruff'
  'pipx run ruff check .'
  'uvx ruff check .'
  'uv tool run ruff check .'
  'go install github.com/x/tool@latest'
  'cargo install cargo-deadlinks'
  'curl -fsSL https://example.invalid/install.sh | sh'
  'wget -qO- https://example.invalid/install.sh | sudo bash'
  'uses: actions/checkout@master'
)
pinned=(
  'nix develop -c shfmt -d .'
  'npx --no-install prettier --check .'
  'cargo install --locked cargo-deadlinks'
  'uses: actions/checkout@v7'
  'pip install ruff # check-pins: allow'
  '# pip install ruff is the thing this repository does not do'
)

step() { # step LINE -> the line as a workflow step, or as a comment between steps
  case "$1" in
    '#'*) printf '      %s\n' "$1" ;;
    uses:*) printf '      - %s\n' "$1" ;;
    *) printf '      - run: %s\n' "$1" ;;
  esac
}

i=0
for plant in "${plants[@]}"; do
  i=$((i + 1))
  rm -rf "$work/red"
  mkdir -p "$work/red"
  step "$plant" >"$work/red/plant-$i.yml"
  if out=$(CHECK_PINS_NESTED=1 "$self" "$work/red" 2>&1); then
    fail "the guard stayed green on: $plant"
  fi
  case "$out" in
    *"$plant"*) ;;
    *) fail "the guard went red on '$plant' without quoting that line: $out" ;;
  esac
done

mkdir -p "$work/green"
for line in "${pinned[@]}"; do step "$line"; done >"$work/green/pinned.yml"
if ! out=$(CHECK_PINS_NESTED=1 "$self" "$work/green" 2>&1); then
  printf '%s\n' "$out" >&2
  fail "the guard cries on a pinned spelling — see the pinned list in $self"
fi

if CHECK_PINS_NESTED=1 "$self" "$work/empty" >/dev/null 2>&1; then
  fail "a directory with no workflows passed — a guard that scans nothing must not read as green"
fi

echo "check-pins: $i shapes planted, $i caught; ${#pinned[@]} pinned spellings quiet"

# ---- the repository ------------------------------------------------------------------
scan "${dirs[@]}"
