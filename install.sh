#!/bin/sh
# ocgen installer / updater for macOS and Linux.
#   curl -fsSL https://raw.githubusercontent.com/OWNER/REPO/main/install.sh | sh
# Env overrides: OCGEN_REPO, OCGEN_VERSION (default: latest), OCGEN_INSTALL_DIR.
# Re-run any time to update to the latest release.
set -eu

REPO="${OCGEN_REPO:-OWNER/REPO}"
VERSION="${OCGEN_VERSION:-latest}"
INSTALL_DIR="${OCGEN_INSTALL_DIR:-$HOME/.local/bin}"

os="$(uname -s)"
arch="$(uname -m)"
case "$os" in
  Darwin) case "$arch" in
            arm64)  target=aarch64-apple-darwin ;;
            x86_64) target=x86_64-apple-darwin ;;
            *) echo "unsupported arch: $arch" >&2; exit 1 ;;
          esac ;;
  Linux)  case "$arch" in
            x86_64|amd64) target=x86_64-unknown-linux-musl ;;
            *) echo "unsupported arch: $arch" >&2; exit 1 ;;
          esac ;;
  *) echo "unsupported OS: $os" >&2; exit 1 ;;
esac

if [ "$VERSION" = latest ]; then
  tag="$(curl -fsSL "https://api.github.com/repos/$REPO/releases/latest" \
          | grep -o '"tag_name":[^,]*' | head -1 | sed 's/.*"\([^"]*\)"$/\1/')"
else
  tag="$VERSION"
fi
[ -n "$tag" ] || { echo "could not resolve a release for $REPO" >&2; exit 1; }

asset="ocgen-$tag-$target.tar.gz"
url="https://github.com/$REPO/releases/download/$tag/$asset"
tmp="$(mktemp -d)"
echo "Downloading $asset ..."
curl -fsSL "$url" -o "$tmp/$asset"
tar xzf "$tmp/$asset" -C "$tmp"
mkdir -p "$INSTALL_DIR"
cp "$tmp/ocgen" "$INSTALL_DIR/ocgen"
chmod +x "$INSTALL_DIR/ocgen"
rm -rf "$tmp"

echo "Installed ocgen $tag to $INSTALL_DIR"
case ":$PATH:" in
  *":$INSTALL_DIR:"*) ;;
  *) echo "Add $INSTALL_DIR to your PATH to run 'ocgen'." ;;
esac
"$INSTALL_DIR/ocgen" --version || true
