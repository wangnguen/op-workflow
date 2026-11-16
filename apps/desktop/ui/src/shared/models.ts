// Short model names for the step bar and Settings. Which models exist comes from the CLIs on
// the machine (`step_models`); no model name or price is written here.

/** "claude-opus-5-5" → "Opus 5.5", "claude-haiku-4-5-20251001" → "Haiku 4.5",
 *  "claude-sonnet-5" → "Sonnet 5"; other names stay as they are. */
export function modelName(id: string): string {
  const m = /^claude-([a-z]+)-(\d+)(?:-(\d{1,2})(?=$|[-[]))?/.exec(id);
  if (!m || !m[1] || !m[2]) return id;
  const family = `${m[1].charAt(0).toUpperCase()}${m[1].slice(1)}`;
  return m[3] ? `${family} ${m[2]}.${m[3]}` : `${family} ${m[2]}`;
}

/** "Opus 5.5 · high"; empty when nothing is known. */
export function modelText(model?: string, effort?: string): string {
  return [model ? modelName(model) : undefined, effort].filter(Boolean).join(" · ");
}
