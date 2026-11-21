# Jeeves 云端部署

公网 HTTPS 入口将流量发给 [Go 统一网关](../jeeves-ts/packages/gateway/README.zh.md)，同时部署私网中的 Go 控制面。Kubernetes 运行并隔离每个租户的 Rust Runtime，控制面计量套餐内模型用量。[架构](../docs/architecture.zh.md)

## 构建与安装

使用网关的 Dockerfile 构建网关镜像，使用 `docker build -f control-plane/Dockerfile -t jeeves-control-plane .` 构建控制面，使用 `docker build -t jeeves .` 构建 Runtime。以上命令均从仓库根目录执行。可选的 TypeScript 渠道适配器随 Runtime 打包。

使用 [Kubernetes 示例](../deploy/k8s/control-plane.yaml)，并准备私网 PostgreSQL、默认 StorageClass、支持 NetworkPolicy 的 CNI 和 HTTPS Ingress。替换示例中的域名、镜像和 IngressClass。在 Jeeves 命名空间配置 `gateway-env`、`cp-env` 和公网 TLS 证书。分别为网关和控制面设置不同的数据库或角色。生产环境启用数据库 TLS 和加密备份。示例只为网关 Service 配置公网入口。限制私网访问，在 Istio 环境可应用 [mTLS 示例](../deploy/k8s/runtime-mtls.yaml)。根据批准的工具和渠道政策限制租户出站流量。

| 控制面环境变量 | 用途 |
| --- | --- |
| `CP_DATABASE_URL`、`CP_STATE_KEY` | PostgreSQL 和独立的至少 32 字节凭据加密密钥 |
| `CP_GATEWAY_TOKEN` | 与网关 `GW_CONTROL_TOKEN` 一致的服务凭据 |
| `CP_IMAGE`、`CP_NAMESPACE` | Rust Runtime 镜像和 Kubernetes 命名空间 |
| `CP_MODEL_URL`、`CP_MODEL_UPSTREAM`、`CP_MODEL_KEY` | 私网模型代理地址、支持 Anthropic Messages API 的 HTTPS 提供方地址和平台密钥 |
| `CP_MONTHLY_TOKENS`、`CP_IDLE` | 月度 token 用量和空闲周期，默认 200 万及 20 分钟 |
| `CP_DESKTOP_GATEWAY_URL` | 获授权的 Runtime 发起 Mac 中继所用的私网网关地址 |

## 租户生命周期与配额

首次访问工作区会申请租户 Runtime。控制面为该租户协调一个 Deployment、一个 Service、受限凭据和持久卷声明。网关等待 Runtime 就绪。每个租户 Pod 受 Kubernetes CPU 和内存限制，Rust Runtime 另有限制并发回合数和工作区存储。当 Runtime 空闲时，控制面检查活动回合、任务和计划后再缩容，休眠保留持久卷。

模型代理先保守预留额度，再向提供方转发请求。随后按提供方报告结算一次，结果不明时保留预留额供运营人员核对。每个租户的套餐内额度由部署配置确定。SaaS 套餐购买、发票和提供方实际账单需要单独对接计费系统。

## 账号与任务迁移

切换前备份 PostgreSQL 和 Runtime，并保护备份。暂停旧账号写入，使用旧版本和原状态密钥导出加密账号文件。启动 Go 控制面，并将网关指向目标数据库，设置 `GW_IMPORT_LEGACY` 为加密文件路径、`GW_LEGACY_STATE_KEY` 为旧密钥，再运行一次 Go 网关。启动正式网关监听前移除这两个迁移环境变量。迁移保留用户和租户 ID、密码哈希、双因素信息、租户凭据和已登记的模型用量。用户需要重新登录。

切换时复用原 Kubernetes 命名空间和租户 PVC，或按 [Runtime 备份恢复说明](../docs/operations.zh.md#数据与备份)迁移。将流量切到新部署前，抽查账号、历史任务 ID 和可恢复的工作。迁移验证前保留加密导出和旧版本发布包。
