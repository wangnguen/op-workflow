import { useState } from "react";
import { agentLabel, stepHint, stepLabel, vi } from "../../i18n/vi";
import { errorMessage, setStepChoice } from "../../ipc/client";
import type { AgentKind, SettingsView, StepChoiceRequest, StepSetting } from "../../ipc/types";
import { formatDuration, formatUsd } from "../../shared/format";
import { claudeModel, priceText } from "../../shared/models";
import { SegmentedControl } from "../../shared/ui/SegmentedControl";
import { CUSTOM, effortOptions, modelOptions } from "./stepChoice";

const t = vi.settings.steps;

interface Props {
  view: SettingsView | null;
  onSaved: (view: SettingsView) => void;
}

/** Settings → Các bước AI: model and effort per step, with prices and past runs for reference. */
export function StepsPage({ view, onSaved }: Props) {
  const [error, setError] = useState<string | null>(null);
  const [savedStep, setSavedStep] = useState<string | null>(null);
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
      {error && <p className="note note--error">{error}</p>}
      <div className="set-group">
        {rows.map((row, i) => (
          <StepRow
            key={row.step}
            index={i + 2}
            row={row}
            codexModels={view?.codexModels ?? []}
            saved={savedStep === row.step}
            onSave={save}
          />
        ))}
      </div>
      <p className="set-note">{t.flags}</p>
    </>
  );
}

function StepRow({ index, row, codexModels, saved, onSave }: {
  index: number;
  row: StepSetting;
  codexModels: readonly string[];
  saved: boolean;
  onSave: (c: StepChoiceRequest) => void;
}) {
  const [custom, setCustom] = useState<string | null>(null);
  const agent: AgentKind = row.agent;
  const send = (patch: Partial<StepChoiceRequest>) =>
    onSave({
      step: row.step,
      agent: row.agentChoice ? agent : undefined,
      model: row.model,
      effort: row.effort,
      ...patch,
    });
  const models = modelOptions(row, agent, codexModels);
  const known = claudeModel(row.model ?? row.lastModel);
  const price = agent === "codex" ? t.priceUnknown : known ? t.price(known.label, priceText(known)) : undefined;
  const history = t.history(
    row.runs,
    row.avgCostUsd !== undefined ? formatUsd(row.avgCostUsd) : undefined,
    row.avgSeconds !== undefined ? formatDuration(row.avgSeconds * 1000) : undefined,
  );

  return (
    <div className="set-row set-row--stacked set-step" data-setting-row={`step:${row.step}`}>
      <div className="set-row-text">
        <span className="set-row-label" title={stepHint[row.step]}>
          {index}. {stepLabel[row.step]}
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
              options={(["codex", "claude"] as const).map((a) => ({ value: a, label: agentLabel[a] }))}
              value={agent === "codex" ? "codex" : "claude"}
              onChange={(a) => onSave({ step: row.step, agent: a })}
              label={`${t.agent} · ${stepLabel[row.step]}`}
            />
          ) : (
            <span className={`chip chip--${agent}`}>{agentLabel[agent]}</span>
          )}
        </label>
        <label className="set-step-field set-step-field--grow">
          <span className="faint">{t.model}</span>
          <select
            className="input"
            value={custom !== null ? CUSTOM : (row.model ?? "")}
            onChange={(e) => {
              const v = e.target.value;
              if (v === CUSTOM) {
                setCustom(row.model ?? "");
                return;
              }
              setCustom(null);
              send({ model: v || undefined });
            }}
          >
            {models.map((o) => (
              <option key={o.value} value={o.value}>
                {o.label}
              </option>
            ))}
          </select>
        </label>
        <label className="set-step-field">
          <span className="faint">{t.effort}</span>
          <select className="input" value={row.effort ?? ""} onChange={(e) => send({ effort: e.target.value || undefined })}>
            {effortOptions(row, agent).map((o) => (
              <option key={o.value} value={o.value}>
                {o.label}
              </option>
            ))}
          </select>
        </label>
      </div>
      {custom !== null && (
        <span className="set-inline">
          <input
            className="mono"
            autoFocus
            autoComplete="off"
            spellCheck={false}
            placeholder={t.customPlaceholder}
            value={custom}
            onChange={(e) => setCustom(e.target.value)}
            onKeyDown={(e) => {
              if (e.key === "Enter" && custom.trim()) {
                send({ model: custom.trim() });
                setCustom(null);
              } else if (e.key === "Escape") {
                e.stopPropagation();
                setCustom(null);
              }
            }}
          />
          <button
            className="btn btn--sm btn--primary"
            disabled={!custom.trim()}
            onClick={() => {
              send({ model: custom.trim() });
              setCustom(null);
            }}
          >
            {t.customSave}
          </button>
        </span>
      )}
      <span className="set-row-hint">
        {[price, agent === "codex" ? t.codexNote : undefined, row.agentChoice ? t.agentNote : undefined]
          .filter(Boolean)
          .join(" ")}
      </span>
    </div>
  );
}
