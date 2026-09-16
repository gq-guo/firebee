# Firebee — Rust API Client 设计文档

日期：2025-09-16
状态：已确认

## 1. 目标与定位

一个用 Rust 编写的桌面 API client，功能类似精简版 Postman。核心设计目标：

- **快**：毫秒级启动，低内存占用，界面永不因请求阻塞
- **稳定**：数据损坏不崩溃，错误友好提示，Core 层可完整单测
- **克制**：不做大而全，只保留开发者日常调试 API 的核心功能

## 2. 功能范围

### MVP 包含

1. **HTTP 请求**：GET/POST/PUT/DELETE/PATCH 等方法，自定义 Header、Query 参数、Body（JSON / 文本 / 表单）
2. **响应展示**：JSON 格式化与语法高亮、状态码、耗时、响应大小、响应头
3. **集合（Collection）管理**：请求分组保存，支持文件夹嵌套
4. **环境变量**：多套环境（dev/staging/prod），`{{variable}}` 语法在 URL/Header/Body 中替换
5. **认证**：Bearer Token、Basic Auth、API Key
6. **历史记录**：自动记录最近请求（上限 500 条），点击可回填编辑器
7. **导出**：生成 curl 命令、Python (requests) 代码，一键复制

### 明确不做（YAGNI）

- 团队协作 / 云同步 / 账号体系
- Mock Server、API 文档生成、自动化测试脚本、监控
- WebSocket / gRPC / GraphQL 专用支持（v1 仅 HTTP/REST）
- 插件系统
- 导入 Postman collection（MVP 只做导出）

## 3. 技术选型

| 用途 | 选型 | 理由 |
|---|---|---|
| GUI | `egui` + `eframe` | 纯 Rust，单二进制，启动快，跨平台 |
| HTTP 客户端 | `reqwest`（rustls-tls） | 成熟稳定，rustls 避免依赖系统 OpenSSL |
| 异步运行时 | `tokio` | reqwest 配套 |
| 序列化 | `serde` + `serde_json` | 存储格式全用 JSON |
| JSON 语法高亮 | `syntect` | egui 生态常用搭配 |
| 日志 | `tracing` | 输出到数据目录日志文件 |
| HTTP mock 测试 | `wiremock` | Core 层 http 模块单测 |

## 4. 架构

### 4.1 分层

核心原则：**UI 永远不阻塞**。

```
┌─────────────────────────────────────────┐
│  UI 层 (egui)                            │
│  侧边栏 / 请求面板 / 响应面板 / 环境栏      │
└──────────────┬───────────────────────────┘
               │ channel 消息（发请求/收结果）
┌──────────────▼───────────────────────────┐
│  Core 层（不依赖 UI，可单测）              │
│  models · storage · http · vars · export │
└──────────────────────────────────────────┘
```

HTTP 请求在独立 tokio worker 线程执行，通过 `std::sync::mpsc` channel 把结果送回 UI。Core 层不引用 egui，可纯单元测试。

### 4.2 模块划分

```
src/
  main.rs            # 入口，启动 eframe，初始化 worker 线程与 channel
  core/
    models.rs        # Request, Response, Collection, Environment, HistoryEntry
    storage.rs       # JSON 持久化（原子写入、损坏恢复）
    http.rs          # 请求执行（reqwest 封装、计时、错误映射、取消）
    vars.rs          # {{variable}} 替换
    export.rs        # curl / Python 代码生成
  ui/
    app.rs           # 根 App 状态，分发各面板
    sidebar.rs       # 集合树 + 历史记录
    request_panel.rs # 方法/URL/Params/Headers/Body/Auth 编辑
    response_panel.rs# 响应展示（Body/Headers/元信息）
    env_bar.rs       # 顶部环境选择与管理
```

## 5. 界面布局

```
┌──────────────────────────────────────────────────────────┐
│ [环境选择: Dev ▾]  [⚙ 管理环境]              [+ 新请求]   │ ← 顶栏
├────────────┬─────────────────────────────────────────────┤
│ 集合        │ [GET ▾] {{base_url}}/users     [发送] [导出▾]│
│ ▾ 用户模块  │ ┌─────────────────────────────────────────┐│
│   • 登录    │ │ Params │ Headers │ Body │ Auth          ││
│   • 列表    │ └─────────────────────────────────────────┘│
│ ▾ 订单模块  │ ┌─ 响应 ──────────── 200 OK · 87ms · 2.1KB ┐│
│ ────────── │ │ Body (JSON高亮) │ Headers │               ││
│ 历史记录    │ │ { "id": 1, ... }                        ││
│ • 10:32 GET│ └─────────────────────────────────────────┘│
│ • 10:15 POS│                                             │
└────────────┴─────────────────────────────────────────────┘
```

关键交互：

- 左侧树：集合（文件夹嵌套）→ 请求；下方可切换到"历史记录"，点击历史回填编辑器
- 顶部环境切换全局生效；未定义的 `{{variable}}` 在发送前红色提示并阻止发送（可强制发送）
- 导出按钮弹窗显示生成的 curl / Python 代码，一键复制

## 6. 数据流

一次请求的完整生命周期：

```
用户点发送
  → UI 校验 + 变量替换 (vars)
  → 构造 Job 经 channel 发给 worker 线程
  → reqwest 执行，计时
  → 结果（响应/错误）经 channel 回 UI
  → 更新响应面板 + 追加历史记录（异步写盘）
```

## 7. 持久化

- 存储位置：系统标准数据目录（macOS `~/Library/Application Support/firebee/`）
- 三个 JSON 文件：`collections.json`、`environments.json`、`history.json`，人类可读、可 git 管理
- 写入策略："写临时文件 + 原子 rename"，防止半截文件
- 保存时机：集合/环境修改防抖 500ms 保存；退出时强制 flush
- 历史记录上限 500 条，FIFO 淘汰

## 8. 错误处理

- **网络层**：超时默认 30s（可配）；连接失败 / DNS / TLS 错误映射为友好中文提示，显示在响应面板（不弹窗打断）；请求可中途取消
- **存储层**：读 JSON 失败时备份原文件为 `.bak` 并以空数据启动，绝不因数据损坏起不来
- **UI 层**：worker 线程内捕获所有错误，不拖垮 UI
- **日志**：`tracing` 写入数据目录日志文件

## 9. 测试策略

- Core 层单元测试（不依赖 GUI）：
  - `vars`：变量替换边界（嵌套、未定义、转义）
  - `storage`：读写往返、损坏文件恢复、原子写入
  - `http`：wiremock 本地 mock server 测请求构造、超时、错误映射
  - `export`：curl / Python 生成结果快照对比
- UI 层不做自动化测试，保持极薄，靠 Core 测试 + 手动冒烟清单
