# Firebee

用 Rust + Tauri 编写的轻量桌面 API client（精简版 Postman）。设计目标：快、稳定、克制。

后端是 Rust（存储 / HTTP / 变量替换 / 导出），前端是零依赖的 HTML/CSS/JS（无 npm 工具链）。

## 功能

**请求**
- 全部常用方法；Params / Headers / Body（JSON、Text、Form）/ Auth（Bearer、Basic、API Key）
- Header 名与 Content-Type / Accept 值自动补全；JSON body 一键格式化并定位错误
- 可取消、超时可配；多个请求可同时在飞，响应按请求保留（切换不丢，内存中最多 50 条）
- Cookie 在会话内自动保持，登录后可连着调会话接口；顶栏一键清除

**响应**
- 状态码与原因短语、耗时、大小、响应头；JSON 语法高亮
- JSONPath 过滤：`$.data.orders.*.app.key`、`$.data.orders.2.app.key`、`$..key`
- 体内查找（⌘F，Enter / Shift+Enter 跳转）；可折叠树视图；超大响应先截断再按需全量
- HTML 沙箱预览、图片预览、保存到文件

**变量与环境**
- 多环境切换，`{{variable}}` 在 URL / 参数 / 头 / 体 / 认证中替换；未定义变量发送前提示，再点一次强制发送
- 输入 `{{` 自动补全环境变量与动态变量
- 动态变量：`{{$uuid}}`、`{{$timestamp}}`、`{{$isoTimestamp}}`、`{{$randomInt}}`

**集合与历史**
- 文件夹嵌套；右键 / `⋯` 菜单重命名、复制、删除，删除可撤销
- 拖拽排序与跨文件夹移动；⌘点击多选，批量删除
- 侧栏搜索：按集合 / 文件夹 / 请求名和 URL 过滤（⌘F 聚焦）
- 历史最近 500 条，点击回填
- 导入：粘贴 curl 到 URL 框或菜单导入；文件导入 Firebee 导出 / Postman Collection v2.x / Postman Environment
- 导出：curl（JSON 压成一行，方便粘贴终端）、Python (requests)；集合导出为 Firebee JSON

**界面**
- 三栏布局，分隔条可拖动，双击恢复；快捷键 ⌘↩ 发送、⌘S 存入集合、⌘N 新请求、⌘F 查找
- 所有数据本地 JSON 文件持久化，原子写入，损坏自动备份

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
