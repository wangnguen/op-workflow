// Models and effort levels the user can pick per step (Settings → Các bước AI). Reference only:
// the app never picks one by itself; "theo máy" passes no flag.
import type { AgentKind } from "../ipc/types";

export interface ModelInfo {
  id: string;
  label: string;
  /** USD per 1M input / output tokens */
  inputUsd: number;
  outputUsd: number;
}

/** Claude models with their list prices (PLAN.md §12). */
export const CLAUDE_MODELS: readonly ModelInfo[] = [
  { id: "claude-opus-5-5", label: "Opus 5.5", inputUsd: 4, outputUsd: 20 },
  { id: "claude-sonnet-5-5", label: "Sonnet 5.5", inputUsd: 2, outputUsd: 10 },
  { id: "claude-haiku-5-5", label: "Haiku 5.5", inputUsd: 0.1, outputUsd: 0.5 },
  { id: "claude-fable-5-2", label: "Fable 5.2", inputUsd: 8, outputUsd: 40 },
];

/** `claude --effort` values; Codex `model_reasoning_effort` values. */
export const EFFORTS: Record<"claude" | "codex", readonly string[]> = {
  claude: ["low", "medium", "high", "xhigh", "max"],
  codex: ["minimal", "low", "medium", "high", "xhigh"],
};

export const effortsFor = (agent: AgentKind): readonly string[] => (agent === "codex" ? EFFORTS.codex : EFFORTS.claude);

export const claudeModel = (id: string | undefined): ModelInfo | undefined => CLAUDE_MODELS.find((m) => m.id === id);

/** "claude-opus-5-5" → "Opus 5.5"; other names stay as they are. */
export function modelName(id: string): string {
  const known = claudeModel(id);
  if (known) return known.label;
  // dated ids such as claude-opus-5-5-20261101
  const m = /^claude-([a-z]+)-(\d+)-(\d+)/.exec(id);
  if (m && m[1] && m[2] && m[3]) return `${m[1].charAt(0).toUpperCase()}${m[1].slice(1)} ${m[2]}.${m[3]}`;
  return id;
}

/** "Opus 5.5 · high"; empty when nothing is known. */
export function modelText(model?: string, effort?: string): string {
  return [model ? modelName(model) : undefined, effort].filter(Boolean).join(" · ");
}

/** "$4 / $20" per 1M tokens in / out. */
export function priceText(m: ModelInfo): string {
  const usd = (n: number) => `$${n < 1 ? n.toFixed(2) : n}`;
  return `${usd(m.inputUsd)} / ${usd(m.outputUsd)}`;
}
