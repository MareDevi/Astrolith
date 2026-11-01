<script setup lang="ts">
interface Props {
  title: string;
  tone?: "light" | "dark";
  count?: number;
  tags?: string[];
}

const props = withDefaults(defineProps<Props>(), {
  tone: "light",
  count: 0,
  tags: () => [],
});
</script>

<template>
  <article class="vue-card" :data-tone="props.tone" :data-count="props.count">
    <h3>{{ props.title }}</h3>
    <p v-if="props.tags.length">{{ props.tags.join(" · ") }}</p>
    <slot />
  </article>
</template>

<style scoped>
.vue-card[data-tone="dark"] {
  color: white;
  background: #1f2933;
}
</style>
