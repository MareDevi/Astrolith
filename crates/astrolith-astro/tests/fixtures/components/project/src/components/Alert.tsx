import type { AlertProps } from "./alert-props";

/**
 * Static-analysis fixture for the React/TypeScript parser. It is never rendered.
 */
export default function Alert({
  level = "info",
  children,
  count = 0,
  dismissible = false,
}: AlertProps) {
  return (
    <section data-level={level} data-count={count}>
      {children}
    </section>
  );
}
