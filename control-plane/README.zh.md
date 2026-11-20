# Jeeves 控制面

云端外壳，引擎本身不变：**每个用户一个 Pod、一块卷**（silo 多租户）。无依赖，Node ≥ 22.18 直接运行 TypeScript。

| 部件 | 文件 | 作用 |
|---|---|---|
| 账号 | `src/accounts.ts` | 邮箱 + scrypt 密码、签名会话 cookie、每用户的 ops 与模型令牌、月度用量账本 |
| 清单 | `src/manifests.ts` | 每用户 Secret + PVC + Deployment（`Recreate`，醒着 1 副本/睡着 0）+ Service |
| 集群 | `src/cluster.ts` | `kubectl apply/scale/delete`；测试用内存版 |
| 模型代理 | `src/proxy.ts` | `/v1/*`：校验用户令牌与月度额度、加上平台密钥、统计 token |
| 服务 | `src/server.ts` | 注册登录、`/app/*` 路由到用户自己的 Pod、按需唤醒、空闲休眠 |

守住设计的几条规则：

- **平台模型密钥永不进用户 Pod。** Pod 只拿自己的 ops 令牌和每用户模型令牌；`models.json` 指向本代理，令牌以 `env:` 指针引用。渲染出的清单里出现密钥形字段，测试就失败。
- **休眠保留卷。** 无正在处理的应用写请求，且任务列表所有分页均只有终态任务时，才允许休眠。读取任务状态失败会保持运行。空闲账号在 Jeeves 自己 25 秒排空后缩到 0（`CP_IDLE_MINUTES`，默认 20），下个请求唤醒。`alwaysOn` 账号不参与休眠，定时与渠道由此 24 小时在线。
- **超额就直说。** 代理回 429，提示"在设置 → 模型里填自己的 key"；用户填了自己的 key 就完全绕开平台额度。
- **搬家**：本地 `jeeves home export <目录>`，在 Pod 里 `jeeves home import <目录>`（反向亦然）。密钥与登录态不导出，需重新给。

配置（环境变量）：`CP_SESSION_SECRET`、`CP_IMAGE`、`CP_MODEL_URL`、`CP_MODEL_UPSTREAM`、`CP_MODEL_KEY`，可选 `CP_STATE`、`CP_NAMESPACE`、`CP_IDLE_MINUTES`、`CP_MONTHLY_TOKENS`、`CP_PORT`。用 `deploy/k8s/control-plane.yaml` 部署，在本目录 `npm test` 测试。

尚未包含：支付、邮箱验证、界面（接口即契约）、TLS（在入口终结）、每用户网络策略（请在集群里加一条禁止 Pod 间互访的策略）。
