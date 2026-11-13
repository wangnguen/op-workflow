// SVG text does not wrap or ellipsize by itself; these helpers do it by character count.

/** Cut to `max` characters with "…" when longer. */
export function truncate(s: string, max: number): string {
  return s.length > max ? `${s.slice(0, max - 1)}…` : s;
}

/** Split into lines of at most `max` characters, breaking at spaces (long words are cut). */
export function wrapText(s: string, max: number): string[] {
  const lines: string[] = [];
  let line = "";
  for (const word of s.trim().split(/\s+/)) {
    if (!word) continue;
    let w = word;
    while (w.length > max) {
      if (line) {
        lines.push(line);
        line = "";
      }
      lines.push(w.slice(0, max));
      w = w.slice(max);
    }
    if (!line) line = w;
    else if (line.length + 1 + w.length <= max) line = `${line} ${w}`;
    else {
      lines.push(line);
      line = w;
    }
  }
  if (line) lines.push(line);
  return lines;
}
