#!/bin/sh
# CI universal 打包用：tauri-cli 只给主程序 firebee 做 lipo，firebee-cli 这个第二 bin 不会合并，
# 打包阶段去 universal 目录拷它就报 does not exist。这里在 cargo 编完、打包前补上。
# 主程序刚编过，cargo 复用产物，只多编 CLI 本身。
set -eu
cargo build --release --bin firebee-cli --target aarch64-apple-darwin --target x86_64-apple-darwin
mkdir -p target/universal-apple-darwin/release
lipo -create -output target/universal-apple-darwin/release/firebee-cli \
  target/aarch64-apple-darwin/release/firebee-cli \
  target/x86_64-apple-darwin/release/firebee-cli
