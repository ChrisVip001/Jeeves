<script setup lang="ts">
import { Plus } from "lucide-vue-next";
import { useUiText } from "./translate";
const t = useUiText();
defineProps<{ disabled?: boolean }>();
const emit = defineEmits<{ files: [] }>();
</script>
<template>
  <div class="attachment-picker">
    <button
      type="button"
      class="attachment-trigger"
      :disabled="disabled"
      :aria-label="t('添加文件')"
      :title="t('添加文件或图片（每个不超过 20MB）')"
      @click="emit('files')"
    >
      <Plus :size="18" />
    </button>
  </div>
</template>
