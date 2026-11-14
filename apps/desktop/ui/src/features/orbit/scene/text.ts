// SVG text does not wrap or ellipsize by itself; these helpers do it by display width.
//
// Width is counted in "units": one unit is a Latin/Vietnamese character, two units are a CJK or
// full-width character (Japanese titles are common in tasks). Counting characters instead made
// a Japanese line twice as wide as its card (2026-11-14).

/** CJK, kana, Hangul, full-width forms, geometric shapes such as ■, emoji: two units wide. */
function isWide(c: number): boolean {
  return (
    (c >= 0x1100 && c <= 0x115f) ||
    (c >= 0x25a0 && c <= 0x25ff) ||
    (c >= 0x2e80 && c <= 0x303e) ||
    (c >= 0x3041 && c <= 0x33ff) ||
    (c >= 0x3400 && c <= 0x4dbf) ||
    (c >= 0x4e00 && c <= 0x9fff) ||
    (c >= 0xa000 && c <= 0xa4cf) ||
    (c >= 0xac00 && c <= 0xd7a3) ||
    (c >= 0xf900 && c <= 0xfaff) ||
    (c >= 0xfe30 && c <= 0xfe4f) ||
    (c >= 0xff00 && c <= 0xff60) ||
    (c >= 0xffe0 && c <= 0xffe6) ||
    (c >= 0x1f300 && c <= 0x1faff) ||
    (c >= 0x20000 && c <= 0x3fffd)
  );
}

export const charUnits = (ch: string): number => (isWide(ch.codePointAt(0) ?? 0) ? 2 : 1);

/** Display width of `s` in units. */
export function textUnits(s: string): number {
  let n = 0;
  for (const ch of s) n += charUnits(ch);
  return n;
}

/** The longest start of `s` that fits in `max` units, and the rest. */
function cut(s: string, max: number): [string, string] {
  let used = 0;
  let head = "";
  for (const ch of s) {
    const u = charUnits(ch);
    if (used + u > max && head) break;
    head += ch;
    used += u;
  }
  return [head, s.slice(head.length)];
}

/** Cut to `max` units with "…" when wider. */
export function truncate(s: string, max: number): string {
  return textUnits(s) > max ? `${cut(s, max - 1)[0]}…` : s;
}

/** A word in pieces that may break apart: runs of narrow characters stay whole, each wide
 *  (CJK) character can break from its neighbours, as in Japanese line breaking. */
function pieces(word: string): string[] {
  const out: string[] = [];
  let run = "";
  for (const ch of word) {
    if (charUnits(ch) === 2) {
      if (run) out.push(run);
      run = "";
      out.push(ch);
    } else {
      run += ch;
    }
  }
  if (run) out.push(run);
  return out;
}

/** Split into lines of at most `max` units: at spaces, between CJK characters, and inside
 *  words that are wider than a line. */
export function wrapText(s: string, max: number): string[] {
  const lines: string[] = [];
  let line = "";
  let used = 0;
  const flush = () => {
    if (line) lines.push(line);
    line = "";
    used = 0;
  };
  for (const word of s.trim().split(/\s+/)) {
    if (!word) continue;
    pieces(word).forEach((piece, i) => {
      // a space only between words, never inside one
      const gap = i === 0 && line ? 1 : 0;
      let rest = piece;
      let units = textUnits(rest);
      if (used + gap + units <= max) {
        line += (gap ? " " : "") + rest;
        used += gap + units;
        return;
      }
      flush();
      while (units > max) {
        const [head, tail] = cut(rest, max);
        lines.push(head);
        rest = tail;
        units = textUnits(rest);
      }
      line = rest;
      used = units;
    });
  }
  flush();
  return lines;
}
