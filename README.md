# Firebee

用 Rust + Tauri 编写的轻量桌面 API client（精简版 Postman）。设计目标：快、稳定、克制。

后端是 Rust（存储 / HTTP / 变量替换 / 导出），前端是零依赖的 HTML/CSS/JS（无 npm 工具链）。

## 功能

- HTTP 请求：全部常用方法，Params / Headers / Body（JSON、Text、Form）
- 响应展示：JSON 语法高亮、状态码、耗时、大小、响应头
- 集合管理：文件夹嵌套、右键重命名/删除/复制，JSON 文件持久化；集合导出为 Firebee JSON，导入 Firebee 导出 / Postman Collection v2.x / Postman Environment
- 环境变量：多环境切换，`{{variable}}` 替换（未定义变量发送前提示，二次点击强制发送）
- 认证：Bearer Token、Basic Auth、API Key（Header / Query）
- 历史记录：最近 500 条，点击回填编辑器
- 导出：curl 命令、Python (requests) 代码，一键复制
- 导入：粘贴 curl 命令到 URL 框或通过菜单导入（-X/-H/-d/--data-urlencode/-u/-G 等）
- 响应 JSONPath 过滤：`$.data.orders.*.app.key`、`$.data.orders.2.app.key`、`$..key`
- 请求可取消、超时可配置（顶栏右侧）；多个请求可同时在飞，响应按请求保留在内存里（切换不丢，最多 50 条）
- 三栏宽度可拖动，双击分隔条恢复默认

## 运行

```bash
cargo tauri dev        # 开发（需要 cargo install tauri-cli）
cargo run --release    # 直接运行（前端资源已编译进二进制）
```

## 测试

```bash
cargo test          # 34 个单元/集成测试（core 层全覆盖，http 用 wiremock）
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
