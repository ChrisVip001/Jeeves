<script setup lang="ts">
import { computed, ref, watch } from "vue";
import RemoteBrowser from "./RemoteBrowser.vue";
import BrowserDownloads from "./BrowserDownloads.vue";
import { Globe } from "lucide-vue-next";
import { useUiText } from "./translate";
import type { BrowserCenter } from "./useBrowsers";
const props = defineProps<{
  center: BrowserCenter;
  taskId: string;
  cancelled: boolean;
}>();
const t = useUiText();
const url = ref("");
const origins = ref("");
const local = ref(false);
const profile = ref("");
const attach = ref<"local" | "chrome" | "cdp">("local");
const endpoint = ref("http://127.0.0.1:9222");
const extra = ref("");
const remote = ref(false);
const row = computed(() =>
  props.center.state.sessions.find((item) => item.taskId === props.taskId),
);
const active = computed(
  () => row.value && !["closed", "needsAttention"].includes(row.value.status),
);
const busy = computed(() => !!props.center.state.busy[props.taskId]);
const frame = computed(() => props.center.state.frames[props.taskId]);
const nativeChrome = computed(() => row.value?.provider === "chrome");
watch(() => row.value?.status, status => { if (status !== "human") remote.value = false; });
async function takeover() {
  if (!row.value) return;
  await props.center.control(row.value, "takeover");
  if (row.value?.status === "human" && !nativeChrome.value) remote.value = true;
}
function label(status: string) {
  const labels: Record<string, string> = {
    starting: t("正在打开"),
    agent: t("Jeeves 正在使用"),
    human: t("由你控制"),
    paused: t("浏览器已暂停"),
    pausing: t("正在暂停"),
    closed: t("浏览器已关闭"),
    needsAttention: t("请检查中断的操作"),
  };
  return labels[status] || status;
}

async function start() {
  await props.center.start({
    taskId: props.taskId,
    url: url.value,
    origins: origins.value
      .split(/\n/)
      .map((value) => value.trim())
      .filter(Boolean),
    allowLocalhost: attach.value !== "chrome" && local.value,
    profileId: attach.value === "local" ? profile.value.trim() || undefined : undefined,
    provider:
      attach.value === "cdp"
        ? { kind: "cdpAttach", endpoint: endpoint.value.trim() }
        : { kind: attach.value },
  });
}
</script>
<template>
  <details class="jeeves-browser-panel">
    <summary>
      <Globe :size="14" />{{ t("任务浏览器")
      }}<span v-if="row"> · {{ label(row.status) }}</span>
    </summary>
    <p v-if="center.state.errors.list" class="task-error" role="status">
      {{ center.state.errors.list }}
    </p>
    <p v-if="center.state.errors[taskId]" class="task-error" role="status">
      {{ center.state.errors[taskId] }}
    </p>
    <template v-if="active && row">
      <p class="browser-url" :title="row.url">
        {{ row.title || row.url }}<small v-if="row.title">{{ row.url }}</small>
      </p>
      <p v-if="row.profileId" class="meta">
        {{ t("登录档案") }}: <code>{{ row.profileId }}</code>
      </p>
      <p v-if="row.handoffReason" class="meta">{{ row.handoffReason }}</p>
      <p v-if="nativeChrome && row.status === 'human'" class="meta">
        {{ t("请在 Chrome 的任务标签页登录或操作，完成后回到这里交还控制。") }}
      </p>
      <div class="browser-buttons">
        <button
          v-if="row.status !== 'human'"
          type="button"
          class="quiet-button"
          :disabled="busy"
          @click="takeover"
        >
          {{ nativeChrome ? t("在 Chrome 中接管") : t("接管浏览器") }}
        </button>
        <button
          v-if="['human', 'paused'].includes(row.status)"
          type="button"
          class="quiet-button"
          :disabled="busy || cancelled"
          @click="center.control(row, 'return')"
        >
          {{ t("交还给 Jeeves") }}
        </button>
        <button
          v-if="row.status === 'human' && row.profileId"
          type="button"
          class="quiet-button"
          :disabled="busy"
          @click="center.control(row, 'saveProfile')"
        >
          {{ t("保存登录态") }}
        </button>
        <button
          v-if="row.status === 'human' && row.profileId"
          type="button"
          class="text-button"
          :disabled="busy"
          @click="center.control(row, 'forgetProfile')"
        >
          {{ t("清除登录态") }}
        </button>
        <button
          v-if="row.status === 'agent'"
          type="button"
          class="quiet-button"
          :disabled="busy"
          @click="center.control(row, 'pause')"
        >
          {{ t("暂停浏览器") }}
        </button>
        <button
          v-if="!nativeChrome || row.status !== 'human'"
          type="button"
          class="quiet-button"
          :disabled="busy"
          @click="row.status === 'human' ? remote = true : center.frame(taskId)"
        >
          {{ row.status === "human" ? t("远程操作") : t("查看当前画面") }}
        </button>
        <button
          type="button"
          class="text-button"
          :disabled="busy"
          @click="center.control(row, 'close')"
        >
          {{ t("关闭浏览器") }}
        </button>
      </div>
      <BrowserDownloads v-if="!row.provider" :center="center" :row="row" />
      <Teleport to="body">
        <RemoteBrowser v-if="remote && row.status === 'human' && !nativeChrome" :center="center" :row="row" @close="remote = false">
          <template #executor><slot name="executor" /></template>
        </RemoteBrowser>
      </Teleport>
      <details>
        <summary>{{ t("允许的网站") }}</summary>
        <ul>
          <li v-for="origin in row.origins" :key="origin">
            <code>{{ origin }}</code>
          </li>
        </ul>
        <form
          v-if="row.status === 'human'"
          @submit.prevent="center.control(row, 'allowOrigin', extra)"
        >
          <label
            >{{ t("添加网站地址")
            }}<input
              v-model="extra"
              type="url"
              placeholder="https://example.com"
              required
              :disabled="busy"
          /></label>
          <button type="submit" class="quiet-button" :disabled="busy">
            {{ t("允许此网站") }}
          </button>
        </form>
      </details>
      <figure v-if="frame && row.status !== 'human'">
        <img :src="frame.dataUrl" :alt="t('浏览器当前画面')" />
        <figcaption>
          {{ t("只读画面，点击刷新更新") }} ·
          {{ new Date(frame.observedAt).toLocaleTimeString() }}
        </figcaption>
      </figure>
    </template>
    <template v-else>
      <p v-if="row?.error" class="task-error">{{ row.error }}</p>
      <p v-if="!center.state.available" class="task-empty">
        {{ t("浏览器组件尚未安装，安装含浏览器的 Jeeves 版本后可用。") }}
      </p>
      <form v-else-if="!cancelled" @submit.prevent="start">
        <label
          >{{ t("起始网址")
          }}<input
            v-model="url"
            type="url"
            placeholder="https://example.com"
            maxlength="2048"
            required
            :disabled="busy"
        /></label>
        <label
          >{{ t("浏览器来源")
          }}<select v-model="attach" :disabled="busy">
            <option value="local">{{ t("为此任务新开浏览器") }}</option>
            <option v-if="center.state.chromeAvailable" value="chrome">{{ t("使用我的 Chrome") }}</option>
            <option value="cdp">{{ t("接管本机已打开的浏览器") }}</option>
          </select>
        </label>
        <p v-if="attach === 'chrome'" class="task-empty">
          {{ t("在运行中的 Chrome 打开 chrome://inspect/#remote-debugging 启用远程调试，然后在 Chrome 中批准连接。任务会在新标签页使用当前登录状态。") }}
        </p>
        <p v-else-if="attach === 'local'" class="task-empty">
          {{ t("为此任务打开独立浏览器，仅允许访问你选择的网站。登录信息不会沿用日常浏览器；如需保留登录态，请填写登录档案名称。") }}
        </p>
        <label v-if="attach === 'cdp'"
          >{{ t("回环 CDP 地址")
          }}<input
            v-model="endpoint"
            type="text"
            autocomplete="off"
            placeholder="http://127.0.0.1:9222"
            maxlength="512"
            :disabled="busy"
        /></label>
        <label v-if="attach === 'local'"
          >{{ t("登录档案名称（可选）")
          }}<input
            v-model="profile"
            type="text"
            autocomplete="off"
            placeholder="work-shop"
            maxlength="64"
            pattern="[A-Za-z0-9_-]{1,64}"
            :disabled="busy"
        /></label>
        <details>
          <summary>{{ t("更多访问范围") }}</summary>
          <label
            >{{ t("其他网站来源，每行一个")
            }}<textarea
              v-model="origins"
              placeholder="https://accounts.example.com"
              rows="2"
              :disabled="busy"
            />
          </label>
          <label class="browser-checkbox"
            ><input v-model="local" type="checkbox" :disabled="busy" />{{
              t("允许指定的本机测试网站")
            }}</label
          >
        </details>
        <button type="submit" class="quiet-button" :disabled="busy">
          {{ busy ? t("正在打开") : t("打开任务浏览器") }}
        </button>
      </form>
    </template>
  </details>
