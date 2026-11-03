export interface BadgeProps {
  /** Badge text. */
  label: string;
  /** Colour tone. */
  tone?: "neutral" | "accent";
  /** Optional counter. */
  count?: number;
  /** Keywords. */
  tags?: string[];
}
