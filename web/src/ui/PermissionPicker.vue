<script setup lang="ts">
import { useUiText } from "./translate";
const t = useUiText();
import { computed, ref } from "vue";
import {
  Check,
  ChevronDown,
  Shield,
  BookOpen,
  SlidersHorizontal,
  Zap,
} from "lucide-vue-next";
import type { PermissionMode, SessionPermissionResponse } from "./types";
const props = defineProps<{
  policy?: SessionPermissionResponse;
  disabled?: boolean;
  saving?: boolean;
  loading?: boolean;
  error?: string;
}>();
const emit = defineEmits<{
  change: [mode: PermissionMode, confirmed: boolean];
  manage: [];
  retry: [];
}>();
const open = ref(false);
const confirmation = ref<HTMLDialogElement>();
const options = [
  {
    mode: "readOnly" as const,
    name: t("只读"),
    description: t("查看与分析，禁止有副作用的工具"),
    icon: BookOpen,
  },
  {
    mode: "standard" as const,
    name: t("标准权限"),
    description: t("有副作用的操作执行前确认"),
    icon: SlidersHorizontal,
  },
  {
    mode: "fullAccess" as const,
    name: t("完全访问"),
    description: t("在已启用能力与约束内自动执行"),
    icon: Zap,
  },
];
const label = computed(
  () =>
    options.find((o) => o.mode === props.policy?.mode)?.name || t("运行时权限"),
);
function choose(mode: PermissionMode) {
  if (mode === props.policy?.mode) {
    open.value = false;
    return;
  }
  if (mode === "fullAccess") {
    open.value = false;
    confirmation.value?.showModal();
  } else {
    emit("change", mode, false);
    open.value = false;
  }
}
</script>
<template>
  <div class="permissions" @keydown.esc="open = false">
    <button
      type="button"
      class="context-button"
      :aria-expanded="open"
      :disabled="disabled || saving"
      @click="open = !open"
    >
      <Shield :size="15" />{{
        saving ? t("正在应用…") : loading ? t("读取权限中…") : label
      }}<ChevronDown :size="13" />
    </button>
    <template v-if="open"
      ><button
        type="button"
        class="popover-dismiss"
        tabindex="-1"
        :aria-label="t('关闭权限菜单')"
        @click="open = false"
      ></button>
      <section
        class="permissions-popover"
        role="dialog"
        :aria-label="t('会话权限')"
      >
        <p class="popover-label">{{ t("当前对话的权限") }}</p>
        <div v-if="!policy" class="permission-status" role="status">
          <p>
            {{ error || (loading ? t("读取权限中…") : t("尚未读取会话权限")) }}
          </p>
          <button
            type="button"
            class="quiet-button"
            :disabled="loading || disabled"
            @click="emit('retry')"
          >
            {{ t("重新读取权限") }}
          </button>
        </div>
        <div class="picker-list">
          <button
            type="button"
            v-for="option in options"
            :key="option.mode"
            class="model-option"
            :disabled="!policy || disabled || saving || loading"
            @click="choose(option.mode)"
          >
            <component :is="option.icon" :size="17" /><span
              ><strong>{{ option.name }}</strong
              ><small>{{ option.description }}</small></span
            ><Check v-if="policy?.mode === option.mode" :size="16" />
          </button>
        </div>
        <footer>
          <p>
            {{
              t("只影响当前对话的后续操作。能力开关、允许规则和沙箱仍然生效。")
            }}
          </p>
          <button
            type="button"
            @click="
              emit('manage');
              open = false;
            "
          >
            {{ t("权限与安全设置") }}
          </button>
        </footer>
      </section></template
    >
    <dialog ref="confirmation" class="permission-confirm">
      <Shield :size="25" />
      <h3>{{ t("允许此对话完全访问？") }}</h3>
      <p>
        {{
          t(
            "Jeeves 将在已启用的能力和授权范围内自动执行操作，减少逐次确认。允许规则、硬性拒绝规则及沙箱仍然生效。",
          )
        }}
      </p>
      <p>
        {{
          t(
            "此设置保存在当前会话中，不会自动开启文件写入、命令执行或系统权限。已发生的操作不会被权限切换撤销。",
          )
        }}
      </p>
      <footer>
        <button
          type="button"
          class="quiet-button"
          @click="confirmation?.close()"
        >
          {{ t("取消") }}</button><button
          type="button"
          class="primary-button"
          @click="
            confirmation?.close();
            emit('change', 'fullAccess', true);
          "
        >
          {{ t("允许完全访问") }}
        </button>
      </footer>
    </dialog>
  </div>
</template>
