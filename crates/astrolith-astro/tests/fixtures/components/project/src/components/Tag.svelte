<script lang="ts">
  /** Badge label. */
  export let label: string;
  /** Colour tone. */
  export let tone: "neutral" | "accent" = "neutral";
  /** Optional counter. */
  export let count: number = 0;
</script>

<span class="tag" data-tone={tone}>{label} ({count})<slot /></span>
