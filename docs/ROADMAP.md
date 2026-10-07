# 路线规划（2026-10）

原则不变：不堆功能、默认简单、调试优先、开发者原生。排序依据是**使用中断风险 ÷ 实现成本**，不是功能高级程度。

## P0 — 0.4 "Never Leave Firebee"（已完成）

缺了就被迫切回 curl / Postman 的能力，按成本从低到高做：

| # | 能力 | 现状 | 做法 |
|---|------|------|------|
| 1 | 信任系统 CA（企业 CA、Charles/mitmproxy 根证书） | `rustls-tls` 只带 webpki 公共根，钥匙串里的 CA 不生效 | reqwest 加 `rustls-tls-native-roots` feature |
| 2 | 系统代理 | `default-features = false` 顺手关掉了 `system-proxy`，从 Finder 启动的 app 读不到 `HTTP_PROXY` | reqwest 加 `system-proxy` feature |
| 3 | 关闭 SSL 校验开关 | 无 | 请求级 `insecure: bool`，默认 false |
| 4 | Multipart / 文件上传 | Body 只有 JSON/Text/Form | `BodyType::Multipart`，复用现有 `form: Vec<KeyValue>`，每行多一个 `is_file` |
| 5 | Binary body | 无 | `BodyType::Binary`，`body` 存文件路径 |
| 6 | SSE / 流式响应 | `resp.bytes().await` 读完才显示，流式接口要么等到关连接要么超时 | `Content-Type: text/event-stream` 时按 chunk 推给前端追加 |
| 7 | Actual Request | 导出 curl 已经先替换变量再拼 URL | 只读 "Final Request" 面板，展示变量替换、继承 header/auth、Cookie 之后的最终请求 |

## P1 — 0.5 "Debug Faster"（已完成，PKCE 与 DNS/Connect/TLS 拆分除外）

- 自定义 Proxy（HTTP/HTTPS + bypass 列表）
- mTLS（PEM / PKCS12 客户端证书）、自定义 CA 文件
- OAuth2 Client Credentials（本质是预请求 + Capture；Authorization Code + PKCE 需要 loopback 回调，后置）
- OpenAPI 3.x / Swagger 2.0 导入
- 响应 TTFB / Total（DNS/Connect/TLS 拆分 reqwest 不暴露，要自写 connector，放 P2）
- Cookie Manager

## P2 — 0.6 "Developer Workflow"

顺序有依赖，先定文件格式：

1. Git-friendly Project Format（一请求一文件，可放进项目目录）——这是存储层重构，影响原子写入、备份、导入导出
2. Secret 变量 + Keychain（和 1 一起设计"哪些字段不进文件"，单独提前做会返工）
3. CLI（`src/core` 已不依赖 UI，新增 `[[bin]]` 即可）
4. Assertions（Status / Header / JSONPath / Time，声明式，不引入 JS）
5. Collection Runner（顺序执行 + Capture 就是 Request Chain，不单独做）

## P3 — 0.7 "Protocol Expansion"

WebSocket；gRPC 看真实反馈。

## 明确不做

Cloud 账号 / Team Workspace、在线文档发布、完整 Mock 平台、Visual Flow Builder、完整 JS Runtime、几十种 Auth、AI Chat。

## 每个版本的 Gate

- 普通用户不碰高级能力时主界面复杂度不变
- 新增设置默认可用，只在遇到特殊网络/认证问题时才进高级配置
- Send 主路径延迟不变
- 真实项目里 curl / Postman 切换次数减少，功能完成不等于版本完成
