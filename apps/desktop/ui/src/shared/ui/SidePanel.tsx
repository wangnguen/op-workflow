import { useCallback, useRef, useState, type KeyboardEvent, type PointerEvent, type ReactNode } from "react";
import { vi } from "../../i18n/vi";
import { Icon } from "./Icon";
import "./sidePanel.css";

/** Width of the strip left when the panel is hidden (it holds the button that brings it back). */
const RAIL = 36;
/** Dragged narrower than this share of `min`, the panel hides. */
const HIDE_AT = 0.6;
/** What the panel never takes from the rest of the screen. */
const MAIN_MIN = 360;
const STEP = 16;

interface Size {
  width: number;
  hidden: boolean;
}

const clamp = (n: number, lo: number, hi: number) => Math.max(lo, Math.min(hi, n));

function load(key: string, initial: number, min: number, max: number): Size {
  try {
    const saved = JSON.parse(localStorage.getItem(key) ?? "null") as Partial<Size> | null;
    if (saved && typeof saved.width === "number") {
      return { width: clamp(saved.width, min, max), hidden: saved.hidden === true };
    }
  } catch {
    // nothing saved, or storage is off: the default width
  }
  return { width: initial, hidden: false };
}

function save(key: string, size: Size) {
  try {
    localStorage.setItem(key, JSON.stringify(size));
  } catch {
    // private window: the width just is not remembered
  }
}

interface Props {
  /** which edge of the screen the panel sits on (its handle is on the other edge) */
  side: "left" | "right";
  /** localStorage key: each panel remembers its own width and whether it is hidden */
  storageKey: string;
  min: number;
  max: number;
  initial: number;
  /** names the panel in the hide / show buttons, e.g. "danh sách phiên" */
  name: string;
  className?: string;
  /** the panel's content; `hide` is for a hide button inside it */
  children: (hide: () => void) => ReactNode;
}

/** A side panel whose edge drags: wider, narrower, or all the way in to hide it (a thin strip
 *  with a button stays to bring it back). Double-click or Enter on the edge hides / shows it,
 *  arrow keys resize it. Width and hidden state are remembered per panel. The content keeps the
 *  width it had while the panel slides, so nothing reflows during the animation. */
export function SidePanel({ side, storageKey, min, max, initial, name, className, children }: Props) {
  const [size, setSize] = useState<Size>(() => load(storageKey, initial, min, max));
  const [dragging, setDragging] = useState(false);
  const root = useRef<HTMLDivElement>(null);
  const current = useRef(size);
  current.current = size;

  const commit = useCallback(
    (next: Size) => {
      setSize(next);
      save(storageKey, next);
    },
    [storageKey],
  );
  // hidden by its button: the focus goes to the button that brings it back, not lost
  const hide = useCallback(() => {
    commit({ ...current.current, hidden: true });
    requestAnimationFrame(() => root.current?.querySelector<HTMLButtonElement>(".side-panel-rail button")?.focus());
  }, [commit]);
  const show = useCallback(() => commit({ ...current.current, hidden: false }), [commit]);
  // the screen around the panel keeps room for the rest
  const limit = () => {
    const room = root.current?.parentElement?.clientWidth;
    return room ? clamp(room - MAIN_MIN, min, max) : max;
  };
  // dragging right widens a left panel and narrows a right one
  const dir = side === "left" ? 1 : -1;

  const onPointerDown = (e: PointerEvent<HTMLDivElement>) => {
    if (e.button !== 0) return;
    e.preventDefault();
    const handle = e.currentTarget;
    handle.setPointerCapture(e.pointerId);
    const start = current.current;
    const startX = e.clientX;
    const from = start.hidden ? RAIL : start.width;
    const most = limit();
    let next = start;
    setDragging(true);
    document.body.classList.add("is-col-resizing");
    const move = (ev: globalThis.PointerEvent) => {
      const w = from + dir * (ev.clientX - startX);
      // hidden, it keeps the width it had so showing it again restores that
      next = w < min * HIDE_AT ? { width: start.width, hidden: true } : { width: clamp(w, min, most), hidden: false };
      setSize(next);
    };
    const up = () => {
      handle.removeEventListener("pointermove", move);
      handle.removeEventListener("pointerup", up);
      handle.removeEventListener("pointercancel", up);
      document.body.classList.remove("is-col-resizing");
      setDragging(false);
      commit(next);
    };
    handle.addEventListener("pointermove", move);
    handle.addEventListener("pointerup", up);
    handle.addEventListener("pointercancel", up);
  };

  const onKeyDown = (e: KeyboardEvent<HTMLDivElement>) => {
    const s = current.current;
    let next: Size | undefined;
    if (e.key === "Enter" || e.key === " ") next = { ...s, hidden: !s.hidden };
    else if (e.key === "ArrowRight" || e.key === "ArrowLeft") {
      const grow = (e.key === "ArrowRight" ? 1 : -1) * dir;
      next = { width: clamp(s.width + grow * STEP, min, limit()), hidden: false };
    } else if (e.key === "Home") next = { width: min, hidden: false };
    else if (e.key === "End") next = { width: limit(), hidden: false };
    if (!next) return;
    e.preventDefault();
    commit(next);
  };

  const shown = size.hidden ? RAIL : size.width;
  return (
    <div
      ref={root}
      className={`side-panel side-panel--${side}${size.hidden ? " is-hidden" : ""}${dragging ? " is-dragging" : ""}${className ? ` ${className}` : ""}`}
      style={{ width: shown }}
    >
      <div className="side-panel-clip">
        <div className="side-panel-in" style={{ width: size.width }} inert={size.hidden} aria-hidden={size.hidden}>
          {children(hide)}
        </div>
        <div className="side-panel-rail" inert={!size.hidden}>
          <button className="btn btn--ghost btn--icon" title={vi.side.show(name)} aria-label={vi.side.show(name)} onClick={show}>
            <Icon name="chevron" size={13} className={side === "right" ? "is-back" : undefined} />
          </button>
        </div>
      </div>
      <div
        className="side-panel-handle"
        role="separator"
        aria-orientation="vertical"
        aria-label={vi.side.resize(name)}
        aria-valuemin={min}
        aria-valuemax={max}
        aria-valuenow={size.hidden ? 0 : size.width}
        tabIndex={0}
        title={vi.side.resizeHint}
        onPointerDown={onPointerDown}
        onDoubleClick={() => commit({ ...current.current, hidden: !current.current.hidden })}
        onKeyDown={onKeyDown}
      />
    </div>
  );
}

/** The hide button for the top of a side panel ("‹" on a left panel, "›" on a right one). */
export function HideSideButton({ side, name, onHide }: { side: "left" | "right"; name: string; onHide: () => void }) {
  return (
    <button className="btn btn--ghost btn--icon side-panel-hide" title={vi.side.hide(name)} aria-label={vi.side.hide(name)} onClick={onHide}>
      <Icon name="chevron" size={13} className={side === "left" ? "is-back" : undefined} />
    </button>
  );
}
