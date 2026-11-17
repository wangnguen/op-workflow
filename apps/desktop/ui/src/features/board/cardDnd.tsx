// Card drag and drop on the Task board, with dnd-kit. Mouse, pen or touch: a press that travels
// 6 px becomes a drag (controls inside a card keep their clicks). Keyboard: Space/Enter lifts the
// focused card, ←/→ moves it a column, Space/Enter drops, Esc cancels. What a drop does is
// `dropRule`'s answer; this file only moves the card and animates the copy that follows.
import {
  KeyboardSensor,
  PointerSensor,
  closestCenter,
  pointerWithin,
  useDraggable,
  useDroppable,
  useSensor,
  useSensors,
  type CollisionDetection,
  type DropAnimationFunction,
  type KeyboardCoordinateGetter,
} from "@dnd-kit/core";
import { useCallback, type PointerEvent as ReactPointerEvent, type ReactNode } from "react";
import { vi } from "../../i18n/vi";
import type { TaskColumn } from "../../ipc/types";
import { TaskCardView, type TaskCardProps } from "./TaskCardView";

/** Pointer travel before a press becomes a drag (a click on the card stays a click). */
const START_PX = 6;
/** Controls inside a card keep their own pointer behaviour. */
const NO_DRAG = "button, a, input, textarea, select, label, summary, [data-no-drag]";
/** Slide back / fade out; `--ease-out` of the motion tokens. */
const BACK_MS = 240;
const FADE_MS = 160;
const EASE_OUT = "cubic-bezier(0.22, 1, 0.36, 1)";

/** Main button presses that do not start on a control inside the card. */
class CardPointerSensor extends PointerSensor {
  static activators = [
    {
      eventName: "onPointerDown" as const,
      handler: ({ nativeEvent: e }: ReactPointerEvent) =>
        e.isPrimary && e.button === 0 && !(e.target as Element | null)?.closest(NO_DRAG),
    },
  ];
}

/** A column's place on screen, for moving a lifted card with the arrow keys. */
export interface ColumnBox {
  id: string;
  left: number;
  width: number;
}

/** Centre x of the column next to the one the card is over (`at`, else the one under `x`), in
 *  direction `dir`; nothing at the ends. */
export function nextColumnCenter(cols: ColumnBox[], x: number, dir: -1 | 1, at?: string): number | undefined {
  const sorted = [...cols].sort((a, b) => a.left - b.left);
  let i = at === undefined ? -1 : sorted.findIndex((c) => c.id === at);
  if (i < 0) i = sorted.findIndex((c) => x >= c.left && x <= c.left + c.width);
  if (i < 0) return undefined;
  const next = sorted[i + dir];
  return next && next.left + next.width / 2;
}

/** ←/→ jump a whole column (the folded "Đã xong" strip included); ↑/↓ do nothing. */
const columnKeys: KeyboardCoordinateGetter = (event, { currentCoordinates, context }) => {
  const dir = event.code === "ArrowRight" ? 1 : event.code === "ArrowLeft" ? -1 : 0;
  const rect = context.collisionRect;
  if (!dir || !rect) return undefined;
  event.preventDefault();
  const cols = context.droppableContainers.getEnabled().flatMap((c) => {
    const r = context.droppableRects.get(c.id);
    return r ? [{ id: String(c.id), left: r.left, width: r.width }] : [];
  });
  const cx = rect.left + rect.width / 2;
  const target = nextColumnCenter(cols, cx, dir, context.over ? String(context.over.id) : undefined);
  return target === undefined ? undefined : { x: currentCoordinates.x + target - cx, y: currentCoordinates.y };
};

/** The column under the pointer; with the keyboard, the column the card is centred on. */
export const cardCollision: CollisionDetection = (args) =>
  args.pointerCoordinates ? pointerWithin(args) : closestCenter(args);

export function useCardSensors() {
  return useSensors(
    useSensor(CardPointerSensor, { activationConstraint: { distance: START_PX } }),
    useSensor(KeyboardSensor, { coordinateGetter: columnKeys }),
  );
}

/** Where the copy of the card goes after a drop. */
export type DropEnd = "back" | "fade";

/** The copy of the card after a drop: it stays (dimmed) until the board has answered, then fades
 *  where it was dropped (the card moved, or a dialog took over) or slides back onto its card
 *  (refused, cancelled, failed). The card itself stays faded until then. */
export function makeDropAnimation(outcome: () => Promise<DropEnd>): DropAnimationFunction {
  return async ({ active, dragOverlay, transform }) => {
    const source = active.node;
    const copy = dragOverlay.node;
    source.classList.add("is-drag-source");
    const waiting = window.setTimeout(() => copy.classList.add("is-waiting"), 120);
    const end = await outcome();
    window.clearTimeout(waiting);
    const at = (x: number, y: number) => `translate3d(${x}px, ${y}px, 0)`;
    const from = at(transform.x, transform.y);
    const opacity = getComputedStyle(copy).opacity;
    copy.classList.remove("is-waiting");
    const animation =
      end === "back"
        ? copy.animate(
            [
              { transform: from, opacity },
              {
                transform: at(
                  transform.x + active.rect.left - dragOverlay.rect.left,
                  transform.y + active.rect.top - dragOverlay.rect.top,
                ),
                opacity: "1",
              },
            ],
            { duration: BACK_MS, easing: EASE_OUT, fill: "forwards" },
          )
        : copy.animate(
            [
              { transform: from, opacity },
              { transform: `${from} scale(0.96)`, opacity: "0" },
            ],
            { duration: FADE_MS, easing: EASE_OUT, fill: "forwards" },
          );
    await animation.finished.catch(() => undefined);
    source.classList.remove("is-drag-source");
  };
}

/** A card that can be picked up; the card itself is the handle. */
export function DraggableCard(props: Omit<TaskCardProps, "drag" | "dragging">) {
  const { setNodeRef, setActivatorNodeRef, attributes, listeners, isDragging } = useDraggable({
    id: props.card.wpId,
    data: { card: props.card },
    attributes: { role: "group", roleDescription: vi.board.drag.role },
  });
  const ref = useCallback(
    (el: HTMLElement | null) => {
      setNodeRef(el);
      setActivatorNodeRef(el);
    },
    [setNodeRef, setActivatorNodeRef],
  );
  return <TaskCardView {...props} drag={{ ref, attributes, listeners }} dragging={isDragging} />;
}

/** A board column that takes dropped cards. */
export function DropColumn({
  id,
  className,
  onClick,
  children,
}: {
  id: TaskColumn;
  className: string;
  onClick?: () => void;
  children: ReactNode;
}) {
  const { setNodeRef } = useDroppable({ id });
  return (
    <section ref={setNodeRef} data-column={id} className={className} onClick={onClick}>
      {children}
    </section>
  );
}
