import { useState } from "react";
import { agentLabel, stepHint, stepLabel, vi } from "../../i18n/vi";
import { errorMessage, setStepChoice, setVerifyMcp, setStepLimits } from "../../ipc/client";
import type { AgentCatalog, AgentKind, SettingsView, StepChoiceRequest, StepModels, StepSetting } from "../../ipc/types";
import { formatDateTime, formatDuration, formatHourMinute, formatUsd } from "../../shared/format";
import { stepNumber } from "../../shared/steps";
import { SegmentedControl } from "../../shared/ui/SegmentedControl";
import { ModeSelect } from "./ModeSelect";
import { agentNotes, effortForModel, effortOptions, modeHint, modelEntry, modelOptions, modeOptions } from "./stepChoice";

const t = vi.settings.steps;

interface Props {
  view: SettingsView | null;
  onSaved: (view: SettingsView) => void;
  /** lists read from the CLIs on this machine; `null` until the first read ends */
  models: StepModels | null;
  modelsBusy: boolean;
  onReadModels: () => void;
}

/** Settings → Các bước AI: agent, model, effort and permission mode per step, picked from what
 *  the CLIs on the machine list, with past runs for reference. */
export function StepsPage({ view, onSaved, models, modelsBusy, onReadModels }: Props) {
  const [error, setError] = useState<string | null>(null);
  const [savedStep, setSavedStep] = useState<string | null>(null);
  const [mcpBusy, setMcpBusy] = useState(false);
  const rows = view?.steps ?? [];

  const save = (choice: StepChoiceRequest) => {
    setError(null);
    setStepChoice(choice)
      .then((v) => {
        onSaved(v);
        setSavedStep(choice.step);
      })
      .catch((e: unknown) => setError(errorMessage(e)));
  };

  return (
    <>
      <p className="set-intro">{t.intro}</p>
      <section className="set-section set-mcp">
        <label className="set-mcp-label">
          <input type="checkbox" role="switch" checked={view?.verifyMcp ?? true} disabled={!view || mcpBusy}
            onChange={(e) => {
              setError(null);
              setMcpBusy(true);
              setVerifyMcp(e.target.checked).then(onSaved).catch((e: unknown) => setError(errorMessage(e))).finally(() => setMcpBusy(false));
            }} />
          {t.verifyMcp}
        </label>
        <div className="set-mcp-hints">
          <p className="set-row-hint set-mcp-hint" aria-hidden={view?.verifyMcp === false} style={{ visibility: view?.verifyMcp === false ? "hidden" : "visible" }}>{t.mcpOn}</p>
          <p className="set-row-hint set-mcp-hint" aria-hidden={view?.verifyMcp !== false} style={{ visibility: view?.verifyMcp !== false ? "hidden" : "visible" }}>{t.mcpOff}</p>
        </div>
      </section>
      <div className="set-source" aria-live="polite">
        <span className="set-source-lines">
          <SourceLine name="Claude Code" catalog={models?.claude} />
          <SourceLine name="Codex" catalog={models?.codex} />
        </span>
        <button className="btn btn--sm" disabled={modelsBusy} onClick={onReadModels}>
          {modelsBusy && models ? t.rereading : t.reread}
        </button>
      </div>
      <p className={`set-save-status ${error ? "note note--error" : ""}`} aria-live="polite">{error ?? "\u00a0"}</p>
      {/* in a section like the other pages, so the note below sits under the box, not on its border */}
      <section className="set-section">
        <div className="set-group">
          {rows.map((row) => (
            <StepRow
              key={row.step}
              number={stepNumber(row.step)}
              row={row}
              catalog={models ? (row.agent === "codex" ? models.codex : models.claude) : undefined}
              saved={savedStep === row.step}
              onSave={save}
              onSaveLimits={(limits) => {
                setError(null);
                setStepLimits(limits).then((v) => { onSaved(v); setSavedStep(row.step); })
                  .catch((e: unknown) => setError(errorMessage(e)));
              }}
            />
          ))}
        </div>
      </section>
      <p className="set-note">{t.flags}</p>
    </>
  );
}

/** Where one agent's list came from. Always one line, so the page does not move when it loads. */
function SourceLine({ name, catalog }: { name: string; catalog: AgentCatalog | undefined }) {
  if (!catalog) return <span className="set-source-line faint">{t.reading(name)}</span>;
  if (!catalog.fromCli) {
    // the last list this CLI gave (saved by the core), else nothing but "Theo máy"
    const text = catalog.readAt
      ? t.lastList(name, catalog.error ?? "", formatDateTime(catalog.readAt))
      : t.fallback(name, catalog.error ?? "");
    return <span className="set-source-line set-source-line--warn">{text}</span>;
  }
  const time = catalog.readAt ? formatHourMinute(catalog.readAt) : undefined;
  return <span className="set-source-line">{t.source(name, catalog.cliVersion, catalog.models.length, time)}</span>;
}

