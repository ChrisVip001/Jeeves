import { t } from "./i18n.ts";
import { el } from "./render.ts";

let pending: Promise<string | null> | null = null;
let declined = false;
export function retryDeploymentToken(): void {
  declined = false;
}
/** Coalesce simultaneous 401s into one keyboard-accessible connection dialog. */
export function askDeploymentToken(): Promise<string | null> {
  if (pending) return pending;
  if (declined) return Promise.resolve(null);
  pending = new Promise((resolve) => {
    const dialog = el("dialog", {
      class: "connect-dialog",
      "aria-label": t("连接 Jeeves"),
    });
    const input = el("input", {
      type: "password",
      required: true,
      autofocus: true,
      autocomplete: "off",
      "aria-label": t("访问令牌"),
    });
    const form = el(
      "form",
      {},
      el("h2", { text: t("连接 Jeeves") }),
      el("p", {
        text: t("此服务需要访问令牌，请输入服务端配置的 JEEVES_OPS_TOKEN。"),
      }),
      el("label", {}, t("访问令牌"), input),
      el("small", {
        text: t("令牌只保存在当前浏览器，用于连接你的 Jeeves 服务。"),
      }),
    );
    const cancel = el("button", {
      type: "button",
      class: "quiet-button",
      text: t("取消"),
    });
    form.append(
      el(
        "footer",
        {},
        cancel,
        el("button", {
          type: "submit",
          class: "primary-button",
          text: t("连接"),
        }),
      ),
    );
    dialog.append(form);
    let result: string | null = null;
    const finish = () => {
      declined = result === null;
      dialog.remove();
      pending = null;
      resolve(result);
    };
    dialog.addEventListener("close", finish, { once: true });
    cancel.addEventListener("click", () => dialog.close());
    form.addEventListener("submit", (event) => {
      event.preventDefault();
      result = input.value.trim();
      if (result) dialog.close();
    });
    document.body.append(dialog);
    dialog.showModal();
  });
  return pending;
}
