import { useEffect, useState } from "react";
import { vi } from "../../i18n/vi";
import { errorMessage, getWorkspaceRepos, newSession } from "../../ipc/client";
import { Icon } from "../../shared/ui/Icon";
import { repoName } from "./chatState";
import "../task/flowDialogs.css";

const t = vi.workspace.newDialog;

interface Props {
  onClose: () => void;
  onCreated: (id: string) => void;
}

/** "+ Phiên mới": pick a repo (or paste a folder), get an empty session to type into. */
export function NewSessionDialog({ onClose, onCreated }: Props) {
  const [repos, setRepos] = useState<string[]>();
  const [repo, setRepo] = useState("");
  const [path, setPath] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string>();

  useEffect(() => {
    let alive = true;
    getWorkspaceRepos()
      .then((list) => {
        if (!alive) return;
        setRepos(list);
        setRepo((r) => r || list[0] || "");
      })
      .catch((e: unknown) => alive && setError(errorMessage(e)));
    return () => {
      alive = false;
    };
  }, []);

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key !== "Escape") return;
      e.stopPropagation();
      if (!busy) onClose();
    };
    window.addEventListener("keydown", onKey, true);
    return () => window.removeEventListener("keydown", onKey, true);
  }, [busy, onClose]);

  const chosen = path.trim() || repo;
  const create = () => {
    if (!chosen) return;
    setBusy(true);
    setError(undefined);
    newSession({ repo: chosen })
      .then((s) => onCreated(s.id))
      .catch((e: unknown) => {
        setError(errorMessage(e));
        setBusy(false);
      });
  };

  return (
    <div className="scrim flow-scrim" onPointerDown={(e) => e.target === e.currentTarget && !busy && onClose()}>
      <div className="sheet flow-sheet" role="dialog" aria-modal="true" aria-labelledby="ws-new-title">
        <header className="sheet-head">
          <span className="sheet-glyph">
            <Icon name="folder" size={16} />
          </span>
          <div className="sheet-titles">
            <h2 id="ws-new-title">{t.title}</h2>
            <p>{t.body}</p>
          </div>
        </header>
        <div className="sheet-body flow-body">
          <div className="flow-choices ws-repos" role="radiogroup" aria-label={t.repo}>
            {repos === undefined && !error && <p className="dim">{t.loading}</p>}
            {repos?.length === 0 && <p className="dim">{t.none}</p>}
            {repos?.map((r) => (
              <label key={r} className={`flow-choice${!path.trim() && repo === r ? " is-picked" : ""}`}>
                <input
                  type="radio"
                  name="ws-repo"
                  checked={!path.trim() && repo === r}
                  onChange={() => {
                    setRepo(r);
                    setPath("");
                  }}
                />
                <span className="flow-choice-text">
                  <strong>{repoName(r)}</strong>
                  <small className="faint mono">{r}</small>
                </span>
              </label>
            ))}
          </div>
          <label className="ask-field">
            <span className="faint">{t.path}</span>
            <input
              className="input mono"
              value={path}
              placeholder={t.pathPlaceholder}
              onChange={(e) => setPath(e.target.value)}
              onKeyDown={(e) => e.key === "Enter" && create()}
            />
          </label>
          {error && <p className="note note--error">{error}</p>}
        </div>
        <footer className="sheet-foot flow-foot">
          <span className="flow-actions">
            <button className="btn btn--ghost btn--sm" disabled={busy} onClick={onClose}>
              {t.cancel}
            </button>
            <button className="btn btn--sm btn--primary" disabled={busy || !chosen} onClick={create} autoFocus>
              {t.create}
            </button>
          </span>
        </footer>
      </div>
    </div>
  );
}
