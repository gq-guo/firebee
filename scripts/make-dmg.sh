#!/usr/bin/env sh
# 把 .app 打成 DMG（APFS + Applications 快捷方式）。
# Tauri 自带的 bundle_dmg.sh 用 HFS+，在 macOS 26 上 hdiutil 会报 "Resource busy"，所以自己做。
# 用法：scripts/make-dmg.sh <path/to/Firebee.app> <out.dmg>
set -eu
app="${1:?app path}"; out="${2:?output dmg}"
name="$(basename "$app" .app)"
stage="$(mktemp -d)"
trap 'rm -rf "$stage"' EXIT
cp -R "$app" "$stage/"
ln -s /Applications "$stage/Applications"
rm -f "$out"
hdiutil create -volname "$name" -srcfolder "$stage" -fs APFS -format UDZO -ov "$out" >/dev/null
echo "created $out ($(du -h "$out" | cut -f1))"
