#!/bin/sh
# ocgen installer / updater for macOS and Linux.
#   curl -fsSL https://raw.githubusercontent.com/Konstankino/ocgen/main/install.sh | sh
# Env overrides: OCGEN_REPO, OCGEN_VERSION (default: latest), OCGEN_INSTALL_DIR.
# Re-run any time to update to the latest release.
#
# The archive is checked against the release's SHA256SUMS before anything is
# installed (needs sha256sum or shasum); a release from before checksums were
# published installs with a warning. OCGEN_INSTALL_SKIP_VERIFY=1 installs
# without the check, e.g. on a machine with neither tool.
#
# The binary is replaced by a rename, never rewritten in place, so updating
# works while ocgen runs (the notes viewer, a hook): running processes keep the
# old file.
set -eu

REPO="${OCGEN_REPO:-Konstankino/ocgen}"
VERSION="${OCGEN_VERSION:-latest}"
INSTALL_DIR="${OCGEN_INSTALL_DIR:-$HOME/.local/bin}"
tmp=""
new=""

say() { printf '%s\n' "$*"; }
die() { printf '%s\n' "$*" >&2; exit 1; }

cleanup() {
  [ -z "$tmp" ] || rm -rf "$tmp"
  [ -z "$new" ] || rm -f "$new"
}

# The release target for this machine (OS from `uname -s`, CPU from `uname -m`).
detect_target() {
  os="$(uname -s)"
  arch="$(uname -m)"
  case "$os" in
    Darwin)
      # A shell under Rosetta reports x86_64 on Apple silicon: install the
      # native build.
      if [ "$arch" = x86_64 ] && [ "$(sysctl -n sysctl.proc_translated 2>/dev/null || true)" = 1 ]; then
        arch=arm64
      fi
      case "$arch" in
        arm64|aarch64) echo aarch64-apple-darwin ;;
        x86_64)        echo x86_64-apple-darwin ;;
        *) die "unsupported arch: $arch" ;;
      esac ;;
    Linux)
      case "$arch" in
        x86_64|amd64)  echo x86_64-unknown-linux-musl ;;
        aarch64|arm64) echo aarch64-unknown-linux-musl ;;
        *) die "unsupported arch: $arch" ;;
      esac ;;
    *) die "unsupported OS: $os" ;;
  esac
}

# The SHA-256 of file $1 (lowercase hex), with whichever tool this system has.
sha256_of() {
  if command -v sha256sum >/dev/null 2>&1; then
    sha256sum "$1" | awk '{ print tolower($1) }'
  elif command -v shasum >/dev/null 2>&1; then
    shasum -a 256 "$1" | awk '{ print tolower($1) }'
  else
    return 1
  fi
}

# The checksum SHA256SUMS file $1 lists for asset $2 (empty when none).
listed_sum() {
  awk -v f="$2" '$2 == f || $2 == "*" f { print tolower($1); exit }' "$1"
}

# Check archive $1 (named $2 in the release) against the release's SHA256SUMS
# at $3.
verify() {
  if [ "${OCGEN_INSTALL_SKIP_VERIFY:-}" = 1 ]; then
    say "Skipping the checksum check (OCGEN_INSTALL_SKIP_VERIFY=1)."
    return 0
  fi
  if ! command -v sha256sum >/dev/null 2>&1 && ! command -v shasum >/dev/null 2>&1; then
    die "Cannot verify $2: neither sha256sum nor shasum is installed.
Install one (coreutils or perl), or set OCGEN_INSTALL_SKIP_VERIFY=1 to install without the check."
  fi
  sums="$(dirname "$1")/SHA256SUMS"
  code="$(curl -sSL "$3" -o "$sums" -w '%{http_code}')" || code=000
  if [ "$code" = 404 ]; then
    # Releases before v0.5.0 shipped without checksums.
    say "Warning: this release has no SHA256SUMS (it predates checksums) - installing $2 unchecked."
    return 0
  fi
  [ "$code" = 200 ] || die "Cannot verify $2: could not download $3 (HTTP $code).
Try again, or set OCGEN_INSTALL_SKIP_VERIFY=1 to install it without the check."
  want="$(listed_sum "$sums" "$2")"
  [ -n "$want" ] || die "Cannot verify $2: SHA256SUMS does not list it."
  got="$(sha256_of "$1")"
  [ "$got" = "$want" ] || die "Checksum mismatch for $2 - not installing.
  expected $want
  got      $got"
  say "Checksum OK ($2)."
}

# Put binary $1 at $2 by renaming a copy made next to it: atomic, and a running
# ocgen keeps its old file instead of failing with "text file busy".
install_binary() {
  new="$(dirname "$2")/.ocgen.new.$$"
  cp "$1" "$new"
  chmod +x "$new"
  mv -f "$new" "$2"
  new=""
}

main() {
  target="$(detect_target)"
  if [ "$VERSION" = latest ]; then
    tag="$(curl -fsSL "https://api.github.com/repos/$REPO/releases/latest" \
            | grep -o '"tag_name":[^,]*' | head -1 | sed 's/.*"\([^"]*\)"$/\1/')"
  else
    tag="$VERSION"
  fi
  [ -n "$tag" ] || die "could not resolve a release for $REPO"

  asset="ocgen-$tag-$target.tar.gz"
  base="https://github.com/$REPO/releases/download/$tag"
  tmp="$(mktemp -d)"
  trap cleanup EXIT
  trap 'exit 130' INT
  trap 'exit 143' TERM
  say "Downloading $asset ..."
  curl -fsSL "$base/$asset" -o "$tmp/$asset"
  verify "$tmp/$asset" "$asset" "$base/SHA256SUMS"
  tar xzf "$tmp/$asset" -C "$tmp"
  [ -f "$tmp/ocgen" ] || die "ocgen not found in $asset"
  mkdir -p "$INSTALL_DIR"
  install_binary "$tmp/ocgen" "$INSTALL_DIR/ocgen"

  say "Installed ocgen $tag to $INSTALL_DIR"
  case ":$PATH:" in
    *":$INSTALL_DIR:"*) ;;
    *) say "Add $INSTALL_DIR to your PATH to run 'ocgen'." ;;
  esac
  "$INSTALL_DIR/ocgen" --version || true
}

# Everything above only defines functions: a download cut short runs nothing.
main "$@"
