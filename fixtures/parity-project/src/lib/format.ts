/** Collapse whitespace so MDX imports have a deterministic analysis target. */
export function formatLabel(value: string): string {
  return value.trim().replace(/\s+/g, " ");
}