function StepRow({ number, row, catalog, saved, onSave, onSaveLimits }: {
  /** the step's real place in the flow: "8", or "7 · 9" for both Sửa lỗi */
  number: string;
  row: StepSetting;
  catalog: AgentCatalog | undefined;
  saved: boolean;
  onSave: (c: StepChoiceRequest) => void;
  onSaveLimits: (c: { step: StepSetting["step"]; maxBudgetUsd?: number; timeoutMin?: number }) => void;
}) {
  const [resetNote, setResetNote] = useState<string | null>(null);
  const agent: AgentKind = row.agent;
  const send = (patch: Partial<StepChoiceRequest>) =>
    onSave({
      step: row.step,
      agent: row.agentChoice ? agent : undefined,
      model: row.model,
      effort: row.effort,
      permissionMode: row.permissionMode,
      ...patch,
    });
  /** A new model keeps the effort only if that model takes it. */
  const pickModel = (model: string | undefined) => {
    const effort = effortForModel(catalog, row.effort, model);
    const entry = modelEntry(catalog, model);
    setResetNote(row.effort && !effort ? t.effortReset(row.effort, entry?.label ?? model ?? t.machine) : null);
    send({ model, effort });
  };
  const models = modelOptions(row, catalog);
  const efforts = effortOptions(row, catalog);
  const modes = modeOptions(row, catalog);
  const entry = modelEntry(catalog, row.model);
  const loading = catalog === undefined;
  const about = entry?.description ? `${entry.label}: ${entry.description}.` : undefined;
  const history = t.history(
    row.runs,
    row.avgCostUsd !== undefined ? formatUsd(row.avgCostUsd) : undefined,
    row.avgSeconds !== undefined ? formatDuration(row.avgSeconds * 1000) : undefined,
  );

  return (
    <div className="set-row set-row--stacked set-step" data-setting-row={`step:${row.step}`}>
      <div className="set-row-text">
        <span className="set-row-label" title={stepHint[row.step]}>
          {number}. {stepLabel[row.step]}
        </span>
        <span className="set-row-hint">
          {history}
          {saved && <span className="set-ok"> · {t.saved}</span>}
        </span>
      </div>
      <div className="set-step-controls">
        <label className="set-step-field">
          <span className="faint">{t.agent}</span>
          {row.agentChoice ? (
            <SegmentedControl
              options={(["claude", "codex"] as const).map((a) => ({ value: a, label: agentLabel[a] }))}
              value={agent === "codex" ? "codex" : "claude"}
              onChange={(a) => {
                setResetNote(null);
                onSave({ step: row.step, agent: a });
              }}
              label={`${t.agent} · ${stepLabel[row.step]}`}
            />
          ) : (
            <span className={`chip chip--${agent}`}>{agentLabel[agent]}</span>
          )}
          <span className="set-row-hint set-choice-hint">{t.agentHint}</span>
        </label>
        <label className="set-step-field set-step-field--grow">
          <span className="faint">{t.model}</span>
          <select
            className="input"
            disabled={loading}
            title={loading ? t.waitList : undefined}
            value={row.model ?? ""}
            onChange={(e) => pickModel(e.target.value || undefined)}
          >
            {models.map((o) => (
              <option key={o.value} value={o.value}>
                {o.label}
              </option>
            ))}
          </select>
          <span className="set-row-hint set-choice-hint">{t.modelHint}</span>
        </label>
        <label className="set-step-field">
          <span className="faint">{t.effort}</span>
          <select
            className="input"
            disabled={loading || efforts.length === 1}
            title={loading ? t.waitList : undefined}
            value={row.effort ?? ""}
            onChange={(e) => {
              setResetNote(null);
              send({ effort: e.target.value || undefined });
            }}
          >
            {efforts.map((o) => (
              <option key={o.value} value={o.value}>
                {o.label}
              </option>
            ))}
          </select>
          <span className="set-row-hint set-choice-hint">{t.effortHint}</span>
        </label>
      </div>
      {/* its own row in every step, so all rows look alike whatever the mode's name */}
      <div className="set-step-controls set-step-mode">
        <div className="set-step-field set-step-field--mode">
          <span className="faint">{t.permission}</span>
          <ModeSelect
            value={row.permissionMode ?? ""}
            options={modes}
            disabled={loading}
            title={loading ? t.waitList : undefined}
            label={`${t.permission} · ${stepLabel[row.step]}`}
            onChange={(mode) => {
              setResetNote(null);
              send({ permissionMode: mode });
            }}
          />
        </div>
        <span className="set-row-hint set-step-mode-hint">{modeHint(row)}</span>
      </div>
      <StepLimits key={`${row.maxBudgetUsd}:${row.timeoutMin}`} row={row} onSave={onSaveLimits} />
      <span className="set-row-hint set-warn set-reset-note">{resetNote ?? "\u00a0"}</span>
      <span className="set-row-hint set-step-about">{[about, ...agentNotes(row)].filter(Boolean).join(" ")}</span>
    </div>
  );
}

function StepLimits({ row, onSave }: { row: StepSetting; onSave: (c: { step: StepSetting["step"]; maxBudgetUsd?: number; timeoutMin?: number }) => void }) {
  const [budget, setBudget] = useState(row.maxBudgetUsd?.toString() ?? "");
  const [minutes, setMinutes] = useState(row.timeoutMin?.toString() ?? "");
  return (
    <form className="set-step-controls set-step-limits" onSubmit={(e) => {
      e.preventDefault();
      onSave({ step: row.step, maxBudgetUsd: budget ? Number(budget) : undefined, timeoutMin: minutes ? Number(minutes) : undefined });
    }}>
      <label className="set-step-field">
        <span className="faint">{t.budget}</span>
        <input className="input" type="number" min="0.01" step="0.01" value={budget} placeholder={String(row.defaultBudgetUsd ?? "")}
          onChange={(e) => setBudget(e.target.value)} />
        <span className="set-row-hint set-choice-hint">{t.budgetHint}</span>
      </label>
      <label className="set-step-field">
        <span className="faint">{t.timeout}</span>
        <input className="input" type="number" min="1" max="4294967295" step="1" value={minutes} placeholder={String(row.defaultTimeoutMin ?? "")}
          onChange={(e) => setMinutes(e.target.value)} />
        <span className="set-row-hint set-choice-hint">{t.timeoutHint}</span>
      </label>
      <button className="btn btn--sm" type="submit">{t.saveLimits}</button>
    </form>
  );
}
