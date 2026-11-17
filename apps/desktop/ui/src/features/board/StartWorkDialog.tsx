import { useCallback, useEffect, useMemo, useRef, useState, type ReactNode } from "react";
import { stepLabel, vi } from "../../i18n/vi";
import {
  deleteBranch,
  errorCode,
  errorMessage,
  fetchRepo,
  getRepoState,
  getStartOptions,
  getTaskDetail,
  startTask,
} from "../../ipc/client";
import type { BranchOption, RepoOption, RepoState, StartOptions, StartResult, TaskCard, TaskDetail } from "../../ipc/types";
import { carryOnOptions } from "../task/carryOn";
import { CarryOnFields, useCarryOn } from "../task/CarryOnFields";
import { formatRelative } from "../../shared/format";
import { Icon } from "../../shared/ui/Icon";
import { SegmentedControl } from "../../shared/ui/SegmentedControl";
import { StatusChip } from "../../shared/ui/StatusChip";
import { branchParts, pickDefaultBase, scopeOf, splitBranches, trackText, type BranchScope } from "./startWork";

interface Props {
  task: TaskCard;
  /** other tasks in "Đang làm", to warn when they use the same repo */
  inProgress: TaskCard[];
  onClose: () => void;
  onStarted: (result: StartResult) => void;
}

const t = vi.start;

interface RowProps {
  b: BranchOption;
  picked: boolean;
  current: boolean;
  lastBase: boolean;
  onPick: (name: string) => void;
  /** absent for main branches, the checked-out one and branches of tasks in progress */
  onDelete?: () => void;
  /** the confirm panel, while this branch is being deleted */
  confirm?: ReactNode;
}

/** A branch being deleted: what the user confirmed so far. */
interface Deleting {
  name: string;
  remote: boolean;
  /** "force": git refused because of unmerged commits; ask once more */
  stage: "ask" | "force";
  /** remote only: the user types the branch name to confirm */
  typed: string;
  busy: boolean;
  error?: string;
}

