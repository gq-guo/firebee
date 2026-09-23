# Firebee

用 Rust + Tauri 编写的轻量桌面 API client（精简版 Postman）。设计目标：快、稳定、克制。

后端是 Rust（存储 / HTTP / 变量替换 / 导出），前端是零依赖的 HTML/CSS/JS（无 npm 工具链）。

## 功能

**请求**
- 全部常用方法；Params / Headers / Body（JSON、Text、Form）/ Auth（Bearer、Basic、API Key）
- Params / Headers 表格末尾常驻空白行，直接输入即新增；Header 名与 Content-Type / Accept 值自动补全；JSON body 一键格式化并定位错误
- 可取消、超时可配；多个请求可同时在飞，响应按请求保留（切换不丢，内存中最多 50 条）
- Cookie 在会话内自动保持，登录后可连着调会话接口；Environment › Manage… 里可清除

**响应**
- 状态码与原因短语、耗时、大小、响应头；JSON 语法高亮
- JSONPath 过滤：`$.data.orders.*.app.key`、`$.data.orders.2.app.key`、`$..key`
- 体内查找（⌘F，Enter / Shift+Enter 跳转）；可折叠树视图；超大响应先截断再按需全量
- HTML 沙箱预览、图片预览、保存到文件

**变量与环境**
- 多环境切换，`{{variable}}` 在 URL / 参数 / 头 / 体 / 认证中替换；URL 里的变量实时着色（可解析绿、未解析橙），Send 旁显示未解析数量，再点一次强制发送
- 输入 `{{` 自动补全环境变量与动态变量
- 动态变量：`{{$uuid}}`、`{{$timestamp}}`、`{{$isoTimestamp}}`、`{{$randomInt}}`

**集合与历史**
- 文件夹嵌套；右键 / `⋯` 菜单重命名、复制、删除，删除可撤销
- 拖拽排序与跨文件夹移动；⌘点击多选，批量删除
- 请求可 Pin，置顶到侧栏 Pinned 区，常用接口不用再翻文件夹
- 侧栏搜索：按集合 / 文件夹 / 请求名和 URL 过滤（⌘F 聚焦）
- 历史最近 500 条，点击回填；可删单条或清空（可撤销）
- 导入：把 curl 粘贴到 URL 框即覆盖到当前请求（保留名字）；集合菜单可导入为新请求；文件导入 Firebee 导出 / Postman Collection v2.x / Postman Environment
- 导出：curl（JSON 压成一行，方便粘贴终端）、Python (requests)；集合导出为 Firebee JSON

**界面**
- 三栏布局，分隔条可拖动，双击恢复；快捷键在菜单栏 Request 菜单可见：⌘↩ 发送、⌘N 新请求、⌘F 查找、⇧⌘F 过滤集合
- 没有手动保存：每个请求都在集合里，任何改动防抖写入本地 JSON（原子写入，损坏自动备份）

## 运行

```bash
cargo tauri dev        # 开发（需要 cargo install tauri-cli）
cargo run --release    # 直接运行（前端资源已编译进二进制）
```

## 测试

```bash
cargo test          # 36 个单元/集成测试（core 层全覆盖，http 用 wiremock）
cargo clippy --all-targets -- -D warnings
```

## 发布

版本号只维护在 `VERSION`，`Cargo.toml` 与 `tauri.conf.json` 的 `version` 必须与之一致（`scripts/check-version.sh` 会校验，CI 也会跑）。

```bash
# 1. 升版本：改 VERSION、Cargo.toml、tauri.conf.json 三处为同一个号，例如 0.1.1
scripts/check-version.sh            # 本地校验
# 2. 提交后打 tag 推送，Release 工作流自动打包
git tag v0.1.1 && git push origin v0.1.1
```

Release 工作流（`.github/workflows/release.yml`）先校验 tag 与 `VERSION` 一致，再在 macOS runner 上构建 universal（Intel + Apple Silicon）的 `.dmg` / `.app.zip`，附 `SHA256SUMS.txt` 发布到 GitHub Release。tag 与版本号不一致时直接失败，不会产出包。

本地打包：`cargo tauri build --target universal-apple-darwin` 得到 `.app`，再 `scripts/make-dmg.sh target/universal-apple-darwin/release/bundle/macos/Firebee.app Firebee.dmg` 生成 DMG（APFS，避开 macOS 26 上 HFS+ 的 hdiutil 问题）。包未经 Apple 公证，首次打开需 `xattr -dr com.apple.quarantine /Applications/Firebee.app` 或右键 → 打开。

## 架构

```
src/
  core/         models · storage · vars · http · export · import · postman   （不依赖 UI，全部可单测）
  commands.rs   Tauri commands：load/save 数据、send/cancel 请求（tokio 异步，可取消）、变量检查、导出
  lib.rs        注册 commands 与托管状态；main.rs 只做日志初始化
ui/
  index.html · style.css · app.js   全部 UI 状态在前端；数据形态与 core/models.rs 的 serde 格式一致
tauri.conf.json · capabilities/ · build.rs · icons/
```

前端选中集合中的请求后直接引用该对象，编辑即写回集合并防抖保存（egui 版编辑的是副本，不会回写）。

## 数据位置

macOS: `~/Library/Application Support/firebee/`
- `collections.json` / `environments.json` / `history.json`（原子写入，损坏自动备份为 .bak）
- `firebee.log.*`（运行日志）

## 设计文档

- `docs/superpowers/specs/2025-09-16-firebee-api-client-design.md`
- `docs/superpowers/plans/2025-09-16-firebee-api-client.md`
