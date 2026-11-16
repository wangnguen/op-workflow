import { Fragment, useCallback, useEffect, useMemo, useRef, useState } from "react";
import { severityLabel, vi } from "../../i18n/vi";
import { errorMessage, getTaskDiff } from "../../ipc/client";
import type { DiffFile, DiffLine, DiffView, FindingView, TaskDetail } from "../../ipc/types";
import { Icon } from "../../shared/ui/Icon";
import { SegmentedControl } from "../../shared/ui/SegmentedControl";
import { COLLAPSE_OVER, fileLines, findingsByLine, outsideFindings, splitRows } from "./diff";

const t = vi.flow.diff;
type Mode = "unified" | "split";
const MODE_KEY = "opw.diffMode";

function savedMode(): Mode {
  try {
    return localStorage.getItem(MODE_KEY) === "split" ? "split" : "unified";
  } catch {
    return "unified";
  }
}

/** The task's change since it left its base: file list on the left, the diff on the right,
 *  review findings right under their line. */
export function DiffTab({ detail: d }: { detail: TaskDetail }) {
  const [view, setView] = useState<DiffView>();
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string>();
  const [mode, setMode] = useState<Mode>(savedMode);
  const [open, setOpen] = useState<Record<string, boolean>>({});
  const sections = useRef(new Map<string, HTMLElement>());
  // what the first read said about "since last look" holds while the tab is open: that read
  // already saved this look (a second read, e.g. React's dev double effect, would compare with it)
  const first = useRef<{ seenBefore: boolean; changed: Set<string> }>(undefined);
  const remember = (v: DiffView): DiffView => {
    first.current ??= { seenBefore: v.seenBefore, changed: new Set() };
    const seen = first.current;
    for (const f of v.files) if (f.changed) seen.changed.add(f.path);
    const files = v.files.map((f) => ({ ...f, changed: seen.changed.has(f.path) }));
    return {
      ...v,
      files,
      seenBefore: seen.seenBefore || seen.changed.size > 0,
      changedSinceSeen: files.filter((f) => f.changed).length,
    };
  };

  const load = useCallback(() => {
    setLoading(true);
    setError(undefined);
    getTaskDiff(d.wpId, true)
      .then((v) => setView(remember(v)))
      .catch((e: unknown) => setError(errorMessage(e)))
      .finally(() => setLoading(false));
  }, [d.wpId]);
  useEffect(load, [load]);

  const pickMode = (m: Mode) => {
    setMode(m);
    try {
      localStorage.setItem(MODE_KEY, m);
    } catch {
      // private window: the choice lasts for this session only
    }
  };

  const outside = useMemo(() => (view ? outsideFindings(d.findings, view) : []), [view, d.findings]);
  const isOpen = (f: DiffFile) => open[f.path] ?? fileLines(f) <= COLLAPSE_OVER;
  const jump = (path: string) => {
    setOpen((o) => ({ ...o, [path]: true }));
    requestAnimationFrame(() => sections.current.get(path)?.scrollIntoView({ block: "start", behavior: "smooth" }));
  };

  if (!view) {
    return (
      <div className="task-pane">
        {error ? <p className="note note--error">{error}</p> : <p className="dim">{loading ? t.loading : ""}</p>}
      </div>
    );
  }

  return (
    <div className="task-pane diff">
      <div className="diff-head">
        <span>
          <strong>{t.summary(view.files.length, view.added, view.removed)}</strong>{" "}
          <span className="faint">{t.against(view.base, view.forkPoint)}</span>
          {view.seenBefore && (
            <span className={`chip${view.changedSinceSeen > 0 ? " chip--new" : ""}`}>{t.changedSince(view.changedSinceSeen)}</span>
          )}
        </span>
        <span className="diff-head-actions">
          <SegmentedControl
            options={[
              { value: "unified" as const, label: t.unified },
              { value: "split" as const, label: t.split },
            ]}
            value={mode}
            onChange={pickMode}
            label={t.viewLabel}
          />
          <button className="btn btn--ghost btn--sm" disabled={loading} onClick={load}>
            <Icon name="refresh" size={12} />
            {t.refresh}
          </button>
        </span>
      </div>
      {error && <p className="note note--error">{error}</p>}
      {view.note && <p className="note note--warn">{view.note}</p>}
      {view.truncated && <p className="note">{t.truncated}</p>}
      {outside.length > 0 && <p className="faint">{t.outside(outside.length)}</p>}
      {view.files.length === 0 && <p className="dim">{t.none}</p>}

      {view.files.length > 0 && (
        <div className="diff-layout">
          <nav className="diff-files" aria-label={t.files}>
            {view.files.map((f) => (
              <button key={f.path} className="diff-file-link" title={f.path} onClick={() => jump(f.path)}>
                <span className={`diff-dot diff-dot--${f.change}`} aria-hidden="true" />
                <span className="diff-file-name">
                  <span>{baseName(f.path)}</span>
                  {dirOf(f.path) && <span className="faint diff-file-dir">{dirOf(f.path)}</span>}
                </span>
                {f.changed && <span className="diff-changed" title={t.changedHint} />}
              </button>
            ))}
          </nav>
          <div className="diff-body">
            {view.files.map((f) => (
              <section
                key={f.path}
                className="diff-file"
                ref={(el) => {
                  if (el) sections.current.set(f.path, el);
                  else sections.current.delete(f.path);
                }}
              >
                <header className="diff-file-head">
                  <button
                    className="btn btn--ghost btn--icon btn--sm"
                    aria-expanded={isOpen(f)}
                    title={isOpen(f) ? t.collapse : t.expand}
                    onClick={() => setOpen((o) => ({ ...o, [f.path]: !isOpen(f) }))}
                  >
                    <span className={`diff-caret${isOpen(f) ? " is-open" : ""}`} aria-hidden="true">
                      ▸
                    </span>
                  </button>
                  <span className="mono diff-path" title={f.path}>
                    {f.path}
                  </span>
                  <span className={`chip diff-change diff-change--${f.change}`}>{t.change[f.change]}</span>
                  {f.untracked && <span className="chip">{t.untracked}</span>}
                  {f.changed && (
                    <span className="chip chip--new" title={t.changedHint}>
                      {t.changed}
                    </span>
                  )}
                  <span className="diff-stat mono">
                    <span className="diff-plus">+{f.added}</span> <span className="diff-minus">−{f.removed}</span>
                  </span>
                </header>
                {f.oldPath && <p className="faint diff-renamed">{t.renamed(f.oldPath)}</p>}
                {isOpen(f) && <FileBody file={f} mode={mode} findings={d.findings} />}
              </section>
            ))}
          </div>
        </div>
      )}
    </div>
  );
}