const shortName = (name: string) => name.replace(/^origin\//, "");

function BranchRow({ b, picked, current, lastBase, onPick, onDelete, confirm }: RowProps) {
  const { prefix, leaf } = branchParts(b.name);
  const track = trackText(b.track);
  return (
    <li className={`branch-row${onDelete ? " has-delete" : ""}${confirm ? " is-confirming" : ""}`}>
      <button
        type="button"
        role="option"
        aria-selected={picked}
        className={`branch${picked ? " is-picked" : ""}`}
        onClick={() => onPick(b.name)}
      >
        <span className="branch-radio" />
        <Icon name={b.remote ? "cloud" : "branch"} size={13} className="branch-icon" />
        <span className="mono branch-name" title={b.name}>
          {prefix && <span className="branch-prefix">{prefix}</span>}
          {leaf}
        </span>
        {track && (
          <span className="branch-track" title={b.track}>
            {track}
          </span>
        )}
        <span className="branch-tags">
          {current && <span className="chip chip--accent">{t.current}</span>}
          {lastBase && <span className="chip chip--shell">{t.lastUsed}</span>}
        </span>
        <span className="branch-age">{formatRelative(b.committedAt, Date.now())}</span>
      </button>
      {onDelete && (
        <button
          type="button"
          className="branch-del"
          title={b.remote ? t.deleteRemoteHint : t.deleteLocalHint}
          aria-label={`${b.remote ? t.deleteRemoteHint : t.deleteLocalHint}: ${b.name}`}
          onClick={onDelete}
        >
          <Icon name="trash" size={13} />
        </button>
      )}
      {confirm}
    </li>
  );
}

/** Pick repo, base branch and branch name, then create the branch locally (never pushed). */
export function StartWorkDialog({ task, inProgress, onClose, onStarted }: Props) {
  // a task that finished a round and came back to the Inbox: ask where to carry on
  const finished = task.column === "inbox" && !!task.step && !!task.finishedAt;
  const [history, setHistory] = useState<TaskDetail>();
  useEffect(() => {
    if (!finished) return;
    let live = true;
    getTaskDetail(task.wpId)
      .then((d) => live && setHistory(d))
      .catch(() => undefined);
    return () => {
      live = false;
    };
  }, [finished, task.wpId]);
  const carryOptions = history ? carryOnOptions({ ...history, status: "done" }) : undefined;
  const carry = useCarryOn(carryOptions);
  const [opts, setOpts] = useState<StartOptions | null>(null);
  const [optsError, setOptsError] = useState<string | null>(null);
  const [extraRepos, setExtraRepos] = useState<RepoOption[]>([]);
  const [repoPath, setRepoPath] = useState("");
  const [otherPath, setOtherPath] = useState("");
  const [repo, setRepo] = useState<RepoState | null>(null);
  const [repoBusy, setRepoBusy] = useState<"load" | "fetch" | null>(null);
  const [repoError, setRepoError] = useState<string | null>(null);
  const [filter, setFilter] = useState("");
  const [scope, setScope] = useState<BranchScope>("local");
  const [base, setBase] = useState("");
  const [branch, setBranch] = useState("");
  const [branchTouched, setBranchTouched] = useState(false);
  const [showDirty, setShowDirty] = useState(false);
  const [starting, setStarting] = useState(false);
  const [startError, setStartError] = useState<string | null>(null);
  const [del, setDel] = useState<Deleting | null>(null);
  const filterRef = useRef<HTMLInputElement>(null);
  const request = useRef(0);
  const baseRef = useRef(base);
  baseRef.current = base;

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key !== "Escape") return;
      // Esc first closes an open delete confirmation, then the dialog
      if (del) {
        if (!del.busy) setDel(null);
        return;
      }
      if (!starting) onClose();
    };
    document.addEventListener("keydown", onKey);
    return () => document.removeEventListener("keydown", onKey);
  }, [onClose, starting, del]);

  useEffect(() => {
    let alive = true;
    getStartOptions(task.wpId)
      .then((o) => {
        if (!alive) return;
        setOpts(o);
        setBranch(o.branch);
        setRepoPath(o.repo ?? "");
      })
      .catch((e: unknown) => alive && setOptsError(errorMessage(e)));
    return () => {
      alive = false;
    };
  }, [task.wpId]);

  /** Apply a fresh repo state; `keepBase` after a fetch or re-check of the same repo. */
  const applyRepo = useCallback(
    (state: RepoState, keepBase: boolean) => {
      setRepo(state);
      setRepoError(null);
      const suggested = opts?.branchSource === "default" ? state.suggestedBranch : undefined;
      const nextBranch = branchTouched ? branch : (suggested ?? opts?.branch ?? branch);
      if (!branchTouched) setBranch(nextBranch);
      const prev = baseRef.current;
      const kept = keepBase && state.branches.some((b) => b.name === prev);
      if (kept) return;
      const next = pickDefaultBase(state, nextBranch);
      setBase(next);
      // open the tab that holds the pre-selected base
      const picked = state.branches.find((b) => b.name === next);
      setScope(picked ? scopeOf(picked) : "local");
    },
    [opts, branch, branchTouched],
  );

  const loadRepo = useCallback(
    (path: string, mode: "load" | "fetch", keepBase: boolean) => {
      const id = ++request.current;
      setRepoBusy(mode);
      setStartError(null);
      if (mode === "load" && !keepBase) setRepo(null);
      const call = mode === "fetch" ? fetchRepo : getRepoState;
      call(path, task.wpId)
        .then((s) => id === request.current && applyRepo(s, keepBase))
        .catch((e: unknown) => {
          if (id !== request.current) return;
          setRepoError(errorMessage(e));
          if (mode === "load") setRepo(null);
        })
        .finally(() => id === request.current && setRepoBusy(null));
    },
    [task.wpId, applyRepo],
  );

  // load branches whenever the picked repo changes (applyRepo is intentionally not a dependency)
  const loadRef = useRef(loadRepo);
  loadRef.current = loadRepo;
  useEffect(() => {
    setDel(null);
    if (!opts || !repoPath) {
      setRepo(null);
      return;
    }
    loadRef.current(repoPath, "load", false);
    filterRef.current?.focus();
  }, [opts, repoPath]);

  const repos = useMemo(() => [...(opts?.repos ?? []), ...extraRepos], [opts, extraRepos]);
  const picked = repos.find((r) => r.path === repoPath);
  const sharing = picked ? inProgress.filter((c) => c.repo === picked.name) : [];

  const split = useMemo(() => splitBranches(repo?.branches ?? [], filter), [repo, filter]);
  const shown = split[scope];
  const otherScope: BranchScope = scope === "local" ? "remote" : "local";
  const scopeName = (s: BranchScope) => (s === "local" ? t.local : t.remote);

  const name = branch.trim();
  const localExists = repo?.branches.some((b) => !b.remote && b.name === name) ?? false;
  const remoteName = `origin/${name}`;
  const remoteExists = !localExists && (repo?.branches.some((b) => b.remote && b.name === remoteName) ?? false);
  const dirty = repo?.dirtyCount ?? 0;
  const canStart = !!repo && dirty === 0 && !!base && name !== "" && !starting && repoBusy === null;

  const pickBase = (b: string) => {
    setBase(b);
    const found = repo?.branches.find((x) => x.name === b);
    if (found) setScope(scopeOf(found));
  };

  // tasks in "Đang làm" keep their branch; main branches and the checked-out one are never offered
  const inUse = new Set(inProgress.filter((c) => c.repo === repo?.name && c.branch).map((c) => c.branch));
  const canDelete = (b: BranchOption) =>
    !b.protected && repoBusy === null && (b.remote || (b.name !== repo?.current && !inUse.has(b.name)));

  const runDelete = (force: boolean) => {
    if (!repo || !del) return;
    const target = del;
    setDel({ ...target, busy: true, error: undefined });
    deleteBranch({ repo: repo.path, branch: target.name, remote: target.remote, force, wpId: task.wpId })
      .then((s) => {
        setDel(null);
        applyRepo(s, true);
      })
      .catch((e: unknown) => {
        const notMerged = errorCode(e) === "branch.notMerged";
        setDel({
          ...target,
          busy: false,
          stage: notMerged ? "force" : target.stage,
          error: notMerged ? undefined : errorMessage(e),
        });
      });
  };

  const confirmFor = (b: BranchOption): ReactNode => {
    if (!del || del.name !== b.name) return undefined;
    const short = shortName(b.name);
    const typedOk = !del.remote || del.typed.trim() === short;
    const text = del.remote
      ? t.deleteRemoteAsk(short)
      : del.stage === "force"
        ? t.deleteNotMerged(short)
        : t.deleteLocalAsk(short);
    const doLabel = del.remote ? t.deleteRemoteDo : del.stage === "force" ? t.deleteForceDo : t.deleteLocalDo;
    return (
      <div className="branch-confirm" role="alertdialog" aria-label={text}>
        <p className={del.remote || del.stage === "force" ? "branch-confirm-warn" : undefined}>
          <Icon name="alert" size={13} />
          <span>{text}</span>
        </p>
        {del.remote && (
          <input
            className="mono"
            autoFocus
            spellCheck={false}
            placeholder={t.deleteTypeName(short)}
            aria-label={t.deleteTypeName(short)}
            value={del.typed}
            disabled={del.busy}
            onChange={(e) => setDel({ ...del, typed: e.target.value })}
            onKeyDown={(e) => e.key === "Enter" && typedOk && !del.busy && runDelete(false)}
          />
        )}
        {del.error && <p className="note note--error">{del.error}</p>}
        <div className="branch-confirm-actions">
          <button className="btn btn--sm btn--ghost" disabled={del.busy} onClick={() => setDel(null)}>
            {t.deleteCancel}
          </button>
          <button
            className="btn btn--sm btn--danger"
            disabled={del.busy || !typedOk}
            onClick={() => runDelete(del.stage === "force")}
          >
            {del.busy ? t.deleting : doLabel}
          </button>
        </div>
      </div>
    );
  };

  const addOther = () => {
    const path = otherPath.trim();
    if (!path) return;
    setRepoError(null);
    getRepoState(path, task.wpId)
      .then((s) => {
        if (!repos.some((r) => r.path === s.path)) {
          setExtraRepos((xs) => [...xs, { path: s.path, name: s.name, suggested: false, remembered: false }]);
        }
        setOtherPath("");
        setRepoPath(s.path);
      })
      .catch((e: unknown) => setRepoError(errorMessage(e)));
  };

  const start = () => {
    if (!repo || !canStart) return;
    setStarting(true);
    setStartError(null);
    const carryOn = finished ? carry.pick()?.carryOn : undefined;
    startTask({ wpId: task.wpId, repo: repo.path, base, branch: name, carryOn })
      .then(onStarted)
      .catch((e: unknown) => {
        setStartError(errorMessage(e));
        if (errorCode(e) === "repo.dirty") loadRepo(repo.path, "load", true);
      })
      .finally(() => setStarting(false));
  };

  return (
    <div className="scrim" onPointerDown={(e) => e.target === e.currentTarget && !starting && onClose()}>
      <div className="sheet start-sheet" role="dialog" aria-modal="true" aria-labelledby="start-title">
        <header className="sheet-head">
          <span className="sheet-glyph">
            <Icon name="branch" size={16} />
          </span>
          <div className="sheet-titles">
            <h2 id="start-title">{t.title}</h2>
            <p title={`${task.id} · ${task.title}`}>
              <span className="mono">{task.id}</span> · {task.title}
            </p>
          </div>
          <button
            className="btn btn--ghost btn--icon"
            disabled={starting}
            aria-label={t.cancel}
            title={t.cancel}
            onClick={onClose}
          >
            <Icon name="close" size={15} />
          </button>
        </header>

        <div className="sheet-body">
          {!opts && !optsError && <p className="dim">{t.loading}</p>}
          {optsError && <p className="dialog-error">{optsError}</p>}

          {opts && (
            <>
              <section className="field">
                <label className="label" htmlFor="sw-repo">
                  {t.repo}
                </label>
                {repos.length > 0 && (
                  <select id="sw-repo" className="input" value={repoPath} onChange={(e) => setRepoPath(e.target.value)}>
                    {!repoPath && <option value="">{t.repoPick}</option>}
                    {repos.map((r) => (
                      <option key={r.path} value={r.path}>
                        {r.name}
                        {r.suggested ? " ★" : ""}
                      </option>
                    ))}
                  </select>
                )}
                {repos.length === 0 && <small className="dim">{t.repoNone(opts.workspaces.join(", "))}</small>}
                {picked && (
                  <small className="repo-meta">
                    <Icon name="folder" size={12} />
                    <span className="mono" title={picked.path}>
                      {picked.path}
                    </span>
                    {picked.suggested && <span className="chip chip--shell">{t.repoSuggested}</span>}
                    {!picked.suggested && picked.remembered && <span className="chip">{t.repoRemembered}</span>}
                  </small>
                )}
                {sharing.map((c) => (
                  <p key={c.id} className="note note--warn">
                    <Icon name="alert" size={13} />
                    <span>{t.sharedRepo(c.id, c.branch ?? "?")}</span>
                  </p>
                ))}
                <details className="repo-other" open={repos.length === 0}>
                  <summary>{t.repoOther}</summary>
                  <span className="repo-other-row">
                    <input
                      className="input mono"
                      placeholder={t.repoOtherPlaceholder}
                      value={otherPath}
                      spellCheck={false}
                      onChange={(e) => setOtherPath(e.target.value)}
                      onKeyDown={(e) => e.key === "Enter" && addOther()}
                    />
                    <button className="btn" disabled={!otherPath.trim()} onClick={addOther}>
                      {t.repoAdd}
                    </button>
                  </span>
                </details>
              </section>

              {repoError && <p className="dialog-error">{repoError}</p>}

              {repoPath && (
                <section className="field">
                  <div className="field-head">
                    <span className="label" id="sw-base-label">
                      {t.base}
                    </span>
                    <SegmentedControl
                      label={t.scope}
                      value={scope}
                      onChange={setScope}
                      options={[
                        {
                          value: "local",
                          label: t.local,
                          icon: <Icon name="laptop" size={13} />,
                          count: repo ? split.local.length : undefined,
                          title: t.localHint,
                        },
                        {
                          value: "remote",
                          label: t.remote,
                          icon: <Icon name="cloud" size={13} />,
                          count: repo ? split.remote.length : undefined,
                          title: t.remoteHint,
                        },
                      ]}
                    />
                    <span className="field-head-spacer" />
                    <button
                      className="btn btn--sm btn--ghost"
                      title={t.fetchHint}
                      disabled={repoBusy !== null}
                      onClick={() => loadRepo(repoPath, "fetch", true)}
                    >
                      <Icon name="refresh" size={13} className={repoBusy === "fetch" ? "is-spinning" : undefined} />
                      {repoBusy === "fetch" ? t.fetching : t.fetch}
                    </button>
                  </div>
                  <label className="input search">
                    <Icon name="search" size={13} />
                    <input
                      ref={filterRef}
                      placeholder={t.baseFilter}
                      aria-label={t.baseFilter}
                      value={filter}
                      spellCheck={false}
                      onChange={(e) => setFilter(e.target.value)}
                    />
                  </label>
                  <ul className="branch-list" role="listbox" aria-labelledby="sw-base-label">
                    {!repo && repoBusy && <li className="branch-empty">{t.baseLoading}</li>}
                    {repo && shown.length === 0 && (
                      <li className="branch-empty">
                        <span>{t.noBranches}</span>
                        {split[otherScope].length > 0 && (
                          <button className="btn btn--sm" onClick={() => setScope(otherScope)}>
                            {t.otherScope(split[otherScope].length, scopeName(otherScope))}
                          </button>
                        )}
                      </li>
                    )}
                    {shown.map((b) => (
                      <BranchRow
                        key={b.name}
                        b={b}
                        picked={base === b.name}
                        current={b.name === repo?.current}
                        lastBase={b.name === repo?.lastBase}
                        onPick={pickBase}
                        onDelete={
                          canDelete(b)
                            ? () => setDel({ name: b.name, remote: b.remote, stage: "ask", typed: "", busy: false })
                            : undefined
                        }
                        confirm={confirmFor(b)}
                      />
                    ))}
                  </ul>
                  <small className="picked-base">
                    {t.picked}{" "}
                    {base ? <span className="mono">{base}</span> : <span className="faint">{t.pickedNone}</span>}
                  </small>
                </section>
              )}

              <section className="field">
                <label className="label" htmlFor="sw-branch">
                  {t.branch}
                </label>
                <input
                  id="sw-branch"
                  className="input mono"
                  value={branch}
                  spellCheck={false}
                  onChange={(e) => {
                    setBranch(e.target.value);
                    setBranchTouched(true);
                  }}
                  onKeyDown={(e) => e.key === "Enter" && start()}
                />
                {repo && name && (
                  <small className="faint">
                    {localExists && t.branchExists}
                    {remoteExists && (
                      <>
                        {t.branchRemote(remoteName)}{" "}
                        {base !== remoteName && (
                          <button className="btn btn--sm" onClick={() => pickBase(remoteName)}>
                            {t.useAsBase}
                          </button>
                        )}
                      </>
                    )}
                    {!localExists && !remoteExists && t.branchNew}
                    {!branchTouched && repo.suggestedBranch === name && opts.branchSource === "default" && (
                      <> · {t.branchConvention}</>
                    )}
                  </small>
                )}
              </section>

              <dl className="start-facts">
                <dt>{t.status}</dt>
                <dd className="start-status">
                  <StatusChip name={task.nxStatus} color={task.nxStatusColor} />
                  <span className="dim">{t.statusNoteShort}</span>
                </dd>
                {opts.issueLinks.length > 0 && (
                  <>
                    <dt>{t.issues}</dt>
                    <dd className="dim mono issue-links">{opts.issueLinks.join("  ")}</dd>
                  </>
                )}
              </dl>

              {dirty > 0 && (
                <div className="dialog-warn dialog-warn--stack">
                  <span className="dialog-warn-row">
                    <span>⚠ {t.dirty(dirty)}</span>
                    <span className="dialog-warn-actions">
                      <button className="btn btn--sm" onClick={() => setShowDirty((v) => !v)}>
                        {showDirty ? t.hide : t.view}
                      </button>
                      <button
                        className="btn btn--sm"
                        disabled={repoBusy !== null}
                        onClick={() => loadRepo(repoPath, "load", true)}
                      >
                        {t.recheck}
                      </button>
                    </span>
                  </span>
                  {showDirty && (
                    <pre className="dirty-list mono">
                      {repo?.dirty.join("\n")}
                      {repo && repo.dirtyCount > repo.dirty.length && "\n…"}
                    </pre>
                  )}
                </div>
              )}
            </>
          )}

          {task.step && !finished && (
            <p className="note start-history">{vi.carryOn.startPaused(stepLabel[task.step.kind])}</p>
          )}
          {finished && (
            <section className="field start-history">
              <span className="label" id="start-carry">
                {vi.carryOn.startTitle}
              </span>
              {carryOptions ? (
                <CarryOnFields options={carryOptions} state={carry} labelledBy="start-carry" />
              ) : (
                <small className="dim">{vi.carryOn.loading}</small>
              )}
              <small className="faint">{vi.carryOn.startKeeps}</small>
            </section>
          )}

          {startError && <p className="dialog-error">{startError}</p>}
        </div>

        <footer className="sheet-foot">
          <small className="faint">{t.footNote}</small>
          <span className="dialog-actions">
            <button className="btn" disabled={starting} onClick={onClose}>
              {t.cancel}
            </button>
            <button className="btn btn--primary" disabled={!canStart} onClick={start}>
              {starting ? t.starting : t.confirm}
            </button>
          </span>
        </footer>
      </div>
    </div>
  );
}
