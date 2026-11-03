<script lang="ts">
  let {
    title,
    count = 1,
    children,
  }: {
    /** Widget title. */
    title: string;
    /** Number of rows. */
    count?: number;
    children?: unknown;
    /** Optional hint below the title. */
    hint?: string;
  } = $props();
</script>

<div>{title} ({count})<slot /></div>
