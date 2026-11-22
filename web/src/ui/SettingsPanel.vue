<script setup lang="ts">
import { useUiText } from "./translate";
const t = useUiText();
import { computed, nextTick, onMounted, reactive, ref } from "vue";
import JeevesLogo from "./JeevesLogo.vue";
import ConnectionsPanel from "./ConnectionsPanel.vue";
import type { CapabilityCard } from "../protocol";
import {
  ArrowLeft,
  ChevronDown,
  Cpu,
  Folder,
  Inbox,
  KeyRound,
  Plug,
  Plus,
  Settings2,
  Shield,
  X,
} from "lucide-vue-next";
import type { SettingsActions } from "./types";
import type { ModelInfo, ModelProfileView, SettingsView } from "./types";
const props = defineProps<{
  profiles: ModelProfileView[];
  models: ModelInfo[];
  config?: SettingsView;
  busy: boolean;
  actions: SettingsActions;
  initialTab?: string;
  policy?: import("./types").SessionPermissionResponse;
}>();
const emit = defineEmits<{ close: []; changed: []; mcp: [] }>();
const dialog = ref<HTMLDialogElement>();
const tab = ref(props.initialTab || "general");
const editing = ref(false);
const advanced = ref(false);
const saving = ref(false);
const notice = ref("");
const error = ref(false);
const form = reactive({
  id: "",
  provider: props.models[0]?.provider || "",
  model: "",
  baseUrl: "",
  keyEnv: "",
  apiKey: "",
});
const limits = reactive({
  maxIterations: props.config?.maxIterations ?? 12,
  maxTokens: props.config?.maxTokens ?? 4096,
});
const active = computed(() => props.profiles.find((p) => p.active));
const tabs = [
  { id: "general", name: t("通用"), icon: Settings2 },
  { id: "inbox", name: t("收件箱"), icon: Inbox },
  { id: "models", name: t("模型与连接"), icon: Cpu },
  { id: "permissions", name: t("权限与安全"), icon: Shield },
  { id: "connections", name: t("连接管理"), icon: Plug },
  { id: "data", name: t("数据与存储"), icon: Folder },
];
function source(value: string | null) {
  if (value?.startsWith("env:"))
    return t("环境变量 {name}", { name: value.slice(4) });
  return (
    (
      {
        keychain: t("系统钥匙串"),
        file: t("本地凭据文件"),
        memory: t("仅本次运行"),
        none: t("无需凭据"),
      } as Record<string, string>
    )[value || "none"] ||
    value ||
    t("无需凭据")
  );
}
function edit(profile?: ModelProfileView) {
  Object.assign(form, {
    id: profile?.id || "",
    provider: profile?.provider || props.models[0]?.provider || "",
    model: profile?.model || "",
    baseUrl: profile?.baseUrl || "",
    keyEnv: "",
    apiKey: "",
  });
  editing.value = true;
  advanced.value = !!profile?.baseUrl;
  notice.value = "";
}
async function saveModel() {
  saving.value = true;
  notice.value = "";
  error.value = false;
  try {
    const result = await props.actions.saveModel({
      id: form.id.trim(),
      provider: form.provider,
      model: form.model.trim() || null,
      smallModel: null,
      baseUrl: form.baseUrl.trim() || null,
      keyEnv: form.keyEnv.trim() || null,
      apiKey: form.apiKey || null,
      vision: null,
      makeActive: true,
    });
    notice.value = t("已保存并设为默认模型。凭据：{source}", {
      source: source(result.keyStoredIn),
    });
    editing.value = false;
    emit("changed");
  } catch (e) {
    notice.value = String(e);
    error.value = true;
  } finally {
    saving.value = false;
    form.apiKey = "";
  }
}
async function toggleCapability(
  key: "execEnabled" | "writeEnabled" | "delegateEnabled",
  value: boolean,
) {
  saving.value = true;
  notice.value = "";
  error.value = false;
  try {
    await props.actions.saveConfig({ patch: { [key]: value } });
    notice.value = t("已保存，下一次请求起生效。");
    emit("changed");
  } catch (e) {
    notice.value = String(e);
    error.value = true;
  } finally {
    saving.value = false;
  }
}
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
async function saveLimits() {
  saving.value = true;
  notice.value = "";
  error.value = false;
  try {
    await props.actions.saveConfig({ patch: limits });
    notice.value = t("运行参数已保存，将用于后续请求。");
    emit("changed");
  } catch (e) {
    notice.value = String(e);
    error.value = true;
  } finally {
    saving.value = false;
  }
}
const larkSection = ref<HTMLElement>();
const pageHeading = ref<HTMLElement>();
async function manageConnection(target: "models" | "permissions" | "lark" | "mcp", card: CapabilityCard) {
  if (target === "mcp") {
    dialog.value?.close();
    emit("mcp");
    return;
  }
  if (target === "lark") {
    larkSection.value?.focus();
    return;
  }
  tab.value = target;
  editing.value = false;
  notice.value = "";
  if (target === "models") {
    const profile = props.profiles.find(p => `model:${p.id}` === card.id);
    if (profile) edit(profile);
  }
  await nextTick();
  pageHeading.value?.focus();
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
          editing = false;
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
        <p>
          {{
            tab === "general"
              ? t("管理 Jeeves 的默认行为。")
              : tab === "models"
                ? t("连接你自己的模型，随时切换。")
                : tab === "permissions"
                  ? t("了解 Jeeves 可以访问和执行的内容。")
                  : tab === "connections"
                    ? t("查看服务、模型和工具的连接状态，处理需要你的连接。")
                  : t("你的对话和记忆保存在 Jeeves 服务所在的设备上。")
          }}
        </p>
      </header>
      <div v-if="tab === 'general'" class="preferences-content">
        <h3>{{ t("个人中心") }}</h3>
        <a href="/account/">{{ t("个人中心") }}</a>
        <h3>{{ t("默认模型") }}</h3>
        <div class="setting-row">
          <div>
            <strong>{{
              active?.model || config?.model || t("尚未配置")
            }}</strong>
            <p>
              {{
                active?.provider ||
                config?.provider ||
                t("添加一个模型连接以开始对话")
              }}
            </p>
          </div>
          <button class="quiet-button" @click="tab = 'models'">
            {{ t("管理模型") }}
          </button>
        </div>
        <h3>{{ t("运行参数") }}</h3>
        <form @submit.prevent="saveLimits">
          <label class="setting-row"
            ><div>
              <strong>{{ t("每轮最大步骤") }}</strong>
              <p>{{ t("限制一次请求中的工具执行与推理轮数。") }}</p>
            </div>
            <input
              v-model.number="limits.maxIterations"
              required
              type="number"
              min="1"
              max="1000"
              :aria-label="t('每轮最大步骤')" /></label
          ><label class="setting-row"
            ><div>
              <strong>{{ t("最大输出 Token") }}</strong>
              <p>{{ t("限制模型单次回答的输出长度。") }}</p>
            </div>
            <input
              v-model.number="limits.maxTokens"
              required
              type="number"
              min="1"
              :aria-label="t('最大输出 Token')"
          /></label>
          <div class="form-footer">
            <span>{{ t("修改后用于后续请求") }}</span><button
              class="primary-button"
              :disabled="saving || busy || !config"
            >
              {{ t("保存更改") }}
            </button>
          </div>
        </form>
      </div>
      <div v-if="tab === 'models'" class="preferences-content">
        <template v-if="!editing"
          ><div class="section-heading">
            <h3>
              {{ t("模型连接") }}<span>{{ profiles.length }}</span>
            </h3>
            <button class="quiet-button" :disabled="busy" @click="edit()">
              <Plus :size="15" />{{ t("添加连接") }}
            </button>
          </div>
          <div v-if="!profiles.length" class="settings-empty">
            <Cpu :size="28" stroke-width="1.3" /><strong>{{
              t("连接你的第一个模型")
            }}</strong>
            <p>
              {{ t("支持本地模型和云端服务商。") }}<br />{{
                t("密钥由 Jeeves 的凭据存储管理。")
              }}
            </p>
            <button class="primary-button" :disabled="busy" @click="edit()">
              {{ t("添加模型连接") }}
            </button>
          </div>
          <button
            v-for="profile in profiles"
            :key="profile.id"
            class="model-connection"
            :disabled="busy"
            @click="edit(profile)"
          >
            <span class="connection-icon"><Cpu :size="19" /></span
            ><span class="connection-details"
              ><strong
                >{{ profile.model || profile.id }}
                <span v-if="profile.active" class="soft-badge">{{
                  t("默认")
                }}</span></strong
              ><span>{{ profile.id }} · {{ profile.provider }}</span
              ><small
                ><KeyRound :size="11" />{{ source(profile.keySource) }}</small
              ></span
            ><span class="connection-edit">{{ t("编辑") }}</span>
          </button>
          <p class="settings-footnote">
            {{ t("默认模型对所有对话的后续请求生效。执行期间暂不可修改。") }}
          </p></template
        >
        <form v-else class="connection-form" @submit.prevent="saveModel">
          <button
            type="button"
            class="text-button"
            @click="
              editing = false;
              form.apiKey = '';
            "
          >
            <ArrowLeft :size="14" />{{ t("返回模型连接") }}</button
          ><label
            >{{ t("连接名称")
            }}<input
              v-model="form.id"
              required
              maxlength="80"
              :placeholder="t('例如：工作模型')" /></label
          ><label
            >{{ t("服务商")
            }}<select v-model="form.provider" required>
              <option
                v-for="model in models"
                :key="model.provider"
                :value="model.provider"
              >
                {{ model.label || model.provider }}
              </option>
            </select></label
          ><label
            >{{ t("模型名称")
            }}<input
              v-model="form.model"
              :placeholder="t('留空使用服务商默认模型')" /></label
          ><label
            >API Key<input
              v-model="form.apiKey"
              type="password"
              autocomplete="new-password"
              :placeholder="t('留空保留已有凭据')"
            /><small>{{
              t("只在保存时提交，不会显示已保存的密钥。")
            }}</small></label
          ><button
            type="button"
            class="text-button advanced-toggle"
            @click="advanced = !advanced"
          >
            {{ t("高级连接选项")
            }}<ChevronDown :size="14" :class="{ rotated: advanced }" />
          </button>
          <div v-if="advanced" class="advanced-fields">
            <label
              >{{ t("自定义地址")
              }}<input
                v-model="form.baseUrl"
                type="url"
                placeholder="https://…/v1" /></label
            ><label
              >{{ t("凭据环境变量")
              }}<input
                v-model="form.keyEnv"
                :placeholder="t('例如 OPENAI_API_KEY')"
              /><small>{{
                t("填写变量名后，Jeeves 从运行环境读取凭据。")
              }}</small></label
            >
          </div>
          <div class="form-footer">
            <button
              type="button"
              class="quiet-button"
              @click="
                editing = false;
                form.apiKey = '';
              "
            >
              {{ t("取消") }}</button><button
              class="primary-button"
              :disabled="saving || busy"
            >
              {{ saving ? t("保存中…") : t("保存并设为默认") }}
            </button>
          </div>
        </form>
      </div>
      <div v-if="tab === 'permissions'" class="preferences-content">
        <h3>{{ t("执行权限") }}</h3>
        <div class="permission-summary">
          <Shield :size="22" />
          <div>
            <strong>{{
              policy?.mode === "readOnly"
                ? t("只读")
                : policy?.mode === "standard"
                  ? t("标准权限")
                  : policy?.mode === "fullAccess"
                    ? t("完全访问")
                    : t("沿用运行时权限")
            }}</strong>
            <p>
              {{
                t(
                  "会话预设决定后续操作的确认方式；可用能力仍受启动配置与沙箱限制。",
                )
              }}
            </p>
          </div>
        </div>
        <div class="setting-row">
          <div>
            <strong>{{ t("当前对话的工作目录") }}</strong>
            <p>{{ t("在输入框的工作目录菜单中单独设置。") }}</p>
          </div>
        </div>
        <div class="setting-row">
          <div>
            <strong>{{ t("操作确认") }}</strong>
            <p>
              {{
                t(
                  "需要授权的操作会在对话中展示具体动作，可选择允许一次或拒绝。",
                )
              }}
            </p>
          </div>
        </div>
        <h3>{{ t("能力状态") }}</h3>
        <div
          v-for="entry in [
            {
              key: 'execEnabled',
              name: t('命令执行'),
              enabled: policy?.execEnabled,
            },
            {
              key: 'writeEnabled',
              name: t('文件写入'),
              enabled: policy?.writeEnabled,
            },
            {
              key: 'delegateEnabled',
              name: t('任务委派'),
              enabled: policy?.delegateEnabled,
            },
          ]"
          :key="entry.name"
          class="setting-row"
        >
          <label class="capability-toggle"
            ><input
              type="checkbox"
              :checked="!!entry.enabled"
              :disabled="!policy || saving || busy"
              @change="
                toggleCapability(
                  entry.key as 'execEnabled' | 'writeEnabled' | 'delegateEnabled',
                  ($event.target as HTMLInputElement).checked,
                )
              "
            /><strong>{{ entry.name }}</strong></label
          ><span class="soft-badge">{{
            !policy
              ? t("未连接")
              : !entry.enabled
                ? t("关闭")
                : policy.mode === "readOnly"
                  ? t("只读已限制")
                  : policy.mode === "standard"
                    ? t("需逐次确认")
                    : t("已启用")
          }}</span>
        </div>
        <p v-if="notice" class="notice" :class="{ error }">{{ notice }}</p>
        <p class="settings-footnote">
          {{
            t(
              "在输入框的权限菜单中切换当前会话的预设。切换不会自动开启关闭的能力；已开始的操作需单独中断。只读和标准模式暂停自动钩子与委派，避免绕过逐次确认。",
            )
          }}
        </p>
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
      <div v-if="tab === 'data'" class="preferences-content">
        <h3>{{ t("存储位置") }}</h3>
        <div class="setting-row">
          <div>
            <strong>{{ t("Jeeves 数据目录") }}</strong>
            <p class="path-value">{{ config?.home || t("连接服务后显示") }}</p>
          </div>
        </div>
        <p class="settings-footnote">
          {{
            t("保存会话、记忆和运行记录。工作目录用于当前项目，两者分别管理。")
          }}
        </p>
        <h3>{{ t("凭据") }}</h3>
        <p class="settings-footnote">
          {{
            t(
              "优先读取环境变量，再使用系统钥匙串；不可用时使用受限权限的本地凭据文件。模型连接中会标明实际来源。",
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
