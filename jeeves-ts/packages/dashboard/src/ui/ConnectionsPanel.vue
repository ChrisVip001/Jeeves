<script setup lang="ts">
import { computed, onMounted, ref } from "vue";
import { Check, CircleHelp, Cpu, Globe, Link, Plug, RefreshCw, Terminal } from "lucide-vue-next";
import type { CapabilityCard, CapabilityKind, CapabilityState } from "../protocol";
import type { SettingsActions } from "./types";
import { useUiText } from "./translate";
import ConnectorSettings from "./ConnectorSettings.vue";
import ConnectorCatalog from "./ConnectorCatalog.vue";
import ConnectorToolPolicy from "./ConnectorToolPolicy.vue";
import "./connections.css";

const t = useUiText();
const props = defineProps<{ actions: Pick<SettingsActions, "capabilityList" | "capabilityProbe" | "connectorSettings" | "saveConnectorSettings" | "startConnectorOAuth" | "connectorCatalog" | "connectorTools" | "setConnectorTool" | "saveMcpServer">; vaultNames: string[] }>();
const emit = defineEmits<{ manage: [target: "models" | "permissions" | "lark" | "mcp", card: CapabilityCard] }>();
const cards = ref<CapabilityCard[]>([]);
const loading = ref(false);
const pending = ref<string | null>(null);
const error = ref(false);
const probeError = ref<string | null>(null);
const expanded = ref<string | null>(null);
const loaded = ref(false);
const connectorEditor = ref<"calendar" | "email" | null>(null);
const connectorSettingsPanel = ref<InstanceType<typeof ConnectorSettings>>();
const authLink = ref<{ id: string; href: string } | null>(null);
const authError = ref<string | null>(null);
const readyCount = computed(() => cards.value.filter((card) => card.state === "ready").length);
const icons = { model: Cpu, mcp: Plug, connector: Link, channel: Globe, executor: Terminal } satisfies Record<CapabilityKind, unknown>;
const ordered = computed(() => [...cards.value].sort((a, b) => Number(a.state === "ready") - Number(b.state === "ready")));
const existingNames = computed(() => cards.value.filter(card => card.kind === "mcp").map(card => card.id.slice(4)));

