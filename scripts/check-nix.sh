#!/usr/bin/env bash
# Other repositories take this file through the vendoring cascade (references/bump-cascade.md
# in https://github.com/rokokol/ci-skill). Nobody edits a copy in place. A change happens
# here, and it reaches them from here.
# Its own code needs bash 3.2 and POSIX tools only, so a macOS runner runs it unchanged.
# To read the Nix it gets, it also needs nixfmt, statix, deadnix, jq, nix-instantiate, nix and git.
# The self-test reads that line as the list of tools the preflight must refuse to run without.
# It reads that Nix four ways: as text after nixfmt, as the tree nix-instantiate re-prints,
# as the JSON that flake.lock already is, and as the values nix evaluates the outputs to.
# There is no degraded mode. A Nix repository holds nix by definition, and a missing linter
# stops the run rather than shortens it
set -euo pipefail

usage() {
  cat <<'EOF'
check-nix.sh — holds Nix sources to what the formatter cannot check

nixfmt owns layout. statix owns the language's antipatterns. deadnix owns dead code.
This checker calls those three and adds only what none of them can see: the shape of a
module header, the scope a `with` opens, whether a derivation carries a meta, and whether
the lock names a path that exists on one machine alone.
Every run proves each check able to fail. It plants one defect into a canonical file and
requires the rejection. A copy therefore falsifies itself wherever it runs.
Nothing here is repository-specific, and this belongs in a repository's own gate

  check-nix.sh [-C DIR] [-N NAMESPACE]... [--static] [PATH...]
  check-nix.sh --template [module|package|flake]
  check-nix.sh --list-rules

  -C DIR       the repository root. It defaults to the git toplevel of the working
               directory, else to that directory. flake.nix and flake.lock live here
  -N NAMESPACE a prefix this repository declares its own options under, such as rokokol
               or programs.screen-shader. Repeatable. A repository that declares options
               and names none of these is told so, rather than passed
  PATH...      check these files instead of every .nix file under DIR. The flake and lock
               rules then run only when flake.nix is among them
  --static     run only what reads files, and nothing that evaluates the flake. It exists
               for the nix flake check sandbox, which has no network and no store. The
               summary then says the evaluated half did not run. Nothing chooses this
               flag on its own: a missing tool is a refusal, not a quiet pass
  --template   print a canonical module, package or flake and exit
  --list-rules print every rule with its tier and mechanism, and exit

Environment: CHECK_NIX_NESTED=1 runs the checks and skips the self-test. A gate that calls
this more than once uses it, so one copy is not proven twice. CHECK_NIX_GOLDEN, set to
anything, makes the pinned shape of the parser's output deliberately wrong. The self-test
uses it to prove the refusal that shape is pinned by
Nothing here reaches the network
Exit 0 when clean. Exit 1 with one `check-nix: <what>` line per finding. Exit 2 on a usage
error, an unreadable path, a missing tool, a moved tool output, or nothing to check
EOF
}

die() { # the request itself is wrong
  printf 'check-nix: %s\n' "$1" >&2
  exit 2
}

# ---- the rules, listed once ------------------------------------------------------------
# machine: --list-rules prints this, and the self-test walks it to be sure every rule has
# a planted defect. A rule that exists in neither place is a rule nobody proved
rules() {
  cat <<'EOF'
nixfmt-formatted	delegated	a file nixfmt would rewrite
statix	delegated	an antipattern statix names, minus the one lint that groups options by prefix
deadnix	delegated	an unused binding, lambda argument or inherit
file-kebab-case	names	a component of a tracked .nix file's path that is not kebab-case
with-at-file-level	text	a with whose scope is the whole file body
module-arg-order	header	formals not in the order: the standard ones, the rest alphabetically, then ...
arg-line-shape	header	up to two named formals split across lines, or three and more on one
file-comment-placement	header	a module's file comment above its header, or held off its body by a blank line
with-over-let	tree	a with whose body binds names of its own
lookup-path	tree	<nixpkgs> or NIX_PATH, which read the machine rather than the lock
pkgs-lib-with-lib-arg	tree	pkgs.lib where lib is already an argument
self-src-unwrapped	tree	a derivation src taken straight from inputs.self
derivation-meta	tree	a derivation with no meta
default-nix-imports-only	tree	a default.nix that binds anything but imports
lock-no-local-input	lock	an input locked to an absolute path on one machine
flake-formatter	eval	a flake with no formatter, or one that is not nixfmt-tree
meta-description-grammar	eval	a package whose meta.description breaks the grammar nixpkgs asks for
meta-license	eval	a package with no meta.license
list-with-pkgs	tree	a list whose every element is pkgs.something, written without with pkgs
options-namespaced	tree	an option declared under a prefix this repository has not claimed
EOF
}

# ---- templates ---------------------------------------------------------------------------
template_module() {
  cat <<'EOF'
{
  config,
  lib,
  pkgs,
  ...
}:

# What this module is for, and why it has this arrangement. The comment sits after the
# argument header and abuts the body. The reader meets it before the first attribute
let
  cfg = config.example.thing;
in
{
  options.example.thing = {
    enable = lib.mkEnableOption "the thing";

    port = lib.mkOption {
      type = lib.types.port;
      default = 8080;
      description = "Loopback port the thing binds";
    };
  };

  config = lib.mkIf cfg.enable {
    environment.systemPackages = with pkgs; [ jq ];

    services.thing = {
      enable = true;
      inherit (cfg) port;
    };
  };
}
EOF
}

template_package() {
  cat <<'EOF'
{
  lib,
  stdenvNoCC,
  fetchFromGitHub,
}:

# Why this package lives here, and does not come from nixpkgs. A derivation carries the
# reason for its own existence, and meta carries what a reader needs after that
stdenvNoCC.mkDerivation (finalAttrs: {
  pname = "example";
  version = "1.0.0";

  src = fetchFromGitHub {
    owner = "example";
    repo = "example";
    tag = "v${finalAttrs.version}";
    hash = "sha256-AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=";
  };

  installPhase = ''
    runHook preInstall
    install -Dm755 example -t $out/bin
    runHook postInstall
  '';

  meta = {
    description = "Example of the shape a package takes in this family";
    homepage = "https://github.com/example/example";
    license = lib.licenses.mit;
    mainProgram = "example";
    platforms = lib.platforms.all;
  };
})
EOF
}

template_flake() {
  cat <<'EOF'
{
  description = "One line saying what this flake is, in the present tense";

  inputs.nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";

  outputs =
    { nixpkgs, ... }:
    let
      inherit (nixpkgs) lib;
      systems = [
        "x86_64-linux"
        "aarch64-linux"
      ];
      forAllSystems = f: lib.genAttrs systems (system: f nixpkgs.legacyPackages.${system});
    in
    {
      packages = forAllSystems (pkgs: {
        default = pkgs.callPackage ./nix/package.nix { };
      });

      devShells = forAllSystems (pkgs: {
        default = pkgs.mkShell { packages = with pkgs; [ jq ]; };
      });

      formatter = forAllSystems (pkgs: pkgs.nixfmt-tree);
    };
}
EOF
}

# ---- arguments ----------------------------------------------------------------------------
root=""
namespaces=""
paths=()
static=0
while (($#)); do
  case "$1" in
    -C)
      (($# >= 2)) || die "-C needs a directory"
      root="$2"
      shift 2
      ;;
    -N)
      (($# >= 2)) || die "-N needs a namespace"
      namespaces="${namespaces:+$namespaces }$2"
      shift 2
      ;;
    --static)
      static=1
      shift
      ;;
    --list-rules)
      rules
      exit 0
      ;;
    --template)
      case "${2:-module}" in
        module) template_module ;;
        package) template_package ;;
        flake) template_flake ;;
        *) die "no such template: $2 — module, package or flake" ;;
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
      paths+=("$1")
      shift
      ;;
  esac
done

if [[ -z "$root" ]]; then
  root=$(git rev-parse --show-toplevel 2>/dev/null || pwd)
fi
[[ -d "$root" ]] || die "cannot read $root as a directory"
for p in ${paths[@]+"${paths[@]}"}; do
  [[ -r "$p" ]] || die "cannot read $p"
done

findings=0
finding() {
  printf 'check-nix: %s\n' "$1" >&2
  findings=$((findings + 1))
}

# With a template, because the BSD mktemp on macOS wants one
work=$(mktemp -d "${TMPDIR:-/tmp}/check-nix.XXXXXX")
trap 'rm -rf "$work"' EXIT

# ---- the tools ----------------------------------------------------------------------------
# This runs before a single rule reads a line. It refuses a machine without a tool, and it
# never checks that machine with less.
# A shorter run on a missing tool lost the argument: an extractor that finds nothing must
# never read as "nothing drifted"
tool_preflight() {
  local missing=""
  local t
  for t in nixfmt statix deadnix jq nix-instantiate nix git; do
    command -v "$t" >/dev/null 2>&1 || missing="${missing:+$missing, }$t"
  done
  [[ -z "$missing" ]] ||
    die "needs $missing — nix develop -c is where the pinned ones live"
}
tool_preflight

