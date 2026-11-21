import { smtpMailer } from "./smtp.ts";
import { accountMailer } from "./mail.ts";
import { Accounts } from "./accounts.ts";
import { Kubectl } from "./cluster.ts";
import { singleRuntime } from "./single-runtime.ts";
import type { Config } from "./server.ts";

export function deployment(env: NodeJS.ProcessEnv) {
  const need = (k: string) => {
    const value = env[k];
    if (!value) throw new Error(k + " is required");
    return value;
  };
  const rootKey = need("CP_STATE_KEY");
  const signingKey = need("CP_SESSION_SECRET");
  if (
    Buffer.byteLength(rootKey) < 32 ||
    Buffer.byteLength(signingKey) < 32 ||
    rootKey === signingKey
  )
    throw new Error("configure two independent secrets of at least 32 bytes");
  const migrate = process.argv.includes("--migrate-accounts");
  const accounts = new Accounts(
    env.CP_STATE ?? "control-plane.json",
    signingKey,
    rootKey,
    migrate,
  );
  if (migrate) process.exit(0);
  const publicOrigin = new URL(need("CP_PUBLIC_ORIGIN"));
  if (
    publicOrigin.protocol !== "https:" ||
    publicOrigin.origin !== publicOrigin.href.replace(/\/$/, "")
  )
    throw new Error("CP_PUBLIC_ORIGIN must be an HTTPS origin");
  const fixed = env.CP_SINGLE_RUNTIME_URL;
  if (fixed) {
    const runtime = new URL(fixed);
    if (
      runtime.origin !== fixed ||
      !(
        runtime.protocol === "https:" ||
        (runtime.protocol === "http:" &&
          ["localhost", "127.0.0.1", "[::1]"].includes(runtime.hostname))
      )
    )
      throw new Error(
        "CP_SINGLE_RUNTIME_URL must be an HTTPS origin or a loopback HTTP origin",
      );
  }
  const singleToken = fixed ? need("CP_SINGLE_RUNTIME_TOKEN") : undefined;
  const owner = fixed
    ? singleRuntime(accounts, need("CP_OWNER_EMAIL"), env.CP_OWNER_PASSWORD)
    : undefined;
  const cfg: Config = {
    publicOrigin: publicOrigin.origin,
    mail: env.CP_SMTP_URL
      ? smtpMailer(
          publicOrigin.origin,
          need("CP_MAIL_FROM"),
          env.CP_SMTP_URL,
          need("CP_SMTP_USER"),
          need("CP_SMTP_PASSWORD"),
          env.CP_SMTP_AUTH,
        )
      : accountMailer(
          publicOrigin.origin,
          need("CP_MAIL_FROM"),
          need("CP_RESEND_API_KEY"),
        ),
    dashboardDir: env.CP_DASHBOARD_DIR,
    image: fixed ? "" : need("CP_IMAGE"),
    namespace: env.CP_NAMESPACE ?? "jeeves",
    publicModelUrl: fixed ? "" : need("CP_MODEL_URL"),
    idleMinutes: Number(env.CP_IDLE_MINUTES ?? 20),
    proxy: {
      upstream: fixed ? "" : need("CP_MODEL_UPSTREAM"),
      key: fixed ? "" : need("CP_MODEL_KEY"),
      monthlyTokens: Number(env.CP_MONTHLY_TOKENS ?? 2_000_000),
    },
    ...(fixed && owner && singleToken
      ? {
          podAddress: () => fixed,
          single: {
            get ownerId() {
              return owner.ownerId;
            },
            registrationEmail: owner.registrationEmail,
            token: singleToken,
          },
        }
      : {}),
  };
  if (!fixed) {
    const provider = new URL(cfg.proxy.upstream);
    if (
      provider.protocol !== "https:" ||
      provider.username ||
      provider.password
    )
      throw new Error(
        "CP_MODEL_UPSTREAM must be a credential-free HTTPS endpoint",
      );
  }
  return {
    accounts,
    cluster: owner?.cluster ?? new Kubectl(cfg.namespace),
    cfg,
    port: Number(env.CP_PORT ?? 8080),
    bind: env.CP_BIND ?? (fixed ? "127.0.0.1" : "0.0.0.0"),
  };
}
