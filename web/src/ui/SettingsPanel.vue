<script setup lang="ts">
import { useUiText } from "./translate";
const t = useUiText();
import { onMounted, reactive, ref } from "vue";
import JeevesLogo from "./JeevesLogo.vue";
import ConnectionsPanel from "./ConnectionsPanel.vue";
import { Bell, Inbox, Plug, X } from "lucide-vue-next";
import type { SettingsActions } from "./types";
const props = defineProps<{
  busy: boolean;
  actions: SettingsActions;
  initialTab?: "notifications" | "connections" | "inbox";
}>();
const emit = defineEmits<{ close: []; mcp: [] }>();
const dialog = ref<HTMLDialogElement>();
const tab = ref(props.initialTab ?? "notifications");
const saving = ref(false);
const notice = ref("");
const error = ref(false);
const tabs = [
  { id: "notifications", name: t("审批通知"), icon: Bell },
  { id: "connections", name: t("连接管理"), icon: Plug },
  { id: "inbox", name: t("收件箱"), icon: Inbox },
] as const;
const lark = reactive({
  enabled: false,
  appId: "",
  appSecret: "",
  allow: "",
  intl: false,
});
const larkStatus = ref<import("./types").LarkStatus>();
function larkLoaded(status: import("./types").LarkStatus) {
  larkStatus.value = status;
  Object.assign(lark, {
    enabled: status.config.enabled,
    appId: status.config.appId,
    allow: status.config.allow.join(", "),
    intl: status.config.domain === "lark",
    appSecret: "",
  });
}
async function loadLark() {
  try {
    larkLoaded(await props.actions.lark());
  } catch (e) {
    notice.value = String(e);
    error.value = true;
  }
}
async function saveLark(enabled: boolean) {
  saving.value = true;
  notice.value = "";
  error.value = false;
  try {
    larkLoaded(
      await props.actions.saveLark(
        {
          enabled,
          appId: lark.appId,
          allow: lark.allow.split(",").map((id) => id.trim()),
          dispatchers: larkStatus.value?.config.dispatchers ?? [],
          domain: lark.intl ? "lark" : "feishu",
        },
        lark.appSecret || null,
      ),
    );
    notice.value = enabled ? t("渠道已启动。") : t("渠道已停止。");
  } catch (e) {
    notice.value = String(e);
    error.value = true;
  } finally {
    saving.value = false;
  }
}
async function claimLark(openId: string) {
  try {
    larkLoaded(await props.actions.claimLark(openId));
    notice.value = t("已确认为主人。");
  } catch (e) {
    notice.value = String(e);
    error.value = true;
  }
}
const inboxItems = ref<import("./types").InboxItem[] | null>(null);
const inboxError = ref("");
async function loadInbox() {
  try {
    inboxItems.value = (await props.actions.inbox()).items;
    inboxError.value = "";
  } catch (e) {
    inboxItems.value = null;
    inboxError.value = String(e);
  }
}
const secretNames = ref<string[]>([]);
const secret = reactive({ name: "", value: "" });
async function loadVault() {
  try {
    secretNames.value = (await props.actions.vault()).names;
  } catch (e) {
    notice.value = String(e);
    error.value = true;
  }
}
async function saveSecret() {
  try {
    secretNames.value = (
      await props.actions.saveSecret(secret.name, secret.value)
    ).names;
    notice.value = t("凭据已保存，Jeeves 只能使用、看不到。");
    error.value = false;
  } catch (e) {
    notice.value = String(e);
    error.value = true;
  } finally {
    secret.value = "";
    secret.name = "";
  }
}
async function removeSecret(name: string) {
  try {
    secretNames.value = (await props.actions.removeSecret(name)).names;
  } catch (e) {
    notice.value = String(e);
    error.value = true;
  }
}
const larkSection = ref<HTMLElement>();
const pageHeading = ref<HTMLElement>();
function manageConnection(target: "lark" | "mcp") {
  if (target === "lark") larkSection.value?.focus();
  else {
    dialog.value?.close();
    emit("mcp");
  }
}
onMounted(() => {
  dialog.value?.showModal();
  if (tab.value === "connections") pageHeading.value?.focus();
  void loadLark();
  void loadInbox();
  void loadVault();
});
</script>
<template>
  <dialog
    ref="dialog"
    class="preferences"
    @close="emit('close')"
    @click="
      (e) => {
        if (e.target === dialog) dialog?.close();
      }
    "
  >
    <nav class="preferences-nav" :aria-label="t('设置分类')">
      <h2>{{ t("设置") }}</h2>
      <button
        v-for="item in tabs"
        :key="item.id"
        :class="{ active: tab === item.id }"
        :aria-current="tab === item.id ? 'page' : undefined"
        @click="
          tab = item.id;
          notice = '';
        "
      >
        <component :is="item.icon" :size="17" />{{ item.name }}
      </button>
      <div class="preferences-brand">
        <JeevesLogo :size="22" /> <span>Jeeves</span>
      </div>
    </nav>
    <section class="preferences-page">
      <button
        class="preferences-close icon-button"
        :aria-label="t('关闭设置')"
        @click="dialog?.close()"
      >
        <X :size="19" />
      </button>
      <header>
        <h2 ref="pageHeading" tabindex="-1">{{ tabs.find((t) => t.id === tab)?.name }}</h2>
      </header>
      <div v-if="tab === 'notifications'" class="preferences-content">
        <slot name="approval-notifications" />
      </div>
      <div v-if="tab === 'inbox'" class="preferences-content">
        <h3>{{ t("等你处理") }}</h3>
        <p v-if="inboxError" class="notice error" role="alert">{{ inboxError }}</p>
        <p v-else-if="inboxItems?.length === 0">{{ t("没有待处理的事。") }}</p>
        <div v-for="item in inboxItems ?? []" :key="item.kind + item.id" class="setting-row">
          <div>
            <strong>{{ item.title }}</strong>
            <small>
              {{ item.kind === "task" ? t("任务需要你拍板") : t("草稿，未发送") }}
            </small>
            <pre v-if="item.preview" class="inbox-preview">{{ item.preview }}</pre>
          </div>
        </div>
        <button type="button" class="quiet-button" @click="loadInbox">{{ t("刷新") }}</button>
      </div>
      <div v-if="tab === 'connections'" class="preferences-content">
        <ConnectionsPanel :actions="actions" :vault-names="secretNames" @manage="manageConnection" />
        <h3 ref="larkSection" tabindex="-1">{{ t("飞书 / Lark") }}</h3>
        <p>
          {{ t("状态") }}:
          <span class="soft-badge">{{
            larkStatus?.state === "running"
              ? t("运行中")
              : larkStatus?.state === "exited"
                ? t("已退出")
                : t("未启动")
          }}</span>
        </p>
        <form class="connection-settings-form" @submit.prevent="saveLark(true)">
          <label
            >App ID<input v-model="lark.appId" required placeholder="cli_…"
          /></label>
          <label
            >App Secret<input
              v-model="lark.appSecret"
              type="password"
              autocomplete="new-password"
              :placeholder="
                larkStatus?.hasSecret ? t('留空保留已有密钥') : ''
              " /></label
          ><label
            >{{ t("允许对话的人（open_id，逗号分隔）")
            }}<input v-model="lark.allow" placeholder="ou_…" /><small>{{
              t("留空则第一个发消息的人会出现在下方，由你确认为主人。")
            }}</small></label
          ><label class="capability-toggle"
            ><input v-model="lark.intl" type="checkbox" />{{
              t("国际版 Lark")
            }}</label
          >
          <div class="form-footer">
            <button
              type="button"
              class="quiet-button"
              :disabled="saving"
              @click="saveLark(false)"
            >
              {{ t("停止") }}</button
            ><button class="primary-button" :disabled="saving || busy">
              {{ saving ? t("保存中…") : t("保存并启动") }}
            </button>
          </div>
        </form>
        <div v-if="larkStatus?.candidate" class="setting-row">
          <div>
            <strong>{{ t("等待确认的发送者") }}</strong>
            <p>{{ larkStatus.candidate }}</p>
          </div>
          <button
            type="button"
            class="primary-button"
            @click="claimLark(larkStatus.candidate)"
          >
            {{ t("确认为主人") }}
          </button>
        </div>
        <p v-if="notice" class="notice" :class="{ error }">{{ notice }}</p>
        <details v-if="larkStatus?.recent.length">
          <summary>{{ t("最近日志") }}</summary>
          <pre>{{ larkStatus.recent.join("\n") }}</pre>
        </details>
        <h3>{{ t("凭据库") }}</h3>
        <p>{{ t("存下密码或令牌，Jeeves 的命令可以用，但永远读不到原文。") }}</p>
        <div v-for="name in secretNames" :key="name" class="setting-row vault-credential-row">
          <div><strong>{{ name }}</strong><small>$JEEVES_SECRET_{{ name }}</small></div>
          <button type="button" class="quiet-button connection-action" @click="removeSecret(name)">
            {{ t("删除") }}
          </button>
        </div>
        <form class="connection-settings-form" @submit.prevent="saveSecret">
          <label>{{ t("名称（大写字母、数字、下划线）") }}<input v-model="secret.name" required placeholder="GITHUB_TOKEN" /></label>
          <label>{{ t("值") }}<input v-model="secret.value" type="password" autocomplete="new-password" required /></label>
          <div class="form-footer"><button class="primary-button">{{ t("保存凭据") }}</button></div>
        </form>
        <p class="settings-footnote">
          {{
            t(
              "还需要在飞书开放平台开启机器人、订阅 im.message.receive_v1，并选择长连接方式。",
            )
          }}
        </p>
      </div>
      <p
        v-if="notice"
        class="preferences-notice"
        :class="{ error }"
        role="status"
      >
        {{ notice }}
      </p>
    </section>
  </dialog>
</template>
