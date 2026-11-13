import type { KeyboardEvent, MouseEvent } from "react";

/** Props that make an SVG element behave like a button (mouse + keyboard). */
export function interactive(label: string, onActivate: () => void, selected = false) {
  return {
    role: "button" as const,
    tabIndex: 0,
    "aria-label": label,
    "aria-pressed": selected,
    onClick: (e: MouseEvent) => {
      e.stopPropagation();
      onActivate();
    },
    onKeyDown: (e: KeyboardEvent) => {
      if (e.key === "Enter" || e.key === " ") {
        e.preventDefault();
        onActivate();
      }
    },
  };
}
