<script setup lang="ts">
import { computed, nextTick, onMounted, ref, useId, watch } from "vue";
import { CalendarDays, Mail } from "lucide-vue-next";
import type { CalendarConnectorSettings, ConnectorSettingsResponse, ConnectorSettingsSaveParams, EmailConnectorSettings } from "../protocol";
import type { SettingsActions } from "./types";
import { useUiText } from "./translate";

const t = useUiText();
const props = defineProps<{ actions: Pick<SettingsActions, "connectorSettings" | "saveConnectorSettings">; vaultNames: string[] }>();
const emit = defineEmits<{ saved: [] }>();
const open = defineModel<"calendar" | "email" | null>({ required: true });
const editorHeading = ref<HTMLElement>();
const rows = ref<HTMLElement>();
const settings = ref<ConnectorSettingsResponse | null>(null);
const loading = ref(false);
const failed = ref(false);
const saving = ref(false);
const saveError = ref(false);
const saved = ref(false);
const credentialList = useId();

// The Runtime already redacts legacy references. Validate new input and vault
// suggestions here for the form, not as a second credential store.
const sourcePattern = "(?:env:[A-Za-z0-9_]+|vault:[A-Z0-9_]{1,40})";
const validSource = new RegExp(`^${sourcePattern}$`);
const vaultReferences = computed(() => props.vaultNames.map(name => `vault:${name}`).filter(value => validSource.test(value)));
const calendar = ref<CalendarConnectorSettings>({ url: "", username: null, credentialSource: null, timezone: "UTC" });
const email = ref<EmailConnectorSettings>({ username: "", from: "", credentialSource: "", imap: { host: "", port: 993, security: "tls" }, smtp: { host: "", port: 465, security: "tls" }, allowRecipients: [], live: false });
const recipients = ref("");
const parsedRecipients = computed(() => [...new Set(recipients.value.split(/[,，\n]/).map(v => v.trim()).filter(Boolean))]);
const legacyPlain = computed(() => email.value.imap.security === "plain" || email.value.smtp.security === "plain");
const calendarSourceMissing = computed(() => Boolean(calendar.value.username?.trim()) !== Boolean(calendar.value.credentialSource?.trim()));
const calendarUrlInvalid = computed(() => {
  if (!calendar.value.url) return false; // native required validation handles empty fields
  try {
    const url = new URL(calendar.value.url);
    return url.protocol !== "https:" || !url.hostname || Boolean(url.username || url.password || url.search || url.hash);
  } catch { return true; }
});
const calendarLegacyPointer = computed(() => {
  const current = settings.value?.calendar;
  return current != null && current.username !== null && current.credentialSource === null;
});
const emailLegacyPointer = computed(() => settings.value?.email?.credentialSource === "");
watch(parsedRecipients, (value) => { if (!value.length) email.value.live = false; });

function reset(kind: "calendar" | "email") {
  if (kind === "calendar") {
    const source = settings.value?.calendar;
    calendar.value = source
      ? { ...source }
      : { url: "", username: null, credentialSource: null, timezone: "UTC" };
  } else {
    const source = settings.value?.email;
    email.value = source
      ? { ...source, imap: { ...source.imap }, smtp: { ...source.smtp }, allowRecipients: [...source.allowRecipients], live: source.live && source.allowRecipients.length > 0 }
      : { username: "", from: "", credentialSource: "", imap: { host: "", port: 993, security: "tls" }, smtp: { host: "", port: 465, security: "tls" }, allowRecipients: [], live: false };
    recipients.value = email.value.allowRecipients.join("\n");
  }
}

async function load() {
  loading.value = true;
  failed.value = false;
  try {
    settings.value = await props.actions.connectorSettings();
    reset("calendar"); reset("email");
    if (open.value) { await nextTick(); editorHeading.value?.focus(); }
  } catch {
    // Do not render arbitrary server errors; they may contain credentials or URLs.
    failed.value = true;
  } finally { loading.value = false; }
}
function select(kind: "calendar" | "email") {
  if (saving.value) return;
  if (open.value === kind) { editorHeading.value?.focus(); return; }
  open.value = kind;
}
defineExpose({ select });
watch(open, async (kind) => {
  saveError.value = false;
  saved.value = false;
  if (kind) { await nextTick(); editorHeading.value?.focus(); }
});
async function close(kind: "calendar" | "email", discard: boolean) {
  if (discard) reset(kind);
  open.value = null;
  await nextTick();
  rows.value?.querySelector<HTMLButtonElement>(`[data-edit=${kind}]`)?.focus();
}
async function save(kind: "calendar" | "email") {
  if (saving.value || !settings.value) return;
  const user = calendar.value.username?.trim() || null;
  const source = calendar.value.credentialSource?.trim() || null;
  const mail = email.value;
  const payload: ConnectorSettingsSaveParams = kind === "calendar"
    ? { calendar: { url: calendar.value.url.trim(), timezone: calendar.value.timezone.trim(), username: user, credentialSource: source }, email: null }
    : { calendar: null, email: { username: mail.username.trim(), from: mail.from.trim(), credentialSource: mail.credentialSource.trim(), imap: { ...mail.imap, host: mail.imap.host.trim() }, smtp: { ...mail.smtp, host: mail.smtp.host.trim() }, allowRecipients: parsedRecipients.value, live: mail.live } };
  saving.value = true;
  saved.value = false;
  saveError.value = false;
  try {
    settings.value = await props.actions.saveConnectorSettings(payload);
    reset(kind);
    await close(kind, false);
    saved.value = true;
    emit("saved");
  } catch { saveError.value = true; }
  finally { saving.value = false; }
}
onMounted(load);
</script>