# No document describes the canonical form nix-instantiate --parse prints, and every rule
# that reads the tree greps it. So one expression holds every construct those rules look
# for. This parses it and compares the line against the one it gave when somebody wrote them.
# A release that moves the printer stops the run and shows what it printed instead.
# It never passes a check that quietly looks at nothing
tree_probe() {
  cat <<'EOF'
{ inputs, lib, pkgs, ... }:
{
  a = with pkgs; [ jq ];
  b = with lib; let c = 1; in c;
  d = pkgs.lib.mkForce 1;
  e = import <nixpkgs> { };
  f = "a {brace} b";
  g = pkgs.stdenvNoCC.mkDerivation { src = "${inputs.self}/assets"; meta = { }; };
}
EOF
}

tree_golden() {
  # The one hook a test needs: a golden that deliberately no longer describes the printer.
  # The refusal below must be shown able to fire on a machine where the printer did not move
  [[ -z "${CHECK_NIX_GOLDEN:-}" ]] || {
    printf 'a line the printer would never print\n'
    return 0
  }
  cat <<'EOF'
({ inputs, lib, pkgs, ... }: { a = (with pkgs; [ (jq) ]); b = (with lib; (let c = 1; in c)); d = ((pkgs).lib.mkForce 1); e = (import (__findFile __nixPath "nixpkgs") { }); f = "a {brace} b"; g = ((pkgs).stdenvNoCC.mkDerivation { meta = { }; src = ((inputs).self + "/assets"); }); })
EOF
}

tree_preflight() {
  tree_probe >"$work/probe.nix"
  nix-instantiate --parse "$work/probe.nix" >"$work/probe.out" 2>"$work/probe.err" ||
    die "nix-instantiate could not read the built-in probe: $(tr '\n' ' ' <"$work/probe.err")"
  tree_golden >"$work/probe.want"
  cmp -s "$work/probe.out" "$work/probe.want" || {
    printf 'check-nix: the tree printed by %s is not the one tree_golden describes — either the printer moved or the golden did\n' \
      "$(nix-instantiate --version 2>/dev/null || echo 'nix, version unknown')" >&2
    printf 'check-nix: the probe printed this instead, which is what tree_golden would become:\n' >&2
    sed 's/^/  /' "$work/probe.out" >&2
    exit 2
  }
}
tree_preflight

# This disables one statix lint for every repository at once. Each repository does not argue
# it again. repeated_keys wants `services.nginx.enable` and `services.postgresql.enable`
# folded into one nested `services`, because their paths share a first segment.
# In a module that segment is an address into the global option tree. The author did not
# choose it as a structure, so the lint groups by a string prefix and not by subject.
# The module system also merges definitions across files, which is what the nesting imitates
cat >"$work/statix.toml" <<'EOF'
disabled = ["repeated_keys"]
EOF

# ---- the files ----------------------------------------------------------------------------
# A git repository gets git ls-files, not find. git already knows what it ignores, and it
# does not descend into a result symlink that points at the store.
# --others with --exclude-standard makes a file visible that nobody staged yet. Without it a
# new module passes here and fails in CI, where git tracks the same file.
# That failure is the one a gate exists to move earlier.
# The rules and the file names read this one walk, so they cannot disagree on what a
# repository holds. The paths are relative to $root
git_nix_files() {
  git -C "$root" ls-files --cached --others --exclude-standard -- '*.nix'
}

nix_files() {
  if ((${#paths[@]})); then
    printf '%s\n' "${paths[@]}"
  elif git -C "$root" rev-parse --git-dir >/dev/null 2>&1; then
    git_nix_files | sed "s|^|$root/|"
  else
    find "$root" -name '*.nix' -type f -not -path '*/.git/*' -not -path '*/result/*'
  fi
}

files=$(nix_files)
[[ -n "$files" ]] || die "no .nix file under $root — nothing to check"
file_count=$(printf '%s\n' "$files" | grep -c .)

# ---- delegated: nixfmt ---------------------------------------------------------------------
# Not `nix fmt -- --ci`. The checker also runs over a directory with no flake at all, and a
# repository's formatter output wraps this same binary with treefmt
check_nixfmt() {
  local f
  printf '%s\n' "$files" | while IFS= read -r f; do
    [[ -n "$f" ]] || continue
    nixfmt --check "$f" >/dev/null 2>&1 || printf '%s\n' "$f"
  done
}

# ---- delegated: statix ----------------------------------------------------------------------
# One file per call: statix takes a single target, and a directory walk would follow a result
# symlink into the store. errfmt is FILE>LINE:COL:SEVERITY:CODE:message, one finding per line
check_statix() {
  local f
  printf '%s\n' "$files" | while IFS= read -r f; do
    [[ -n "$f" ]] || continue
    statix check -o errfmt -c "$work/statix.toml" "$f" 2>/dev/null || :
  done
}

# ---- delegated: deadnix ----------------------------------------------------------------------
# One JSON object per file with a results array; jq flattens it to the shape the other two
# report in. is_generated owns the exemption for a generated file, and this does not restate
# it. NixOS writes that file and a repository does not edit it, so its unused pkgs argument
# is nobody's finding
check_deadnix() {
  local f
  printf '%s\n' "$files" | while IFS= read -r f; do
    [[ -n "$f" ]] || continue
    is_generated "$f" && continue
    deadnix -o json "$f" 2>/dev/null |
      jq -r '.results[]? | "\(.line):\(.column): \(.message)"' |
      sed "s|^|$f:|"
  done
}

# ---- the excuses ---------------------------------------------------------------------------------
# check-nix.allow holds what a repository knows and this checker cannot: `ID PATH [TEXT]`
# per line. A path that ends in / stands for everything under it, as vendor-sync.sh already
# spells a directory.
# An entry that excuses nothing is itself a finding. An excuse must not outlive its reason.
# The standard states the same rule about a comment that carries a date instead of a cause
allow_file="$root/check-nix.allow"
: >"$work/allow"
: >"$work/allow.used"
if [[ -r "$allow_file" ]]; then
  lineno=0
  while IFS= read -r line || [[ -n "$line" ]]; do
    lineno=$((lineno + 1))
    case "$line" in '' | '#'*) continue ;; esac
    read -r entry_id entry_path _ <<<"$line"
    [[ -n "$entry_id" && -n "$entry_path" ]] ||
      die "$allow_file:$lineno: an entry is ID PATH [TEXT], and this one is: $line"
    printf '%s\t%s\n' "$entry_id" "$entry_path" >>"$work/allow"
  done <"$allow_file"
fi

rel_path() { # rel_path PATH -> the path as the repository spells it
  case "$1" in "$root"/*) printf '%s' "${1#"$root"/}" ;; *) printf '%s' "$1" ;; esac
}

excused() { # excused ID PATH — and the entry that did it is marked used
  local id="$1" path eid epath
  path=$(rel_path "$2")
  while IFS=$'\t' read -r eid epath; do
    [[ "$eid" == "$id" ]] || continue
    case "$epath" in
      */) case "$path" in "$epath"*) ;; *) continue ;; esac ;;
      *) [[ "$path" == "$epath" ]] || continue ;;
    esac
    printf '%s\t%s\n' "$eid" "$epath" >>"$work/allow.used"
    return 0
  done <"$work/allow"
  return 1
}

finding_unless_excused() { # finding_unless_excused ID PATH MESSAGE
  excused "$1" "$2" || finding "$3"
}

# ---- generated files -------------------------------------------------------------------------
# NixOS writes hardware-configuration.nix and a repository does not edit it. Its unused pkgs
# argument, its header and its comments are nobody's finding. This is the one exemption by
# default
is_generated() { # is_generated FILE
  case "$1" in */hardware-configuration.nix) return 0 ;; esac
  return 1
}

# ---- the tree ------------------------------------------------------------------------------------
# nix-instantiate --parse re-prints the parsed expression as canonical Nix on one line. It
# adds every parenthesis, removes every comment and normalises every layout choice.
# This is the checker's equivalent of `shfmt --to-json`, and it has one measured limit.
# It sorts attribute keys and lambda formals alphabetically. So it answers what shape a file
# has, and never what order anybody wrote it in. Order is a question for the text
tree_of() { # tree_of FILE -> the canonical line, cached
  local f="$1" cached
  cached="$work/tree.$(printf '%s' "$f" | tr -c 'A-Za-z0-9' '.')"
  [[ -s "$cached" ]] || nix-instantiate --parse "$f" >"$cached" 2>/dev/null || :
  cat "$cached"
}

is_lambda() { # is_lambda FILE — does the file evaluate to a function taking formals
  case "$(tree_of "$1" | head -c 2)" in '({') return 0 ;; esac
  return 1
}

