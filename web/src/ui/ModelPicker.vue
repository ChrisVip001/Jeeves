<script setup lang="ts">
import { useUiText } from "./translate";
const t = useUiText();
import { computed, ref } from "vue";
import { Check, ChevronDown, Cpu, Plus, Search } from "lucide-vue-next";
import type { ModelProfileView } from "./types";
const props = defineProps<{
  profiles: ModelProfileView[];
  fallback?: string;
  busy: boolean;
}>();
const emit = defineEmits<{ select: [id: string]; manage: [] }>();
const open = ref(false);
const search = ref("");
const active = computed(() => props.profiles.find((p) => p.active));
const filtered = computed(() =>
  props.profiles.filter((p) =>
    `${p.id} ${p.model} ${p.provider}`
      .toLowerCase()
      .includes(search.value.toLowerCase()),
  ),
);
</script>
<template>
  <div class="model-picker" @keydown.esc="open = false">
    <button
      type="button"
      class="model-trigger"
      :aria-expanded="open"
      aria-haspopup="dialog"
      @click="open = !open"
    >
      <span>{{ active?.model || fallback || t("选择模型") }}</span><ChevronDown
        :size="13"
      /></button
    ><template v-if="open"
      ><button
        type="button"
        class="popover-dismiss"
        tabindex="-1"
        :aria-label="t('关闭模型菜单')"
        @click="open = false"
      ></button>
      <section
        class="model-popover"
        role="dialog"
        :aria-label="t('选择默认模型')"
      >
        <div class="picker-search">
          <Search :size="15" /><input
            v-model="search"
            @keydown.enter.prevent
            :placeholder="t('搜索模型')"
            :aria-label="t('搜索模型')"
          />
        </div>
        <p class="popover-label">{{ t("默认模型") }}</p>
        <div class="picker-list">
          <button
            type="button"
            v-for="profile in filtered"
            :key="profile.id"
            class="model-option"
            :disabled="busy"
            @click="
              emit('select', profile.id);
              open = false;
            "
          >
            <Cpu :size="17" /><span
              ><strong>{{ profile.model || profile.id }}</strong
              ><small>{{ profile.provider }} · {{ profile.id }}</small></span
            ><Check v-if="profile.active" :size="16" />
          </button>
          <p v-if="!filtered.length" class="picker-empty">
            {{
              profiles.length ? t("没有匹配的模型") : t("还没有配置模型连接")
            }}
          </p>
        </div>
        <footer>
          <span>{{
            busy ? t("执行期间暂不可切换") : t("对所有对话的后续请求生效")
          }}</span><button
            type="button"
            @click="
              emit('manage');
              open = false;
            "
          >
            <Plus :size="14" />{{ t("管理模型连接") }}
          </button>
        </footer>
      </section></template
    >
  </div>
</template>