<template>
  <section class="local-connectors" :aria-label="t('本地连接器')">
    <div class="local-connectors-heading">
      <h3>{{ t('本地连接器') }}</h3>
      <p>{{ t('保存后下次运行生效，不影响正在运行的任务。') }}</p>
    </div>
    <p v-if="loading" class="connection-message" role="status">{{ t('正在读取连接配置…') }}</p>
    <div v-else-if="failed" class="local-connector-failure">
      <p class="connection-message" role="alert">{{ t('无法读取连接配置。请检查 Jeeves 服务后重试。') }}</p>
      <button type="button" class="quiet-button connection-action" @click="load">{{ t('重新检查') }}</button>
    </div>
    <template v-else-if="settings">
      <datalist :id="credentialList"><option v-for="source in vaultReferences" :key="source" :value="source" /></datalist>
      <div ref="rows" class="local-connector-rows">
        <div class="local-connector-row">
          <CalendarDays :size="18" aria-hidden="true" />
          <div><strong>{{ t('日历') }}</strong><p>{{ settings.calendar ? t('已配置 · 只读') : t('尚未配置') }}</p></div>
          <button type="button" class="quiet-button connection-action" data-edit="calendar" :aria-expanded="open === 'calendar'" :disabled="saving" @click="select('calendar')">{{ settings.calendar ? t('编辑配置') : t('添加连接') }}</button>
        </div>
        <div class="local-connector-row">
          <Mail :size="18" aria-hidden="true" />
          <div><strong>{{ t('邮件') }}</strong><p>{{ settings.email ? (settings.email.live && settings.email.allowRecipients.length ? t('已配置 · 经确认发送') : t('已配置 · 不直接发送')) : t('尚未配置') }}</p></div>
          <button type="button" class="quiet-button connection-action" data-edit="email" :aria-expanded="open === 'email'" :disabled="saving" @click="select('email')">{{ settings.email ? t('编辑配置') : t('添加连接') }}</button>
        </div>
      </div>
      <p v-if="saved" class="connection-message" role="status">{{ t('已保存，下一次请求起生效。') }}</p>
      <form v-if="open === 'calendar'" data-connector="calendar" class="local-connector-editor connection-settings-form" @submit.prevent="save('calendar')">
        <h4 ref="editorHeading" tabindex="-1">{{ t('日历配置') }}</h4>
        <p class="connection-message">{{ t('读取 CalDAV 或 ICS 日历，不会修改日程。') }}</p>
        <label>{{ t('日历网址') }}<input v-model.trim="calendar.url" name="url" type="url" placeholder="https://…" required autocomplete="off" :aria-invalid="calendarUrlInvalid" /><small>{{ t('仅支持不含嵌入凭据、查询参数或片段的 HTTPS 网址。') }}</small></label>
        <label>{{ t('时区（IANA）') }}<input v-model.trim="calendar.timezone" name="timezone" placeholder="Asia/Shanghai" required autocomplete="off" /></label>
        <label>{{ t('日历账号（公开日历可留空）') }}<input v-model.trim="calendar.username" name="username" autocomplete="off" /></label>
        <label>{{ t('凭据来源') }}<input v-model.trim="calendar.credentialSource" name="credentialSource" :list="credentialList" :pattern="sourcePattern" :required="Boolean(calendar.username) || calendarLegacyPointer" placeholder="vault:CALENDAR_PASSWORD" autocomplete="off" spellcheck="false" /><small>{{ t('填 vault:名称 或 env:变量名；密码存入下方凭据库，不填在这里。') }}</small></label>
        <p v-if="calendarLegacyPointer" class="connection-message" role="status">{{ t('旧凭据来源未显示，请重新填写受支持的凭据引用。') }}</p>
        <p v-if="calendarSourceMissing" class="connection-message" role="status">{{ t('账号和凭据来源必须一起填写或一起留空。') }}</p>
        <p v-if="saveError" class="connection-message" role="alert">{{ t('配置未保存，请检查字段和服务后重试。') }}</p>
        <div class="form-footer"><button type="button" class="quiet-button connection-action" :disabled="saving" @click="close('calendar', true)">{{ t('取消') }}</button><button type="submit" class="primary-button connection-action" :disabled="saving || calendarUrlInvalid || calendarSourceMissing || (calendarLegacyPointer && !calendar.credentialSource)">{{ saving ? t('保存中…') : t('保存') }}</button></div>
      </form>
      <form v-if="open === 'email'" data-connector="email" class="local-connector-editor connection-settings-form" @submit.prevent="save('email')">
        <h4 ref="editorHeading" tabindex="-1">{{ t('邮件配置') }}</h4>
        <label>{{ t('邮件账号') }}<input v-model.trim="email.username" name="username" required autocomplete="off" /></label>
        <label>{{ t('发件地址') }}<input v-model.trim="email.from" name="from" required placeholder="name@example.com" autocomplete="off" /></label>
        <label>{{ t('凭据来源') }}<input v-model.trim="email.credentialSource" name="credentialSource" :list="credentialList" :pattern="sourcePattern" required placeholder="vault:MAIL_PASSWORD" autocomplete="off" spellcheck="false" /><small>{{ t('填 vault:名称 或 env:变量名；密码存入下方凭据库，不填在这里。') }}</small></label>
        <p v-if="emailLegacyPointer" class="connection-message" role="status">{{ t('旧凭据来源未显示，请重新填写受支持的凭据引用。') }}</p>
        <fieldset class="connector-server-fields">
          <legend>{{ t('接收邮件 · IMAP') }}</legend>
          <label>{{ t('服务器') }}<input v-model.trim="email.imap.host" name="imapHost" required placeholder="imap.example.com" autocomplete="off" /></label>
          <div class="connector-server-options">
            <label>{{ t('端口') }}<input v-model.number="email.imap.port" type="number" name="imapPort" min="1" max="65535" step="1" required /></label>
            <label>{{ t('传输加密') }}<select v-model="email.imap.security" name="imapSecurity"><option v-if="email.imap.security === 'plain'" value="plain" disabled>{{ t('明文（需更改）') }}</option><option value="tls">TLS</option><option value="starttls">STARTTLS</option></select></label>
          </div>
        </fieldset>
        <fieldset class="connector-server-fields">
          <legend>{{ t('发送邮件 · SMTP') }}</legend>
          <label>{{ t('服务器') }}<input v-model.trim="email.smtp.host" name="smtpHost" required placeholder="smtp.example.com" autocomplete="off" /></label>
          <div class="connector-server-options">
            <label>{{ t('端口') }}<input v-model.number="email.smtp.port" type="number" name="smtpPort" min="1" max="65535" step="1" required /></label>
            <label>{{ t('传输加密') }}<select v-model="email.smtp.security" name="smtpSecurity"><option v-if="email.smtp.security === 'plain'" value="plain" disabled>{{ t('明文（需更改）') }}</option><option value="tls">TLS</option><option value="starttls">STARTTLS</option></select></label>
          </div>
        </fieldset>
        <p v-if="legacyPlain" class="connection-message" role="status">{{ t('旧配置使用明文传输。请选择 TLS 或 STARTTLS 后再保存。') }}</p>
        <fieldset class="connector-server-fields connector-authority">
          <legend>{{ t('发送权限') }}</legend>
          <label>{{ t('收件人白名单（每行一个邮箱）') }}<textarea v-model="recipients" name="allowRecipients" rows="2" placeholder="name@example.com" spellcheck="false"></textarea><small>{{ t('留空仅允许读取邮件。') }}</small></label>
          <label class="capability-toggle"><input v-model="email.live" type="checkbox" name="live" :disabled="parsedRecipients.length === 0" />{{ t('允许经人工确认后真实发送') }}</label>
          <p class="connection-message">{{ t('默认只写草稿；开启后每次真实发送仍须确认具体收件人和内容。') }}</p>
        </fieldset>
        <p v-if="saveError" class="connection-message" role="alert">{{ t('配置未保存，请检查字段和服务后重试。') }}</p>
        <div class="form-footer"><button type="button" class="quiet-button connection-action" :disabled="saving" @click="close('email', true)">{{ t('取消') }}</button><button type="submit" class="primary-button connection-action" :disabled="saving || legacyPlain">{{ saving ? t('保存中…') : t('保存') }}</button></div>
      </form>
    </template>
  </section>
</template>
