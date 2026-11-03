<script setup lang="ts">
const props = defineProps({
  /** Pill text. */
  text: { type: String, required: true },
  /** Colour tone. */
  tone: { type: String, default: "neutral" },
  count: Number,
});
</script>

<template>
  <span class="pill" :data-tone="props.tone"><slot />{{ props.text }}</span>
</template>
