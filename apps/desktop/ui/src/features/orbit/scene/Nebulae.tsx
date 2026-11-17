import { useState } from "react";
import { vi } from "../../../i18n/vi";
import type { InboxItem } from "../../../ipc/types";
import type { ItemDragStart } from "../OrbitScene";
import { nebulaLayout, type Point } from "../geometry";
import type { LayoutOverrides } from "../useLayout";
import { nebulaTone } from "../nebulaTone";
import { interactive } from "./interactive";
import { textUnits, truncate, wrapText } from "./text";

interface Props {
  inbox: InboxItem[];
  /** loading / error / missing URL; replaces the "inbox empty" text */
  notice: string | undefined;
  selectedTaskId: string | undefined;
  onSelect: (taskId: string) => void;
  layout: LayoutOverrides;
  onDragStart: ItemDragStart;
}

const round1 = (n: number) => Math.round(n * 10) / 10;
/** First label line sits this far below the nebula centre at full size. */
const LABEL_Y = 50;
const MIN_TEXT_SCALE = 0.85;
/** Rough width of one text unit (a Latin character at 11–12px Inter; CJK takes two), to size
 *  label backdrops. */
const CHAR_W = 6.3;
/** The bold card title runs a little wider. */
const TITLE_CHAR_W = 6.8;
const HINT_MAX_CHARS = 64;
/** Full-title card shown on hover. */
const CARD_LINE_CHARS = 38;
const CARD_LINE_H = 15;

export const nebulaKey = (taskId: string) => `nebula:${taskId}`;
/** One gradient per nebula: its priority's colour and strength. */
const gradientId = (taskId: string) => `g-neb-${taskId.replace(/[^\w-]/g, "_")}`;

/** Lines on a dark rounded card, centred on (0, top); drawn last so nothing covers it. */
function TextCard({ at, title, lines }: { at: Point; title?: string; lines: string[] }) {
  const longest = Math.max(...lines.map((l) => textUnits(l) * CHAR_W), title ? textUnits(title) * TITLE_CHAR_W : 0);
  const w = Math.round(longest + 24);
  const rows = lines.length + (title ? 1 : 0);
  const h = rows * CARD_LINE_H + 12;
  return (
    <g className="text-card" transform={`translate(${at.x} ${at.y})`} aria-hidden="true">
      <rect x={-w / 2} y={0} width={w} height={h} rx={8} className="text-card-bg" />
      {title && (
        <text y={17} className="text-card-title">
          {title}
        </text>
      )}
      {lines.map((l, k) => (
        <text key={k} y={17 + (k + (title ? 1 : 0)) * CARD_LINE_H} className="text-card-line">
          {l}
        </text>
      ))}
    </g>
  );
}

/** The empty centre: a faint protostar where the task's star will ignite, plus one quiet hint. */
function IdleCore({ count, notice }: { count: number; notice: string | undefined }) {
  const [hover, setHover] = useState(false);
  const full = notice ?? (count > 0 ? vi.orbit.noActiveHint : vi.orbit.inboxEmpty);
  const cut = full.length > HINT_MAX_CHARS;
  const hint = truncate(full, HINT_MAX_CHARS);
  const w = Math.round(textUnits(hint) * CHAR_W + 24);
  return (
    <g className={`idle-core${notice ? " has-notice" : ""}`}>
      <circle r={44} className="idle-slot" />
      <circle r={18} className="idle-glow" />
      <circle r={5} className="idle-ember" />
      <g
        transform="translate(0 70)"
        onPointerEnter={() => setHover(true)}
        onPointerLeave={() => setHover(false)}
      >
        <rect x={-w / 2} y={-11} width={w} height={22} rx={11} className="idle-pill" />
        <text y={4} className="idle-hint">
          {hint}
        </text>
      </g>
      {count > 1 && !notice && (
        <text y={98} className="idle-sub">
          {vi.orbit.dragHint}
        </text>
      )}
      {hover && cut && <TextCard at={{ x: 0, y: 86 }} lines={wrapText(full, 56)} />}
    </g>
  );
}

