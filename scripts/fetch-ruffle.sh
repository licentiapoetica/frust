#!/bin/sh
# Downloads the pinned Ruffle web build (Flash emulator, served at /s/ruffle/) into public/s/ruffle.
# The tarball is the npm release of @ruffle-rs/ruffle; its SHA-256 is checked before anything is
# unpacked. To update: set RUFFLE_VERSION and RUFFLE_SHA256 here and in the Dockerfile
# (sha256sum of https://registry.npmjs.org/@ruffle-rs/ruffle/-/ruffle-<version>.tgz).
#
#   scripts/fetch-ruffle.sh [target dir]      (default: public/s/ruffle)
set -eu

RUFFLE_VERSION="${RUFFLE_VERSION:-0.2.0-nightly.2026.4.21}"
RUFFLE_SHA256="${RUFFLE_SHA256:-44a803178047324e35b7a85a7ddf37da8a66a93a840e9f57f83ce74f8e83c6df}"
RUFFLE_URL="${RUFFLE_URL:-https://registry.npmjs.org/@ruffle-rs/ruffle/-/ruffle-${RUFFLE_VERSION}.tgz}"
dest="${1:-public/s/ruffle}"

tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT INT TERM

curl -fsSL --proto '=https,file' -o "$tmp/ruffle.tgz" "$RUFFLE_URL"
echo "$RUFFLE_SHA256  $tmp/ruffle.tgz" | sha256sum -c - >/dev/null
mkdir "$tmp/x"
tar xzf "$tmp/ruffle.tgz" -C "$tmp/x" --no-same-owner

# Only what the browser loads (+ licenses); source maps are left out
rm -rf "$dest.new"
mkdir -p "$dest.new"
cp "$tmp/x/package/"*.js "$tmp/x/package/"*.wasm "$tmp/x/package/LICENSE_"* "$tmp/x/package/package.json" "$dest.new/"
rm -rf "$dest"
mv "$dest.new" "$dest"
echo "Ruffle $RUFFLE_VERSION -> $dest"