const dirOf = (p: string) => (p.includes("/") ? p.slice(0, p.lastIndexOf("/") + 1) : "");
const baseName = (p: string) => p.slice(p.lastIndexOf("/") + 1);

function FileBody({ file, mode, findings }: { file: DiffFile; mode: Mode; findings: FindingView[] }) {
  const byLine = useMemo(() => findingsByLine(findings, file), [findings, file]);
  if (file.binary) return <p className="dim diff-empty">{t.binary}</p>;
  return (
    <div className={`diff-table diff-table--${mode}`}>
      {file.hunks.map((h, hi) => (
        <Fragment key={hi}>
          <div className="diff-hunk mono">{h.section ? `@@ ${h.section}` : "@@"}</div>
          {mode === "unified"
            ? h.lines.map((l, li) => (
                <Fragment key={li}>
                  <UnifiedRow line={l} />
                  <Findings list={l.kind !== "removed" && l.new !== undefined ? byLine.get(l.new) : undefined} />
                </Fragment>
              ))
            : splitRows(h).map((r, ri) => (
                <Fragment key={ri}>
                  <div className="diff-row diff-row--split">
                    <Cell line={r.left} side="old" />
                    <Cell line={r.right} side="new" />
                  </div>
                  <Findings list={r.right?.new !== undefined ? byLine.get(r.right.new) : undefined} />
                </Fragment>
              ))}
        </Fragment>
      ))}
      {file.truncated && <p className="faint diff-empty">{t.truncated}</p>}
    </div>
  );
}

const SIGN: Record<DiffLine["kind"], string> = { context: " ", added: "+", removed: "−" };

function UnifiedRow({ line }: { line: DiffLine }) {
  return (
    <div className={`diff-row diff-line--${line.kind}`}>
      <span className="diff-no">{line.old ?? ""}</span>
      <span className="diff-no">{line.new ?? ""}</span>
      <span className="diff-code">
        <span className="diff-sign">{SIGN[line.kind]}</span>
        {line.text}
      </span>
    </div>
  );
}

function Cell({ line, side }: { line: DiffLine | undefined; side: "old" | "new" }) {
  if (!line) return <span className="diff-cell diff-cell--empty" />;
  const kind = line.kind === "context" ? "context" : line.kind;
  return (
    <span className={`diff-cell diff-line--${kind}`}>
      <span className="diff-no">{side === "old" ? line.old : line.new}</span>
      <span className="diff-code">{line.text}</span>
    </span>
  );
}

function Findings({ list }: { list: FindingView[] | undefined }) {
  if (!list?.length) return null;
  return (
    <div className="diff-findings" aria-label={t.findingAt}>
      {list.map((f) => (
        <div key={f.fingerprint} className={`diff-finding finding--${f.severity}`}>
          <span className={`sev sev--${f.severity}`}>{severityLabel[f.severity]}</span>
          <span>
            <strong>{f.title}</strong>
            {f.detail && <span className="faint"> · {f.detail}</span>}
          </span>
        </div>
      ))}
    </div>
  );
}
