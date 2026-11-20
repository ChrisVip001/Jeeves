<script setup lang="ts">
import { computed, ref } from "vue";
import { preferenceRequest } from "./api";
import { t } from "./i18n";
import type { DeviceGrantView, DeviceListResponse, DevicePairParams, DeviceRevokeParams } from "./protocol";

// Manage approval-only grants here. Changing a multi-capability grant also rotates its identity.
const devices = ref<DeviceGrantView[] | null>(null);
const busy = ref(false);
const key = ref("");
const confirm = ref<DeviceGrantView | null>(null);
const message = ref("");
const active = computed(() => devices.value?.filter(row => !row.revoked && row.capabilities.includes("approvalReview")) ?? []);
const existing = computed(() => devices.value?.find(row => row.deviceId === key.value.trim()));
async function load() {
  if (busy.value) return;
  busy.value = true;
  confirm.value = null;
  message.value = "";
  try { devices.value = (await preferenceRequest<DeviceListResponse>("/api/device")).devices; }
  catch { devices.value = null; message.value = t("无法读取配对设备。"); }
  finally { busy.value = false; }
}
async function change(revoke: boolean) {
  if (!devices.value || busy.value) return;
  const row = revoke ? confirm.value : existing.value;
  if (revoke && (!row || row.capabilities.length !== 1 || row.capabilities[0] !== "approvalReview")) return;
  if (!revoke && (row && !row.revoked || !key.value.trim())) return;
  busy.value = true;
  message.value = "";
  try {
    const params = { deviceId: revoke ? row!.deviceId : key.value.trim(), expectedGeneration: row?.generation ?? 0 };
    const updated = revoke
      ? await preferenceRequest<DeviceGrantView>("/api/device/revoke", params satisfies DeviceRevokeParams)
      : await preferenceRequest<DeviceGrantView>("/api/device/pair", { ...params, capabilities:["approvalReview"] } satisfies DevicePairParams);
    devices.value = [...devices.value.filter(d => d.deviceId !== updated.deviceId), updated];
    key.value = "";
    message.value = revoke ? t("此设备的审批权限已撤销。") : t("已为此设备授予审批权限。");
  } catch {
    devices.value = null;
    message.value = t("无法确认授权是否已更改。请重新读取设备列表。");
  } finally {
    confirm.value = null;
    busy.value = false;
  }
}
</script>
<template>
  <details class="device-pairing" @toggle="($event.currentTarget as HTMLDetailsElement).open && load()">
    <summary>{{ t("管理审批设备") }}</summary>
    <template v-if="devices">
      <form @submit.prevent="change(false)">
        <label class="push-key">
          {{ t("待配对设备的公钥") }}
          <input v-model="key" type="text" required autocomplete="off" autocapitalize="off" spellcheck="false" dir="ltr" />
        </label>
        <p class="settings-footnote">{{ t("请与设备上显示的完整公钥逐字核对。新配对仅授予审批权限。") }}</p>
        <button type="submit" class="quiet-button" :disabled="busy || !key.trim() || (!!existing && !existing.revoked)">
          {{ t("授予审批权限") }}
        </button>
        <p v-if="existing && !existing.revoked" role="status" class="settings-footnote">
          {{ existing.capabilities.includes("approvalReview") ? t("此设备已有审批权限。") : t("此设备已有其他授权。请在设备配对接口中保留原有能力并添加审批权限。") }}
        </p>
      </form>
      <ul v-if="active.length" class="paired-devices">
        <li v-for="device in active" :key="device.deviceId">
          <code dir="ltr">{{ device.deviceId }}</code>
          <p v-if="device.capabilities.length > 1" class="settings-footnote">
            {{ t("此设备还有其他权限。请通过设备配对接口调整授权范围。") }}
          </p>
          <template v-else-if="confirm?.deviceId === device.deviceId">
            <p class="settings-footnote">{{ t("撤销后，此设备已有的审批通知也会失效。") }}</p>
            <button type="button" class="quiet-button" :disabled="busy" @click="confirm = null">{{ t("取消") }}</button>
            <button type="button" class="quiet-button" :disabled="busy" @click="change(true)">{{ t("确认撤销") }}</button>
          </template>
          <button v-else type="button" class="quiet-button" :disabled="busy" @click="confirm = device">{{ t("撤销审批权限") }}</button>
        </li>
      </ul>
    </template>
    <button v-else-if="!busy" type="button" class="quiet-button" @click="load">{{ t("重新读取设备列表") }}</button>
    <p role="status" class="settings-footnote">{{ message }}</p>
  </details>
</template>
<style scoped>
.device-pairing { margin-top: 20px; border-top: 1px solid var(--line); }
.device-pairing summary { padding: 14px 0; cursor: pointer; color: var(--text); font-weight: 500; }
.push-key { display: grid; gap: 6px; }
.push-key input { min-width: 0; width: 100%; box-sizing: border-box; font-family: var(--mono); }
.device-pairing button { min-height: 44px; }
.paired-devices { padding: 0; list-style: none; }
.paired-devices li { padding: 12px 0; border-top: 1px solid var(--line); }
.paired-devices code { display: block; margin-bottom: 8px; overflow-wrap: anywhere; }
.paired-devices button + button { margin-inline-start: 8px; }
</style>
