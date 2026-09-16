# Firebee

用 Rust + egui 编写的轻量桌面 API client（精简版 Postman）。设计目标：快、稳定、克制。

## 功能

- HTTP 请求：全部常用方法，Params / Headers / Body（JSON、Text、Form）
- 响应展示：JSON 语法高亮、状态码、耗时、大小、响应头
- 集合管理：文件夹嵌套、右键重命名/删除，JSON 文件持久化
- 环境变量：多环境切换，`{{variable}}` 替换（未定义变量发送前提示，二次点击强制发送）
- 认证：Bearer Token、Basic Auth、API Key（Header / Query）
- 历史记录：最近 500 条，点击回填编辑器
- 导出：curl 命令、Python (requests) 代码，一键复制
- 请求可取消、超时可配置（顶栏右侧）

## 运行

```bash
cargo run --release
```

## 测试

```bash
cargo test          # 24 个单元/集成测试（core 层全覆盖，http 用 wiremock）
cargo clippy --all-targets -- -D warnings
```

## 架构

```
src/
  core/    models · storage · vars · http · export   （不依赖 UI，全部可单测）
  worker.rs  专用 tokio 线程执行请求，mpsc channel 与 UI 通信（UI 永不阻塞）
  ui/      egui 三栏界面：顶栏环境、侧边栏集合/历史、请求面板、响应面板
```

## 数据位置

macOS: `~/Library/Application Support/firebee/`
- `collections.json` / `environments.json` / `history.json`（原子写入，损坏自动备份为 .bak）
- `firebee.log.*`（运行日志）

## 设计文档

- `docs/superpowers/specs/2025-09-16-firebee-api-client-design.md`
- `docs/superpowers/plans/2025-09-16-firebee-api-client.md`
