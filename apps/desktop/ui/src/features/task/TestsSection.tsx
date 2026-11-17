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

/** "PASS 3/3", "FAIL 1/3", "NG 1/4". */
export function testTally(test: TestView): string {
  const ok = test.cases.filter((c) => c.ok).length;
  const word = test.passed ? (test.step === "browserTest" ? "OK" : "PASS") : test.step === "browserTest" ? "NG" : "FAIL";
  return test.cases.length > 0 ? `${word} ${ok}/${test.cases.length}` : word;
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

function TestCard({ test, busy, onAction, onOpenPath }: { test: TestView } & Omit<Props, "tests">) {
  return (
    <article className={`tl-test tl-test--${test.passed ? "pass" : "fail"}`}>
      <div className="tl-test-head">
        <StatusIcon state={test.passed ? "done" : "failed"} size={15} />
        <strong>{stepLabel[test.step]}</strong>
        <span className={`chip ${test.passed ? "chip--done" : "chip--failed"}`}>{testTally(test)}</span>
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
  return (
    <li className={`tl-case${c.ok ? "" : " tl-case--bad"}`}>
      <Icon name={c.ok ? "check" : "close"} size={12} />
      <span className="tl-case-text">
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
