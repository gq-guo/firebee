# 发布流程

版本号只维护在 `VERSION`，另外三处必须与之一致：`Cargo.toml` 的 `[package] version`、`tauri.conf.json` 的 `version`、`Cargo.lock` 里 `firebee` 包的 `version`。`scripts/check-version.sh` 会校验这四处（用真正的 JSON/TOML 解析，不是 grep），Release 工作流第一步就跑它。

```bash
# 1. 升版本：改 VERSION、Cargo.toml、tauri.conf.json，再同步 lock
vim VERSION Cargo.toml tauri.conf.json
cargo update -p firebee             # 同步 Cargo.lock
scripts/check-version.sh            # 本地校验四处一致
# 2. 提交合并到 master 后打 tag 推送，Release 工作流自动打包
git tag v0.1.3 && git push origin v0.1.3
```

Release 工作流（`.github/workflows/release.yml`）的 `gate` 作业先校验版本号，再跑 `fmt` / `clippy` / `test`——tag 推送不触发 `ci.yml`（它只在 push master 和 PR 上跑），所以门禁必须在这里重跑一遍，否则测试挂了照样能发版。通过后才在 macOS runner 上构建 universal（Intel + Apple Silicon）的 `.dmg` / `.app.zip`，附 `SHA256SUMS.txt` 发布到 GitHub Release。tag 与版本号不一致时直接失败，不会产出包。

## App 内更新

app 从 `releases/latest/download/latest.json` 检查新版本，下载 `Firebee-<tag>-macos-universal.app.tar.gz` 后用 `tauri.conf.json` 里 `plugins.updater.pubkey` 验签，验不过就拒绝安装。这套签名是 Tauri 自己的 minisign 密钥，跟 Apple 签名无关，也不花钱。

一次性配置：

```bash
cargo tauri signer generate -w ~/.tauri/firebee.key   # 私钥自己保管好，丢了老用户就收不到更新
```

- 公钥（`~/.tauri/firebee.key.pub` 的内容）填进 `tauri.conf.json` 的 `plugins.updater.pubkey`；还是占位符时 Release 工作流的 `gate` 会直接失败
- 私钥内容存为仓库 Secret `TAURI_SIGNING_PRIVATE_KEY`，设了密码再加 `TAURI_SIGNING_PRIVATE_KEY_PASSWORD`

Release 工作流只在 CI 里开 `createUpdaterArtifacts`，产出签名的 tar.gz，再用 GitHub 自动生成的 release notes 拼出 `latest.json`（弹窗里显示的更新说明就是它）。本地 `cargo tauri build` 不产出更新包，也不需要私钥。

`scripts/install.sh` 依赖 Release 产物的命名 `Firebee-<tag>-macos-universal.app.zip` 和 `SHA256SUMS.txt`，改打包步骤时别改名。

## 本地打包

```bash
cargo tauri build --target universal-apple-darwin
scripts/make-dmg.sh target/universal-apple-darwin/release/bundle/macos/Firebee.app Firebee.dmg
```

DMG 用 APFS，避开 macOS 26 上 HFS+ 的 hdiutil 问题。包未经 Apple 公证：用 `install.sh` 安装不需要额外操作；浏览器下载的 DMG 首次打开前需 `xattr -dr com.apple.quarantine /Applications/Firebee.app`。
