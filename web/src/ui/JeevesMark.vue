<script setup lang="ts">
import { computed } from "vue";
import { useUiText } from "./translate";
import type { MarkState } from "./today";
const t = useUiText();
const props = defineProps<{ state: MarkState }>();
const label = computed(
  () =>
    ({
      working: t("Jeeves：在做事"),
      waiting: t("Jeeves：等你"),
      done: t("Jeeves：都做完了"),
    })[props.state],
);
</script>
<template>
  <!-- The one element that moves on its own: breathes while working, amber while waiting,
       folds in when everything is done. -->
  <svg
    class="jeeves-mark"
    :data-state="state"
    viewBox="0 0 28 28"
    role="img"
    :aria-label="label"
  >
    <circle class="ring" cx="14" cy="14" r="11" />
    <circle class="core" cx="14" cy="14" r="4" />
  </svg>
</template>
