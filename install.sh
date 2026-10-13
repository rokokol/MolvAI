#!/usr/bin/env bash
# The shape of the script comes from the huix-standard skill
set -euo pipefail

here="$(cd -- "$(dirname -- "$(readlink -f "${BASH_SOURCE[0]}")")" && pwd)"

# The version lives in one place: Cargo.toml of the workspace. A second source
# would drift apart from the first, so here the version is read and not repeated
# The first line is taken from the value and not through `| head`: a reader that stops
# early leaves sed writing into a closed pipe, and under pipefail this SIGPIPE becomes
# the status of a read that succeeded
VERSION="$(sed -n '/^\[workspace\.package\]/,/^\[/s/^version *= *"\([^"]*\)".*/\1/p' \
  "$here/Cargo.toml")"
VERSION="${VERSION%%$'\n'*}"
if [[ -z "$VERSION" ]]; then
  echo "install.sh: cannot read the version from Cargo.toml — the parsing is broken, not the project" >&2
  exit 2
fi

PREFIX="${PREFIX:-$HOME/.local}"
DESTDIR="${DESTDIR:-}"
OS_RELEASE="${OS_RELEASE:-/etc/os-release}"

usage() {
  cat <<EOF
install MolvAI $VERSION into the user prefix

Installs MolvAI on a system without Nix. The script does exactly what \`nix develop\`
plus \`cargo build\` give: it puts the binary into the user prefix, and when asked, a
desktop file and an autostart entry, and it writes a manifest that --uninstall uses to remove everything

The script never installs dependencies: it collects the list of what is missing and prints
the exact commands for the distribution, and a person makes the decision

Each run brings the prefix to exactly the flags that are given: a run without a flag
removes what this flag installed last time

usage: ./install.sh [options]
  -h, --help          show this help and exit
  -v, --version       print the version and exit
  -c, --check         only check the dependencies, install nothing
      --prefix DIR    install prefix (now: $PREFIX; the PREFIX variable)
      --destdir DIR   staging root: files go to DESTDIR/PREFIX,
                      the live system is not touched (the DESTDIR variable)
      --desktop       install a desktop file into the application menu
      --autostart     start MolvAI at login
      --uninstall     remove everything the last run recorded, by the manifest
      --purge         with --uninstall: delete the settings and the phrase journal
      --keep-history  with --uninstall: keep the settings and the journal without asking

Administrator rights are not needed: by default everything goes to \$HOME/.local.
An install over an existing one keeps the settings and the journal, because they are outside the prefix

exit codes:
  0  done; this also covers help, version and a successful dependency check
  1  usage or environment error: unknown option, option without a value,
     relative --prefix, incompatible flags, missing dependencies, a build that gave
     no binary
  2  the version could not be read from Cargo.toml — the parsing is broken, not the project
EOF
}

# need_value OPTION VALUE... — refuse if nothing follows the option. `${2:?}` prints
# the message of bash itself and exits with code 1, but a missing argument is a usage
# error, and the script must report it in its own voice and with its own code
need_value() {
  [[ -n "${2:-}" ]] && return 0
  echo "install.sh: a directory is needed after $1" >&2
  usage >&2
  exit 1
}

UNINSTALL=0
CHECK_ONLY=0
DESKTOP=0
AUTOSTART=0
PURGE=0
KEEP_HISTORY=0
config_given=""

while [[ $# -gt 0 ]]; do
  case "$1" in
    -h | --help)
      usage
      exit 0
      ;;
    -v | --version)
      echo "MolvAI $VERSION"
      exit 0
      ;;
    -c | --check)
      CHECK_ONLY=1
      shift
      ;;
    --prefix)
      need_value "$@"
      PREFIX="$2"
      shift 2
      ;;
    --destdir)
      need_value "$@"
      DESTDIR="$2"
      shift 2
      ;;
    --desktop)
      DESKTOP=1
      config_given="$1"
      shift
      ;;
    --autostart)
      AUTOSTART=1
      config_given="$1"
      shift
      ;;
    --uninstall)
      UNINSTALL=1
      shift
      ;;
    --purge)
      PURGE=1
      shift
      ;;
    --keep-history)
      KEEP_HISTORY=1
      shift
      ;;
    *)
      echo "install.sh: unknown option: $1" >&2
      usage >&2
      exit 1
      ;;
  esac
done

if [[ "$PREFIX" != /* ]]; then
  echo "install.sh: PREFIX must be an absolute path: $PREFIX" >&2
  exit 1
fi
if ((UNINSTALL)) && [[ -n "$config_given" ]]; then
  echo "install.sh: --uninstall cannot be combined with $config_given" >&2
  exit 1
fi
if ((PURGE)) && ((KEEP_HISTORY)); then
  echo "install.sh: --purge and --keep-history exclude each other" >&2
  exit 1
fi

root="${DESTDIR%/}$PREFIX"
share_runtime="$PREFIX/share/molva"
share="${DESTDIR%/}$share_runtime"
manifest="$share/install-manifest"

config_dir="${XDG_CONFIG_HOME:-$HOME/.config}/molva"
data_dir="${XDG_DATA_HOME:-$HOME/.local/share}/molva"
autostart_file="${XDG_CONFIG_HOME:-$HOME/.config}/autostart/molva.desktop"

# --- manifest ---------------------------------------------------------------------------
# Each created path is recorded in its final form, without DESTDIR: the manifest travels
# inside the staging tree and stays correct wherever this tree ends up

old_paths=()
if [[ -f "$manifest" ]]; then
  mapfile -t old_paths < <(grep -v '^#' "$manifest")
fi

installed=()

put() { # put MODE SOURCE FINAL_PATH
  install -D -m "$1" "$2" "${DESTDIR%/}$3"
  installed+=("$3")
}

# write_file MODE FINAL_PATH CONTENT. The content is passed as an argument and not through
# a pipeline: the right side of a pipeline is a subshell, and the manifest entry from it was lost
write_file() {
  install -d "$(dirname "${DESTDIR%/}$2")"
  printf '%s\n' "$3" >"${DESTDIR%/}$2"
  chmod "$1" "${DESTDIR%/}$2"
  installed+=("$2")
}

prune() { # remove parent directories that became empty, without leaving the prefix
  local dir stop
  dir="$(dirname "${DESTDIR%/}$1")"
  stop="$root"
  while [[ "$dir" == "$stop"/* ]]; do
    rmdir "$dir" 2>/dev/null || break
    dir="$(dirname "$dir")"
  done
}

# --- uninstall --------------------------------------------------------------------------

if ((UNINSTALL)); then
  if [[ ! -f "$manifest" ]]; then
    echo "MolvAI: nothing to remove in $root — there is no install manifest"
    exit 0
  fi

  # The daemon holds the hotkey; while it is alive, removing the files is pointless
  if [[ -z "$DESTDIR" ]] && command -v molva >/dev/null 2>&1; then
    molva quit >/dev/null 2>&1 || true
  fi

  removed=()
  while IFS= read -r path; do
    [[ -z "$path" || "$path" == \#* ]] && continue
    if [[ -e "${DESTDIR%/}$path" || -L "${DESTDIR%/}$path" ]]; then
      removed+=("$path")
    fi
    rm -f "${DESTDIR%/}$path"
    prune "$path"
  done <"$manifest"
  rm -f "$manifest"
  rmdir "$share" 2>/dev/null || true

  echo "MolvAI $VERSION removed from $root"
  echo "Removed components:"
  if ((${#removed[@]})); then
    printf '  - %s\n' "${removed[@]}"
  else
    echo "  - nothing: the files were already absent"
  fi

  # A staging tree has no live state: the settings and the journal do not belong to it
  if [[ -n "$DESTDIR" ]]; then
    exit 0
  fi

  # The settings and the journal live outside the prefix: the user decides their fate, not the manifest
  history_paths=()
  [[ -e "$config_dir" ]] && history_paths+=("$config_dir")
  [[ -e "$data_dir" ]] && history_paths+=("$data_dir")

  if ((${#history_paths[@]} == 0)); then
    echo "No settings or journal found — nothing to remove"
    exit 0
  fi

  if ((KEEP_HISTORY)); then
    echo "Kept (--keep-history):"
    printf '  - %s\n' "${history_paths[@]}"
    exit 0
  fi

  if ! ((PURGE)); then
    echo
    echo "The settings and the phrase journal remain:"
    printf '  - %s\n' "${history_paths[@]}"
    if [[ -t 0 ]]; then
      read -r -p "Remove them too? [y/N] " answer
      case "$answer" in
        [yY] | [yY][eE][sS]) PURGE=1 ;;
      esac
    else
      echo "No input available, the history is kept: repeat with --purge or --keep-history"
      exit 0
    fi
  fi

  if ((PURGE)); then
    rm -rf "${history_paths[@]}"
    echo "Removed:"
    printf '  - %s\n' "${history_paths[@]}"
  else
    echo "Kept:"
    printf '  - %s\n' "${history_paths[@]}"
  fi
  exit 0
fi

# --- dependency check: refuse loudly and install nothing ---------------------------------
# Required ones: the install is impossible without them. Session ones: the user's work
# session provides them, one warning each, and the install goes on

missing=()
absent=()

need() { command -v "$1" >/dev/null 2>&1 || missing+=("$1"); }
want() { command -v "$1" >/dev/null 2>&1 || absent+=("$1"); }

need install
need sed

binary="$here/target/release/molva"
if [[ ! -x "$binary" ]]; then
  need cargo
  need cc
fi

# The way to paste text depends on the session type, so the check follows it
case "${XDG_SESSION_TYPE:-}" in
  wayland)
    want wtype
    want wl-copy
    ;;
  x11)
    want xdotool
    want xclip
    ;;
esac

distro_id() {
  sed -n 's/^ID\(_LIKE\)\?=//p' "$OS_RELEASE" 2>/dev/null | tr -d '"' | tr '\n' ' '
}

guidance() {
  # One recommended way for each system. The lines to run are printed as `  $ command`
  # — two spaces, a dollar sign, a space: both a person and a test read this format
  case "$(uname -s)" in
    Darwin)
      echo "Install them on macOS:"
      echo '  $ brew install cmake pkg-config'
      echo "  plus the Xcode command line tools: xcode-select --install"
      return
      ;;
    MINGW* | MSYS* | CYGWIN*)
      echo "Install them on Windows:"
      echo '  $ winget install Rustlang.Rustup Kitware.CMake'
      echo "  plus Build Tools for Visual Studio with the C++ workload"
      return
      ;;
  esac
  case " $(distro_id) " in
    *" nixos "*)
      echo "On NixOS the flake of the repository gives everything needed, and nothing must be installed in the system:"
      echo '  $ nix develop --command ./install.sh'
      ;;
    *" arch "*)
      echo "Install them on Arch:"
      echo '  $ sudo pacman -S --needed base-devel cmake pkgconf alsa-lib libxkbcommon rustup'
      echo "For Wayland, also:"
      echo '  $ sudo pacman -S --needed wtype wl-clipboard'
      ;;
    *" debian "* | *" ubuntu "*)
      echo "Install them on Debian or Ubuntu:"
      echo '  $ sudo apt install build-essential cmake pkg-config libasound2-dev libxkbcommon-dev rustup'
      echo "For Wayland, also:"
      echo '  $ sudo apt install wtype wl-clipboard'
      ;;
    *" fedora "* | *" rhel "*)
      echo "Install them on Fedora:"
      echo '  $ sudo dnf install gcc-c++ cmake pkgconf-pkg-config alsa-lib-devel libxkbcommon-devel rustup'
      echo "For Wayland, also:"
      echo '  $ sudo dnf install wtype wl-clipboard'
      ;;
    *" suse "* | *" opensuse "*)
      echo "Install them on openSUSE:"
      echo '  $ sudo zypper install gcc-c++ cmake pkg-config alsa-devel libxkbcommon-devel rustup'
      echo "For Wayland, also:"
      echo '  $ sudo zypper install wtype wl-clipboard'
      ;;
    *)
      echo "Install them with the package manager of your system:"
      echo "  a C and C++ compiler, cmake, pkg-config, ALSA headers, rustup"
      echo "  https://rustup.rs"
      ;;
  esac
}

if ((${#missing[@]})); then
  {
    echo "install.sh: dependencies are missing:"
    printf '  - %s\n' "${missing[@]}"
    echo
    guidance
  } >&2
  exit 1
fi

if ((${#absent[@]})); then
  printf 'install.sh: not found (the work session provides it, the install goes on): %s\n' \
    "${absent[@]}" >&2
  echo "install.sh: without them paste falls back to the clipboard — a working mode, but not the best one" >&2
fi

if ((CHECK_ONLY)); then
  echo "install.sh: the required dependencies are present, the install is possible"
  exit 0
fi

# --- install ----------------------------------------------------------------------------

if [[ ! -x "$binary" ]]; then
  echo "install.sh: building MolvAI $VERSION (this takes long the first time)"
  (cd "$here" && cargo build --locked --release --bin molva)
fi
if [[ ! -x "$binary" ]]; then
  echo "install.sh: the build did not give $binary" >&2
  exit 1
fi

put 755 "$binary" "$PREFIX/bin/molva"
put 644 "$here/LICENSE" "$share_runtime/LICENSE"
put 644 "$here/NOTICE" "$share_runtime/NOTICE"
put 644 "$here/THIRD-PARTY.md" "$share_runtime/THIRD-PARTY.md"

desktop_entry() {
  cat <<EOF
[Desktop Entry]
Type=Application
Name=MolvAI
Comment=System-wide voice input
Exec=$PREFIX/bin/molva daemon
Terminal=false
Categories=Utility;Accessibility;
X-GNOME-Autostart-enabled=true
EOF
}

if ((DESKTOP)) || ((AUTOSTART)); then
  write_file 644 "$PREFIX/share/applications/molva.desktop" "$(desktop_entry)"
fi

if ((AUTOSTART)); then
  if [[ -n "$DESTDIR" ]]; then
    echo "install.sh: --autostart skipped: with --destdir the live state is not touched" >&2
  else
    write_file 644 "$autostart_file" "$(desktop_entry)"
  fi
fi

# The declarative meaning of the flags: what the last run installed and this one did not is removed
for path in "${old_paths[@]}"; do
  keep=0
  for now in "${installed[@]}"; do
    [[ "$path" == "$now" ]] && keep=1
  done
  ((keep)) || {
    rm -f "${DESTDIR%/}$path"
    prune "$path"
  }
done

{
  echo "# MolvAI $VERSION install manifest"
  printf '%s\n' "${installed[@]}"
} >"$manifest"

echo "MolvAI $VERSION installed in $root"
printf '  - %s\n' "${installed[@]}"
echo "Manifest: $manifest"

case ":$PATH:" in
  *":$PREFIX/bin:"*) ;;
  *)
    echo
    echo "install.sh: $PREFIX/bin is not in PATH. Add this to the file of your shell:"
    echo "  export PATH=\"$PREFIX/bin:\$PATH\""
    ;;
esac

echo
echo "Next: molva model download small && molva devices && molva test-inject"
echo "Uninstall: $here/install.sh --uninstall"
