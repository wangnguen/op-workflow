import { useState } from "react";
import { agentLabel, vi } from "../../i18n/vi";
import { errorMessage, installCli, openLogin, reconnectQuota } from "../../ipc/client";
import type { Provider, ProviderLink, QuotaSnapshot } from "../../ipc/types";
import { formatCountdown, formatRelative } from "../../shared/format";
import { Icon } from "../../shared/ui/Icon";
import { linkProblem, remainingPct } from "./quota";

interface Props {
  quota: QuotaSnapshot[];
  links: ProviderLink[];
  now: number;
  /** reload snapshots after a reconnect or sign-in request */
  onChanged: () => void;
}

const PROVIDERS: Provider[] = ["claude", "codex"];
const q = vi.quota;

/** Status line + "Kết nối lại" / "Đăng nhập" for one provider. */
export function LinkActions({
  provider,
  link,
  now,
  onChanged,
  compact,
}: {
  provider: Provider;
  link: ProviderLink | undefined;
  now: number;
  onChanged: () => void;
  /** settings rows show the buttons only */
  compact?: boolean;
}) {
  const [error, setError] = useState<string | null>(null);
  const run = (p: Promise<unknown>) => {
    setError(null);
    p.then(onChanged).catch((e: unknown) => setError(errorMessage(e)));
  };
  const problem = linkProblem(link);
  const connecting = link?.status === "connecting";
  // signed in and connected: nothing to do here (the popover header has "Làm mới")
  const showLogin = link?.status === "signedOut" || link?.status === "error";
  // the CLI is not on this machine: install it from here
  const showInstall = link?.status === "missing";
  const showReconnect = compact || (link !== undefined && link.status !== "ok");
  return (
    <div className="qp-link">
      {!compact && problem && link?.message && (
        <p className={`note note--${link.status === "signedOut" ? "warn" : "error"}`}>
          <Icon name="alert" size={13} />
          <span>
            {link.message}
            <span className="qp-since"> · {formatRelative(link.since, now)}</span>
          </span>
        </p>
      )}
      {!compact && connecting && (
        <p className="qp-connecting">
          <Icon name="refresh" size={12} className="is-spinning" />
          {q.connecting}
        </p>
      )}
      {(showLogin || showReconnect || showInstall) && (
        <span className="qp-actions">
          {showInstall && (
            <button
              className="btn btn--sm btn--primary"
              title={q.installHint(agentLabel[provider])}
              onClick={() => run(installCli(provider))}
            >
              {q.install}
            </button>
          )}
          {showLogin && (
            <button
              className={`btn btn--sm${link?.status === "signedOut" ? " btn--primary" : ""}`}
              title={q.loginHint(agentLabel[provider])}
              onClick={() => run(openLogin(provider))}
            >
              {q.login}
            </button>
          )}
          {showReconnect && (
            <button
              className={`btn btn--sm${link?.status === "error" ? " btn--primary" : " btn--ghost"}`}
              title={q.reconnectHint}
              disabled={connecting}
              onClick={() => run(reconnectQuota(provider))}
            >
              <Icon name="refresh" size={12} className={connecting ? "is-spinning" : undefined} />
              {q.reconnect}
            </button>
          )}
        </span>
      )}
      {error && <p className="note note--error">{error}</p>}
    </div>
  );
}

export function QuotaPopover({ quota, links, now, onChanged }: Props) {
  return (
    <div className="quota-pop" role="dialog" aria-label={q.title}>
      <header>
        <h3>{q.title}</h3>
        <button className="btn btn--sm btn--ghost" title={q.reconnectAllHint} onClick={() => void reconnectQuota().then(onChanged)}>
          <Icon name="refresh" size={12} />
          {q.refresh}
        </button>
      </header>
      {PROVIDERS.map((provider) => {
        const snap = quota.find((x) => x.provider === provider);
        const link = links.find((l) => l.provider === provider);
        if (!snap && !link) return null;
        return (
          <section key={provider} className={`qp-provider qp-provider--${provider}`}>
            <div className="qp-head">
              <span className={`chip chip--${provider}`}>{agentLabel[provider]}</span>
              {link && (
                <span className={`qp-status qp-status--${link.status}`}>
                  <i />
                  {q.linkStatus[link.status]}
                </span>
              )}
              {snap?.plan && (
                <span className="faint">
                  {q.plan} {snap.plan}
                </span>
              )}
            </div>
            {snap?.windows.map((w) => {
              const left = remainingPct(w, now);
              return (
                <div key={w.kind} className="qp-window">
                  <div className="qp-line">
                    <span>{w.kind === "fiveHour" ? q.fiveHour : q.sevenDay}</span>
                    <span className={left < 20 ? "qp-low" : ""}>
                      <b>{Math.round(left)}%</b> {q.remaining}
                    </span>
                  </div>
                  <div className={`bar${provider === "codex" ? " bar--codex" : ""}${left < 20 ? " bar--hot" : ""}`}>
                    <span style={{ width: `${left}%` }} />
                  </div>
                  <div className="qp-meta faint">{formatCountdown(w.resetsAt, now, q.staleAfterReset)}</div>
                </div>
              );
            })}
            {snap && (
              <div className="qp-source faint">
                {q.source}: <span className="mono">{snap.source}</span> · {q.updated} {formatRelative(snap.observedAt, now)}
              </div>
            )}
            {link && <LinkActions provider={provider} link={link} now={now} onChanged={onChanged} />}
          </section>
        );
      })}
      <p className="qp-note faint">{q.estimateNote}</p>
    </div>
  );
}
