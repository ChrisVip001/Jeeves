<script setup lang="ts">
import { computed, nextTick, onMounted, ref } from "vue";
import { Plus } from "lucide-vue-next";
import type { ConnectorCatalogEntry, ConnectorCategory } from "../protocol";
import type { McpServerSpec } from "../api";
import type { SettingsActions } from "./types";
import { useUiText } from "./translate";

const t = useUiText();
const props = defineProps<{
  actions: Pick<SettingsActions, "connectorCatalog" | "saveMcpServer">;
  existingNames: string[];
  canAdd: boolean;
}>();
const emit = defineEmits<{ saved: [] }>();
const entries = ref<ConnectorCatalogEntry[]>([]);
const loading = ref(false);
const loadFailed = ref(false);
const editor = ref(false);
const entry = ref<ConnectorCatalogEntry | null>(null);
const name = ref("");
const transport = ref<"http" | "stdio">("http");
const url = ref("");
const auth = ref<"none" | "bearer" | "oauth">("none");
const authEnv = ref("");
const command = ref("");
const args = ref("");
const saving = ref(false);
const saveFailed = ref(false);
const saved = ref(false);
const heading = ref<HTMLElement>();
const sectionHeading = ref<HTMLElement>();
let origin: HTMLElement | null = null;

const duplicate = computed(() => props.existingNames.includes(name.value));
const validUrl = computed(() => {
  try {
    const parsed = new URL(url.value);
    return (parsed.protocol === "https:" || (parsed.protocol === "http:" && auth.value === "none"))
      && Boolean(parsed.hostname) && !parsed.username && !parsed.password && !parsed.search && !parsed.hash;
  } catch { return false; }
});
const valid = computed(() => props.canAdd && /^[a-zA-Z0-9_-]{1,64}$/.test(name.value) && !duplicate.value
  && (transport.value === "stdio" ? Boolean(command.value.trim()) : validUrl.value && (auth.value !== "bearer" || /^[A-Z_][A-Z0-9_]*$/.test(authEnv.value))));

function categoryLabel(category: ConnectorCategory) {
  return { calendar: t("日历"), mail: t("邮件"), tasks: t("任务"), notes: t("笔记"), files: t("文件"), contacts: t("联系人"), maps: t("地图"), other: t("其他") }[category];
}
function setupLabel(item: ConnectorCatalogEntry) {
  switch (item.setup) {
    case "builtin": return t("内建连接器，通过本地设置配置。");
    case "providerSetup": return t("需要先在服务商处配置，不支持直接添加。");
    case "bearer": return t("需要 Bearer 令牌的环境变量。");
    case "oauth": return t("保存后可发起 OAuth 授权。");
    default: return t("无需认证。");
  }
}
function canConfigure(item: ConnectorCatalogEntry) {
  return item.endpoint !== null && ["none", "bearer", "oauth"].includes(item.setup);
}
function docsLink(item: ConnectorCatalogEntry) {
  const parsed = new URL(item.documentationUrl);
  return parsed.protocol === "https:" && !parsed.username && !parsed.password ? parsed.href : undefined;
}
async function load() {
  loading.value = true;
  loadFailed.value = false;
  try { entries.value = (await props.actions.connectorCatalog()).entries; }
  catch { loadFailed.value = true; }
  finally { loading.value = false; }
}
async function start(item: ConnectorCatalogEntry | null, event: MouseEvent) {
  if (!props.canAdd || saving.value) return;
  origin = event.currentTarget as HTMLElement;
  entry.value = item;
  name.value = item?.id ?? "";
  url.value = item?.endpoint ?? "";
  transport.value = "http";
  auth.value = item?.setup === "oauth" || item?.setup === "bearer" ? item.setup : "none";
  command.value = args.value = authEnv.value = "";
  saveFailed.value = saved.value = false;
  editor.value = true;
  await nextTick();
  heading.value?.focus();
}
async function close() {
  editor.value = false;
  authEnv.value = url.value = command.value = args.value = "";
  await nextTick();
  if (!saved.value) origin?.focus();
  else sectionHeading.value?.focus();
}
async function save() {
  if (!valid.value || saving.value) return;
  saving.value = true;
  saveFailed.value = false;
  const spec: McpServerSpec = { name: name.value, createOnly: true };
  if (entry.value) spec.manifest = entry.value.manifest;
  if (transport.value === "stdio") {
    spec.command = command.value.trim();
    spec.args = args.value.split(/\r?\n/).filter(line => line.trim() !== "");
  } else {
    spec.url = url.value;
    if (auth.value === "bearer") spec.auth_env = authEnv.value;
    if (auth.value === "oauth") spec.oauth = true;
  }
  try {
    const result = await props.actions.saveMcpServer(spec);
    if (!result.ok) throw new Error("Save refused");
    saved.value = true;
    await close();
    emit("saved");
  } catch { saveFailed.value = true; }
  finally { saving.value = false; }
}
onMounted(load);
</script>

