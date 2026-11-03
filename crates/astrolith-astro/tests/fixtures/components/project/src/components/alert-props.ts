import type { ReactNode } from "react";

export interface AlertProps {
  /** Visual severity of the alert. */
  level?: "info" | "warn" | "error";
  /** Alert content. */
  children?: ReactNode;
  /** Optional numeric badge. */
  count?: number;
  /** Whether the alert offers a dismiss control. */
  dismissible?: boolean;
}
