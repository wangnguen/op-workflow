import { useState } from "react";
import { vi } from "../../i18n/vi";
import type { AskAnswer, AskQuestion, SessionAsk } from "../../ipc/types";
import { Markdown } from "../../shared/Markdown";
import { Icon } from "../../shared/ui/Icon";

const t = vi.workspace.ask;

interface Props {
  ask: SessionAsk;
  /** an answer is being sent */
  busy: boolean;
  onAnswer: (answer: AskAnswer) => void;
}

/** A button with what it does written under it (no hover needed). */
export function Choice({
  label,
  about,
  tone,
  disabled,
  onClick,
}: {
  label: string;
  about: string;
  tone?: "primary" | "danger";
  disabled: boolean;
  onClick: () => void;
}) {
  return (
    <button className={`ask-choice${tone ? ` ask-choice--${tone}` : ""}`} disabled={disabled} onClick={onClick}>
      <strong>{label}</strong>
      {about && <small>{about}</small>}
    </button>
  );
}

/** What a session waits for you to answer, inside its chat. */
export function AskCard({ ask, busy, onAnswer }: Props) {
  switch (ask.kind) {
    case "permission":
      return <PermissionAsk ask={ask} busy={busy} onAnswer={onAnswer} />;
    case "question":
      return <QuestionAsk ask={ask} busy={busy} onAnswer={onAnswer} />;
    case "plan":
      return <PlanAsk ask={ask} busy={busy} onAnswer={onAnswer} />;
    case "trust":
      return (
        <section className="ask">
          <header className="ask-head">
            <Icon name="folder" size={14} />
            <strong>{t.trustTitle}</strong>
          </header>
          <p className="ask-text">{t.trustBody(ask.summary)}</p>
          <div className="ask-choices">
            <Choice
              label={t.trust}
              about={t.trustAbout}
              tone="primary"
              disabled={busy}
              onClick={() => onAnswer({ kind: "trust", trust: true })}
            />
            <Choice
              label={t.noTrust}
              about={t.noTrustAbout}
              disabled={busy}
              onClick={() => onAnswer({ kind: "trust", trust: false })}
            />
          </div>
        </section>
      );
  }
}

function PermissionAsk({ ask, busy, onAnswer }: Props) {
  const [why, setWhy] = useState("");
  return (
    <section className="ask">
      <header className="ask-head">
        <Icon name="alert" size={14} />
        <strong>{t.permissionTitle(ask.tool)}</strong>
      </header>
      {ask.summary && <pre className="ask-code">{ask.summary}</pre>}
      {ask.detail && <p className="ask-text dim">{ask.detail}</p>}
      <div className="ask-choices">
        <Choice
          label={t.allow}
          about={t.allowAbout}
          tone="primary"
          disabled={busy}
          onClick={() => onAnswer({ kind: "allow" })}
        />
        {ask.always && (
          <Choice
            label={t.always}
            about={t.alwaysAbout(ask.always)}
            disabled={busy}
            onClick={() => onAnswer({ kind: "allow", always: true })}
          />
        )}
        <Choice
          label={why.trim() ? t.denySend : t.deny}
          about={why.trim() ? t.denySendAbout : t.denyAbout}
          tone="danger"
          disabled={busy}
          onClick={() => onAnswer({ kind: "deny", message: why.trim() || undefined })}
        />
      </div>
      <label className="ask-field">
        <span className="faint">{t.denyWhy}</span>
        <input
          className="input"
          value={why}
          placeholder={t.denyWhyPlaceholder}
          disabled={busy}
          onChange={(e) => setWhy(e.target.value)}
        />
      </label>
    </section>
  );
}

