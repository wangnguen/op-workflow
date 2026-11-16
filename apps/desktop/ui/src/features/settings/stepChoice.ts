// Options of Settings → Các bước AI (pure, easy to test).
import { vi } from "../../i18n/vi";
import type { AgentKind, StepSetting } from "../../ipc/types";
import { CLAUDE_MODELS, effortsFor, modelName, priceText } from "../../shared/models";

const t = vi.settings.steps;

/** Value of the "type another name" entry in the model list. */
export const CUSTOM = "__custom";

export interface Option {
  value: string;
  label: string;
}

/** Model list: "theo máy" first, then the agent's known models, the current pick if it is
 *  not one of them, then "Gõ tên khác…". */
export function modelOptions(row: StepSetting, agent: AgentKind, codexModels: readonly string[]): Option[] {
  const machine = row.lastModel && !row.model ? t.machineWas(modelName(row.lastModel)) : t.machine;
  const known: Option[] =
    agent === "codex"
      ? codexModels.map((m, i) => ({ value: m, label: i === 0 ? `${m} (mặc định của Codex)` : m }))
      : CLAUDE_MODELS.map((m) => ({ value: m.id, label: `${m.label} · ${priceText(m)}` }));
  const out: Option[] = [{ value: "", label: machine }, ...known];
  if (row.model && !known.some((o) => o.value === row.model)) out.push({ value: row.model, label: row.model });
  out.push({ value: CUSTOM, label: t.custom });
  return out;
}

/** Effort list: "theo máy" (or the app's own default) first, then the agent's levels. */
export function effortOptions(row: StepSetting, agent: AgentKind): Option[] {
  const first = row.effortDefault && agent === row.agent ? t.appDefault(row.effortDefault) : t.machine;
  return [{ value: "", label: first }, ...effortsFor(agent).map((e) => ({ value: e, label: e }))];
}
