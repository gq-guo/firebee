# Firebee

[![CI](https://github.com/gq-guo/firebee/actions/workflows/ci.yml/badge.svg)](https://github.com/gq-guo/firebee/actions/workflows/ci.yml)
[![Release](https://img.shields.io/github/v/release/gq-guo/firebee)](https://github.com/gq-guo/firebee/releases/latest)
[![License: 0BSD](https://img.shields.io/badge/license-0BSD-blue)](LICENSE)

轻量、快速的桌面 API client，一个精简版 Postman。用 Rust + Tauri 编写，不用登录、不上云，数据全部存在本地 JSON。

- **轻**：安装包约 10 MB，启动即用；前端是零依赖的 HTML/CSS/JS，没有 npm 工具链
- **离线**：没有账号、没有同步、没有遥测，集合和环境都是本机上的普通文件
- **兼容 Postman**：直接导入 Postman Collection v2.x / Environment，动态变量名字一致
- **稳**：自动保存采用原子写入，文件损坏时自动备份；core 层有完整单测

> 目前只发布 macOS 版本（Intel + Apple Silicon universal）。Tauri 本身跨平台，欢迎提 PR 补上 Windows / Linux 的打包。

## 安装（macOS）

```bash
curl -fsSL https://raw.githubusercontent.com/gq-guo/firebee/master/scripts/install.sh | sh
```

脚本会下载最新 Release，校验 SHA256 后装到 `/Applications`。包没有经过 Apple 公证，但 curl 下载的文件不带 quarantine 标记，所以打开时不会被 Gatekeeper 拦截。也可以在 [Releases](https://github.com/gq-guo/firebee/releases) 手动下载 `.dmg`，拖进 Applications 后执行一次 `xattr -dr com.apple.quarantine /Applications/Firebee.app`。

## 功能

**请求**
- 全部常用方法；Params / Headers / Body（JSON、Text、Form）/ Auth（Bearer、Basic、API Key）
- GraphQL：新建时选 New GraphQL request，Query + Variables（JSON）编辑，按 `{"query","variables"}` POST；Postman 的 GraphQL body 可导入
- Params / Headers 表格末尾常驻空白行，直接输入即新增；Header 名与 Content-Type / Accept 值自动补全；JSON body 一键格式化并定位错误
- 可取消、超时可配；多个请求可同时在飞，响应按请求保留（切换不丢，内存中最多 50 条）
- Capture：2xx 响应后按 JSONPath 把值写进当前环境变量（`token` ← `$.data.token`），登录拿 token 不用手动复制
- Cookie 在会话内自动保持，登录后可连着调会话接口；Environment › Manage… 里可清除
- 重定向：顶栏可关；开着时也只跟同一 host（换 host 必停）。跟过的每一跳都在响应区列出来，停下来时显示 Location 并可一键填进 URL。reqwest 换 host 时只剥 Authorization / Cookie，`X-API-Key` 这类自定义头会原样发过去，被控制的接口一个 302 就能取走密钥

**响应**
- 状态码与原因短语、耗时、大小、响应头；JSON 语法高亮
- JSONPath 过滤：`$.data.orders.*.app.key`、`$.data.orders.2.app.key`、`$..key`
- 体内查找（⌘F，Enter / Shift+Enter 跳转）；可折叠树视图；超大响应先截断再按需全量
- HTML 沙箱预览、图片预览、保存到文件

**变量与环境**
- 多环境切换，`{{variable}}` 在 URL / 参数 / 头 / 体 / 认证中替换；URL 里的变量实时着色（可解析绿、未解析橙），Send 旁显示未解析数量，再点一次强制发送
- 输入 `{{` 自动补全环境变量与动态变量
- 动态变量：`{{$uuid}}`、`{{$timestamp}}`、`{{$randomEmail}}`、`{{$randomFullName}}` 等 29 个（名字与 Postman 一致），
  部分支持参数：`{{$randomInt(1,100)}}`、`{{$randomString(8)}}`、`{{$randomPrice(10,50)}}`；输入 `{{` 有补全

**集合与历史**
- 文件夹嵌套；右键 / `⋯` 菜单重命名、复制、删除，删除可撤销
- 拖拽排序与跨文件夹移动；⌘点击多选，批量删除
- 请求可 Pin，置顶到侧栏 Pinned 区，常用接口不用再翻文件夹
- 侧栏搜索：按集合 / 文件夹 / 请求名和 URL 过滤（⌘F 聚焦）
- 历史最近 500 条，点击回填；最近 100 条连响应体一起留着（单条上限 64KB），点回去直接看当时的返回，不用重发
- 集合 / 文件夹可配公共 header 与 auth（右键 → Shared headers & auth…），下属请求自动带上；请求自己同名的覆盖它，选 **No auth** 则一条凭据都不带（含继承来的 Authorization 头）
- 导入集合时，里面带的 capture 规则一律先关掉——它们会改写你的环境变量，看过再开
- 导入：把 curl 粘贴到 URL 框即覆盖到当前请求（保留名字）；集合菜单可导入为新请求；文件导入 Firebee 导出 / Postman Collection v2.x / Postman Environment
- 导出：curl（JSON 压成一行，方便粘贴终端）、Python (requests)；集合导出为 Firebee JSON

**界面**
- 标签页：多个请求同时开着来回切，关了重开还在。⌘W 关标签，⌥⌘←/→ 切换，中键点标签也能关
- 三栏布局，分隔条可拖动，双击恢复；快捷键在菜单栏 Request 菜单可见：⌘↩ 发送、⌘N 新请求、⌘F 查找、⇧⌘F 过滤集合
- 没有手动保存：每个请求都在集合里，任何改动防抖写入本地 JSON（写临时文件 → fsync → rename → fsync 目录，文件权限 0600，解析失败时备份为带时间戳的 .bak 且不覆盖旧备份）
- 选中的环境记在本机，重启后还在
- 自动更新：启动时和之后每 24 小时检查一次新版本，可以跳过该版本或稍后提醒；选择更新后在后台下载，下次启动生效（Firebee › Check for Updates… 可手动检查）

## 从源码构建

需要 Rust stable 和 Tauri CLI（`cargo install tauri-cli`）。

```bash
git clone https://github.com/gq-guo/firebee.git && cd firebee
cargo tauri dev        # 开发模式
cargo run --release    # 直接运行（前端资源已编译进二进制）
cargo tauri build      # 打包 .app
```

## 数据位置

macOS: `~/Library/Application Support/firebee/`
- `collections.json` / `environments.json` / `history.json`（原子写入，损坏时自动备份为 .bak）
- `firebee.log.*`（运行日志）

卸载时删掉 `/Applications/Firebee.app` 和上面这个目录即可。

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

前端选中集合中的请求后直接引用该对象，编辑即写回集合并防抖保存。

## 参与贡献

欢迎提 Issue 和 PR。提交前请确保以下命令都能通过（CI 也会跑）：

```bash
cargo test
cargo clippy --all-targets -- -D warnings
cargo fmt --all -- --check
```

- 发布流程见 [docs/RELEASING.md](docs/RELEASING.md)
- 最初的设计文档在 [docs/superpowers/](docs/superpowers/)

## 许可

[0BSD](LICENSE)：随便用、随便改、随便分发，商用也可以，不需要保留署名。软件按原样提供，不附带任何担保。
