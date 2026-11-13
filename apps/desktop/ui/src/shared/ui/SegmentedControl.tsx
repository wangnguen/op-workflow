import { useLayoutEffect, useRef, useState, type KeyboardEvent, type ReactNode } from "react";

export interface Segment<V extends string> {
  value: V;
  label: string;
  icon?: ReactNode;
  /** small number after the label, e.g. how many items the segment holds */
  count?: number;
  title?: string;
}

interface Props<V extends string> {
  options: Segment<V>[];
  value: V;
  onChange: (value: V) => void;
  /** accessible name of the group */
  label: string;
  /** stretch segments to equal widths */
  fill?: boolean;
  className?: string;
}

interface Thumb {
  x: number;
  y: number;
  w: number;
  h: number;
}

/** Radio group drawn as one track; a single thumb slides to the picked segment. */
export function SegmentedControl<V extends string>({ options, value, onChange, label, fill, className }: Props<V>) {
  const root = useRef<HTMLDivElement>(null);
  const [thumb, setThumb] = useState<Thumb | null>(null);
  const [ready, setReady] = useState(false);
  const labels = options.map((o) => `${o.label}${o.count ?? ""}`).join("|");

  useLayoutEffect(() => {
    const el = root.current;
    if (!el) return;
    const measure = () => {
      const on = el.querySelector<HTMLElement>('[aria-checked="true"]');
      if (!on || !on.offsetWidth) return;
      setThumb({ x: on.offsetLeft, y: on.offsetTop, w: on.offsetWidth, h: on.offsetHeight });
    };
    measure();
    const ro = typeof ResizeObserver === "undefined" ? null : new ResizeObserver(measure);
    ro?.observe(el);
    // animate moves only, not the first placement
    const id = requestAnimationFrame(() => setReady(true));
    return () => {
      ro?.disconnect();
      cancelAnimationFrame(id);
    };
  }, [value, labels]);

  const onKey = (e: KeyboardEvent<HTMLDivElement>) => {
    const step = e.key === "ArrowRight" || e.key === "ArrowDown" ? 1 : e.key === "ArrowLeft" || e.key === "ArrowUp" ? -1 : 0;
    if (!step) return;
    e.preventDefault();
    const n = options.length;
    const at = options.findIndex((o) => o.value === value);
    const next = (at + step + n) % n;
    const opt = options[next];
    if (!opt) return;
    onChange(opt.value);
    e.currentTarget.querySelectorAll<HTMLButtonElement>('[role="radio"]')[next]?.focus();
  };

  return (
    <div
      ref={root}
      className={`seg${fill ? " seg--fill" : ""}${className ? ` ${className}` : ""}`}
      role="radiogroup"
      aria-label={label}
      onKeyDown={onKey}
    >
      {thumb && (
        <span
          className={`seg-thumb${ready ? " is-ready" : ""}`}
          aria-hidden="true"
          style={{ width: thumb.w, height: thumb.h, transform: `translate(${thumb.x}px, ${thumb.y}px)` }}
        />
      )}
      {options.map((o) => {
        const on = o.value === value;
        return (
          <button
            key={o.value}
            type="button"
            role="radio"
            aria-checked={on}
            tabIndex={on ? 0 : -1}
            title={o.title}
            className="seg-item"
            onClick={() => onChange(o.value)}
          >
            {o.icon}
            <span>{o.label}</span>
            {o.count !== undefined && <span className="seg-count">{o.count}</span>}
          </button>
        );
      })}
    </div>
  );
}