function stateLabel(state: CapabilityState) {
  return {
    ready: t("可用"), unauthorized: t("未授权"), expired: t("已过期"),
    missingScope: t("权限不足"), blockedByPolicy: t("策略限制"),
    offline: t("无法连接"), unknown: t("未知"),
  }[state] ?? t("未知");
}
function kindLabel(kind: CapabilityKind) {
  return {
    model: t("模型"), connector: t("服务连接器"), mcp: t("MCP 连接器"),
    channel: t("消息渠道"), executor: t("执行器"),
  }[kind];
}
// The protocol promises a pointer, not a value; reject anything outside its documented
// forms. Free-form backend errors are not rendered (a remote service can echo credentials).
function pointer(value: string | null) {
  if (value === "keychain") return t("系统钥匙串");
  if (value === "file") return t("本地凭据文件");
  if (value === "memory") return t("运行时内存（临时）");
  if (value === "vault") return t("凭据库（位置未确认）");
  if (value && /^env:[A-Za-z_][A-Za-z0-9_]*$/.test(value))
    return t("环境变量 {name}", { name: value.slice(4) });
  return t("未提供凭据位置");
}
function explanation(card: CapabilityCard) {
  switch (card.state) {
    case "ready":
      if (card.detail === "drafts only (live is off)" || card.detail === "reads now; sends write drafts only (live is off)") return t("发送仅生成草稿。");
      if (card.detail === "read only: the send allowlist is empty") return t("仅可读取，发送白名单为空。");
      return t("已通过本地连接检查。");
    case "unauthorized": return t("缺少有效凭据或登录。");
    case "expired": return t("登录已过期，需要重新授权。");
    case "missingScope": return t("服务尚未授予所需权限。");
    case "blockedByPolicy": return t("当前安全策略不允许使用。");
    case "offline": return t("尚未建立连接，请检查服务后重试。");
    default: return t("尚无法确认状态，请重新检查。");
  }
}
function actionLabel(card: CapabilityCard) {
  if (editableConnector(card) && ["unauthorized", "blockedByPolicy"].includes(card.state)) return t("编辑配置");
  if (oauthAction(card)) return t("开始授权");
  if (card.state === "unauthorized") return manageTarget(card) ? t("修复连接") : t("查看修复步骤");
  if (card.state === "expired") return t("查看授权步骤");
  if (card.state === "missingScope") return t("检查权限");
  if (card.state === "blockedByPolicy") return t("查看安全策略");
  return t("检查状态");
}
function editableConnector(card: CapabilityCard) {
  if (card.id === "connector:email") return "email";
  if (card.id === "connector:calendar") return "calendar";
  return null;
}
function oauthAction(card: CapabilityCard) {
  if (!["unauthorized", "expired", "missingScope"].includes(card.state)) return false;
  return (card.kind === "mcp" && card.secretPointer === "file")
    || (card.id === "connector:feishu" && (card.state === "expired" || card.detail?.startsWith("no user sign-in:")));
}
function isProbe(card: CapabilityCard) {
  return ["ready", "offline", "unknown"].includes(card.state);
}
function manageTarget(card: CapabilityCard) {
  if (card.state === "blockedByPolicy") return card.kind === "executor" ? "permissions" : null;
  if (card.state !== "unauthorized") return null;
  if (card.kind === "model") return "models";
  if (card.id === "channel:lark") return "lark";
  if (card.kind === "mcp" && card.secretPointer !== "file") return "mcp";
  return null;
}
function instruction(card: CapabilityCard) {
  if (card.state === "blockedByPolicy") {
    if (card.kind === "connector") return t("在 Jeeves 所在设备的 connectors.json 中核对该服务的白名单及配置，按需修改后重启 Jeeves，再检查连接。不要扩大不需要的权限。");
    return t("核对该能力的安全策略，再重新检查连接。没有合适的授权时保持关闭。");
  }
  if (card.state === "missingScope") {
    if (card.kind === "mcp" && card.secretPointer === "file") return t("在服务的授权设置中核对需要的权限，再在 Jeeves 所在设备运行 jeeves mcp login <连接器名> 重新授权。完成后检查连接。");
    return t("在服务的授权设置中核对所需权限，然后重新检查连接。");
  }
  if (card.kind === "mcp") {
    if (card.state === "expired" || card.secretPointer === "file") return t("在 Jeeves 所在设备的终端运行 jeeves mcp login <连接器名>，在浏览器完成授权后重新检查。此处不会显示授权令牌。");
    return t("在 MCP 连接器中核对认证方式与凭据来源，保存后重新检查。");
  }
  if (card.id === "connector:feishu" && (card.state === "expired" || card.detail?.startsWith("no user sign-in:"))) return t("在 Jeeves 所在设备的终端运行 jeeves connector feishu login，在浏览器完成授权后重新检查。");
  if (card.kind === "connector") return t("在 Jeeves 所在设备检查 connectors.json 中的连接配置与凭据来源。修改配置后需重启 Jeeves，再检查连接。");
  return t("在对应服务中完成授权，再重新检查连接。");
}
function dateOf(seconds: number | null) {
  const date = new Date((seconds ?? 0) * 1000);
  return seconds != null && Number.isFinite(date.getTime()) ? date.toLocaleString(document.documentElement.lang || undefined) : t("尚未检查");
}
async function load() {
  loading.value = true;
  error.value = false;
  authLink.value = null;
  authError.value = null;
  expanded.value = null;
  try {
    cards.value = (await props.actions.capabilityList()).cards;
    loaded.value = true;
  } catch {
    error.value = true;
  } finally {
    loading.value = false;
  }
}
async function act(card: CapabilityCard) {
  const editable = editableConnector(card);
  if (editable && ["unauthorized", "blockedByPolicy"].includes(card.state)) {
    connectorSettingsPanel.value?.select(editable);
    return;
  }
  if (oauthAction(card)) {
    pending.value = card.id;
    authLink.value = null;
    authError.value = null;
    expanded.value = card.id;
    try {
      // The runtime owns the callback and returns the provider's actual authorization URL.
      // Never construct a URL from a card, nor navigate automatically to a remote service.
      const result = await props.actions.startConnectorOAuth(card.id);
      const url = new URL(result.authorizationUrl);
      if (url.protocol !== "https:" || url.username || url.password) throw new Error("Invalid authorization URL");
      authLink.value = { id: card.id, href: url.href };
    } catch { authError.value = card.id; }
    finally { pending.value = null; }
    return;
  }
  if (isProbe(card)) {
    pending.value = card.id;
    probeError.value = null;
    try {
      const result = await props.actions.capabilityProbe(card.id);
      cards.value = cards.value.map((current) => current.id === card.id ? result.card : current);
    } catch {
      probeError.value = card.id;
    } finally {
      pending.value = null;
    }
  } else {
    const target = manageTarget(card);
    if (target) emit("manage", target, card);
    else expanded.value = expanded.value === card.id ? null : card.id;
  }
}
onMounted(load);
</script>

