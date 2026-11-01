import type { ReactNode } from "react";

/**
 * Static-analysis fixture for the component builder's React/TypeScript parser.
 * It is never rendered by this Astro project.
 */
export interface AlertProps {
  /** Visual severity of the alert. */
  level?: "info" | "warn" | "error";
  /** Alert content. */
  children?: ReactNode;
  /** Whether the alert offers a dismiss control. */
  dismissible?: boolean;
  /** Optional numeric badge. */
  count?: number;
}

export default function Alert({
  level = "info",
  children,
  dismissible = false,
  count = 0,
}: AlertProps) {
  return (
    <section data-level={level} data-dismissible={dismissible} data-count={count}>
      {children}
    </section>
  );
}
