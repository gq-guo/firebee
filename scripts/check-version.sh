#!/usr/bin/env sh
# 校验版本号一致：VERSION · Cargo.toml · tauri.conf.json ·（可选）git tag
# 用法：scripts/check-version.sh [vX.Y.Z]
set -eu
cd "$(dirname "$0")/.."

ver="$(tr -d ' \r\n' < VERSION)"
cargo_ver="$(sed -n 's/^version *= *"\([^"]*\)".*/\1/p' Cargo.toml | head -1)"
conf_ver="$(sed -n 's/^ *"version": *"\([^"]*\)".*/\1/p' tauri.conf.json | head -1)"

fail() { echo "::error::$1" >&2; exit 1; }

case "$ver" in
  [0-9]*.[0-9]*.[0-9]*) ;;
  *) fail "VERSION must be X.Y.Z, got '$ver'" ;;
esac
[ "$cargo_ver" = "$ver" ] || fail "Cargo.toml version ($cargo_ver) != VERSION ($ver)"
[ "$conf_ver" = "$ver" ] || fail "tauri.conf.json version ($conf_ver) != VERSION ($ver)"

if [ "${1:-}" != "" ]; then
  tag="${1#v}"
  [ "$tag" = "$ver" ] || fail "Release tag v$tag != VERSION ($ver) — bump VERSION, Cargo.toml and tauri.conf.json first"
fi
echo "version ok: $ver"
