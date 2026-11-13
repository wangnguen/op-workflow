import { useRef, useState } from "react";
import { agentLabel, vi } from "../../i18n/vi";
import type { QuotaSnapshot, QuotaWindow } from "../../ipc/types";
import { useDismiss } from "../../shared/hooks";
import { remainingPct } from "./quota";
import { QuotaPopover } from "./QuotaPopover";
import "./quota.css";

interface Props {
  quota: QuotaSnapshot[];
  now: number;
  onRefresh: () => void;
}

const SLOTS: { provider: QuotaSnapshot["provider"]; kind: QuotaWindow["kind"] }[] = [
  { provider: "claude", kind: "fiveHour" },
  { provider: "claude", kind: "sevenDay" },
  { provider: "codex", kind: "fiveHour" },
  { provider: "codex", kind: "sevenDay" },
];

const R = 9;
const C = 2 * Math.PI * R;

/** Four mini ring gauges (remaining %) in the top bar; click opens the popover. */
export function QuotaGauges({ quota, now, onRefresh }: Props) {
  const [open, setOpen] = useState(false);
  const ref = useRef<HTMLDivElement>(null);
  useDismiss(ref, open, () => setOpen(false));

  return (
    <div className="quota" ref={ref}>
      <button
        className="quota-trigger"
        aria-expanded={open}
        aria-label={vi.quota.open}
        title={vi.quota.title}
        onClick={() => setOpen((v) => !v)}
      >
        {SLOTS.map((slot) => {
          const win = quota.find((q) => q.provider === slot.provider)?.windows.find((w) => w.kind === slot.kind);
          const left = remainingPct(win, now);
          const tone = !win ? "unknown" : left < 5 ? "danger" : left < 20 ? "warn" : "ok";
          return (
            <span key={`${slot.provider}-${slot.kind}`} className={`gauge gauge--${slot.provider} gauge--${tone}`}>
              <svg viewBox="-12 -12 24 24" width={24} height={24} aria-hidden="true">
                <circle r={R} className="gauge-track" />
                <circle
                  r={R}
                  className="gauge-fill"
                  strokeDasharray={`${(C * left) / 100} ${C}`}
                  transform="rotate(-90)"
                />
              </svg>
              <span className="gauge-text">
                <span className="gauge-name">
                  {agentLabel[slot.provider]} {slot.kind === "fiveHour" ? "5h" : "7d"}
                </span>
                <span className="gauge-value">{win ? `${Math.round(left)}%` : "?"}</span>
              </span>
            </span>
          );
        })}
      </button>
      {open && <QuotaPopover quota={quota} now={now} onRefresh={onRefresh} />}
    </div>
  );
}
