<script setup lang="ts">
import { ref } from "vue";
import type { ConnectorToolsResponse, ConnectorToolView, ToolEffect } from "../protocol";
import type { SettingsActions } from "./types";
import { useUiText } from "./translate";

const props = defineProps<{ id: string; actions: Pick<SettingsActions, "connectorTools" | "setConnectorTool"> }>();
const t = useUiText();
const policy = ref<ConnectorToolsResponse | null>(null);
const loading = ref(false);
const pending = ref<string | null>(null);
const loadFailed = ref(false);
const saveFailed = ref(false);
const saved = ref(false);

function effectLabel(effect: ToolEffect) {
  return { read: t("读取"), draft: t("生成草稿"), write: t("写入"), send: t("发送"), delete: t("删除") }[effect];
}
async function load() {
  if (loading.value || pending.value) return;
  loading.value = true;
  loadFailed.value = saveFailed.value = saved.value = false;
  try { policy.value = await props.actions.connectorTools(props.id); }
  catch { loadFailed.value = true; policy.value = null; }
  finally { loading.value = false; }
}
function toggle(event: Event) {
  if ((event.currentTarget as HTMLDetailsElement).open) void load();
}
async function change(tool: ConnectorToolView, event: Event) {
  const input = event.currentTarget as HTMLInputElement;
  const disabled = !input.checked;
  // Don't claim a save while the runtime has not accepted it.
  input.checked = !tool.disabled;
  pending.value = tool.name;
  saved.value = saveFailed.value = false;
  try {
    policy.value = await props.actions.setConnectorTool({ id: props.id, tool: tool.name, disabled });
    saved.value = true;
  } catch { saveFailed.value = true; }
  finally { pending.value = null; }
}
</script>
<template>
  <details class="tool-policy" @toggle="toggle">
    <summary>{{ t('工具权限') }}</summary>
    <p v-if="loading" class="connection-message" role="status">{{ t('正在读取工具权限…') }}</p>
    <div v-else-if="loadFailed" class="local-connector-failure"><p role="alert" class="connection-message">{{ t('无法读取工具权限，请重试。') }}</p><button type="button" class="quiet-button connection-action" @click="load">{{ t('重新检查') }}</button></div>
    <template v-else-if="policy">
      <p class="connection-message">{{ policy.editable ? t('启用仅表示可供 Jeeves 使用，仍受运行时权限和审批约束。') : t('此连接由内置配置提供，工具权限在此只读。') }}</p>
      <p v-if="!policy.tools.length" class="connection-message">{{ t('尚无已知工具。连接成功并运行任务后再检查。') }}</p>
      <ul v-else class="tool-policy-list">
        <li v-for="tool in policy.tools" :key="tool.name">
          <label v-if="policy.editable" class="tool-policy-row">
            <span><strong>{{ tool.name }}</strong><small>{{ effectLabel(tool.effect) }} · {{ tool.disabled ? t('已停用') : t('已启用') }}</small></span>
            <input type="checkbox" :checked="!tool.disabled" :disabled="pending !== null" :aria-label="t('启用 {name}', { name: tool.name })" @change="change(tool, $event)" />
          </label>
          <div v-else class="tool-policy-row"><span><strong>{{ tool.name }}</strong><small>{{ effectLabel(tool.effect) }}</small></span><small>{{ tool.disabled ? t('已停用') : t('已启用') }}</small></div>
        </li>
      </ul>
      <p v-if="pending !== null" role="status" class="connection-message">{{ t('保存中…') }}</p>
      <p v-else-if="saved" role="status" class="connection-message">{{ t('工具权限已保存，下次运行生效。') }}</p>
      <p v-else-if="saveFailed" role="alert" class="connection-message">{{ t('工具权限未保存，请重试。') }}</p>
    </template>
  </details>
</template>