# This walks the canonical line with { } [ ] ( ) depth, and it steps over "…" strings.
# A brace inside a string is therefore not a nesting level.
# It answers two questions. Which keys does the body attrset bind at its own level? And what
# does the body open with, once the lambda header is out of the way?
# A Nix identifier may hold an apostrophe. This reads `foo'` as `foo`, and it gives up on a
# `@ args'` binding. Both are legal, and neither appears in the family.
# Both fail towards a finding rather than towards silence, which is the direction an
# unhandled shape has to fail in
# shellcheck disable=SC2016 # $0 here is awk's record, and not expanding it is the whole point
body_awk='
  function skip_lambda(s,   n, i, j, d, c, rest) {
    n = length(s); i = 1
    while (i <= n && substr(s, i, 1) == "(") i++
    # A module that names none of its arguments is a lambda over a plain identifier,
    # `_: { … }`. Its head holds no braces to walk
    rest = substr(s, i)
    if (rest ~ /^[A-Za-z_][A-Za-z0-9_-]*:[[:space:]]/) {
      sub(/^[A-Za-z_][A-Za-z0-9_-]*:[[:space:]]*/, "", rest)
      return rest
    }
    if (substr(s, i, 1) != "{") return substr(s, i)
    j = i; d = 0
    while (j <= n) {
      c = substr(s, j, 1)
      if (c == "{") d++
      else if (c == "}") { d--; if (d == 0) break }
      j++
    }
    rest = substr(s, j + 1)
    if (rest !~ /^[[:space:]]*(@[[:space:]]*[A-Za-z_][A-Za-z0-9_-]*[[:space:]]*)?:/) return substr(s, i)
    sub(/^[[:space:]]*(@[[:space:]]*[A-Za-z_][A-Za-z0-9_-]*[[:space:]]*)?:[[:space:]]*/, "", rest)
    return rest
  }
  # A module usually binds cfg before its body, so the body sits behind a `let … in`.
  # A step over that lets a rule ask about the body of an ordinary module. Without it a rule
  # reaches only the few modules that bind nothing
  function skip_lets(s,   n, i, depth, instr, c) {
    while (1) {
      n = length(s); i = 1
      while (i <= n && substr(s, i, 1) == "(") i++
      if (substr(s, i, 4) != "let ") return substr(s, i)
      i += 4
      depth = 0; instr = 0
      while (i <= n) {
        c = substr(s, i, 1)
        if (instr) {
          if (c == "\\") i++
          else if (c == "\"") instr = 0
          i++; continue
        }
        if (c == "\"") { instr = 1; i++; continue }
        if (c == "(" || c == "[" || c == "{") { depth++; i++; continue }
        if (c == ")" || c == "]" || c == "}") { depth--; i++; continue }
        if (depth == 0 && substr(s, i, 4) == " in ") { i += 4; break }
        i++
      }
      if (i > n) return ""
      s = substr(s, i)
    }
  }
  {
    body = skip_lets(skip_lambda($0))
    if (MODE == "head") { print substr(body, 1, 6); exit }
    n = length(body); i = 1
    while (i <= n && substr(body, i, 1) == "(") i++
    if (substr(body, i, 1) != "{") {
      if (MODE != "ns") print "NOT-AN-ATTRSET"
      exit
    }
    i++
    depth = 0; instr = 0; word = ""
    while (i <= n) {
      c = substr(body, i, 1)
      if (instr) {
        if (c == "\\") i++
        else if (c == "\"") instr = 0
        i++; continue
      }
      if (c == "\"") { instr = 1; i++; continue }
      if (c == "{" || c == "[" || c == "(") { depth++; word = ""; i++; continue }
      if (c == "}" || c == "]" || c == ")") {
        if (c == "}" && depth == 0) break
        # The options block can name more than one namespace, so the flag lives until that
        # block closes rather than until its first key
        if (MODE == "ns" && want_ns && depth == 1) { want_ns = 0; nsword = "" }
        depth--; word = ""; i++; continue
      }
      if (depth == 0) {
        if (c ~ /[A-Za-z0-9_.-]/) { word = word c; i++; continue }
        if (c == "=" && word != "") {
          split(word, seg, ".")
          # The namespace a module declares its own options under is the first key below the
          # body`s `options`. The parser expands a dotted path into nested sets, so that key is
          # always one level down and never part of the name above it
          if (MODE == "ns" && seg[1] == "options") { want_ns = 1 }
          else if (MODE != "ns") print seg[1]
          word = ""; i++; continue
        }
        if (c == ";") { word = ""; i++; continue }
      }
      if (MODE == "ns" && want_ns && depth == 1) {
        if (c ~ /[A-Za-z0-9_.-]/) { nsword = nsword c; i++; continue }
        if (c == "=" && nsword != "") {
          split(nsword, nseg, ".")
          print nseg[1]
          nsword = ""; i++; continue
        }
        if (c == ";") { nsword = ""; i++; continue }
        i++; continue
      }
      i++
    }
  }'

# This prints the inner text of every list whose own level holds no nested list or attrset.
# A plain test can then decide what that list is made of. It steps over strings, so a
# bracket inside one is not a list
# shellcheck disable=SC2016 # $0 here is awk's record, and not expanding it is the whole point
lists_awk='
  {
    n = length($0)
    for (i = 1; i <= n; i++) {
      c = substr($0, i, 1)
      if (c == "\"") {
        i++
        while (i <= n) { d = substr($0, i, 1); if (d == "\\") i++; else if (d == "\"") break; i++ }
        continue
      }
      if (c != "[") continue
      depth = 1; j = i + 1; inner = ""; nested = 0
      while (j <= n && depth > 0) {
        d = substr($0, j, 1)
        if (d == "\"") {
          inner = inner d; j++
          while (j <= n) {
            e = substr($0, j, 1); inner = inner e
            if (e == "\\") { j++; inner = inner substr($0, j, 1) }
            else if (e == "\"") break
            j++
          }
          j++; continue
        }
        if (d == "[") { depth++; nested = 1 }
        else if (d == "]") { depth--; if (depth == 0) break }
        else if (d == "{") nested = 1
        inner = inner d
        j++
      }
      if (!nested && inner != "") print inner
      i = j
    }
  }'

flat_lists() { # flat_lists FILE -> one line per list that holds no nested list or attrset
  tree_of "$1" | awk "$lists_awk"
}

body_keys() { # body_keys FILE -> the keys the body attrset binds at its own level
  tree_of "$1" | awk -v MODE=keys "$body_awk"
}

option_namespaces() { # option_namespaces FILE -> the first key under the body's own `options`
  tree_of "$1" | awk -v MODE=ns "$body_awk"
}

body_head() { # body_head FILE -> the first six characters of the body, the lambda header gone
  tree_of "$1" | awk -v MODE=head "$body_awk"
}

# ---- the header ----------------------------------------------------------------------------------
# This is the one region a reader can take as text without a lexer. Before the first `}:`
# there are no strings and no nesting, measured across every .nix file in the family.
# The awk prints one row — SHAPE, NAMED, VARIADIC, ABOVE, AFTER, GAP, FORMALS — and every
# header rule reads it
header_facts() { # header_facts FILE
  awk '
    BEGIN { state = "pre"; shape = "none"; above = 0; named = 0; variadic = 0; after = 0; gap = 0; f = "" }
    state == "pre" && /^[[:space:]]*$/ { next }
    state == "pre" && /^#/ { above = 1; next }
    state == "pre" && /^\{[[:space:]]*$/ { shape = "multi"; state = "formals"; next }
    state == "pre" && /^\{.*\}[[:space:]]*:/ {
      shape = "inline"
      line = $0
      sub(/^\{/, "", line)
      sub(/\}[[:space:]]*:.*$/, "", line)
      n = split(line, parts, ",")
      for (i = 1; i <= n; i++) {
        gsub(/^[[:space:]]+|[[:space:]]+$/, "", parts[i])
        if (parts[i] == "...") variadic = 1
        else if (parts[i] != "") { named++; f = f (f ? " " : "") parts[i] }
      }
      state = "body"
      next
    }
    state == "pre" { exit }
    state == "formals" && /^\}[[:space:]]*:/ { state = "body"; next }
    state == "formals" {
      line = $0
      gsub(/^[[:space:]]+|,[[:space:]]*$/, "", line)
      gsub(/[[:space:]]+$/, "", line)
      if (line == "...") variadic = 1
      else if (line != "") { named++; f = f (f ? " " : "") line }
      next
    }
    state == "body" && /^[[:space:]]*$/ { if (after) gap = 1; next }
    state == "body" && /^#/ { after = 1; next }
    state == "body" { exit }
    END {
      # `}:` is what makes the lines above it a header. A file that opens with `{` and never
      # reaches one is a plain attrset, and flake.nix is exactly that. Without this the walk
      # reads all 294 of its lines as formals, and any line holding the bare word lib makes
      # the pkgs.lib rule fire on a file that takes no arguments at all
      if (state != "body") { shape = "none"; named = 0; variadic = 0; after = 0; gap = 0; f = "" }
      printf "%s\t%d\t%d\t%d\t%d\t%d\t%s\n", shape, named, variadic, above, after, gap, f
    }
  ' "$1"
}

# The standard module arguments, in the order they come in. Everything after them is
# alphabetical, and `...` is always last.
# nixfmt keeps whichever of the two line shapes the author writes, so the author owns both
# the order and the shape
standard_args="config lib pkgs osConfig"

