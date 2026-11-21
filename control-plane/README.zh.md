# Jeeves 账号网关

[English](README.md) | 简体中文

在网关的 HTTPS 域名打开 `/account/` 登录。个人中心显示邮箱、注册时间、适用时的模型用量以及已登录的会话。点击“打开我的助理”进入工作区。退出某个会话会关闭它正在使用的 Runtime 数据流。修改密码需验证当前密码，并使所有设备重新登录。设备名称来自浏览器报告的平台；设备签名授权和高风险操作审批仍由 Runtime 单独核验。

## 从手机或 Mac 连接

iOS 用户可以用 Safari 登录，进入助理后把**工作区**添加到主屏幕。安装后的 PWA 可能需要单独登录。Mac 用户可以从工作台的“设置 → 通用 → 账号服务地址”进入，也可以用 `JEEVES_RUNTIME_URL=https://jeeves.example.com` 启动桌面壳，直接打开网关。每端独立登录，可分别下线。同一账号看到同一批任务。

## 路由已认证请求

Node 控制面提供由网关构建的工作台，校验随机会话 Cookie，并比对 `/app/<账号 ID>/api/*` 与会话所属账号，再在内网请求中加入该用户的 Runtime Bearer。浏览器只持有 14 天绝对有效期、限定当前主机的 Secure、HttpOnly、SameSite=Lax Cookie。浏览器脚本和 Runtime 都拿不到该 Cookie。网关按 `CP_PUBLIC_ORIGIN` 校验浏览器写请求的来源。个人中心接口只返回展示所需字段。用户登录、操作审批和设备配对各自独立。

| 入口 | 访问与结果 |
|---|---|
| `POST /api/register`、`/api/send-verification`、`/api/forgot` | 提交邮箱，符合条件的地址会收到链接。每人独立 Pod 的部署支持公开注册。 |
| `POST /api/verify`、`/api/reset` | 提交邮箱、邮件令牌和密码。验证时创建账号或核验已有账号的密码。重设会撤销全部会话，保留第二因素。 |
| `POST /api/login` | 提交邮箱和密码。如响应要求第二因素，再附验证码或恢复码提交。通过后获得会话 Cookie。 |
| `POST /api/account/factor/{start,confirm,disable}` | 需要已验证的会话和当前密码。启用时核验验证器动态码，关闭时可用动态码或恢复码。 |
| `GET /api/account` | 本人资料、工作区路径、当前会话和本人其他会话。 |
| `POST /api/logout`、`DELETE /api/account/sessions/<id>` | 撤销当前会话或本人指定的会话。 |
| `POST /api/account/password` | JSON 字段 `current` 和 `replacement`。改密和撤销会话一并持久化。 |
| `/app/<账号 ID>/` | 工作台及其流式 API。应用代码由网关发布的构建产物提供。 |
| `/v1/*` | Pod 模式的模型代理核验专用令牌和月度额度，向提供方转发时加入平台凭据。 |

## 部署网关

使用 Node 22.18 及以上。在仓库根目录运行 `just dashboard-build`，或在根目录运行 `docker build -f control-plane/Dockerfile -t jeeves-control-plane .` 构建网关镜像。`CP_PUBLIC_ORIGIN` 填写对外 HTTPS 源。用密钥管理服务分别生成并提供 `CP_SESSION_SECRET` 和 `CP_STATE_KEY`，各至少 32 字节。给 `CP_STATE` 配持久可写目录，运行单个网关写实例。`CP_PORT`、`CP_BIND` 和 `CP_DASHBOARD_DIR` 可覆盖监听和前端目录。TLS 入口须保留公网 Host，并按实际协议**覆盖** X-Forwarded-Proto。网关监听端口和 Runtime 都放在私网，入口应在展示登录表单前将 HTTP 跳转至 HTTPS 或拒绝请求。

**已有的单个 Runtime。** 设置 `CP_SINGLE_RUNTIME_URL`、与 Runtime 的 `JEEVES_OPS_TOKEN` 一致的 `CP_SINGLE_RUNTIME_TOKEN`，以及 `CP_OWNER_EMAIL`。首次用空账号库启动时，只有 `CP_OWNER_EMAIL` 指定的邮箱可通过邮件验证注册。也可以设置 `CP_OWNER_PASSWORD` 预建所有者，初始化后从启动环境移除。设置 `CP_TLS_CERT` 和 `CP_TLS_KEY` 时网关直接提供 HTTPS。本机使用时将 `CP_BIND` 设为 `127.0.0.1`。在 Mac 验证或找回密码时，可将邮件链接粘贴到等待页面，在当前窗口完成操作。用 `node control-plane/src/server.ts` 启动。此模式只有一个所有者，关闭开放注册，也不会休眠现有 Runtime。后端地址支持 HTTPS 源或回环 HTTP。直接访问独立工作台仍沿用原有访问策略；要求登录的部署只应对外开放账号网关。