<template>
  <section class="connections-page" :aria-label="t('连接状态')">
    <div class="connections-summary">
      <div>
        <h3>{{ t('连接状态') }}</h3>
        <p v-if="loaded" class="connection-tally" aria-live="polite">{{ t('{ready} / {total} 项可用', { ready: readyCount, total: cards.length }) }}</p>
        <p>{{ t('状态来自本地检查，不代表远程服务已接受请求；检查结果最多缓存 60 秒。') }}</p>
      </div>
      <button type="button" class="quiet-button connection-action" :disabled="loading || pending !== null" @click="load">
        <RefreshCw :size="14" aria-hidden="true" />{{ loading ? t('检查中…') : t('重新检查') }}
      </button>
    </div>
    <p v-if="error" class="connection-message" role="alert">{{ t('读取连接状态失败。请确认 Jeeves 服务已更新并重试。') }}</p>
    <p v-else-if="loading && !loaded" class="connection-message" role="status">{{ t('正在读取连接状态…') }}</p>
    <p v-else-if="loaded && !cards.length" class="connection-message">{{ t('尚无已配置的能力。你可以在下方或模型设置中添加连接。') }}</p>
    <ConnectorSettings ref="connectorSettingsPanel" v-model="connectorEditor" :actions="actions" :vault-names="vaultNames" @saved="load" />
    <ConnectorCatalog :actions="actions" :existing-names="existingNames" :can-add="loaded && !loading && !error" @saved="load" />
    <div v-if="cards.length" class="capability-list">
      <article v-for="card in ordered" :key="card.id" class="capability-card" :class="{ 'needs-attention': ['unauthorized', 'expired', 'missingScope', 'blockedByPolicy'].includes(card.state) }">
        <div class="capability-card-title">
          <component :is="icons[card.kind] ?? CircleHelp" :size="17" aria-hidden="true" />
          <div><h4>{{ card.title }}</h4><p>{{ kindLabel(card.kind) }}<template v-if="card.identity"> · {{ card.identity }}</template></p></div>
          <span class="capability-state" role="status"><Check v-if="card.state === 'ready'" :size="14" aria-hidden="true" /><span v-else class="capability-dot" aria-hidden="true"></span>{{ stateLabel(card.state) }}</span>
        </div>
        <p class="capability-explanation">{{ explanation(card) }}</p>
        <dl class="capability-meta">
          <div><dt>{{ t('凭据位置') }}</dt><dd>{{ pointer(card.secretPointer) }}</dd></div>
          <div><dt>{{ t('最近检查') }}</dt><dd>{{ dateOf(card.lastCheckedAt) }}</dd></div>
        </dl>
        <div class="capability-footer">
          <button type="button" class="quiet-button connection-action" :aria-expanded="!isProbe(card) && !manageTarget(card) ? (editableConnector(card) && ['unauthorized', 'blockedByPolicy'].includes(card.state) ? connectorEditor === editableConnector(card) : expanded === card.id) : undefined" :disabled="loading || pending !== null" @click="act(card)">{{ pending === card.id ? (oauthAction(card) ? t('正在准备授权…') : t('检查中…')) : actionLabel(card) }}</button>
        </div>
        <div v-if="expanded === card.id && oauthAction(card)" class="capability-instruction">
          <template v-if="authLink?.id === card.id">
            <p role="status">{{ t('请在 Jeeves 所在设备打开授权页面，完成后点“重新检查”。') }}</p>
            <a class="quiet-button connection-action oauth-link" :href="authLink.href" target="_blank" rel="noopener noreferrer">{{ t('打开授权页面') }}</a>
          </template>
          <template v-else-if="authError === card.id">
            <p role="alert">{{ t('未能开始授权，可重试或使用以下终端步骤。') }}</p>
            <p>{{ instruction(card) }}</p>
          </template>
          <p v-else-if="pending === card.id" role="status">{{ t('正在准备授权…') }}</p>
        </div>
        <p v-else-if="expanded === card.id" class="capability-instruction" role="status">{{ instruction(card) }}</p>
        <p v-if="probeError === card.id" class="connection-message" role="alert">{{ t('检查未成功，请重试。') }}</p>
        <ConnectorToolPolicy v-if="card.kind === 'mcp'" :id="card.id" :actions="actions" />
      </article>
    </div>
  </section>
</template>