expected_order() { # expected_order FORMALS... -> the order they should have been written in
  local given="$*" s rest=""
  for s in $standard_args; do
    case " $given " in *" $s "*) printf '%s ' "$s" ;; esac
  done
  for s in $given; do
    case " $standard_args " in *" $s "*) ;; *) rest="$rest$s"$'\n' ;; esac
  done
  [[ -z "$rest" ]] || printf '%s' "$rest" | LC_ALL=C sort | tr '\n' ' '
}

# A comment block above a module's header is attribution and nothing else. It credits
# vendored third-party work, as a licence header does.
# A URL, or one of the words a credit uses, tells the two apart. An explanation of the
# module belongs after the header, where the reader meets it with the code it explains
is_attribution() { # is_attribution FILE
  awk '
    /^#/ { block = block $0 "\n"; next }
    { exit }
    END {
      if (block ~ /https?:\/\//) { print "yes"; exit }
      if (block ~ /(Author|Copyright|Original|Upstream|Vendored|SPDX)/) { print "yes"; exit }
      print "no"
    }
  ' "$1"
}

# ---- the tree's own rules ---------------------------------------------------------------------
# Everything here reads the canonical line, where no comment survives and the layout is
# normal. A rule therefore asks about the expression, not about how somebody typed it.
# tree_probe pins each form these rules grep for. A release of Nix that moves one stops the
# run, and does not pass it
check_tree() { # check_tree FILE
  local f="$1" tree keys
  tree=$(tree_of "$f")
  [[ -n "$tree" ]] || return 0

  # A with whose scope is the file body. nixfmt writes the `let … in with` form at column 0,
  # and the text anchor catches that one. Only this shows the form on the header's own line
  if [[ "$(body_head "$f")" == "with "* ]]; then
    finding_unless_excused with-at-file-level "$f" \
      "$f: a with at the file level — its scope is every name the file goes on to bind"
  fi

  case "$tree" in
    *"; (let "*)
      # A with over a let is the shape the ban is really about. The body grows bindings, and
      # each new one silently takes a name the with opened
      case "$tree" in
        *"with "*"; (let "*)
          finding_unless_excused with-over-let "$f" \
            "$f: a with over a let — a binding added below silently takes a name the with opened"
          ;;
      esac
      ;;
  esac

  # `<nixpkgs>` parses to __findFile __nixPath, and NIX_PATH is read by name. Both make the
  # evaluation depend on the machine it runs on, which is the one thing a flake exists to stop
  case "$tree" in
    *"__findFile __nixPath"* | *'getEnv "NIX_PATH"'*)
      finding_unless_excused lookup-path "$f" \
        "$f: a lookup path or NIX_PATH — the evaluation then depends on the machine it runs on"
      ;;
  esac

  # `pkgs.lib` where lib is already an argument gives one thing two names. An overlay then
  # hides the fact that the longer name is the same lib
  if [[ "$tree" == *"(pkgs).lib."* ]] && [[ " $(header_facts "$f" | cut -f7) " == *" lib "* ]]; then
    finding_unless_excused pkgs-lib-with-lib-arg "$f" \
      "$f: pkgs.lib where lib is already an argument — call it lib"
  fi

  # A repository asset that reaches a derivation as a plain string ties that derivation's
  # hash to the whole repository, so every commit rebuilds it.
  # builtins.path with a fixed name isolates the asset from the rest
  case "$tree" in
    *'src = ((inputs).self + '* | *'src = ((self + '* | *'src = (self + '*)
      finding_unless_excused self-src-unwrapped "$f" \
        "$f: a derivation takes its src straight from inputs.self — wrap it in builtins.path with a fixed name, or every commit rebuilds it"
      ;;
  esac

  # A derivation with no meta at all. The heuristic is narrow on purpose. It does not find an
  # inline derivation inside a module, nor a runCommand in a flake.
  # A wider one, over every mkDerivation anywhere, would fire on the throwaway derivations a
  # wrapper builds
  case "$tree" in
    *mkDerivation* | *buildGoModule* | *buildRustPackage* | *buildPythonPackage* | *buildNpmPackage*)
      case "$tree" in
        *" meta = "*) ;;
        *)
          finding_unless_excused derivation-meta "$f" \
            "$f: a derivation with no meta — a package says what it is, who may use it and where it runs"
          ;;
      esac
      ;;
  esac

  # A list of nothing but pkgs attributes says pkgs once per element. `with pkgs;` says it
  # once for the list. The scope it opens is a literal with no bindings of its own, which is
  # the case the standard keeps `with` for.
  # A list that mixes pkgs with anything else stays alone. There the with would change what
  # the other elements mean
  local l
  while IFS= read -r l; do
    [[ -n "$l" ]] || continue
    case "$l" in *'((pkgs).'*) ;; *) continue ;; esac
    grep -qE '^ (\(\(pkgs\)\.[A-Za-z0-9_.-]+\) )+$' <<<"$l" || continue
    finding_unless_excused list-with-pkgs "$f" \
      "$f: a list of nothing but pkgs attributes — say it once with \`with pkgs;\` rather than once per element"
    break
  done <<EOF
$(flat_lists "$f")
EOF

  # An option under a prefix nixpkgs owns collides the day nixpkgs adds a module of that
  # name. The collision then arrives as a type error, in a file that did not change.
  # A repository claims a prefix out loud. One that declares options and claims none is told
  # so, and does not pass quietly
  local ns
  for ns in $(option_namespaces "$f"); do
    case " $namespaces " in
      *" $ns "*) continue ;;
    esac
    if [[ -z "$namespaces" ]]; then
      finding_unless_excused options-namespaced "$f" \
        "$f: declares options under \"$ns\" and no namespace is configured — name this repository's own with -N"
    else
      finding_unless_excused options-namespaced "$f" \
        "$f: declares options under \"$ns\", which is not one of: $namespaces"
    fi
    break
  done

  # default.nix belongs to aggregators. The repository's own root is the exception.
  # There it is the entry a bare `nix-build` reaches for, and not a list of modules
  if [[ "$(basename "$f")" == "default.nix" ]] && [[ "$(dirname "$f")" != "$root" ]]; then
    keys=$(body_keys "$f" | tr '\n' ' ')
    if [[ "$keys" != "imports " ]]; then
      finding_unless_excused default-nix-imports-only "$f" \
        "$f: a default.nix binding ${keys:-nothing but a let} — it is reserved for aggregators that only import"
    fi
  fi
}

check_header() { # check_header FILE
  local f="$1" row shape named variadic above after gap formals want
  row=$(header_facts "$f")
  shape=$(printf '%s' "$row" | cut -f1)
  named=$(printf '%s' "$row" | cut -f2)
  variadic=$(printf '%s' "$row" | cut -f3)
  above=$(printf '%s' "$row" | cut -f4)
  after=$(printf '%s' "$row" | cut -f5)
  gap=$(printf '%s' "$row" | cut -f6)
  formals=$(printf '%s' "$row" | cut -f7)

  [[ "$shape" == "inline" || "$shape" == "multi" ]] || return 0

  if [[ "$shape" == "multi" ]] && ((named <= 2)); then
    local plural="arguments are"
    ((named != 1)) || plural="argument is"
    finding_unless_excused arg-line-shape "$f" \
      "$f: $named named $plural stacked — up to two go on one line"
  fi
  if [[ "$shape" == "inline" ]] && ((named >= 3)); then
    finding_unless_excused arg-line-shape "$f" \
      "$f: $named named arguments share a line — three and more go one per line"
  fi

  # Only a module. A package's arguments come in the order nixpkgs writes them: lib, the
  # stdenv, the fetchers, then what it builds against.
  # Alphabetical order would put fetchFromGitHub before stdenvNoCC. No package in nixpkgs or
  # in this family does that
  if ((variadic && named >= 2)); then
    # shellcheck disable=SC2086 # splitting the formals into arguments is the point
    want=$(expected_order $formals)
    # Both sides are space-terminated, so a prefix never matches a longer name
    if [[ "$formals " != "$want" ]]; then
      finding_unless_excused module-arg-order "$f" \
        "$f: arguments are $formals, and the order is ${want% }"
    fi
  fi

  # Only a module. A package or a plain function keeps its comment on line 1. A reader of a
  # file that is not a module looks there first
  if ((variadic)) && ((above)) && [[ "$(is_attribution "$f")" == "no" ]]; then
    finding_unless_excused file-comment-placement "$f" \
      "$f: a module's file comment sits above its header — it goes after it, abutting the body"
  fi
  if ((gap)); then
    finding_unless_excused file-comment-placement "$f" \
      "$f: a blank line holds the file comment off the body it explains"
  fi
  if ((after && above && variadic)); then
    finding_unless_excused file-comment-placement "$f" \
      "$f: the file comment is in both places at once, above the header and after it"
  fi
}

# ---- the run ----------------------------------------------------------------------------------
unformatted=$(check_nixfmt)
if [[ -n "$unformatted" ]]; then
  while IFS= read -r f; do
    [[ -n "$f" ]] || continue
    finding_unless_excused nixfmt-formatted "$f" "$f is not formatted by nixfmt — run nix fmt"
  done <<EOF
$unformatted
EOF
fi

