import { useEffect, useMemo, useRef, useState } from "react";
import { vi } from "../../i18n/vi";
import type { TaskCard } from "../../ipc/types";
import { MOCK_REPOS, ageLabel } from "./startWorkMock";

interface Props {
  task: TaskCard;
  onClose: () => void;
}

const t = vi.start;

/** One dialog to pick repo, base branch and branch name before a task starts. Mock only. */
export function StartWorkDialog({ task, onClose }: Props) {
  const [repoName, setRepoName] = useState(MOCK_REPOS[0]?.name ?? "");
  const repo = MOCK_REPOS.find((r) => r.name === repoName) ?? MOCK_REPOS[0];
  const [filter, setFilter] = useState("");
  const [base, setBase] = useState<string>(() => repo?.branches.find((b) => b.lastUsed)?.name ?? "");
  const [branch, setBranch] = useState(`feature/${task.wpId}`);
  const filterRef = useRef<HTMLInputElement>(null);

  useEffect(() => {
    filterRef.current?.focus();
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") onClose();
    };
    document.addEventListener("keydown", onKey);
    return () => document.removeEventListener("keydown", onKey);
  }, [onClose]);

  const branches = useMemo(() => {
    const q = filter.trim().toLowerCase();
    return (repo?.branches ?? []).filter((b) => b.name.toLowerCase().includes(q));
  }, [repo, filter]);

  const exists = repo?.branches.some((b) => b.name === branch || b.name === `origin/${branch}`) ?? false;
  const dirty = repo?.dirtyFiles ?? 0;

  const pickRepo = (name: string) => {
    setRepoName(name);
    const next = MOCK_REPOS.find((r) => r.name === name);
    setBase(next?.branches.find((b) => b.lastUsed)?.name ?? next?.branches[0]?.name ?? "");
  };

  return (
    <div className="dialog-backdrop" onPointerDown={(e) => e.target === e.currentTarget && onClose()}>
      <div className="dialog" role="dialog" aria-modal="true" aria-labelledby="start-title">
        <header className="dialog-head">
          <h2 id="start-title">{t.title}</h2>
          <span className="dim">
            {task.id} · {task.title}
          </span>
        </header>

        <div className="field">
          <label htmlFor="sw-repo">{t.repo}</label>
          <select id="sw-repo" value={repoName} onChange={(e) => pickRepo(e.target.value)}>
            {MOCK_REPOS.map((r) => (
              <option key={r.name} value={r.name}>
                {r.name}
              </option>
            ))}
          </select>
          <small className="faint">{t.repoSuggested}</small>
        </div>

        <div className="field">
          <label htmlFor="sw-filter">{t.base}</label>
          <input
            id="sw-filter"
            ref={filterRef}
            placeholder={t.baseFilter}
            value={filter}
            onChange={(e) => setFilter(e.target.value)}
          />
          <ul className="branch-list" role="listbox" aria-label={t.base}>
            {branches.length === 0 && <li className="faint branch-empty">{t.noBranches}</li>}
            {branches.map((b) => (
              <li key={b.name}>
                <button
                  role="option"
                  aria-selected={base === b.name}
                  className={`branch${base === b.name ? " is-picked" : ""}`}
                  onClick={() => setBase(b.name)}
                >
                  <span className="branch-radio" />
                  <span className="mono branch-name">{b.name}</span>
                  <span className="faint">{ageLabel(b.ageMin)}</span>
                  <span className="branch-tags">
                    {b.aheadBehind && <span className="chip">{b.aheadBehind}</span>}
                    {b.lastUsed && <span className="chip chip--shell">{t.lastUsed}</span>}
                  </span>
                </button>
              </li>
            ))}
          </ul>
        </div>

        <div className="field">
          <label htmlFor="sw-branch">{t.branch}</label>
          <input id="sw-branch" className="mono" value={branch} onChange={(e) => setBranch(e.target.value)} />
          <small className="faint">{exists ? t.branchExists : t.branchNew}</small>
        </div>

        <div className="field field--row">
          <span className="field-label">{t.status}</span>
          <span className="dim">{t.statusNote(task.nxStatus)}</span>
        </div>

        {dirty > 0 && (
          <div className="dialog-warn">
            <span>⚠ {t.dirty(dirty)}</span>
            <span className="dialog-warn-actions">
              <button className="btn btn--sm" disabled title={t.demoNote}>
                {t.stash}
              </button>
              <button className="btn btn--sm" disabled title={t.demoNote}>
                {t.view}
              </button>
            </span>
          </div>
        )}

        <footer className="dialog-foot">
          <small className="faint">{t.demoNote}</small>
          <span className="dialog-actions">
            <button className="btn" onClick={onClose}>
              {t.cancel}
            </button>
            <button className="btn btn--primary" disabled={dirty > 0 || !base || !branch.trim()} onClick={onClose}>
              {t.confirm}
            </button>
          </span>
        </footer>
      </div>
    </div>
  );
}