**每个账号独立 Runtime。** 设置 `CP_IMAGE`、`CP_MODEL_URL`（Pod 访问模型代理的地址）、`CP_MODEL_UPSTREAM`（模型提供方 HTTPS 端点）与 `CP_MODEL_KEY`。按需配置 `CP_NAMESPACE`、`CP_IDLE_MINUTES`、`CP_MONTHLY_TOKENS`。替换 [Kubernetes 清单](../deploy/k8s/control-plane.yaml)里的域名、镜像和 TLS Secret，选定 Ingress 控制器，配置 HTTP 跳转 HTTPS。每人独享一个 Pod、Service、Secret 和持久卷。随附的入站网络策略依赖支持 NetworkPolicy 的 CNI。休眠前检查所有任务分页，有活动工作或常驻标记的账号继续运行，休眠保留卷。首次配置租户失败可在打开工作区时重试。[运行手册](../docs/operations.zh.md)另有任务数据迁移说明。

## 加密与升级

- **传输。** 使用受信任证书，优先 TLS 1.3，兼容客户端可使用 TLS 1.2。入口需验证上游证书并配置适合流式响应的超时。网关发送 HSTS。Kubernetes 服务域名和网络策略本身不加密流量。使用 Istio 时将入口、控制面和用户 Pod 纳入网格，配置入口到控制面的双向 TLS，再应用[严格 mTLS 策略](../deploy/k8s/runtime-mtls.yaml)。自定义信任域要同步调整服务身份。上线前验证证书、获准连接和明文拒绝。[TLS 指引](https://cheatsheetseries.owasp.org/cheatsheets/Transport_Layer_Security_Cheat_Sheet.html) · [Istio 对端认证](https://istio.io/latest/docs/reference/config/security/peer_authentication/)
- **存储。** 密码使用独立盐值和 scrypt。账号库使用 AES-256-GCM，每次保存生成新随机数，加密密钥从 `CP_STATE_KEY` 派生。会话令牌有 32 字节随机数，库中只保存带密钥的摘要。加密文件与恢复密钥分开保管。轮换会话密钥会使现有 Cookie 失效；更换账号库密钥需要先重加密，或用原密钥恢复。Runtime 的 SQLite、附件、卷快照、备份与日志需要分别配置存储加密和访问策略。生产环境应启用 KMS 管理的卷加密及 Kubernetes Secret 静态加密。[Kubernetes 静态加密](https://kubernetes.io/docs/tasks/administer-cluster/encrypt-data/)
- **信任边界。** 云 Runtime 和已配置的模型服务需要读取发给它们的任务内容。链路加密与存储加密分别保护传输和存储介质。将来如做独立原生 iOS 或 Mac OAuth 客户端，采用系统浏览器授权码加 PKCE，刷新凭据放 Keychain。当前客户端使用第一方网页会话。[原生应用 OAuth 规范](https://datatracker.ietf.org/doc/html/rfc8252)
- **旧账号库。** 停止旧网关，制作受保护的备份，配置两个密钥和原 `CP_STATE` 路径，运行 `node control-plane/src/server.ts --migrate-accounts`。迁移以原子替换加密旧明文文件，保留密码校验数据、用量和租户 ID。用户需要重新登录。历史明文备份按保留策略单独处理。

## 验证邮箱、找回密码与双因素登录

将认证邮件的发件地址填入 `CP_MAIL_FROM`。使用已有邮箱时，设置 `CP_SMTP_URL`（例如 `smtps://smtp.qq.com:465`）、`CP_SMTP_USER` 和 `CP_SMTP_PASSWORD`（邮箱服务商提供的授权码）。QQ 邮箱使用 `CP_SMTP_AUTH=LOGIN`。SMTP 发信需要 curl 8.3 或更高版本，校验 TLS 证书，每次请求提交一封邮件。通过网关密钥管理服务提供凭据。使用 Resend 时，留空 `CP_SMTP_URL`，验证发件域名并提供仅允许发信的 `CP_RESEND_API_KEY`。关闭认证邮件的点击追踪，保留链接中的私密片段。新用户打开邮件后设置密码，网关此时才创建工作区。已有账号和单用户部署的初始账号需要用原密码完成首次邮箱验证，随后即可自助找回密码。邮件链接 15 分钟内有效，成功使用后，同一旧账号状态下的链接一并失效。过期可重新申请。[Resend 发信接口](https://resend.com/docs/api-reference/emails/send-email)

在个人中心输入当前密码，将设置密钥手动添加到支持时间动态码的验证器，输入六位验证码后单独保存八个一次性恢复码。启用或关闭双因素认证会让其他会话下线。遗失验证器时，用密码和一个恢复码登录，再更换验证器。通过邮件重设密码会保留双因素认证。同时丢失验证器和全部恢复码时，无法自助解除认证。[时间动态码规范](https://www.rfc-editor.org/rfc/rfc6238)

在本目录运行 `npm test` 验证账号、鉴权与代理。在仓库根目录运行 `just smoke-account`，用测试 HTTPS、测试收件箱、验证器与恢复码、桌面及手机尺寸的浏览器和真实 Runtime 验证登录。
