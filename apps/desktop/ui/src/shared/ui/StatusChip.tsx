import { chipColors } from "../statusColor";
import { useTheme } from "../useTheme";

interface Props {
  /** OpenProject status name */
  name: string;
  /** `#rrggbb` from OpenProject, if it has one */
  color?: string;
  title?: string;
  className?: string;
}

/** OpenProject status label in the status's own colour (dot) with readable text. */
export function StatusChip({ name, color, title, className }: Props) {
  const { mode } = useTheme();
  const c = chipColors(name, color, mode);
  return (
    <span
      className={`chip status-chip${className ? ` ${className}` : ""}`}
      style={{ color: c.ink, background: c.tint }}
      title={title ?? name}
    >
      <i className="status-chip-dot" style={{ background: c.dot }} />
      {name}
    </span>
  );
}
