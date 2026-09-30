#!/usr/bin/env sh
# 从 GitHub Release 下载 Firebee 并装到 /Applications。
# curl 下载的文件不带 com.apple.quarantine，所以未公证的包也不会被 Gatekeeper 拦。
# 用法：curl -fsSL https://raw.githubusercontent.com/gq-guo/firebee/master/scripts/install.sh | sh
# 指定版本：... | FIREBEE_VERSION=v0.2.0 sh
set -eu

# 整个脚本包在函数里、最后一行才调用：curl | sh 中途断线时 sh 只读到半截函数定义，什么都不会执行
main() {
  repo="gq-guo/firebee"
  dest="/Applications"

  [ "$(uname -s)" = Darwin ] || { echo "Firebee 只支持 macOS" >&2; exit 1; }

  tag="${FIREBEE_VERSION:-}"
  if [ -z "$tag" ]; then
    # releases/latest 会 302 到 /releases/tag/<tag>，取最后一段
    tag="$(curl -fsSLo /dev/null -w '%{url_effective}' "https://github.com/$repo/releases/latest")"
    tag="${tag##*/}"
  fi
  echo "$tag" | grep -Eq '^v[0-9]+\.[0-9]+\.[0-9]+$' || { echo "无效的版本号：$tag" >&2; exit 1; }

  zip="Firebee-$tag-macos-universal.app.zip"
  base="https://github.com/$repo/releases/download/$tag"
  tmp="$(mktemp -d)"
  trap 'rm -rf "$tmp"' EXIT

  echo "下载 Firebee $tag ..."
  curl -fL --progress-bar -o "$tmp/$zip" "$base/$zip"
  curl -fsSL -o "$tmp/SHA256SUMS.txt" "$base/SHA256SUMS.txt"
  (cd "$tmp" && grep " $zip\$" SHA256SUMS.txt | shasum -a 256 -c -) >/dev/null \
    || { echo "SHA256 校验失败" >&2; exit 1; }

  ditto -x -k "$tmp/$zip" "$tmp/out"
  sudo=""
  [ -w "$dest" ] || sudo="sudo"
  # 先把新版放到旁边再换，mv 失败（取消 sudo、磁盘满）时旧版还在
  $sudo rm -rf "$dest/.Firebee.app.new"
  $sudo mv "$tmp/out/Firebee.app" "$dest/.Firebee.app.new"
  $sudo rm -rf "$dest/Firebee.app"
  $sudo mv "$dest/.Firebee.app.new" "$dest/Firebee.app"
  echo "已安装到 $dest/Firebee.app"
}

main "$@"
