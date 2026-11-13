import { useCallback, useEffect, useState } from "react";
import { vi } from "../../i18n/vi";
import { clearNxToken, errorMessage, getSettings, refreshNow, saveNxToken } from "../../ipc/client";
import type { SettingsView } from "../../ipc/types";
import { formatRelative } from "../../shared/format";
import "./settings.css";

interface Props {
  now: number;
  onClose: () => void;
  /** called after the token changes so the board reloads */
  onChanged: () => void;
}

const t = vi.settings;

/** OpenProject URL + token (token stored in Windows Credential Manager) and quota status. */
export function SettingsDialog({ now, onClose, onChanged }: Props) {
  const [view, setView] = useState<SettingsView | null>(null);
  const [token, setToken] = useState("");
  const [baseUrl, setBaseUrl] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const reload = useCallback(() => {
    getSettings()
      .then(setView)
      .catch((e: unknown) => setError(errorMessage(e)));
  }, []);

  useEffect(() => {
    reload();
    const id = window.setInterval(reload, 3000);
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") onClose();
    };
    document.addEventListener("keydown", onKey);
    return () => {
      window.clearInterval(id);
      document.removeEventListener("keydown", onKey);
    };
  }, [reload, onClose]);

  const run = (action: () => Promise<SettingsView>, clearInput: boolean) => {
    setBusy(true);
    setError(null);
    action()
      .then((v) => {
        setView(v);
        if (clearInput) setToken("");
        onChanged();
      })
      .catch((e: unknown) => setError(errorMessage(e)))
      .finally(() => setBusy(false));
  };

  const url = baseUrl ?? view?.nxBaseUrl ?? "";
  const canSave = !busy && url.trim() !== "" && token.trim() !== "";
  const save = () => run(() => saveNxToken(url, token), true);

  const status = !view ? null : view.nxTokenSet ? (view.nxUser ? t.connectedAs(view.nxUser) : t.tokenSet) : t.noToken;

  return (
    <div className="dialog-backdrop" onPointerDown={(e) => e.target === e.currentTarget && onClose()}>
      <div className="dialog settings" role="dialog" aria-modal="true" aria-labelledby="settings-title">
        <header className="dialog-head">
          <h2 id="settings-title">{t.title}</h2>
        </header>

        <section className="settings-section">
          <h3>{t.nxSection}</h3>
          <p className="faint">{t.nxHint}</p>
          {status && (
            <p className={view?.nxTokenSet ? "settings-ok" : "dim"}>
              {status}
              {view?.nxTaskCount !== undefined && <> · {t.tasks(view.nxTaskCount)}</>}
              {view?.nxLastSync && (
                <span className="faint">
                  {" "}
                  · {t.lastSync} {formatRelative(view.nxLastSync, now)}
                </span>
              )}
            </p>
          )}
          {view?.nxError && <p className="settings-error">{view.nxError}</p>}

          <div className="field">
            <label htmlFor="nx-url">{t.baseUrl}</label>
            <input
              id="nx-url"
              type="url"
              className="mono"
              autoComplete="off"
              spellCheck={false}
              placeholder={t.baseUrlPlaceholder}
              value={url}
              onChange={(e) => setBaseUrl(e.target.value)}
            />
          </div>
          <div className="field">
            <label htmlFor="nx-token">{t.token}</label>
            <input
              id="nx-token"
              type="password"
              autoComplete="off"
              spellCheck={false}
              placeholder={t.tokenPlaceholder}
              value={token}
              onChange={(e) => setToken(e.target.value)}
              onKeyDown={(e) => e.key === "Enter" && canSave && save()}
            />
            <small className="faint">{t.tokenStored}</small>
          </div>
          {error && <p className="settings-error">{error}</p>}
          <div className="settings-actions">
            <button className="btn btn--primary" disabled={!canSave} onClick={save}>
              {busy ? t.saving : t.save}
            </button>
            {view?.nxTokenSet && (
              <>
                <button className="btn" disabled={busy} onClick={() => void refreshNow().then(reload)}>
                  {t.refresh}
                </button>
                <button className="btn" disabled={busy} onClick={() => run(clearNxToken, false)}>
                  {t.clear}
                </button>
              </>
            )}
          </div>
        </section>

        <section className="settings-section">
          <h3>{t.quotaSection}</h3>
          <p className="faint">{t.quotaHint}</p>
          <p className="dim">
            {t.claudeAt}: {view?.claudeQuotaAt ? formatRelative(view.claudeQuotaAt, now) : t.never} · {t.codexAt}:{" "}
            {view?.codexQuotaAt ? formatRelative(view.codexQuotaAt, now) : t.never}
          </p>
          {view?.quotaError && <p className="settings-error">{view.quotaError}</p>}
        </section>

        <footer className="dialog-foot">
          <span />
          <span className="dialog-actions">
            <button className="btn" onClick={onClose}>
              {t.close}
            </button>
          </span>
        </footer>
      </div>
    </div>
  );
}
