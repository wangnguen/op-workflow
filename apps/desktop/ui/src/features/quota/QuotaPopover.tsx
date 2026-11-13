import { agentLabel, vi } from "../../i18n/vi";
import type { QuotaSnapshot } from "../../ipc/types";
import { formatCountdown, formatRelative } from "../../shared/format";
import { remainingPct } from "./quota";

interface Props {
  quota: QuotaSnapshot[];
  now: number;
  onRefresh: () => void;
}

export function QuotaPopover({ quota, now, onRefresh }: Props) {
  return (
    <div className="quota-pop" role="dialog" aria-label={vi.quota.title}>
      <header>
        <h3>{vi.quota.title}</h3>
        <button className="btn btn--sm" onClick={onRefresh}>
          ↻ {vi.quota.refresh}
        </button>
      </header>
      {quota.map((q) => (
        <section key={q.provider} className={`qp-provider qp-provider--${q.provider}`}>
          <div className="qp-head">
            <span className={`chip chip--${q.provider}`}>{agentLabel[q.provider]}</span>
            {q.plan && (
              <span className="faint">
                {vi.quota.plan} {q.plan}
              </span>
            )}
          </div>
          {q.windows.map((w) => {
            const left = remainingPct(w, now);
            return (
              <div key={w.kind} className="qp-window">
                <div className="qp-line">
                  <span>{w.kind === "fiveHour" ? vi.quota.fiveHour : vi.quota.sevenDay}</span>
                  <span className={left < 20 ? "qp-low" : ""}>
                    <b>{Math.round(left)}%</b> {vi.quota.remaining}
                  </span>
                </div>
                <div className={`bar${q.provider === "codex" ? " bar--codex" : ""}${left < 20 ? " bar--hot" : ""}`}>
                  <span style={{ width: `${left}%` }} />
                </div>
                <div className="qp-meta faint">{formatCountdown(w.resetsAt, now, vi.quota.staleAfterReset)}</div>
              </div>
            );
          })}
          <div className="qp-source faint">
            {vi.quota.source}: <span className="mono">{q.source}</span> · {vi.quota.updated} {formatRelative(q.observedAt, now)}
          </div>
        </section>
      ))}
      <p className="qp-note faint">{vi.quota.estimateNote}</p>
    </div>
  );
}