statix_rows=$(check_statix)
if [[ -n "$statix_rows" ]]; then
  while IFS= read -r row; do
    [[ -n "$row" ]] || continue
    # FILE>LINE:COL:SEVERITY:CODE:message
    f="${row%%>*}"
    rest="${row#*>}"
    line="${rest%%:*}"
    rest="${rest#*:}"
    col="${rest%%:*}"
    rest="${rest#*:}"
    rest="${rest#*:}"
    code="${rest%%:*}"
    msg="${rest#*:}"
    finding_unless_excused statix "$f" "$f:$line:$col: statix W$code — $msg"
  done <<EOF
$statix_rows
EOF
fi

dead_rows=$(check_deadnix)
if [[ -n "$dead_rows" ]]; then
  while IFS= read -r row; do
    [[ -n "$row" ]] || continue
    finding_unless_excused deadnix "${row%%:*}" "$row (deadnix)"
  done <<EOF
$dead_rows
EOF
fi

# ---- the file names ------------------------------------------------------------------------------
# The path of each .nix file, its directories included. A rename of one must find every
# reference to it, and the author chose the name.
# Not every path in the repository. Measured across fifteen repositories: that wider rule
# gave 152 findings and no true one. It fired on Cargo.toml, on a pytest test_*.py, on a
# zsh completion's leading underscore, on a systemd template unit's @, on an X11 cursor and
# on an Obsidian note's title. Cargo, pytest, zsh, systemd, X11 and Obsidian choose those
# names, not the author, and the list of such conventions has no end.
# Conventional root metadata is the one shape in capitals on purpose
kebab_rows=""
names_read=0
if ((${#paths[@]} == 0)) && git -C "$root" rev-parse --git-dir >/dev/null 2>&1; then
  names_read=1
  kebab_rows=$(git_nix_files | awk '
    {
      n = split($0, part, "/")
      for (i = 1; i <= n; i++) {
        p = part[i]
        # No exemption for capitals. README.md and LICENSE earned one while this read every
        # path; a .nix file named MODULE.nix is a finding, and so is a directory above one
        if (p ~ /^\.?[a-z0-9][a-z0-9.-]*$/) continue
        print $0 "\t" p
        break
      }
    }')
fi
if [[ -n "$kebab_rows" ]]; then
  while IFS=$'\t' read -r path part; do
    [[ -n "$path" ]] || continue
    finding_unless_excused file-kebab-case "$path" "$path: \"$part\" is not kebab-case"
  done <<EOF
$kebab_rows
EOF
fi

# ---- the header, and the with that scopes a whole file ---------------------------------------------
# nixfmt puts a file-level `with` at column 0 only when a `let ... in` comes before it. It
# leaves the form `{ pkgs, ... }: with pkgs; { … }` on one line.
# So the text anchor alone is not enough, and the parse form answers the rest
while IFS= read -r f; do
  [[ -n "$f" ]] || continue
  is_generated "$f" && continue
  if grep -q '^with ' "$f" 2>/dev/null; then
    finding_unless_excused with-at-file-level "$f" \
      "$f: a with at the file level — its scope is every name the file goes on to bind"
  fi
  check_tree "$f"
  is_lambda "$f" || continue
  check_header "$f"
done <<EOF
$files
EOF

# ---- falsification ------------------------------------------------------------------------------
# Every check above shows itself able to fail on this run, and not on the run that wrote it.
# This script's own templates build a canon, and the run confirms the canon clean.
# It then plants one defect per check into a fresh copy. The checker must reject that copy
# for the defect's own stated reason.
# A copy that is merely rejected proves nothing, because any breakage rejects everything
self=$0
canon="$work/canon"

# A git repository on purpose. The name rule reads the repository's own file list. The walk
# over the .nix files also takes its git branch here, and not the find one.
# The plants below therefore exercise the path a consumer runs
build_canon() {
  mkdir -p "$canon/nix"
  template_flake >"$canon/flake.nix"
  template_module >"$canon/module.nix"
  template_package >"$canon/nix/package.nix"
  git -C "$canon" init -q
}

nested() { # nested DIR [ARGS...] -> this script on DIR's copy, falsification skipped
  local d="$1"
  shift
  local mode=()
  ((static == 0)) || mode=(--static)
  # The canon declares its options under `example`. So each copy runs as a repository that
  # claimed that prefix. The plant for an unclaimed prefix names a different one
  CHECK_NIX_NESTED=1 "$BASH" "$self" -C "$d" -N example ${mode[@]+"${mode[@]}"} "$@"
}

copy() { # copy NAME -> a fresh copy of the canon
  local c="$work/$1"
  mkdir -p "$c"
  cp -R "$canon/." "$c/"
  printf '%s\n' "$c"
}

planted=0
# The count lives here rather than beside each call, so a new planted case cannot be left out
# of the number the summary line reports
expect_red() { # expect_red DIR FRAGMENT WHAT
  local d="$1" want="$2" what="$3" out
  if out=$(nested "$d" 2>&1); then
    die "self-test: a copy with $what passed — the check cannot catch it"
  fi
  case "$out" in
    *"$want"*) ;;
    *) die "self-test: a copy with $what was rejected for the wrong reason: $out" ;;
  esac
  planted=$((planted + 1))
}

expect_green() { # expect_green DIR WHAT
  local d="$1" what="$2" out status=0
  out=$(nested "$d" 2>&1) || status=$?
  ((status == 0)) ||
    die "self-test: $what was rejected with exit $status — the checker is broken, not the canon:"$'\n'"$out"
}

append() { # append DIR FILE LINE -> the line added at the end of the file
  printf '%s\n' "$3" >>"$1/$2"
}

# Through the environment rather than -v. awk reads escape sequences in a -v value, so a
# planted line that holds a backslash would arrive changed
plant_after() { # plant_after DIR FILE AFTER-PATTERN LINE
  PAT="$3" LINE="$4" awk '{ print } !done && index($0, ENVIRON["PAT"]) == 1 { print ENVIRON["LINE"]; done = 1 }' \
    "$1/$2" >"$1/$2.new"
  mv "$1/$2.new" "$1/$2"
}

swap_lines() { # swap_lines DIR FILE FIRST SECOND -> the two lines exchanged
  A="$3" B="$4" awk '
    $0 == ENVIRON["A"] { print ENVIRON["B"]; next }
    $0 == ENVIRON["B"] { print ENVIRON["A"]; next }
    { print }
  ' "$1/$2" >"$1/$2.new"
  mv "$1/$2.new" "$1/$2"
}

collapse_header() { # collapse_header DIR FILE -> a stacked header put back on one line
  awk '
    NR == 1 && $0 == "{" { inhdr = 1; line = "{"; next }
    inhdr && /^\}[[:space:]]*:/ { print line " }:"; inhdr = 0; next }
    inhdr {
      gsub(/^[[:space:]]+|,[[:space:]]*$/, "", $0)
      line = line (line == "{" ? " " : ", ") $0
      next
    }
    { print }
  ' "$1/$2" >"$1/$2.new"
  mv "$1/$2.new" "$1/$2"
}

self_test() {
  build_canon

  # The canon must pass before any plant means anything, and it must be what --template
  # prints. This then catches a nixfmt or statix release that moves, and no consumer does
  expect_green "$canon" "the canon this script's own --template prints"

  local c
  # nixfmt: a line nixfmt would rewrite. Two spaces of indentation are the canon's, four are not
  c=$(copy nixfmt-plant)
  append "$c" module.nix '{    a    =    1; }'
  expect_red "$c" "is not formatted by nixfmt" "a line nixfmt would rewrite"

  # statix: `x = x;` is W3 manual_inherit, the house rule spelled as a lint rather than twice
  c=$(copy statix-plant)
  plant_after "$c" module.nix '  cfg = config.example.thing;' '  lib = lib;'
  expect_red "$c" "statix W3" "a binding statix rewrites with inherit"

  # statix again, from the other side. The one disabled lint must stay disabled. Otherwise
  # every module that sets two options under one path prefix fires on the option tree's shape
  c=$(copy statix-disabled-plant)
  printf '_: {\n  services.nginx.enable = true;\n  services.postgresql.enable = true;\n}\n' >"$c/two-services.nix"
  expect_green "$c" "two unrelated options whose paths share a first segment"

  # deadnix: an argument named and never read
  c=$(copy deadnix-plant)
  plant_after "$c" module.nix '  pkgs,' '  unusedArgument,'
  expect_red "$c" "Unused lambda pattern: unusedArgument" "an argument nothing reads"

  # A path component that is not kebab-case. The walk reads the repository's own file list,
  # and --others puts a file there the moment it is written
  c=$(copy kebab-plant)
  printf '_: { }\n' >"$c/Not_Kebab.nix"
  expect_red "$c" 'is not kebab-case' "a module named in snake case"

  # …and a directory above a module counts the same. A rename of it moves every module under
  # it, so it is the author's choice as much as the file is
  c=$(copy kebab-dir-plant)
  mkdir -p "$c/Some_Dir"
  printf '_: { }\n' >"$c/Some_Dir/fine.nix"
  expect_red "$c" 'is not kebab-case' "a directory in snake case above a module"

  # …and nothing else in the tree is judged. Cargo, pytest, zsh, systemd, X11 and Obsidian
  # each require a name the author did not choose, and no list of them ever closes
  c=$(copy kebab-foreign-plant)
  mkdir -p "$c/tests" "$c/completions"
  : >"$c/Cargo.toml"
  : >"$c/tests/test_thing.py"
  : >"$c/completions/_thing"
  expect_green "$c" "names that Cargo, pytest and zsh require"

  # A with whose scope is the file. nixfmt puts it at column 0 only after a `let … in`, and
  # the canon has one. The plant is therefore what a consumer would have written
  c=$(copy with-plant)
  plant_after "$c" module.nix 'in' 'with pkgs;'
  expect_red "$c" "a with at the file level" "a with scoping the whole file"

  # The order of a module's arguments, which nixfmt preserves either way and so cannot hold
  c=$(copy order-plant)
  swap_lines "$c" module.nix '  config,' '  lib,'
  expect_red "$c" "and the order is config lib pkgs" "arguments out of their order"

  # Three named arguments on one line, which nixfmt also preserves
  c=$(copy shape-plant)
  collapse_header "$c" module.nix
  expect_red "$c" "share a line" "three arguments sharing a line"

  # The file comment held off the body by a blank line
  c=$(copy comment-gap-plant)
  plant_after "$c" module.nix '# argument header and abuts the body' ''
  expect_red "$c" "holds the file comment off the body" "a blank line between the comment and the body"

  # An excuse that covers nothing is a finding of its own
  c=$(copy stale-allow-plant)
  printf 'file-kebab-case No_Such_File.txt\n' >"$c/check-nix.allow"
  expect_red "$c" "excuses nothing" "an excuse whose finding is gone"

  # …and an excuse for a rule that did not run this invocation is not stale. A run cannot
  # tell a dead excuse from one whose rule never got its turn, so it must not guess.
  # Both shapes below are what the nix flake check sandbox hands a consumer
  local idle_out idle_status
  c=$(copy idle-eval-allow-plant)
  printf 'flake-formatter flake.nix\n' >"$c/check-nix.allow"
  idle_status=0
  idle_out=$(CHECK_NIX_NESTED=1 "$BASH" "$self" --static -N example -C "$c" 2>&1) || idle_status=$?
  ((idle_status == 0)) ||
    die "self-test: --static called an excuse for an evaluated rule stale (exit $idle_status): $idle_out"
  planted=$((planted + 1))

  c=$(copy idle-names-allow-plant)
  rm -rf "$c/.git"
  printf 'file-kebab-case assets/\n' >"$c/check-nix.allow"
  idle_status=0
  idle_out=$(CHECK_NIX_NESTED=1 "$BASH" "$self" --static -N example -C "$c" 2>&1) || idle_status=$?
  ((idle_status == 0)) ||
    die "self-test: a tree with no repository called a name excuse stale (exit $idle_status): $idle_out"
  planted=$((planted + 1))

  # The tree's own rules. Each plant is a whole file, and not an edit to the canon. The
  # fixture then says plainly what shape it is about, and nothing else in it fires first
  c=$(copy aggregator-plant)
  mkdir -p "$c/sub"
  printf '_: {\n  imports = [ ];\n  services.thing.enable = true;\n}\n' >"$c/sub/default.nix"
  expect_red "$c" "reserved for aggregators" "a default.nix that also configures something"

  c=$(copy aggregator-clean-plant)
  mkdir -p "$c/sub"
  printf '_: {\n  imports = [ ];\n}\n' >"$c/sub/default.nix"
  expect_green "$c" "a default.nix that only imports"

  c=$(copy with-over-let-plant)
  printf '{ lib, ... }:\n{\n  a = with lib; let b = 1; in b;\n}\n' >"$c/scoped.nix"
  expect_red "$c" "a with over a let" "a with whose body binds names of its own"

  c=$(copy lookup-path-plant)
  printf '_: {\n  a = import <nixpkgs> { };\n}\n' >"$c/looked.nix"
  expect_red "$c" "a lookup path or NIX_PATH" "a lookup path"

  c=$(copy pkgs-lib-plant)
  printf '{ lib, pkgs, ... }:\n{\n  a = pkgs.lib.mkForce 1;\n  b = lib.mkDefault 2;\n}\n' >"$c/forced.nix"
  expect_red "$c" "pkgs.lib where lib is already an argument" "pkgs.lib beside a lib argument"

  # …and a file that opens with `{` and never reaches `}:` takes no arguments at all. Every
  # flake.nix has that shape. The header walk must not read its lines as formals: one line
  # holding the bare word lib would otherwise make the rule above fire on a file with no
  # lib to confuse anything with
  c=$(copy attrset-not-header-plant)
  printf '{\n  outputs =\n    { nixpkgs, ... }:\n    {\n      a = nixpkgs.legacyPackages.x86_64-linux.lib.mkForce 1;\n      inherit\n        lib\n        ;\n    };\n}\n' \
    >"$c/looks-like-a-header.nix"
  expect_green "$c" "a plain attrset whose lines are not a lambda's formals"

  c=$(copy self-src-plant)
  # shellcheck disable=SC2016 # ${inputs.self} is Nix interpolation in the planted file, not this shell's
  printf '{ inputs, pkgs, ... }:\n{\n  a = pkgs.stdenvNoCC.mkDerivation {\n    pname = "x";\n    version = "1";\n    src = "${inputs.self}/assets";\n    meta.description = "X";\n  };\n}\n' >"$c/vendored.nix"
  expect_red "$c" "straight from inputs.self" "a derivation src that is the whole repository"

  # A list of nothing but pkgs attributes, and its one-element form, which counts the same.
  # The point is one pkgs for the list, and not one pkgs for each thing in it
  c=$(copy with-pkgs-plant)
  printf '{ pkgs, ... }:\n{\n  a = [\n    pkgs.coreutils\n    pkgs.jq\n  ];\n}\n' >"$c/listed.nix"
  expect_red "$c" "nothing but pkgs attributes" "a list that says pkgs once per element"

  c=$(copy with-pkgs-one-plant)
  printf '{ pkgs, ... }:\n{\n  a = [ pkgs.jq ];\n}\n' >"$c/single.nix"
  expect_red "$c" "nothing but pkgs attributes" "a one-element list that says pkgs anyway"

  # …and the two shapes it must leave alone. One already follows the rule. The other mixes
  # pkgs with something else, where the with would change what the other element means
  c=$(copy with-pkgs-clean-plant)
  printf '{ lib, pkgs, ... }:\n{\n  a = with pkgs; [\n    coreutils\n    jq\n  ];\n  b = [\n    pkgs.jq\n    lib.fakeHash\n  ];\n}\n' >"$c/fine.nix"
  expect_green "$c" "a list already using with pkgs, and one that mixes pkgs with something else"

  c=$(copy no-meta-plant)
  printf '{ pkgs, ... }:\n{\n  a = pkgs.stdenvNoCC.mkDerivation {\n    pname = "x";\n    version = "1";\n  };\n}\n' >"$c/bare.nix"
  expect_red "$c" "a derivation with no meta" "a derivation that says nothing about itself"

  # An input locked to a path on one machine. Two nodes, because a lock with fewer is one
  # this checker reads wrong, and that refusal is a different one
  c=$(copy local-input-plant)
  cat >"$c/flake.lock" <<'LOCK'
{
  "nodes": {
    "local": {
      "locked": { "type": "path", "path": "/home/someone/dev/thing" },
      "original": { "type": "path", "path": "/home/someone/dev/thing" }
    },
    "root": { "inputs": { "local": "local" } }
  },
  "root": "root",
  "version": 7
}
LOCK
  expect_red "$c" "locked to an absolute local path" "an input pointing at one machine's disk"

  # …and the same lock once the repository excuses it. This proves the excuses are read back
  # after the lock rule has run: earlier, and a used excuse still reads as a dead one
  c=$(copy excused-local-input-plant)
  cp "$work/local-input-plant/flake.lock" "$c/flake.lock"
  printf 'lock-no-local-input flake.lock the checkout is this machine only, on purpose\n' >"$c/check-nix.allow"
  expect_green "$c" "a lock finding the repository excuses, with the excuse counted as used"

  # The grammar meta.description must hold to. Only the half that judges gets a plant.
  # The half that obtains needs nix to evaluate a flake, so check.sh tests it on a fixture.
  # Each case runs in a subshell, so the findings it prints stay out of this run's count.
  # A case must draw its own finding and no other, and the last case must draw none.
  # A rule that also fires on a good description still passes a test that only asks for red.
  #
  # The cases run again under a locale that sorts letters the way a dictionary does, if this
  # machine has one. A range such as [a-z] in a bash pattern follows that order, and then
  # matches H. Bash 4.3 added globasciiranges against this, and 5.0 turned it on by default.
  # The bash 3.2 that macOS ships has neither, and the macOS runner sets en_US.UTF-8.
  # The probe asks bash itself, because only its own pattern match shows the order it uses
  local judged row want out pass passes="ambient"
  if LC_ALL=en_US.UTF-8 "$BASH" -c 'shopt -u globasciiranges 2>/dev/null
    case B in [a-c]) exit 0 ;; esac
    exit 1' 2>/dev/null; then
    passes="ambient dictionary"
  else
    dictionary_unjudged=1
  fi
  for pass in $passes; do
    for judged in \
      'default	thing	A thing that does things	MIT	opens with an article' \
      'default	thing	Does things.	MIT	ends with a period' \
      'default	thing	does things	MIT	starts lowercase' \
      'default	Thing	Thing that does things	MIT	opens with the package'"'"'s own name' \
      'default	thing	Does things		no meta.license' \
      'default	thing	Does things	MIT	'; do
      row="${judged%	*}"
      want="${judged##*	}"
      out=$( (
        if [[ "$pass" == dictionary ]]; then
          LC_ALL=en_US.UTF-8
          shopt -u globasciiranges 2>/dev/null || :
        fi
        printf '%s\n' "$row" | meta_judge
      ) 2>&1 || :)
      if [[ -z "$want" ]]; then
        [[ -z "$out" ]] ||
          die "self-test: a description that breaks no rule was named, in the $pass locale: $out"
      else
        case "$out" in
          *"$want"*) ;;
          *) die "self-test: a description this checker should have named was not, in the $pass locale: wanted \"$want\", got \"$out\"" ;;
        esac
        [[ "$(printf '%s\n' "$out" | grep -c 'check-nix:')" == 1 ]] ||
          die "self-test: a description drew findings beside its own, in the $pass locale: $out"
      fi
      planted=$((planted + 1))
    done
  done

  # No document describes the printer the tree rules read, so the golden pins its shape.
  # This asks the refusal rather than the rule. A golden that no longer describes the
  # printer must stop the run. That is the one thing tree_preflight cannot prove about
  # itself
  local out status=0
  out=$(CHECK_NIX_NESTED=1 CHECK_NIX_GOLDEN=moved "$BASH" "$self" -C "$canon" 2>&1) || status=$?
  ((status == 2)) ||
    die "self-test: a golden that no longer matches the printer did not stop the run (got $status): $out"
  case "$out" in
    *"is not the one tree_golden describes"*) ;;
    *) die "self-test: a moved golden was refused for the wrong reason: $out" ;;
  esac
  planted=$((planted + 1))

  # The refusal itself. The checker must refuse a machine without a tool, and never check it
  # with less. The defect goes in the tools rather than in the text, which is the one thing a
  # planted line cannot express.
  # This asks about every tool, and not about one of them. A preflight that quietly drops a
  # tool goes on to report a clean run, while that tool's half of the check does nothing.
  #
  # A copy that exports its own PATH cannot take this plant. A wrapper builds such a copy
  # when it pins the tools beside the script, and the nested run then finds the tool the
  # plant took away. The property still holds there, and the wrapper is what holds it.
  # So the plant steps aside and the summary says which of the two ran
  #
  # The plant takes away one command, and never a directory. A system directory can hold a
  # tool beside mktemp: /usr/bin/jq on macOS 15 and later, nix-instantiate in
  # /run/current-system/sw/bin on NixOS. To drop such a directory drops the userland too,
  # and the run then dies on mktemp before it can refuse anything.
  # So one directory gets a link to each command this PATH finds, the first one winning as
  # in a lookup. The nested run gets that directory alone, without the one link the plant
  # takes. Every tool then sits beside mktemp on every machine, so a plant that takes a
  # directory fails here, and not only on the machine whose layout it did not expect
  #
  # The tools come from the file header and not from the preflight. A list read from the
  # preflight shrinks with it, and a tool the preflight forgets would then be asked about
  # nowhere
  local tool tools p f oldifs links merged="$work/path" out status
  tools=$(grep -m 1 '^# To read the Nix it gets, it also needs ' "$self") ||
    die "self-test: the header no longer says which tools the checker needs"
  tools=${tools#*also needs }
  tools=${tools%.}
  tools=${tools//,/}
  tools=${tools/ and / }
  if grep -q '^export PATH=' "$self"; then
    pinned_path=1
  else
    mkdir "$merged"
    oldifs=$IFS
    IFS=:
    for p in $PATH; do
      IFS=$oldifs
      links=()
      for f in "$p"/*; do
        [[ -x "$f" && ! -d "$f" ]] || continue
        [[ -e "$merged/${f##*/}" || -L "$merged/${f##*/}" ]] || links+=("$f")
      done
      ((${#links[@]} == 0)) || ln -s "${links[@]}" "$merged/"
    done
    IFS=$oldifs

    for tool in $tools; do
      mv "$merged/$tool" "$work/taken" ||
        die "self-test: $tool is not among the commands on PATH, though the preflight found it"
      status=0
      out=$(PATH="$merged" CHECK_NIX_NESTED=1 "$BASH" "$self" -C "$canon" 2>&1) || status=$?
      mv "$work/taken" "$merged/$tool"
      ((status == 2)) ||
        die "self-test: a machine without $tool was not refused with exit 2 (got $status): $out"
      case "$out" in
        *"needs $tool"*) ;;
        *) die "self-test: a machine without $tool was refused without naming it: $out" ;;
      esac
      planted=$((planted + 1))
    done
  fi

  # A generated file is exempt, and the exemption must still match the name NixOS writes.
  # Without a fixture that holds one, an exemption that matches nothing reads as a clean run
  c=$(copy generated-plant)
  printf '{\n  config,\n  lib,\n  modulesPath,\n  unusedByNixos,\n  ...\n}:\n{\n  imports = [ ];\n}\n' \
    >"$c/hardware-configuration.nix"
  expect_green "$c" "a generated hardware-configuration.nix, which a repository does not edit"

  # A list that holds an attrset is not a flat list of packages. The advice about with pkgs
  # would not compile here
  c=$(copy nested-list-plant)
  printf '{ pkgs, ... }:\n{\n  a = [ (pkgs.callPackage ./x.nix { }) ];\n}\n' >"$c/nested.nix"
  expect_green "$c" "a list whose element carries an attrset"

  # A brace inside a string is not a nesting level. The parser sorts keys, so a binding named
  # before `options` comes first in the line the tokeniser walks.
  # Read its brace as nesting, and `options` is no longer a key of the body. The finding
  # below then never happens
  c=$(copy string-brace-plant)
  # The brace has no pair on purpose. A balanced pair inside a string raises the depth and
  # lowers it again. It would leave the walk where it started, and prove nothing
  printf '{ lib, ... }:\n{\n  a = "an opening brace { on its own";\n  options.services.mine.enable = lib.mkEnableOption "mine";\n}\n' \
    >"$c/braced.nix"
  expect_red "$c" 'declares options under "services"' "a brace in a string before the key that is read"

  # The form of `with` that shares the argument header's line, which the text anchor cannot see
  c=$(copy with-inline-plant)
  printf '{ pkgs, ... }: with pkgs;\n{\n  a = jq;\n}\n' >"$c/inline.nix"
  expect_red "$c" "a with at the file level" "a with sharing the argument header's line"

  # An option under a prefix nixpkgs owns, and the same file once a repository claims it
  c=$(copy namespace-plant)
  printf '{ lib, ... }:\n{\n  options.services.mine.enable = lib.mkEnableOption "mine";\n}\n' >"$c/owned.nix"
  expect_red "$c" 'declares options under "services"' "an option under a prefix nixpkgs owns"

  # …and the other half of the rule. A repository that declares options and claims no prefix
  # is told so. The whole rule must not quietly do nothing for want of configuration.
  # This one goes around nested(), which always claims the canon's prefix, so it carries the
  # mode itself. Without that, the sandbox run tries to evaluate a flake it cannot fetch
  local mode=()
  ((static == 0)) || mode=(--static)
  status=0
  out=$(CHECK_NIX_NESTED=1 "$BASH" "$self" -C "$canon" ${mode[@]+"${mode[@]}"} 2>&1) || status=$?
  ((status == 1)) ||
    die "self-test: a repository declaring options with no namespace claimed was not told (exit $status): $out"
  case "$out" in
    *"no namespace is configured"*) ;;
    *) die "self-test: the unclaimed-namespace case was reported as something else: $out" ;;
  esac
  planted=$((planted + 1))

  # A lock this checker cannot read right is a refusal rather than a pass
  c=$(copy short-lock-plant)
  printf '{ "nodes": { "root": { "inputs": { } } }, "root": "root", "version": 7 }\n' >"$c/flake.lock"
  status=0
  out=$(nested "$c" 2>&1) || status=$?
  ((status == 2)) ||
    die "self-test: a lock of one node was not refused with exit 2 (got $status): $out"
  case "$out" in
    *"reading the wrong shape"*) ;;
    *) die "self-test: a one-node lock was refused for the wrong reason: $out" ;;
  esac
  planted=$((planted + 1))

  # And the summary says what did not run, in each of the two ways it can be short.
  # Those sentences exist to prevent one failure: a shorter check that reads like a complete
  # one. So each sentence is itself checked
  out=$(CHECK_NIX_NESTED=1 "$BASH" "$self" --static -C "$canon" 2>&1) || :
  case "$out" in
    *"--static, so nothing that evaluates the flake ran"*) ;;
    *) die "self-test: a --static run did not say the evaluated half was left out: $out" ;;
  esac
  planted=$((planted + 1))

  # The canon has no lock. To force what its formatter evaluates to therefore needs inputs
  # this machine does not hold. That is the other way a run comes up short, and the other
  # sentence
  if ((static == 0)); then
    out=$(CHECK_NIX_NESTED=1 "$BASH" "$self" -C "$canon" -N example 2>&1) || :
    case "$out" in
      *"an output needed inputs this machine does not hold"*) ;;
      *) die "self-test: a run that could not force an output did not say so: $out" ;;
    esac
    planted=$((planted + 1))
  fi
}

