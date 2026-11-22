<script setup lang="ts">
import { useUiText } from "./translate";
const t = useUiText();
import { ref, watch } from "vue";
import { ChevronDown, Folder } from "lucide-vue-next";
const props = defineProps<{
  cwd?: string;
  busy: boolean;
  canBrowse?: boolean;
  automatic?: boolean;
}>();
const emit = defineEmits<{ save: [path: string]; browse: [] }>();
const open = ref(false);
const path = ref(props.cwd || "");
function toggle() {
  if (props.busy) return;
  if (props.canBrowse && !props.automatic) emit("browse");
  else open.value = !open.value;
}
function save() {
  if (props.busy) return;
  emit("save", path.value);
  open.value = false;
}
watch(
  () => props.cwd,
  (value) => (path.value = value || ""),
);
</script>
<template>
  <div class="workspace-picker" @keydown.esc="open = false">
    <button
      type="button"
      class="context-button"
      :title="cwd"
      :aria-expanded="canBrowse ? undefined : open"
      :disabled="busy"
      @click="toggle"
    >
      <Folder :size="15" />{{
        cwd?.split("/").filter(Boolean).pop() ||
        (automatic ? t("自动工作区") : t("工作目录"))
      }}<ChevronDown :size="13" /></button
    ><template v-if="open"
      ><button
        type="button"
        class="popover-dismiss"
        tabindex="-1"
        :aria-label="t('关闭工作目录菜单')"
        @click="open = false"
      ></button>
      <section
        class="workspace-popover"
        role="dialog"
        :aria-label="t('当前工作目录')"
      >
        <h3>{{ t("当前工作目录") }}</h3>
        <p>
          {{
            automatic
              ? t("每个新任务自动创建独立目录，也可以选择已有项目。")
              : t("仅用于这段对话的项目文件。")
          }}
        </p>
        <template v-if="automatic && canBrowse">
          <button
            type="button"
            class="quiet-button"
            @click="
              open = false;
              emit('browse');
            "
          >
            {{ t("选择已有项目") }}
          </button>
          <button
            type="button"
            class="quiet-button"
            @click="
              open = false;
              emit('save', '');
            "
          >
            {{ t("恢复自动工作区") }}
          </button>
        </template>
        <template v-else>
          <label
            >{{ t("文件夹路径")
            }}<input
              v-model="path"
              @keydown.enter.stop.prevent="save"
              :placeholder="t('/Users/你的用户名/项目')"
              :disabled="busy"
          /></label>
          <footer>
            <button type="button" class="text-button" @click="open = false">
              {{ t("取消") }}</button><button
              type="button"
              class="primary-button"
              :disabled="busy"
              @click="save"
            >
              {{ t("保存") }}
            </button>
          </footer>
        </template>
      </section></template
    >
  </div>
</template>
