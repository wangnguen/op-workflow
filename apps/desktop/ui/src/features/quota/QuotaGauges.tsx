import { useRef, useState } from "react";
import { agentLabel, vi } from "../../i18n/vi";
import type { ProviderLink, QuotaSnapshot, QuotaWindow } from "../../ipc/types";
import { useDismiss } from "../../shared/hooks";
import { linkProblem, remainingPct } from "./quota";
import { QuotaPopover } from "./QuotaPopover";
import "./quota.css";

interface Props {
  quota: QuotaSnapshot[];
  links: ProviderLink[];
  now: number;
  /** reload snapshots after a reconnect or sign-in request */
  onChanged: () => void;
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
export function QuotaGauges({ quota, links, now, onChanged }: Props) {
  const [open, setOpen] = useState(false);
  const ref = useRef<HTMLDivElement>(null);
  useDismiss(ref, open, () => setOpen(false));
  const problems = links.filter(linkProblem);
  const title = problems.length
    ? problems.map((l) => `${agentLabel[l.provider]}: ${vi.quota.linkStatus[l.status]}`).join(" · ")
    : vi.quota.title;

  return (
    <div className="quota" ref={ref}>
      <button
        className={`quota-trigger${problems.length ? " has-problem" : ""}`}
        aria-expanded={open}
        aria-label={vi.quota.open}
        title={title}
        onClick={() => setOpen((v) => !v)}
      >
        {SLOTS.map((slot) => {
          const win = quota.find((q) => q.provider === slot.provider)?.windows.find((w) => w.kind === slot.kind);
          const left = remainingPct(win, now);
          const broken = linkProblem(links.find((l) => l.provider === slot.provider));
          const tone = broken ? "broken" : !win ? "unknown" : left < 5 ? "danger" : left < 20 ? "warn" : "ok";
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
                <span className="gauge-value">{broken ? "!" : win ? `${Math.round(left)}%` : "?"}</span>
              </span>
            </span>
          );
        })}
      </button>
      {open && <QuotaPopover quota={quota} links={links} now={now} onChanged={onChanged} />}
    </div>
  );
}
