#!/usr/bin/env bash
# Needs bash 3.2, unsquashfs from squashfs-tools 4.4 or newer, and the
# appimage_squashfs_offset helper in lib/appimage-offset.sh
#
# The check reads the permissions straight from the squashfs and mounts nothing. Under the
# FUSE runtime of AppImage, the files look like the user's own to the process that started
# the image. A run on your own machine therefore passes even with broken permissions.
# The files break for other users: both firejail --appimage and the appimage.github.io
# catalog mount the image as root, but run it as an ordinary user
set -euo pipefail

usage() {
  cat <<'EOF'
check-appimage.sh — can any user read and run an AppImage

  check-appimage.sh FILE...    check the permissions of the files inside each image

A directory must be readable and enterable by everyone. An ordinary file must be
readable by everyone, and an executable file must also be runnable by everyone. Each
violation is printed as a line from the unsquashfs listing

Needs unsquashfs from squashfs-tools 4.4 or newer
No network is needed
Exit 0 the permissions are fine, 1 a violation was found, 2 a call error or a file is not an AppImage
EOF
}

fail() { # the image violates the permissions
  printf 'check-appimage.sh: %s\n' "$1" >&2
  exit 1
}

die() { # the request is wrong
  printf 'check-appimage.sh: %s\n' "$1" >&2
  exit 2
}

HERE=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)
# shellcheck source=lib/appimage-offset.sh
. "$HERE/lib/appimage-offset.sh"

violations=0

# Violations pile up in violations and not in the status of a function. A call in `if` or `||`
# would turn set -e off. A die from a substitution would then become a permission violation
# instead of a call error
check_one() {
  local file="$1" offset listing mode rest
  [[ -f "$file" ]] || die "$file: no such file"
  if ! offset=$(appimage_squashfs_offset "$file"); then
    exit 2
  fi
  listing=$(unsquashfs -lln -o "$offset" "$file") ||
    die "$file: unsquashfs could not read the squashfs at offset $offset"
  [[ -n "$listing" ]] || die "$file: the listing is empty, nothing to check"
  while read -r mode rest; do
    case "$mode" in
      d*)
        [[ "${mode:7:1}" == r && "${mode:9:1}" == x ]] && continue
        ;;
      -*)
        if [[ "${mode:7:1}" == r ]]; then
          case "${mode:1:9}" in
            *x*) [[ "${mode:9:1}" == x ]] && continue ;;
            *) continue ;;
          esac
        fi
        ;;
      *) continue ;;
    esac
    printf '%s: %s %s\n' "$file" "$mode" "$rest" >&2
    violations=$((violations + 1))
  done <<<"$listing"
}

(($# > 0)) || {
  usage >&2
  exit 2
}
case "$1" in
  -h | --help | help)
    usage
    exit 0
    ;;
  -*) die "no such flag: $1" ;;
esac

for file in "$@"; do
  check_one "$file"
done
((violations == 0)) || fail "violations: $violations, not every user can read or run the image"