</template>
<style scoped>
.jeeves-browser-panel {
  margin-top: 14px;
  padding-top: 12px;
  border-top: 1px solid var(--line);
  font-size: 12px;
}
.jeeves-browser-panel > summary {
  display: flex;
  align-items: center;
  gap: 6px;
  cursor: pointer;
  list-style: none;
}
.jeeves-browser-panel form {
  display: grid;
  gap: 10px;
  margin-top: 12px;
}
.jeeves-browser-panel label {
  display: grid;
  gap: 6px;
}
.jeeves-browser-panel input:not([type="checkbox"]),
textarea {
  box-sizing: border-box;
  width: 100%;
  border: 1px solid var(--line);
  border-radius: 7px;
  background: var(--surface);
  color: inherit;
  padding: 8px;
  font: inherit;
}
.jeeves-browser-panel .browser-checkbox {
  display: flex;
  align-items: center;
  gap: 7px;
  margin-top: 10px;
}
.browser-buttons {
  display: flex;
  flex-wrap: wrap;
  gap: 6px;
}
.browser-url {
  overflow-wrap: anywhere;
}
.browser-url small {
  display: block;
  opacity: 0.65;
  margin-top: 4px;
}
.jeeves-browser-panel figure {
  margin: 12px 0 0;
}
.jeeves-browser-panel img {
  width: 100%;
  border: 1px solid var(--line);
  border-radius: 6px;
}
.jeeves-browser-panel figcaption {
  font-size: 11px;
  opacity: 0.65;
  margin-top: 5px;
}
.jeeves-browser-panel code {
  overflow-wrap: anywhere;
}
.jeeves-browser-panel button:disabled {
  opacity: 0.5;
  cursor: wait;
}
.jeeves-browser-panel :focus-visible {
  outline: 2px solid var(--accent);
  outline-offset: 2px;
}
</style>
