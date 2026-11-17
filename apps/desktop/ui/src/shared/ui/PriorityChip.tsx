import { vi } from "../../i18n/vi";
import type { PriorityView } from "../../ipc/types";
import { StatusChip } from "./StatusChip";

/** The task's priority as OpenProject shows it: its name in the admin's colour, plain without one. */
export function PriorityChip({ priority, className }: { priority: PriorityView; className?: string }) {
  const title = vi.board.priorityHint(priority.name);
  return priority.color ? (
    <StatusChip name={priority.name} color={priority.color} className={className} title={title} />
  ) : (
    <span className={`chip${className ? ` ${className}` : ""}`} title={title}>
      {priority.name}
    </span>
  );
}
