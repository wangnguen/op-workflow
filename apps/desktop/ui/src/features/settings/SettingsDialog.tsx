import { useCallback, useEffect, useMemo, useRef, useState, type ReactNode } from "react";
import { vi } from "../../i18n/vi";
import {
  checkUpdate,
  clearNxToken,
  errorMessage,
  getSettings,
  getStepModels,
  getUpdateStatus,
  isMockMode,
  openBrowserDebug,
  openConfig,
  openReleasePage,
  refreshNow,
  revealPath,
  saveNxToken,
  setNotify,
  setOnClose,
  setQuotaProbe,
  setTasksDir,
} from "../../ipc/client";
import type { CloseChoice, NotifyKind, Provider, ProviderLink, SettingsView, StepModels, UpdateView } from "../../ipc/types";
import { LinkActions } from "../quota/QuotaPopover";
import { linkProblem } from "../quota/quota";
import { formatRelative } from "../../shared/format";
import { Icon } from "../../shared/ui/Icon";
import { PAGES, searchEntries, type PageKey } from "./pages";
import { StepsPage } from "./StepsPage";
import { ThemePicker } from "./ThemePicker";
import "./settings.css";

interface Props {
  now: number;
  /** Claude / Codex quota connections (from the board snapshot) */
  links: ProviderLink[];
  onClose: () => void;
  /** called after the token changes so the board reloads */
  onChanged: () => void;
  /** called after a manual update check so the top bar picks up the result */
  onUpdateChanged: () => void;
}

const t = vi.settings;
const FLASH_MS = 1400;
/** `quota.claudeActiveProbeSec` / `quota.claudeProbeMin` defaults in the core. */
const DEFAULT_PROBE_SEC = 60;
const DEFAULT_IDLE_MIN = 15;
const PROBE_CHOICES = [0, 30, 60, 120, 300];
const IDLE_CHOICES = [5, 10, 15, 30, 60];

/** The usual intervals, plus the saved one when config.json holds another value. */
function withSaved(choices: number[], current: number): number[] {
  return choices.includes(current) ? choices : [...choices, current].sort((a, b) => a - b);
}

const CLOSE_CHOICES: CloseChoice[] = ["ask", "tray", "quit"];

const NOTIFY_ROWS: { kind: NotifyKind; id: string; label: string; hint: string }[] = [
  { kind: "comments", id: "notifyComments", label: vi.notify.comments, hint: vi.notify.commentsHint },
  { kind: "steps", id: "notifySteps", label: vi.notify.steps, hint: vi.notify.stepsHint },
  { kind: "quota", id: "notifyQuota", label: vi.notify.quota, hint: vi.notify.quotaHint },
];

/** Small stroke icons for the page list (drawn here, no icon font). */
const ICONS: Record<PageKey, ReactNode> = {
  appearance: (
    <>
      <path
        d="M12 3.5a8.5 8.5 0 0 0 0 17c1.2 0 1.8-.9 1.4-1.9-.5-1.2.3-2.3 1.6-2.3h1.8a3.7 3.7 0 0 0 3.7-3.7C20.5 7.6 16.7 3.5 12 3.5z"
        className="icon-stroke"
      />
      <circle cx={8} cy={11} r={1} className="icon-stroke" />
      <circle cx={11} cy={7.5} r={1} className="icon-stroke" />
      <circle cx={15.5} cy={8.5} r={1} className="icon-stroke" />
    </>
  ),
  connection: <path d="M9 7V3M15 7V3M7 7h10v4a5 5 0 0 1-10 0zM12 16v5" className="icon-stroke" />,
  repos: (
    <>
      <circle cx={6} cy={5} r={2} className="icon-stroke" />
      <circle cx={6} cy={19} r={2} className="icon-stroke" />
      <circle cx={18} cy={7} r={2} className="icon-stroke" />
      <path d="M6 7v10M18 9c0 5-12 3-12 8" className="icon-stroke" />
    </>
  ),
  quota: <path d="M4 16a8 8 0 1 1 16 0M12 16l4-5M4 16h2M18 16h2" className="icon-stroke" />,
  steps: (
    <>
      <circle cx={6} cy={6} r={2} className="icon-stroke" />
      <circle cx={6} cy={18} r={2} className="icon-stroke" />
      <path d="M11 6h9M11 18h9M11 12h9M4 12h4" className="icon-stroke" />
    </>
  ),
  notify: <path d="M6 16V11a6 6 0 1 1 12 0v5l1.5 2h-15z M10 20a2 2 0 0 0 4 0" className="icon-stroke" />,
  about: (
    <>
      <circle cx={12} cy={12} r={9} className="icon-stroke" />
      <path d="M12 11v6M12 7.5v.5" className="icon-stroke" />
    </>
  ),
};