<template>
  <section class="mcp-connections" :aria-label="t('添加 MCP 连接')">
    <div class="connections-summary">
      <div>
        <h3 ref="sectionHeading" tabindex="-1">{{ t('添加 MCP 连接') }}</h3>
        <p>{{ t('从精选目录配置，或添加自己的 MCP 服务。') }}</p>
      </div>
      <button data-add-mcp type="button" class="quiet-button connection-action" :disabled="!canAdd || editor" :aria-expanded="editor && !entry" @click="start(null, $event)"><Plus :size="14" aria-hidden="true" />{{ t('自定义连接') }}</button>
    </div>
    <p v-if="saved" class="connection-message" role="status">{{ t('连接配置已保存，下次运行生效。请从能力卡检查连接或开始授权。') }}</p>
    <form v-if="editor" class="connection-settings-form local-connector-editor" data-connector="mcp" @submit.prevent="save">
      <h4 ref="heading" tabindex="-1">{{ entry ? t('配置 {name}', { name: entry.title }) : t('自定义连接') }}</h4>
      <p v-if="entry" class="connection-message">{{ setupLabel(entry) }}</p>
      <label>{{ t('连接名称') }}<input v-model.trim="name" name="name" required pattern="[a-zA-Z0-9_\-]{1,64}" autocomplete="off" spellcheck="false" :disabled="saving" placeholder="my-service" /><small>{{ t('名称限 1–64 个英文字母、数字、下划线或连字符。') }}</small><small v-if="duplicate" role="status">{{ t('此名称已存在，请使用其他名称。') }}</small></label>
      <label>{{ t('连接方式') }}<select v-model="transport" name="transport" :disabled="!!entry || saving"><option value="http">{{ t('远程 HTTP / HTTPS') }}</option><option value="stdio">{{ t('本机命令（stdio）') }}</option></select></label>
      <template v-if="transport === 'http'">
        <label>{{ t('MCP 服务网址') }}<input v-model.trim="url" name="url" type="url" required :readonly="!!entry" :disabled="saving" placeholder="https://mcp.example.com/mcp" autocomplete="off" spellcheck="false" /><small>{{ t('网址不可包含凭据、查询参数或片段；认证需要 HTTPS。') }}</small></label>
        <label>{{ t('认证方式') }}<select name="auth" v-model="auth" :disabled="!!entry || saving"><option value="none">{{ t('无认证') }}</option><option value="bearer">{{ t('Bearer（环境变量）') }}</option><option value="oauth">OAuth</option></select></label>
        <label v-if="auth === 'bearer'">{{ t('鉴权环境变量名') }}<input v-model.trim="authEnv" name="authEnv" required pattern="[A-Z_][A-Z0-9_]*" autocomplete="off" spellcheck="false" placeholder="MY_SERVICE_TOKEN" :disabled="saving" /><small>{{ t('填写运行环境中的变量名，不填写令牌原文。') }}</small></label>
        <p v-else-if="auth === 'oauth'" class="connection-message">{{ t('保存配置不会登录。请在能力卡发起真实的授权流程。') }}</p>
      </template>
      <template v-else>
        <label>{{ t('启动命令') }}<input v-model.trim="command" name="command" required autocomplete="off" spellcheck="false" :disabled="saving" /></label>
        <label>{{ t('命令参数（每行一个）') }}<textarea name="args" v-model="args" rows="3" autocomplete="off" spellcheck="false" :disabled="saving" /><small>{{ t('Jeeves 会在本机启动该命令。仅使用你信任的程序，不在命令和参数中填写密钥。') }}</small></label>
      </template>
      <p v-if="!entry" class="connection-message">{{ t('自定义工具没有声明 effect 时，运行时按写入权限处理。') }}</p>
      <p v-if="saveFailed" role="alert" class="connection-message">{{ t('配置未保存，请检查字段和服务后重试。') }}</p>
      <div class="form-footer"><button type="button" class="quiet-button connection-action" :disabled="saving" @click="close">{{ t('取消') }}</button><button type="submit" class="primary-button connection-action" :disabled="!valid || saving">{{ saving ? t('保存中…') : t('保存') }}</button></div>
    </form>
    <details class="connector-catalog">
      <summary>{{ t('精选连接器目录') }} <span v-if="entries.length">{{ entries.length }}</span></summary>
      <p v-if="loading" class="connection-message" role="status">{{ t('正在读取连接器目录…') }}</p>
      <div v-else-if="loadFailed" class="local-connector-failure"><p class="connection-message" role="alert">{{ t('无法读取连接器目录，请重试。') }}</p><button type="button" class="quiet-button connection-action" @click="load">{{ t('重新检查') }}</button></div>
      <div v-else class="catalog-list">
        <article v-for="item in entries" :key="item.id" class="catalog-row">
          <div>
            <h4>{{ item.title }} <small>{{ categoryLabel(item.manifest.category) }}</small></h4>
            <p>{{ setupLabel(item) }}</p>
            <a v-if="docsLink(item)" :href="docsLink(item)" target="_blank" rel="noopener noreferrer">{{ t('服务文档') }}</a>
          </div>
          <button v-if="canConfigure(item)" type="button" class="quiet-button connection-action" :disabled="!canAdd || editor || existingNames.includes(item.id)" @click="start(item, $event)">{{ existingNames.includes(item.id) ? t('已配置') : t('配置') }}</button>
        </article>
      </div>
    </details>
  </section>
</template>
