#!/usr/bin/env sh
# 校验版本号一致：VERSION · Cargo.toml · tauri.conf.json · Cargo.lock ·（可选）git tag
# 用法：scripts/check-version.sh [vX.Y.Z]
set -eu
cd "$(dirname "$0")/.."

fail() { echo "::error::$1" >&2; exit 1; }

ver="$(tr -d ' \r\n' < VERSION)"
# 用真正的解析器读，别用 grep/sed：tauri.conf.json 里嵌套层级也可能出现 "version" 键，
# Cargo.toml 里 [workspace.package] 或依赖表也可能排在 [package] 前面。
cargo_ver="$(python3 -c '
import re, sys
s = open("Cargo.toml").read()
m = re.search(r"^\[package\]$(.*?)(?=^\[|\Z)", s, re.M | re.S)
if not m: sys.exit("no [package] section in Cargo.toml")
v = re.search(r"^version\s*=\s*\"([^\"]*)\"", m.group(1), re.M)
if not v: sys.exit("no version in [package]")
print(v.group(1))
')"
conf_ver="$(python3 -c 'import json; print(json.load(open("tauri.conf.json"))["version"])')"
lock_ver="$(python3 -c '
import re, sys
s = open("Cargo.lock").read()
m = re.search(r"^\[\[package\]\]\nname = \"firebee\"\nversion = \"([^\"]*)\"", s, re.M)
if not m: sys.exit("no firebee package in Cargo.lock")
print(m.group(1))
')"

# 严格 X.Y.Z。shell 的 case 是 glob 不是正则，"9zzz.8yy.7xx-QQQ" 能骗过 [0-9]*.[0-9]*.[0-9]*
printf '%s' "$ver" | grep -qE '^[0-9]+\.[0-9]+\.[0-9]+$' \
  || fail "VERSION must be X.Y.Z, got '$ver'"

[ "$cargo_ver" = "$ver" ] || fail "Cargo.toml version ($cargo_ver) != VERSION ($ver)"
[ "$conf_ver" = "$ver" ] || fail "tauri.conf.json version ($conf_ver) != VERSION ($ver)"
[ "$lock_ver" = "$ver" ] || fail "Cargo.lock firebee version ($lock_ver) != VERSION ($ver) — run 'cargo update -p firebee'"

if [ "${1:-}" != "" ]; then
  case "$1" in
    v*) ;;
    *) fail "Release tag must look like vX.Y.Z, got '$1'" ;;
  esac
  tag="${1#v}"
  [ "$tag" = "$ver" ] || fail "Release tag v$tag != VERSION ($ver) — bump VERSION, Cargo.toml, tauri.conf.json and Cargo.lock first"
fi
echo "version ok: $ver"