function Row({ id, label, hint, children, stacked }: {
  id: string;
  label: string;
  hint?: string;
  children?: ReactNode;
  stacked?: boolean;
}) {
  return (
    <div className={`set-row${stacked ? " set-row--stacked" : ""}`} data-setting-row={id}>
      <div className="set-row-text">
        <span className="set-row-label">{label}</span>
        {hint && <span className="set-row-hint">{hint}</span>}
      </div>
      {children !== undefined && <div className="set-row-control">{children}</div>}
    </div>
  );
}

/** A path from Settings; click opens it in Explorer. */
function PathLink({ path, onError }: { path: string; onError: (e: string | null) => void }) {
  return (
    <button
      type="button"
      className="set-path-link mono"
      title={t.reveal}
      disabled={isMockMode()}
      onClick={() => {
        onError(null);
        revealPath(path).catch((e: unknown) => onError(errorMessage(e)));
      }}
    >
      <span>{path}</span>
      <Icon name="folder" size={12} />
    </button>
  );
}

/** Same folder whatever the slash direction or letter case (Windows paths). */
const normPath = (p: string) =>
  p
    .replace(/\\/g, "/")
    .replace(/\/+$/, "")
    .toLowerCase();
const samePath = (a: string, b: string) => normPath(a) === normPath(b);

/** Where briefs live: open it, move it to the Desktop or elsewhere, or back to the data folder. */
function TasksDirRow({ view, onDone, onError }: {
  view: SettingsView | null;
  onDone: () => void;
  onError: (e: string | null) => void;
}) {
  const [custom, setCustom] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [note, setNote] = useState<string | null>(null);
  const current = view?.tasksDir;
  const suggested = view?.tasksDirSuggested;
  const atSuggested = !!(current && suggested && samePath(current, suggested));
  const atDefault = !!(current && view?.dataDir && samePath(current, `${view.dataDir}/tasks`));

  const apply = (dir?: string) => {
    setBusy(true);
    onError(null);
    setNote(null);
    setTasksDir(dir)
      .then((r) => {
        setNote([t.tasksDirMoved(r.path, r.moved), ...r.warnings].join(". "));
        setCustom(null);
        onDone();
      })
      .catch((e: unknown) => onError(errorMessage(e)))
      .finally(() => setBusy(false));
  };

  return (
    <Row id="tasksDir" label={t.tasksDir} hint={view?.tasksDirChosen ? t.tasksDirHint : t.tasksDirNotChosen} stacked>
      {current ? <PathLink path={current} onError={onError} /> : "—"}
      <span className="set-actions set-actions--start">
        {suggested && !atSuggested && (
          <button className="btn btn--sm btn--primary" disabled={busy} onClick={() => apply(suggested)}>
            <Icon name="folder" size={12} />
            {t.tasksDirToDesktop}
          </button>
        )}
        <button className="btn btn--sm" disabled={busy} onClick={() => setCustom((c) => (c === null ? "" : null))}>
          {t.tasksDirOther}
        </button>
        {view?.tasksDirChosen && !atDefault && (
          <button className="btn btn--sm btn--ghost" disabled={busy} onClick={() => apply(undefined)}>
            {t.tasksDirBack}
          </button>
        )}
      </span>
      {custom !== null && (
        <span className="set-inline">
          <input
            className="mono"
            autoComplete="off"
            spellCheck={false}
            placeholder={t.tasksDirPlaceholder}
            value={custom}
            onChange={(e) => setCustom(e.target.value)}
            onKeyDown={(e) => e.key === "Enter" && custom.trim() && !busy && apply(custom.trim())}
          />
          <button className="btn btn--sm btn--primary" disabled={busy || !custom.trim()} onClick={() => apply(custom.trim())}>
            {t.tasksDirApply}
          </button>
        </span>
      )}
      {note && <span className="set-ok">{note}</span>}
    </Row>
  );
}