# ---- the lock ----------------------------------------------------------------------------------
# flake.lock is JSON and nothing else, so this asks it directly and needs neither network nor store
check_lock() {
  local lock="$root/flake.lock" nodes
  [[ -r "$lock" ]] || return 0
  nodes=$(jq -r '.nodes | length' "$lock" 2>/dev/null) ||
    die "$lock is not JSON this can read"
  ((nodes >= 2)) ||
    die "$lock holds $nodes nodes — this is reading the wrong shape, not an empty lock"

  # A dev override points an input at a local checkout, as url = "path:/home/…" does. It
  # reaches the lock through an ordinary `git add -A`, and then breaks the repository on
  # every machine but one.
  # A relative path stays alone. Those are subflakes of this repository, and they travel
  # with it
  local bad
  bad=$(jq -r '
    .nodes | to_entries[] | .key as $name
    | [(.value.locked // {}), (.value.original // {})][]
    | select(((.type? == "path") and ((.path? // "") | startswith("/")))
             or ((.type? == "git") and ((.url? // "") | startswith("file:///"))))
    | $name
  ' "$lock" | sort -u)
  local n
  for n in $bad; do
    finding_unless_excused lock-no-local-input "$lock" \
      "$lock: \"$n\" is locked to an absolute local path — a dev override reached the lock, and no other machine has it"
  done
}

# ---- what only the evaluation knows ----------------------------------------------------------
# These ask the flake for values rather than for text, so they need its inputs. The nix
# flake check sandbox has neither the network nor the store to fetch those.
# --static leaves these rules out and says so. The run that has the inputs is a plain CI
# step beside the one in the sandbox
meta_judge() { # meta_judge — reads the evaluated packages as JSON on stdin
  local attr pname description license row
  while IFS=$'\t' read -r attr pname description license; do
    [[ -n "$attr" ]] || continue
    row="$root#packages.$attr"
    if [[ -z "$description" ]]; then
      finding_unless_excused meta-description-grammar "$row" \
        "$row: no meta.description — a package says in one sentence what it is"
    else
      case "$description" in
        # A class rather than a range: a range follows the locale's collation order
        [[:lower:]]*) finding_unless_excused meta-description-grammar "$row" \
          "$row: meta.description starts lowercase: \"$description\"" ;;
      esac
      case "$description" in
        *.) finding_unless_excused meta-description-grammar "$row" \
          "$row: meta.description ends with a period: \"$description\"" ;;
      esac
      case "$description" in
        "A "* | "An "* | "The "*) finding_unless_excused meta-description-grammar "$row" \
          "$row: meta.description opens with an article: \"$description\"" ;;
      esac
      case "$description" in
        "$pname"*) finding_unless_excused meta-description-grammar "$row" \
          "$row: meta.description opens with the package's own name: \"$description\"" ;;
      esac
    fi
    [[ -n "$license" ]] ||
      finding_unless_excused meta-license "$row" \
        "$row: no meta.license — what a package may be used for is not a detail"
  done
}

unforced=0            # outputs whose value needed inputs this machine does not hold
pinned_path=0         # this copy exports its own PATH, so no plant can take a tool away from it
dictionary_unjudged=0 # no locale here sorts letters as a dictionary does

check_eval() {
  [[ -r "$root/flake.nix" ]] || return 0
  local system outs name packages rows
  system=$(nix config show system 2>/dev/null) || system=""
  [[ -n "$system" ]] || die "nix could not say what system this is"

  # --offline throughout, because the help promises this reaches no network.
  # The names of a flake's outputs evaluate without a force of its inputs. So any flake can
  # answer whether it declares a formatter.
  # What that formatter evaluates to needs the inputs. The run leaves that half out, and
  # says so aloud
  outs=$(nix eval --offline --impure --json --expr "builtins.attrNames (builtins.getFlake \"$root\")" 2>/dev/null) ||
    {
      unforced=1
      return 0
    }
  case "$outs" in
    *'"formatter"'*)
      if name=$(nix eval --offline --no-write-lock-file --raw "$root#formatter.$system.name" 2>/dev/null); then
        case "$name" in
          nixfmt-tree-*) ;;
          *) finding_unless_excused flake-formatter "$root/flake.nix" \
            "$root/flake.nix: formatter is $name — the family's is nixfmt-tree, which walks a whole tree rather than the files it is handed" ;;
        esac
      else
        unforced=1
      fi
      ;;
    *)
      finding_unless_excused flake-formatter "$root/flake.nix" \
        "$root/flake.nix: no formatter output — nix fmt then does nothing and CI has nothing to run"
      ;;
  esac

  case "$outs" in
    *'"packages"'*) ;;
    # A flake with no packages is not a finding: a module or a skill repository has none
    *) return 0 ;;
  esac
  # meta.license is one license, a list of them, or a bare string, as nixpkgs allows all three.
  # A list counts as licensed when any of its entries names a license
  packages=$(nix eval --offline --no-write-lock-file --json "$root#packages.$system" --apply '
    ps: let
      named = l: if builtins.isString l then l else l.spdxId or (l.shortName or "");
      licenses = l: if builtins.isList l then map named l else [ (named l) ];
    in builtins.mapAttrs (n: p: {
      pname = p.pname or n;
      description = p.meta.description or "";
      license = builtins.concatStringsSep " "
        (builtins.filter (s: s != "") (licenses (p.meta.license or "")));
    }) ps' 2>/dev/null) || {
    unforced=1
    return 0
  }
  # Into a variable first, and not down a pipe: the last stage of a pipe runs in a subshell,
  # so every finding meta_judge counted there was lost before the exit code was decided
  rows=$(printf '%s' "$packages" |
    jq -r 'to_entries[] | [.key, .value.pname, .value.description, .value.license] | @tsv')
  meta_judge <<<"$rows"
}

