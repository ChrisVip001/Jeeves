<script setup lang="ts">
import icon from "./assets/jeeves-icon.svg";

withDefaults(defineProps<{ size?: number }>(), { size: 32 });
</script>

<template>
  <img
    :src="icon"
    :width="size"
    :height="size"
    class="jeeves-logo"
    alt=""
    aria-hidden="true"
    draggable="false"
  />
</template>

<style scoped>
.jeeves-logo {
  display: block;
  flex: none;
}
</style>