function QuestionAsk({ ask, busy, onAnswer }: Props) {
  const questions = ask.questions ?? [];
  // question → picked labels; "Khác" text kept apart
  const [picked, setPicked] = useState<Record<string, string[]>>({});
  const [other, setOther] = useState<Record<string, string>>({});
  const answerOf = (q: AskQuestion) => {
    const labels = (picked[q.question] ?? []).filter((l) => l !== t.other);
    const own = (other[q.question] ?? "").trim();
    if ((picked[q.question] ?? []).includes(t.other) && own) labels.push(own);
    return labels.join(", ");
  };
  const ready = questions.length > 0 && questions.every((q) => answerOf(q) !== "");
  const toggle = (q: AskQuestion, label: string) =>
    setPicked((p) => {
      const now = p[q.question] ?? [];
      const next = q.multiSelect
        ? now.includes(label)
          ? now.filter((l) => l !== label)
          : [...now, label]
        : [label];
      return { ...p, [q.question]: next };
    });
  return (
    <section className="ask">
      <header className="ask-head">
        <Icon name="info" size={14} />
        <strong>{t.questionTitle}</strong>
      </header>
      {questions.map((q) => {
        const now = picked[q.question] ?? [];
        return (
          <fieldset key={q.question} className="ask-question" disabled={busy}>
            <legend>
              {q.header && <span className="chip">{q.header}</span>}
              <span>{q.question}</span>
            </legend>
            {[...q.options, { label: t.other, description: "" }].map((o) => (
              <label key={o.label} className={`flow-choice${now.includes(o.label) ? " is-picked" : ""}`}>
                <input
                  type={q.multiSelect ? "checkbox" : "radio"}
                  name={q.question}
                  checked={now.includes(o.label)}
                  onChange={() => toggle(q, o.label)}
                />
                <span className="flow-choice-text">
                  <strong>{o.label}</strong>
                  {o.description && <small className="faint">{o.description}</small>}
                </span>
              </label>
            ))}
            {now.includes(t.other) && (
              <input
                className="input"
                autoFocus
                value={other[q.question] ?? ""}
                placeholder={t.otherPlaceholder}
                onChange={(e) => setOther((o) => ({ ...o, [q.question]: e.target.value }))}
              />
            )}
          </fieldset>
        );
      })}
      <div className="ask-choices">
        <Choice
          label={t.answer}
          about=""
          tone="primary"
          disabled={busy || !ready}
          onClick={() => onAnswer({ kind: "answers", answers: Object.fromEntries(questions.map((q) => [q.question, answerOf(q)])) })}
        />
        <Choice label={t.skip} about={t.skipAbout} disabled={busy} onClick={() => onAnswer({ kind: "deny" })} />
      </div>
    </section>
  );
}

function PlanAsk({ ask, busy, onAnswer }: Props) {
  const [feedback, setFeedback] = useState("");
  return (
    <section className="ask ask--plan">
      <header className="ask-head">
        <Icon name="file" size={14} />
        <strong>{t.planTitle}</strong>
      </header>
      {ask.plan && (
        <div className="ask-plan">
          <Markdown text={ask.plan} className="task-md" />
        </div>
      )}
      <div className="ask-choices">
        <Choice
          label={t.approveAuto}
          about={t.approveAutoAbout}
          tone="primary"
          disabled={busy}
          onClick={() => onAnswer({ kind: "approvePlan", mode: "auto" })}
        />
        <Choice
          label={t.approveEdits}
          about={t.approveEditsAbout}
          disabled={busy}
          onClick={() => onAnswer({ kind: "approvePlan", mode: "acceptEdits" })}
        />
        <Choice
          label={t.approveManual}
          about={t.approveManualAbout}
          disabled={busy}
          onClick={() => onAnswer({ kind: "approvePlan", mode: "default" })}
        />
      </div>
      <label className="ask-field">
        <span className="faint">{t.feedback}</span>
        <textarea
          className="input"
          rows={2}
          value={feedback}
          placeholder={t.feedbackPlaceholder}
          disabled={busy}
          onChange={(e) => setFeedback(e.target.value)}
        />
      </label>
      <div className="ask-choices">
        <Choice
          label={t.reject}
          about={t.rejectAbout}
          disabled={busy}
          onClick={() => onAnswer({ kind: "rejectPlan", feedback: feedback.trim() || undefined })}
        />
      </div>
    </section>
  );
}
