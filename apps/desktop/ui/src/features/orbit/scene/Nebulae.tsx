import { useState } from "react";
import { vi } from "../../../i18n/vi";
import type { InboxItem } from "../../../ipc/types";
import { formatTokens, formatUsd } from "../../../shared/format";
import type { ItemDragStart, MapSummary } from "../OrbitScene";
import { nebulaLayout, type Point } from "../geometry";
import type { LayoutOverrides } from "../useLayout";
import { nebulaTone } from "../nebulaTone";
import { interactive } from "./interactive";
import { textUnits, truncate, wrapText } from "./text";

interface Props {
  inbox: InboxItem[];
  /** what the sun in the centre shows */
  summary: MapSummary | undefined;
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
const sun = vi.orbit.sun;
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

/** The centre while no task is in progress: a sun with what goes on now (sessions running,
 *  waiting for you, today's spend). */
function IdleCore({ summary }: { summary: MapSummary | undefined }) {
  const running = summary?.running ?? 0;
  const waiting = summary?.waiting ?? 0;
  const today = summary?.today;
  const money = today ? sun.today(formatUsd(today.costUsd), formatTokens(today.tokens)) : undefined;
  return (
    <g className={`idle-core${running > 0 ? " is-busy" : ""}`}>
      <title>{sun.title(running, waiting, today ? formatUsd(today.costUsd) : undefined, today ? formatTokens(today.tokens) : undefined)}</title>
      <circle r={70} className="idle-slot" />
      <circle r={60} className="idle-glow" />
      <circle r={52} className="idle-sun" />
      <text y={money ? -11 : -4} className="idle-line idle-line--main">
        {sun.running(running)}
      </text>
      <text y={money ? 8 : 15} className={`idle-line${waiting > 0 ? " idle-line--waiting" : ""}`}>
        {sun.waiting(waiting)}
      </text>
      {money && (
        <text y={27} className="idle-line idle-line--faint">
          {money}
        </text>
      )}
    </g>
  );
}

/** No active task: inbox tasks drift as dim nebulae waiting to ignite. Drag to rearrange. */
export function Nebulae({ inbox, summary, selectedTaskId, onSelect, layout, onDragStart }: Props) {
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
      <IdleCore summary={summary} />
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
