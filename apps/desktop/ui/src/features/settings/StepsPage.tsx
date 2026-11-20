import { useCallback, useEffect, useId, useState } from "react";
import { agentLabel, stepHint, stepLabel, vi } from "../../i18n/vi";
import {
  errorMessage,
  getAiClis,
  installCli,
  isMockMode,
  openLogin,
  setStepChoice,
  setVerifyMcp,
  setStepLimits,
} from "../../ipc/client";
import type {
  AgentCatalog,
  AgentKind,
  AiCli,
  Provider,
  SettingsView,
  StepChoiceRequest,
  StepModels,
  StepSetting,
} from "../../ipc/types";
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
  const [clis, setClis] = useState<AiCli[]>();
  const [cliNote, setCliNote] = useState<string | null>(null);
  const rows = view?.steps ?? [];
  const readClis = useCallback(() => {
    getAiClis()
      .then(setClis)
      .catch((e: unknown) => setError(errorMessage(e)));
  }, []);
  useEffect(readClis, [readClis]);
  const verify = view?.verifyMcp ?? true;

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
      <section className="set-section">
        <h4 className="set-section-title">{t.clis}</h4>
        <div className="set-group">
          {(["claude", "codex"] as const).map((p) => (
            <CliRow
              key={p}
              provider={p}
              cli={clis?.find((c) => c.provider === p)}
              catalog={models?.[p]}
              onStarted={(note) => {
                setError(null);
                setCliNote(note);
              }}
              onError={(e) => {
                setCliNote(null);
                setError(e);
                readClis();
              }}
            />
          ))}
          <div className="set-row set-row--actions">
            <span className="set-row-hint" aria-live="polite">{cliNote ?? "\u00a0"}</span>
            <span className="set-actions">
              <button
                className="btn btn--sm"
                disabled={modelsBusy}
                onClick={() => {
                  setCliNote(null);
                  readClis();
                  onReadModels();
                }}
              >
                {modelsBusy && models ? t.rereading : t.reread}
              </button>
            </span>
          </div>
        </div>
      </section>
      <section className="set-section">
        <div className="set-group">
          <div className="set-row" data-setting-row="verifyMcp">
            <div className="set-row-text">
              <span className="set-row-label">{t.verifyMcp}</span>
              {/* both lines hold their place, so switching moves nothing */}
              <span className="set-row-hint set-swap">
                <span aria-hidden={!verify} className={verify ? undefined : "is-off"}>{t.mcpOn}</span>
                <span aria-hidden={verify} className={verify ? "is-off" : undefined}>{t.mcpOff}</span>
              </span>
            </div>
            <div className="set-row-control">
              <input
                type="checkbox"
                className="set-check"
                aria-label={t.verifyMcp}
                checked={verify}
                disabled={!view || mcpBusy}
                onChange={(e) => {
                  setError(null);
                  setMcpBusy(true);
                  setVerifyMcp(e.target.checked)
                    .then(onSaved)
                    .catch((x: unknown) => setError(errorMessage(x)))
                    .finally(() => setMcpBusy(false));
                }}
              />
            </div>
          </div>
        </div>
      </section>
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
    </>
  );
}

/** Claude Code or Codex on this machine: installed, version, signed in, the models it lists,
 *  with Cài / Đăng nhập right there. */
