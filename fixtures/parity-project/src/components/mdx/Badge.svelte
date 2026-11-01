<script lang="ts">
  export let label: string;
  export let count: number = 0;
  export let tone: "neutral" | "accent" = "neutral";
</script>

<span class="badge" data-tone={tone}>{label} ({count})</span>

<style>
  .badge {
    border-radius: 999px;
    padding: 0.125rem 0.5rem;
  }

  .badge[data-tone="accent"] {
    background: #f59e0b;
  }
</style>