/** No active task: inbox tasks drift as dim nebulae waiting to ignite. Drag to rearrange. */
export function Nebulae({ inbox, notice, selectedTaskId, onSelect, layout, onDragStart }: Props) {
  const [hovered, setHovered] = useState<string | null>(null);
  const spots = nebulaLayout(inbox.length);
  const place = (i: number, taskId: string) => {
    const spot = spots[i];
    return spot ? { spot, p: layout[nebulaKey(taskId)] ?? spot } : null;
  };
  const hoverIndex = inbox.findIndex((x) => x.taskId === hovered);
  const hoverItem = inbox[hoverIndex];
  const hoverAt = hoverItem ? place(hoverIndex, hoverItem.taskId) : null;

  return (
    <g className="nebulae">
      <defs>
        {inbox.map((item) => {
          const tone = nebulaTone(item.priority);
          return (
            <g key={item.taskId}>
              <radialGradient id={gradientId(item.taskId)}>
                <stop offset="0%" style={{ stopColor: tone.color, stopOpacity: tone.cloud }} />
                <stop offset="100%" style={{ stopColor: tone.color, stopOpacity: 0 }} />
              </radialGradient>
              <radialGradient id={`${gradientId(item.taskId)}-core`}>
                <stop offset="0%" style={{ stopColor: tone.color, stopOpacity: tone.core }} />
                <stop offset="100%" style={{ stopColor: tone.color, stopOpacity: 0 }} />
              </radialGradient>
            </g>
          );
        })}
      </defs>
      <IdleCore count={inbox.length} notice={inbox.length === 0 ? notice : undefined} />
      {inbox.map((item, i) => {
        const at = place(i, item.taskId);
        if (!at) return null;
        const { spot, p } = at;
        const key = nebulaKey(item.taskId);
        const selected = selectedTaskId === item.taskId;
        const textScale = Math.max(spot.scale, MIN_TEXT_SCALE);
        const show = () => setHovered(item.taskId);
        const hide = () => setHovered((h) => (h === item.taskId ? null : h));
        return (
          <g
            key={item.taskId}
            transform={`translate(${p.x} ${p.y})`}
            className={`nebula${selected ? " is-selected" : ""}${hovered === item.taskId ? " is-hovered" : ""}`}
            onPointerDown={onDragStart(key, p)}
            onPointerEnter={show}
            onPointerLeave={hide}
            onFocus={show}
            onBlur={hide}
            {...interactive(`${item.taskId} ${item.title}`, () => onSelect(item.taskId), selected)}
          >
            <g transform={`scale(${spot.scale})`}>
              <ellipse rx={62} ry={38} fill={`url(#${gradientId(item.taskId)})`} className="nebula-cloud" />
              <ellipse rx={30} ry={18} fill={`url(#${gradientId(item.taskId)}-core)`} className="nebula-core" />
              <circle r={2} className="nebula-seed" />
              {selected && <ellipse rx={44} ry={28} className="select-ring" />}
            </g>
            {/* labels shrink less than the cloud so they stay readable */}
            <g transform={`translate(0 ${round1(LABEL_Y * spot.scale)}) scale(${textScale})`}>
              <text className="nebula-id">{item.taskId}</text>
              <text y={15} className="nebula-title">
                {truncate(item.title, spot.scale < 0.8 ? 22 : 32)}
              </text>
            </g>
          </g>
        );
      })}
      {hoverItem && hoverAt && (
        <TextCard
          at={{ x: hoverAt.p.x, y: hoverAt.p.y + LABEL_Y * hoverAt.spot.scale - 14 }}
          title={[hoverItem.taskId, hoverItem.project, hoverItem.priority?.name].filter(Boolean).join(" · ")}
          lines={wrapText(hoverItem.title, CARD_LINE_CHARS)}
        />
      )}
    </g>
  );
}
