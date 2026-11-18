// Tiến trình: the last result of Harness (step 5) and Test trình duyệt (step 11), above the runs.
// A harness shows the command the app runs again after every Sửa lỗi, its folder, whether the
// negative control FAILed, and the folder it was kept in (with "Bỏ khỏi thư viện").
import { stepLabel, vi } from "../../i18n/vi";
import type { FlowAction, TestCase, TestView } from "../../ipc/types";
import { formatHourMinute } from "../../shared/format";
import { Icon } from "../../shared/ui/Icon";
import { StatusIcon } from "../../shared/ui/StatusIcon";

const t = vi.flow.tests;

interface Props {
  tests: TestView[];
  busy?: string;
  onAction: (action: FlowAction) => void;
  onOpenPath: (path: string) => void;
}

/** How the last run went: every case passed, one failed, or none failed but some could not
 *  be tested (browser test). */
export function testOutcome(test: TestView): "pass" | "fail" | "partial" {
  if (test.passed) return "pass";
  const failed = test.cases.some((c) => !c.ok && !c.untested);
  return !failed && test.cases.some((c) => c.untested) ? "partial" : "fail";
}

/** "PASS 3/3", "FAIL 1/3", "NG 1/4", "OK 4/4 · 2 chưa test" (out of the cases that ran). */
export function testTally(test: TestView): string {
  const untested = test.cases.filter((c) => c.untested).length;
  const ran = test.cases.length - untested;
  const ok = test.cases.filter((c) => c.ok).length;
  const browser = test.step === "browserTest";
  const outcome = testOutcome(test);
  const word = outcome === "fail" ? (browser ? "NG" : "FAIL") : browser ? "OK" : "PASS";
  if (test.cases.length === 0) return word;
  if (ran === 0) return t.noneTested;
  const tally = `${word} ${ok}/${ran}`;
  return untested > 0 ? `${tally} · ${t.untestedCount(untested)}` : tally;
}

const lastPart = (path: string) => path.split(/[\\/]/).filter(Boolean).pop() ?? path;

export function TestsSection({ tests, busy, onAction, onOpenPath }: Props) {
  if (tests.length === 0) return null;
  return (
    <section className="tl-tests" aria-label={t.label}>
      {tests.map((test) => (
        <TestCard key={test.step} test={test} busy={busy} onAction={onAction} onOpenPath={onOpenPath} />
      ))}
    </section>
  );
}

/** What Làm tiếp → Test trình duyệt tells the AI to do: only the cases it could not run. */
export function retestNote(test: TestView): string {
  return t.retestNote(test.cases.filter((c) => c.untested).map((c) => c.name));
}

function TestCard({ test, busy, onAction, onOpenPath }: { test: TestView } & Omit<Props, "tests">) {
  const outcome = testOutcome(test);
  const untested = test.cases.some((c) => c.untested);
  const chip = { pass: "chip--done", fail: "chip--failed", partial: "chip--warn" }[outcome];
  return (
    <article className={`tl-test tl-test--${outcome}`}>
      <div className="tl-test-head">
        <StatusIcon state={outcome === "pass" ? "done" : outcome === "fail" ? "failed" : "waitingUser"} size={15} />
        <strong>{stepLabel[test.step]}</strong>
        <span className={`chip ${chip}`}>{testTally(test)}</span>
        <span className="faint tl-test-when">
          {t.runs(test.runs)} · {formatHourMinute(test.at)}
        </span>
      </div>
      {test.summary && <p className="tl-test-summary">{test.summary}</p>}
      {test.cases.length > 0 && (
        <ul className="tl-test-cases">
          {test.cases.map((c, i) => (
            <CaseRow key={`${c.name}-${i}`} c={c} onOpenPath={onOpenPath} />
          ))}
        </ul>
      )}
      {test.changes && test.changes.length > 0 && (
        <div className="tl-test-changes">
          <span className="faint">{t.changes}</span>
          <ul>
            {test.changes.map((c, i) => (
              <li key={`${i}-${c}`}>{c}</li>
            ))}
          </ul>
        </div>
      )}
      {untested && (
        <div className="tl-actions">
          <button
            className="btn btn--ghost btn--xs"
            disabled={busy !== undefined}
            title={t.retestHint}
            onClick={() => onAction({ type: "reopen", step: "browserTest", note: retestNote(test) })}
          >
            <Icon name="refresh" size={11} />
            {t.retest}
          </button>
          <span className="faint tl-actions-note">{t.retestNote2}</span>
        </div>
      )}
      {test.negative !== undefined && (
        <p className={`note${test.negative ? "" : " note--warn"}`}>
          <Icon name={test.negative ? "info" : "alert"} size={13} />
          <span>
            {test.negative ? t.negativeOk : t.negativeBad}
            {test.negativeDetail && <span className="faint"> · {test.negativeDetail}</span>}
          </span>
        </p>
      )}
      {test.command && (
        <p className="faint tl-test-command">
          {t.command} <code className="mono">{test.command}</code>
        </p>
      )}
      {test.url && (
        <p className="faint tl-test-command">
          {t.url} <code className="mono">{test.url}</code>
        </p>
      )}
      {(test.dir || test.library) && (
        <div className="tl-actions">
          {test.dir && (
            <button className="btn btn--ghost btn--xs" title={test.dir} onClick={() => onOpenPath(test.dir ?? "")}>
              <Icon name="folder" size={11} />
              {t.openDir}
            </button>
          )}
          {test.library && (
            <>
              <button className="btn btn--ghost btn--xs" title={test.library} onClick={() => onOpenPath(test.library ?? "")}>
                <Icon name="layers" size={11} />
                {t.library(lastPart(test.library))}
              </button>
              <button
                className="btn btn--ghost btn--xs"
                disabled={busy !== undefined}
                title={t.dropHint}
                onClick={() => onAction({ type: "dropHarness" })}
              >
                <Icon name="trash" size={11} />
                {t.drop}
              </button>
            </>
          )}
        </div>
      )}
    </article>
  );
}

function CaseRow({ c, onOpenPath }: { c: TestCase; onOpenPath: (path: string) => void }) {
  const tone = c.untested ? " tl-case--untested" : c.ok ? "" : " tl-case--bad";
  return (
    <li className={`tl-case${tone}`}>
      <Icon name={c.untested ? "alert" : c.ok ? "check" : "close"} size={12} />
      <span className="tl-case-text">
        {c.untested && <span className="tl-case-tag">{t.untested}</span>}
        <span>{c.name}</span>
        {c.detail && <span className="faint"> — {c.detail}</span>}
      </span>
      {c.screenshot && (
        <button className="link-btn" title={c.screenshot} onClick={() => onOpenPath(c.screenshot ?? "")}>
          {t.screenshot}
        </button>
      )}
    </li>
  );
}