function Group({ title, children }: { title?: string; children: ReactNode }) {
  return (
    <section className="set-section">
      {title && <h4 className="set-section-title">{title}</h4>}
      <div className="set-group">{children}</div>
    </section>
  );
}

/** Two-pane settings: page list + search on the left, grouped rows on the right. */
export function SettingsDialog({ now, links, onClose, onChanged, onUpdateChanged }: Props) {
  const [page, setPage] = useState<PageKey>("connection");
  const [query, setQuery] = useState("");
  const [view, setView] = useState<SettingsView | null>(null);
  const [token, setToken] = useState("");
  const [baseUrl, setBaseUrl] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [pathError, setPathError] = useState<string | null>(null);
  const [update, setUpdate] = useState<UpdateView | null>(null);
  const [checking, setChecking] = useState(false);
  const [models, setModels] = useState<StepModels | null>(null);
  const [modelsBusy, setModelsBusy] = useState(false);
  const modelsAsked = useRef(false);
  const contentRef = useRef<HTMLDivElement>(null);
  const navRef = useRef<HTMLElement>(null);

  const reload = useCallback(() => {
    getSettings()
      .then(setView)
      .catch((e: unknown) => setError(errorMessage(e)));
  }, []);

  // Models and effort levels come from the CLIs on the machine (both start for ~2 s, no tokens),
  // so they are read the first time Các bước AI opens, not with every Settings open.
  const readModels = useCallback((refresh: boolean) => {
    setModelsBusy(true);
    getStepModels(refresh)
      .then(setModels)
      .catch((e: unknown) => setError(errorMessage(e)))
      .finally(() => setModelsBusy(false));
  }, []);
  useEffect(() => {
    if (page !== "steps" || modelsAsked.current) return;
    modelsAsked.current = true;
    readModels(false);
  }, [page, readModels]);

  // The parent passes a new `onClose` on every render (its clock ticks each second). Reading it
  // through a ref keeps the effect below to the moment the dialog opens: re-running it moved the
  // focus back to the page list every second, which closed any open dropdown.
  const closeRef = useRef(onClose);
  useEffect(() => {
    closeRef.current = onClose;
  }, [onClose]);

  useEffect(() => {
    reload();
    getUpdateStatus()
      .then(setUpdate)
      .catch(() => undefined);
    const id = window.setInterval(reload, 3000);
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") closeRef.current();
    };
    document.addEventListener("keydown", onKey);
    navRef.current?.querySelector<HTMLElement>('[aria-current="page"]')?.focus();
    return () => {
      window.clearInterval(id);
      document.removeEventListener("keydown", onKey);
    };
  }, [reload]);

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

  const check = () => {
    setChecking(true);
    checkUpdate()
      .then((u) => {
        setUpdate(u);
        onUpdateChanged();
      })
      .catch((e: unknown) => setUpdate((u) => (u ? { ...u, error: errorMessage(e) } : u)))
      .finally(() => setChecking(false));
  };

  const show = (next: PageKey) => {
    setPage(next);
    setQuery("");
    contentRef.current?.scrollTo({ top: 0 });
  };

  const openHit = (next: PageKey, id: string) => {
    show(next);
    window.setTimeout(() => {
      const row = contentRef.current?.querySelector<HTMLElement>(`[data-setting-row="${id}"]`);
      if (!row) return;
      row.scrollIntoView({ block: "center" });
      row.classList.add("is-flash");
      window.setTimeout(() => row.classList.remove("is-flash"), FLASH_MS);
    }, 0);
  };

  const hits = useMemo(() => searchEntries(query), [query]);
  const searching = query.trim() !== "";

  const url = baseUrl ?? view?.nxBaseUrl ?? "";
  // with a token already stored, saving only the URL is fine (the core reuses the stored token)
  const canSave = !busy && url.trim() !== "" && (token.trim() !== "" || view?.nxTokenSet === true);
  const save = () => run(() => saveNxToken(url, token), true);
  const status = !view ? "…" : view.nxTokenSet ? (view.nxUser ? t.connectedAs(view.nxUser) : t.tokenSet) : t.noToken;
  const probeSec = view?.claudeActiveProbeSec ?? DEFAULT_PROBE_SEC;
  const idleMin = view?.claudeProbeMin ?? DEFAULT_IDLE_MIN;

  const versionText = !update
    ? "…"
    : !update.enabled
      ? t.versionOff
      : update.available && update.latest
        ? t.versionAvailable(update.latest)
        : update.latest
          ? `${t.versionUpToDate} (v${update.latest})`
          : update.checkedAt
            ? t.versionUpToDate
            : t.never;

  return (
    <div className="dialog-backdrop" onPointerDown={(e) => e.target === e.currentTarget && onClose()}>
      <div className="set-sheet" role="dialog" aria-modal="true" aria-labelledby="settings-title">
        <h2 id="settings-title" className="sr-only">
          {t.title}
        </h2>
        <aside className="set-side">
          <label className="set-search">
            <span className="sr-only">{t.search}</span>
            <svg viewBox="0 0 24 24" width={14} height={14} aria-hidden="true">
              <path d="M10.5 4a6.5 6.5 0 1 0 0 13a6.5 6.5 0 1 0 0-13M15.5 15.5 20 20" className="icon-stroke" />
            </svg>
            <input
              type="search"
              value={query}
              placeholder={t.search}
              spellCheck={false}
              autoComplete="off"
              onChange={(e) => setQuery(e.target.value)}
              onKeyDown={(e) => {
                const first = hits[0];
                if (e.key === "Enter" && first) openHit(first.page, first.id);
              }}
            />
          </label>
          <nav className="set-nav" aria-label={t.title} ref={navRef}>
            {PAGES.map((p) => {
              const current = p === page && !searching;
              return (
                <button
                  key={p}
                  className={`set-nav-item${current ? " is-on" : ""}`}
                  aria-current={current ? "page" : undefined}
                  onClick={() => show(p)}
                >
                  <svg viewBox="0 0 24 24" width={15} height={15} aria-hidden="true">
                    {ICONS[p]}
                  </svg>
                  <span>{t.pages[p]}</span>
                  {p === "about" && update?.available && <span className="set-nav-dot" aria-hidden="true" />}
                </button>
              );
            })}
          </nav>
          <p className="set-side-note">{t.sidebarNote}</p>
        </aside>

        <div className="set-main">
          <header className="set-head">
            <h3>{searching ? t.results : t.pages[page]}</h3>
            <button className="btn btn--icon btn--ghost" aria-label={t.close} title={t.close} onClick={onClose}>
              <svg viewBox="0 0 24 24" width={15} height={15} aria-hidden="true">
                <path d="M6 6l12 12M18 6 6 18" className="icon-stroke" />
              </svg>
            </button>
          </header>

          <div className="set-content" ref={contentRef}>
            {pathError && <p className="note note--error set-path-error">{pathError}</p>}
            {searching ? (
              hits.length > 0 ? (
                <div className="set-group">
                  {hits.map((h) => (
                    <button key={`${h.page}:${h.id}`} className="set-hit" onClick={() => openHit(h.page, h.id)}>
                      <span className="faint">{t.pages[h.page]}</span>
                      <span aria-hidden="true">›</span>
                      <span>{h.label}</span>
                    </button>
                  ))}
                </div>
              ) : (
                <p className="set-empty">{t.noResults(query.trim())}</p>
              )
            ) : (
              <>
                {page === "connection" && (
                  <>
                    <p className="set-intro">{t.nxHint}</p>
                    <Group title={t.groupStatus}>
                      <Row id="connection" label={t.connection}>
                        <span className={view?.nxTokenSet ? "set-ok" : "dim"}>{status}</span>
                      </Row>
                      {view?.nxTokenSet && (
                        <Row
                          id="sync"
                          label={t.taskCount}
                          hint={view.nxLastSync ? `${t.lastSync} ${formatRelative(view.nxLastSync, now)}` : undefined}
                        >
                          <span className="set-value">{view.nxTaskCount ?? "…"}</span>
                          <button className="btn btn--sm" disabled={busy} onClick={() => void refreshNow().then(reload)}>
                            {t.refresh}
                          </button>
                        </Row>
                      )}
                      {view?.nxError && (
                        <div className="set-row">
                          <span className="settings-error">{view.nxError}</span>
                        </div>
                      )}
                    </Group>
                    <Group title={t.groupAccount}>
                      <Row id="url" label={t.baseUrl} stacked>
                        <input
                          type="url"
                          className="mono"
                          autoComplete="off"
                          spellCheck={false}
                          placeholder={t.baseUrlPlaceholder}
                          value={url}
                          onChange={(e) => setBaseUrl(e.target.value)}
                        />
                      </Row>
                      <Row id="token" label={t.token} hint={view?.nxTokenSet ? t.tokenKeep : t.tokenStored} stacked>
                        <input
                          type="password"
                          autoComplete="off"
                          spellCheck={false}
                          placeholder={t.tokenPlaceholder}
                          value={token}
                          onChange={(e) => setToken(e.target.value)}
                          onKeyDown={(e) => e.key === "Enter" && canSave && save()}
                        />
                      </Row>
                      <div className="set-row set-row--actions">
                        {error && <span className="settings-error">{error}</span>}
                        <span className="set-actions">
                          {view?.nxTokenSet && (
                            <button className="btn" disabled={busy} onClick={() => run(clearNxToken, false)}>
                              {t.clear}
                            </button>
                          )}
                          <button className="btn btn--primary" disabled={!canSave} onClick={save}>
                            {busy ? t.saving : t.save}
                          </button>
                        </span>
                      </div>
                    </Group>
                    <Group title={t.githubSection}>
                      <Row id="github" label={t.githubBrowser} hint={t.githubHint}>
                        <span className={view?.browserDebug ? "set-ok" : "dim"}>
                          {view?.browserDebug ? t.githubOn(view.browserDebug) : t.githubOff}
                        </span>
                      </Row>
                      {!view?.browserDebug && (
                        <div className="set-row set-row--actions">
                          <span className="set-row-hint">{t.githubSteps}</span>
                          <span className="set-actions">
                            <button
                              className="btn btn--sm"
                              disabled={isMockMode()}
                              onClick={() => {
                                setPathError(null);
                                openBrowserDebug().catch((e: unknown) => setPathError(errorMessage(e)));
                              }}
                            >
                              {t.githubOpen}
                            </button>
                          </span>
                        </div>
                      )}
                    </Group>
                  </>
                )}

                {page === "repos" && (
                  <>
                    <Group title={t.workspaces}>
                      <Row id="workspaces" label={t.workspaces} hint={t.workspacesHint} stacked>
                        <ul className="set-list mono">
                          {(view?.workspaces ?? []).map((w) => (
                            <li key={w}>
                              <PathLink path={w} onError={setPathError} />
                            </li>
                          ))}
                        </ul>
                      </Row>
                    </Group>
                    <Group title={t.remembered}>
                      {view && view.projects.length === 0 && (
                        <div className="set-row">
                          <span className="dim">{t.rememberedEmpty}</span>
                        </div>
                      )}
                      {view?.projects.map((p) => (
                        <Row
                          key={p.name}
                          id={`project:${p.name}`}
                          label={p.name}
                          hint={`${t.branchTemplate}: ${p.branch ?? t.defaultTemplate}`}
                          stacked
                        >
                          <ul className="set-list mono">
                            {p.repos.map((r) => (
                              <li key={r}>
                                <PathLink path={r} onError={setPathError} />
                              </li>
                            ))}
                          </ul>
                        </Row>
                      ))}
                    </Group>
                    <p className="set-note">{t.rememberedHint}</p>
                    <Row id="config" label={t.editConfig}>
                      <button className="btn btn--sm" disabled={isMockMode()} onClick={() => void openConfig()}>
                        {t.openConfig}
                      </button>
                    </Row>
                  </>
                )}

                {page === "steps" && (
                  <StepsPage
                    view={view}
                    onSaved={setView}
                    models={models}
                    modelsBusy={modelsBusy}
                    onReadModels={() => readModels(true)}
                  />
                )}

                {page === "quota" && (
                  <>
                    <p className="set-intro">{t.quotaHint}</p>
                    <Group>
                      {(["claude", "codex"] as Provider[]).map((p) => {
                        const at = p === "claude" ? view?.claudeQuotaAt : view?.codexQuotaAt;
                        const link = links.find((l) => l.provider === p);
                        const status = link ? vi.quota.linkStatus[link.status] : undefined;
                        return (
                          <div key={p} className="set-provider">
                            <Row
                              id={p}
                              label={p === "claude" ? t.claudeQuota : t.codexQuota}
                              hint={`${p === "claude" ? t.claudeAt : t.codexAt} ${at ? formatRelative(at, now) : t.never}`}
                            >
                              {link ? (
                                <LinkActions provider={p} link={link} now={now} onChanged={onChanged} compact />
                              ) : (
                                <span className="set-value">{at ? formatRelative(at, now) : t.never}</span>
                              )}
                            </Row>
                            {status && (
                              <p className={`set-link set-link--${link?.status ?? "ok"}`}>
                                {status}
                                {linkProblem(link) && link?.message ? `: ${link.message}` : ""}
                              </p>
                            )}
                          </div>
                        );
                      })}
                      {links.length === 0 && view?.quotaError && (
                        <div className="set-row">
                          <span className="settings-error">{view.quotaError}</span>
                        </div>
                      )}
                    </Group>
                    <Group>
                      <Row id="activeProbe" label={t.activeProbe} hint={t.activeProbeHint}>
                        <select
                          className="input set-probe-select"
                          aria-label={t.activeProbe}
                          disabled={busy || !view}
                          value={probeSec}
                          onChange={(e) => run(() => setQuotaProbe(Number(e.target.value), idleMin), false)}
                        >
                          {withSaved(PROBE_CHOICES, probeSec).map((secs) => (
                            <option key={secs} value={secs}>
                              {secs === 0 ? t.activeProbeOff : t.activeProbeEvery(secs)}
                              {secs === DEFAULT_PROBE_SEC ? t.activeProbeDefault : ""}
                            </option>
                          ))}
                        </select>
                      </Row>
                      <Row id="idleProbe" label={t.idleProbe} hint={t.idleProbeHint}>
                        <select
                          className="input set-probe-select"
                          aria-label={t.idleProbe}
                          disabled={busy || !view}
                          value={idleMin}
                          onChange={(e) => run(() => setQuotaProbe(probeSec, Number(e.target.value)), false)}
                        >
                          {withSaved(IDLE_CHOICES, idleMin).map((mins) => (
                            <option key={mins} value={mins}>
                              {t.activeProbeEvery(mins * 60)}
                              {mins === DEFAULT_IDLE_MIN ? t.activeProbeDefault : ""}
                            </option>
                          ))}
                        </select>
                      </Row>
                    </Group>
                  </>
                )}

                {page === "notify" && (
                  <>
                    <p className="set-intro">{vi.notify.hint}</p>
                    <Group title={vi.notify.title}>
                      {NOTIFY_ROWS.map(({ kind, id, label, hint }) => (
                        <Row key={kind} id={id} label={label} hint={hint}>
                          <input
                            type="checkbox"
                            className="set-check"
                            aria-label={label}
                            checked={view?.notify?.[kind] ?? true}
                            disabled={busy || !view}
                            onChange={(e) => run(() => setNotify(kind, e.target.checked), false)}
                          />
                        </Row>
                      ))}
                    </Group>
                    <Group title={vi.notify.trayLabel}>
                      <Row id="tray" label={vi.notify.trayLabel} hint={vi.notify.tray} />
                      <Row id="onClose" label={vi.close.setting} hint={vi.close.settingHint}>
                        <select
                          className="input set-probe-select"
                          aria-label={vi.close.setting}
                          disabled={busy || !view}
                          value={view?.onClose ?? "ask"}
                          onChange={(e) => run(() => setOnClose(e.target.value as CloseChoice), false)}
                        >
                          {CLOSE_CHOICES.map((choice) => (
                            <option key={choice} value={choice}>
                              {vi.close.choices[choice]}
                            </option>
                          ))}
                        </select>
                      </Row>
                    </Group>
                  </>
                )}

                {page === "appearance" && (
                  <Group title={t.theme}>
                    <div className="set-row set-row--stacked" data-setting-row="theme">
                      <ThemePicker />
                    </div>
                  </Group>
                )}

                {page === "about" && (
                  <>
                    <Group title={t.versionSection}>
                      <Row
                        id="version"
                        label={update ? t.versionCurrent(update.current) : t.versionSection}
                        hint={
                          update?.checkedAt ? `${t.versionChecked} ${formatRelative(update.checkedAt, now)}` : undefined
                        }
                      >
                        <span className={update?.available ? "set-ok" : "dim"}>{versionText}</span>
                      </Row>
                      {update?.error && (
                        <div className="set-row">
                          <span className="settings-error">{update.error}</span>
                        </div>
                      )}
                      <div className="set-row set-row--actions">
                        <span className="set-row-hint">{t.versionHint}</span>
                        <span className="set-actions">
                          <button className="btn" disabled={checking} onClick={check}>
                            {checking ? t.versionChecking : t.versionCheck}
                          </button>
                          {update?.available && (
                            <button className="btn btn--primary" onClick={() => void openReleasePage()}>
                              {t.versionDownload}
                            </button>
                          )}
                        </span>
                      </div>
                    </Group>
                    <Group title={t.groupData}>
                      <TasksDirRow
                        view={view}
                        onDone={() => {
                          reload();
                          onChanged();
                        }}
                        onError={setPathError}
                      />
                      <Row id="data" label={t.dataDir} stacked>
                        {view?.dataDir ? <PathLink path={view.dataDir} onError={setPathError} /> : "—"}
                      </Row>
                      <Row id="configFile" label={t.configFile} stacked>
                        {view?.configPath ? <PathLink path={view.configPath} onError={setPathError} /> : "—"}
                      </Row>
                    </Group>
                  </>
                )}
              </>
            )}
          </div>
        </div>
      </div>
    </div>
  );
}
