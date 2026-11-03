<script setup lang="ts">
import type { BadgeProps } from "./badge-types";

const props = withDefaults(defineProps<BadgeProps>(), {
  tone: "neutral",
  count: 0,
  tags: () => [],
});
</script>

<template>
  <span class="badge" :data-tone="props.tone" :data-count="props.count">
    {{ props.label }}
    <slot />
  </span>
</template>
