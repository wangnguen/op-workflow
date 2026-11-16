// Options of Settings → Các bước AI (pure, easy to test). Only what the CLIs on the machine list
// (`step_models`) can be picked; while the lists load, a select only shows what is picked now.
import { vi } from "../../i18n/vi";
import type { AgentCatalog, AgentModel, StepSetting } from "../../ipc/types";
import { modelName } from "../../shared/models";

const t = vi.settings.steps;

export interface Option {
  value: string;
  label: string;
}

/** "Theo máy" with what it turns out to be: the CLI's configured default, else the last run. */
function machineModel(row: StepSetting, catalog: AgentCatalog | undefined): string {
  if (catalog?.defaultLabel) return t.machineIs(catalog.defaultLabel);
  if (row.lastModel && !row.model) return t.machineWas(modelName(row.lastModel));
  return t.machine;
}

/** Model list: "Theo máy" first, then what the CLI lists, and the saved pick if the CLI no
 *  longer lists it (so the select can show it; it cannot be picked again once left).
 *  Without a catalog (still loading): the current pick only. */
export function modelOptions(row: StepSetting, catalog: AgentCatalog | undefined): Option[] {
  const out: Option[] = [{ value: "", label: machineModel(row, catalog) }];
  const listed = (catalog?.models ?? []).map((m) => ({ value: m.id, label: m.label }));
  out.push(...listed);
  if (row.model && !listed.some((o) => o.value === row.model)) {
    out.push({ value: row.model, label: catalog ? t.notListed(row.model) : row.model });
  }
  return out;
}

/** The catalog entry a step runs with: the picked model, else the CLI's default. */
export function modelEntry(catalog: AgentCatalog | undefined, model: string | undefined): AgentModel | undefined {
  const id = model ?? catalog?.defaultId;
  return id ? catalog?.models.find((m) => m.id === id) : undefined;
}

/** Effort levels for `model` (undefined = the machine's default model). A model the list does
 *  not know (a saved one it no longer lists, an unknown default) gets every level some model
 *  takes. */
export function effortLevels(catalog: AgentCatalog | undefined, model: string | undefined): string[] {
  if (!catalog) return [];
  const entry = modelEntry(catalog, model);
  if (entry) return entry.efforts;
  const all: string[] = [];
  for (const m of catalog.models) for (const e of m.efforts) if (!all.includes(e)) all.push(e);
  return all;
}

/** Effort list: "Theo máy" (with the CLI's default, or the app's own default) first, then the
 *  levels the step's model takes. A model without effort gets one disabled-looking entry. */
export function effortOptions(row: StepSetting, catalog: AgentCatalog | undefined): Option[] {
  const levels = effortLevels(catalog, row.model);
  const takesNone = catalog !== undefined && levels.length === 0 && modelEntry(catalog, row.model) !== undefined;
  const first = takesNone
    ? t.noEffort
    : row.effortDefault
      ? t.appDefault(row.effortDefault)
      : catalog?.defaultEffort
        ? t.machineIs(catalog.defaultEffort)
        : t.machine;
  const out: Option[] = [{ value: "", label: first }, ...levels.map((e) => ({ value: e, label: e }))];
  if (row.effort && !levels.includes(row.effort)) {
    out.push({ value: row.effort, label: catalog ? t.effortNotListed(row.effort) : row.effort });
  }
  return out;
}

/** The effort to keep when the model changes: dropped when the new model does not take it. */
export function effortForModel(catalog: AgentCatalog | undefined, effort: string | undefined, model: string | undefined): string | undefined {
  if (!effort || !catalog) return effort;
  return effortLevels(catalog, model).includes(effort) ? effort : undefined;
}