function CliRow({ provider, cli, catalog, onStarted, onError }: {
  provider: Provider;
  /** `undefined` while the app looks */
  cli: AiCli | undefined;
  catalog: AgentCatalog | undefined;
  onStarted: (note: string) => void;
  onError: (e: string) => void;
}) {
  const name = t.cliName[provider];
  const installed = cli?.path !== undefined;
  const models = !catalog
    ? t.reading(name)
    : catalog.fromCli
      ? t.cliModels(catalog.models.length, catalog.readAt ? formatHourMinute(catalog.readAt) : undefined)
      : catalog.readAt
        ? t.lastList(name, catalog.error ?? "", formatDateTime(catalog.readAt))
        : t.fallback(name, catalog.error ?? "");
  const hint = !cli
    ? t.cliChecking
    : !installed
      ? t.cliMissing
      : [
          cli.version ?? name,
          cli.signedIn === true ? t.cliSignedIn : cli.signedIn === false ? t.cliSignedOut : undefined,
          models,
        ]
          .filter(Boolean)
          .join(" · ");
  const warn = !!cli && (!installed || cli.signedIn === false || (catalog !== undefined && !catalog.fromCli));
  const install = () => installCli(provider).then(() => onStarted(t.installStarted), (e: unknown) => onError(errorMessage(e)));
  const login = () => openLogin(provider).then(() => onStarted(t.loginStarted), (e: unknown) => onError(errorMessage(e)));
  return (
    <div className="set-row" data-setting-row={`cli:${provider}`}>
      <div className="set-row-text">
        <span className="set-row-label">{name}</span>
        <span className={`set-row-hint${warn ? " set-warn" : ""}`}>{hint}</span>
      </div>
      <div className="set-row-control">
        {cli && !installed && (
          <button
            className="btn btn--sm btn--primary"
            disabled={isMockMode()}
            title={cli.installWith ? t.installHint(cli.installWith) : t.installPageHint}
            onClick={() => void install()}
          >
            {cli.installWith ? t.install : t.installPage}
          </button>
        )}
        {cli && installed && (
          <button
            className={`btn btn--sm${cli.signedIn === false ? " btn--primary" : " btn--ghost"}`}
            disabled={isMockMode()}
            title={t.loginHint}
            onClick={() => void login()}
          >
            {cli.signedIn === false ? t.login : t.relogin}
          </button>
        )}
      </div>
    </div>
  );
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
  const notes = agentNotes(row).join(" ");
  // room for the notes kept whenever switching the agent can bring one, so nothing below moves
  const mayNote = notes !== "" || (row.agentChoice && agentNotes({ ...row, agent: "codex" }).length > 0);
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
      {/* two columns, every hint under its own control: Agent | Model, Effort | Quyền, limits */}
      <div className="set-step-grid">
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
          <span className="set-row-hint set-step-hint">{t.agentHint}</span>
        </label>
        <label className="set-step-field">
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
          <span className="set-row-hint set-step-hint set-step-hint--long">{about ? `${about} ${t.modelHint}` : t.modelHint}</span>
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
          {/* a model without the old effort says so here, in the hint's place */}
          <span className={`set-row-hint set-step-hint${resetNote ? " set-warn" : ""}`}>{resetNote ?? t.effortHint}</span>
        </label>
        <div className="set-step-field">
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
          <span className="set-row-hint set-step-hint set-step-hint--long">{modeHint(row)}</span>
        </div>
        <StepLimits key={`${row.maxBudgetUsd}:${row.timeoutMin}`} row={row} onSave={onSaveLimits} />
      </div>
      {mayNote && <span className="set-row-hint set-step-notes">{notes || "\u00a0"}</span>}
    </div>
  );
}

function StepLimits({ row, onSave }: { row: StepSetting; onSave: (c: { step: StepSetting["step"]; maxBudgetUsd?: number; timeoutMin?: number }) => void }) {
  const [budget, setBudget] = useState(row.maxBudgetUsd?.toString() ?? "");
  const [minutes, setMinutes] = useState(row.timeoutMin?.toString() ?? "");
  const minutesId = useId();
  // the form's two cells sit in the step's grid like the controls above
  return (
    <form className="set-step-limits" onSubmit={(e) => {
      e.preventDefault();
      onSave({ step: row.step, maxBudgetUsd: budget ? Number(budget) : undefined, timeoutMin: minutes ? Number(minutes) : undefined });
    }}>
      <label className="set-step-field">
        <span className="faint">{t.budget}</span>
        <input className="input" type="number" min="0.01" step="0.01" value={budget} placeholder={String(row.defaultBudgetUsd ?? "")}
          onChange={(e) => setBudget(e.target.value)} />
        <span className="set-row-hint set-step-hint">{t.budgetHint}</span>
      </label>
      <div className="set-step-field">
        <label className="faint" htmlFor={minutesId}>{t.timeout}</label>
        <div className="set-step-inline">
          <input id={minutesId} className="input" type="number" min="1" max="4294967295" step="1" value={minutes} placeholder={String(row.defaultTimeoutMin ?? "")}
            onChange={(e) => setMinutes(e.target.value)} />
          <button className="btn btn--sm" type="submit">{t.saveLimits}</button>
        </div>
        <span className="set-row-hint set-step-hint">{t.timeoutHint}</span>
      </div>
    </form>
  );
}
