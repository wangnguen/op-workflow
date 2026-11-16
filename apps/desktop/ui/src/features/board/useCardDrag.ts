// Drag a task card to another column with pointer events. HTML5 drag and drop is not used: the
// WebView2 window of Tauri can take file drops for itself and swallow those events.
import { useEffect, useRef, useState, type PointerEvent as ReactPointerEvent } from "react";
import type { TaskCard, TaskColumn } from "../../ipc/types";
import { dropRule, type DropAction } from "./dropRule";

/** Pointer travel before a press becomes a drag (a click on the card stays a click). */
const START_PX = 6;
/** Slide back / fade out; kept in step with `.card-ghost` in board.css. */
const BACK_MS = 240;
const FADE_MS = 160;
/** Controls inside a card keep their own pointer behaviour. */
const NO_DRAG = "button, a, input, textarea, select, label, summary, [data-no-drag]";

export interface DragView {
  wpId: number;
  /** column under the pointer */
  over?: TaskColumn;
}

/** The board's answer to a drop. A promise keeps the card where it was dropped until it settles
 *  (false or a rejection slides it back); nothing = done (a dialog takes over). */
export type OnDrop = (card: TaskCard, to: TaskColumn, action: DropAction) => void | Promise<boolean>;

const columnAt = (x: number, y: number): TaskColumn | undefined =>
  (document.elementFromPoint(x, y)?.closest<HTMLElement>("[data-column]")?.dataset.column as TaskColumn | undefined) ??
  undefined;

export function useCardDrag(onDrop: OnDrop, onRefuse: (reason: string) => void) {
  const [view, setView] = useState<DragView | null>(null);
  const handlers = useRef({ onDrop, onRefuse });
  useEffect(() => {
    handlers.current = { onDrop, onRefuse };
  });
  const stop = useRef<() => void>(undefined);
  useEffect(() => () => stop.current?.(), []);

  const begin = (e: ReactPointerEvent<HTMLElement>, card: TaskCard) => {
    if (e.button !== 0 || !e.isPrimary || stop.current) return;
    if ((e.target as Element).closest(NO_DRAG)) return;
    const source = e.currentTarget;
    const pointer = e.pointerId;
    const x0 = e.clientX;
    const y0 = e.clientY;
    let ghost: HTMLElement | null = null;
    let over: TaskColumn | undefined;
    let dx = 0;
    let dy = 0;
    let frame = 0;

    const place = () => {
      frame = 0;
      if (ghost) ghost.style.transform = `translate(${dx}px, ${dy}px) rotate(1deg)`;
    };
    const finish = () => {
      ghost?.remove();
      ghost = null;
      document.body.classList.remove("is-card-dragging");
      stop.current = undefined;
      setView(null);
    };
    const slideBack = () => {
      if (!ghost) return finish();
      ghost.classList.add("is-returning");
      ghost.style.transform = "translate(0px, 0px)";
      window.setTimeout(finish, BACK_MS + 30);
    };
    const fadeOut = () => {
      ghost?.classList.add("is-dropped");
      window.setTimeout(finish, FADE_MS + 20);
    };
    const detach = () => {
      window.removeEventListener("pointermove", move);
      window.removeEventListener("pointerup", up);
      window.removeEventListener("pointercancel", cancel);
      window.removeEventListener("keydown", key, true);
      window.removeEventListener("blur", cancel);
      if (frame) cancelAnimationFrame(frame);
      frame = 0;
    };
    // the click that follows the release belongs to the drag, not to whatever is under it
    const swallowClick = () => {
      const swallow = (ev: MouseEvent) => {
        ev.stopPropagation();
        ev.preventDefault();
      };
      window.addEventListener("click", swallow, { capture: true, once: true });
      window.setTimeout(() => window.removeEventListener("click", swallow, { capture: true }), 0);
    };

    function move(ev: PointerEvent) {
      if (ev.pointerId !== pointer) return;
      dx = ev.clientX - x0;
      dy = ev.clientY - y0;
      if (!ghost) {
        if (Math.hypot(dx, dy) < START_PX) return;
        const r = source.getBoundingClientRect();
        ghost = source.cloneNode(true) as HTMLElement;
        ghost.classList.add("card-ghost");
        ghost.setAttribute("aria-hidden", "true");
        Object.assign(ghost.style, { left: `${r.left}px`, top: `${r.top}px`, width: `${r.width}px`, height: `${r.height}px` });
        document.body.appendChild(ghost);
        document.body.classList.add("is-card-dragging");
        window.getSelection()?.removeAllRanges();
        setView({ wpId: card.wpId });
      }
      if (!frame) frame = requestAnimationFrame(place);
      const now = columnAt(ev.clientX, ev.clientY);
      if (now !== over) {
        over = now;
        setView({ wpId: card.wpId, over: now });
      }
    }

    function up(ev: PointerEvent) {
      if (ev.pointerId !== pointer) return;
      detach();
      if (!ghost) {
        stop.current = undefined;
        return; // a click, not a drag
      }
      swallowClick();
      place();
      const to = columnAt(ev.clientX, ev.clientY);
      const action = dropRule(card, to);
      if (action.kind === "none" || !to) return slideBack();
      if (action.kind === "refuse") {
        handlers.current.onRefuse(action.reason);
        return slideBack();
      }
      const answer = handlers.current.onDrop(card, to, action);
      if (!answer) return fadeOut();
      ghost.classList.add("is-waiting");
      answer.then(
        (ok) => (ok ? fadeOut() : slideBack()),
        () => slideBack(),
      );
    }

    function cancel() {
      detach();
      slideBack();
    }

    function key(ev: KeyboardEvent) {
      if (ev.key !== "Escape" || !ghost) return;
      ev.preventDefault();
      ev.stopPropagation();
      cancel();
    }

    window.addEventListener("pointermove", move);
    window.addEventListener("pointerup", up);
    window.addEventListener("pointercancel", cancel);
    window.addEventListener("keydown", key, true);
    window.addEventListener("blur", cancel);
    stop.current = () => {
      detach();
      finish();
    };
  };

  return { view, begin };
}
