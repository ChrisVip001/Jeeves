# Jeeves 统一网关

云负载均衡或 Ingress 处理公网 HTTPS。Go 网关提供 Web 和 iOS 工作区，管理账号与会话，并将通过鉴权的请求转发给用户的 Rust Runtime。私网的 Go 控制面按已鉴权的租户返回 Runtime 地址。Mac 设备端使用同一个账号配对，并按需授权所选窗口。[架构](../../../docs/architecture.zh.md)

## 云端配置

在仓库根目录运行 `docker build -f jeeves-ts/packages/gateway/Dockerfile -t jeeves-gateway .`。镜像提供构建好的工作台，监听 8080 端口。其他镜像和基础设施见[托管部署手册](../../../control-plane/README.zh.md)。

| 环境变量 | 用途 |
| --- | --- |
| `GW_PUBLIC_ORIGIN` | 安装客户端及邮件用户访问的 HTTPS 源 |
| `GW_DATABASE_URL` | 存储账号、会话、验证信息和设备配对的 PostgreSQL |
| `GW_SESSION_KEY`、`GW_STATE_KEY` | 两个独立的至少 32 字节随机密钥，所有网关副本共用 |
| `GW_CONTROL_URL`、`GW_CONTROL_TOKEN` | Go 控制面的私网地址和服务凭据 |
| `GW_MAIL_FROM` 以及 `GW_SMTP_URL` 或 `GW_RESEND_API_KEY` | 邮箱验证、密码找回的已认证发件人和投递渠道 |
| `GW_OIDC_ISSUER`、`GW_OIDC_CLIENT_ID`、`GW_OIDC_CLIENT_SECRET` | 可选的 OpenID Connect 提供方。回调必须注册为公网源下的 HTTPS 路径 `/api/oidc/callback` |
| `GW_TRUST_PROXY` | 受信私网入口后的网关设为 `1`，入口保留 Host 并覆盖转发头 |
| `GW_ADDR`、`GW_TLS_CERT`、`GW_TLS_KEY` | 监听地址及网关直接处理 HTTPS 时使用的证书 |

SMTP 支持强制 STARTTLS 的 `smtp://`，以及连接即启用 TLS 的 `smtps://`。通过 `GW_SMTP_USER` 和 `GW_SMTP_PASSWORD` 提供凭据。OIDC 身份以签发者和主体标识为准。已有同邮箱的密码账号时，网关拒绝自动绑定。密码登录支持验证器动态码和一次性恢复码。

## 路由与凭据

浏览器会话 Cookie 仅限当前主机和 HTTPS，且禁止 JavaScript 读取。统一网关根据已鉴权的账号确定 Runtime 地址，并在服务端附加 Runtime 凭据。控制面、租户 Service 和数据库放在私网内。公网入口应支持流式响应、WebSocket 升级，并配置足够长的空闲连接超时。跨内部信任边界的连接使用 TLS 或服务网格。

渠道适配器在获得授权的租户部署中运行。配置和受限渠道凭据见 [Runtime 外壳手册](../../../docs/internals/07-shells.zh.md#78-runtime-托管的-lark-网关)。

## 个人部署

在安装 Go、Rust 和 Node 后，从仓库根目录运行 `just local-up`。浏览器会打开一次性的所有者设置页面，两个服务均只监听回环接口。设置完成后可通过命令打印的地址返回。Runtime 数据和账号数据库默认存放在 `~/.local/share/jeeves-personal`。个人部署使用自己配置的模型，只能在宿主电脑访问，账号独立于托管版。

开发中使用回环 HTTPS 时，配置受信任证书、私有 SQLite、`GW_FIXED_OWNER_EMAIL` 和 `GW_RUNTIME_TOKEN`，并用 `GW_RUNTIME_HOME` 指向 Runtime 数据目录的绝对路径。网关验证该目录中发布的端点后转发工作区请求。固定部署的私网端点可通过 `GW_RUNTIME_URL` 配置。

## 验证

运行 `just go-check` 检查 Go 包。在安装 Chrome 的机器上运行 `just smoke-account`，可以复验浏览器注册、找回密码，以及在独立的桌面和手机尺寸会话之间续办同一任务。测试使用隔离数据、模拟模型和测试收件箱。要使用 PostgreSQL 测试，将 `JEEVES_TEST_GATEWAY_DATABASE` 和 `JEEVES_TEST_CONTROL_DATABASE` 分别指向全新创建的本机 `jeeves_test_gateway` 和 `jeeves_test_control` 数据库。控制面的 Go 测试也可使用后一个变量。