check_lock
((static)) || check_eval

# ---- the excuses, read back --------------------------------------------------------------------
# After every rule has run, so an excuse for the lock or for an evaluated output has had its
# chance to be used.
# An entry that excused nothing this run is a finding of its own. It means one of two
# things. Either the thing it covered is gone, and the line outlived its reason. Or the path
# no longer matches, and the excuse has covered nothing since.
# Nobody should find out about either one a year later.
#
# A rule that never ran this invocation is the exception. Its excuse could not be used, and
# that says nothing about whether the excuse is stale. The nix flake check sandbox meets
# both cases: it sees no git repository, so it checks no name, and --static leaves the
# evaluated rules out. Without this, a repository goes red in the sandbox alone, over an
# excuse that is correct everywhere else
idle_rules=""
((names_read || ${#paths[@]})) || idle_rules="$idle_rules file-kebab-case"
((static == 0 && unforced == 0)) || idle_rules="$idle_rules flake-formatter meta-description-grammar meta-license"
[[ -r "$root/flake.lock" ]] || idle_rules="$idle_rules lock-no-local-input"
if [[ -s "$work/allow" ]]; then
  while IFS=$'\t' read -r eid epath; do
    [[ -n "$eid" ]] || continue
    case " $idle_rules " in *" $eid "*) continue ;; esac
    grep -qxF "$eid	$epath" "$work/allow.used" ||
      finding "$allow_file: \"$eid $epath\" excuses nothing — the finding it covered is gone"
  done <"$work/allow"
fi

# Last, so every rule it plants a defect against exists, and every real finding is already out
[[ -n "${CHECK_NIX_NESTED:-}" ]] || self_test

rule_count=$(rules | grep -c .)
noun="files"
((file_count != 1)) || noun="file"
summary="check-nix: $file_count .nix $noun, 3 tools, $rule_count rules"
((static == 0)) || summary="$summary; --static, so nothing that evaluates the flake ran"
((unforced == 0)) || summary="$summary; an output needed inputs this machine does not hold, so what it evaluates to went unchecked"
# The file list comes from the repository itself. Outside a repository there is nothing to
# hold to the naming convention, and the nix flake check sandbox is where that happens.
# Silence about it would read as names this run checked and found good
((names_read || ${#paths[@]})) || summary="$summary; no repository here, so no name was checked"
((pinned_path == 0)) || summary="$summary; this copy pins its own PATH, so the refusal of a machine without a tool was not planted"
((dictionary_unjudged == 0)) || summary="$summary; no locale here sorts letters as a dictionary does, so the description grammar was judged in this run's locale only"
((planted == 0)) || summary="$summary; $planted planted defects caught"
printf '%s\n' "$summary" >&2

((findings == 0)) || exit 1
exit 0
