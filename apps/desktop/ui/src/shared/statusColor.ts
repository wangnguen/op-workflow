// OpenProject status colours (set by the admin) made readable on the dark UI.

/** Used when OpenProject gives a status no colour: same name, same colour. */
const FALLBACK = ["#4dabf7", "#da77f2", "#69db7c", "#ffa94d", "#ff8787", "#38d9a9", "#ffd43b", "#9775fa"];

const HEX = /^#[0-9a-f]{6}$/i;

export function statusColor(name: string, hex: string | undefined): string {
  if (hex && HEX.test(hex)) return hex.toLowerCase();
  let h = 0;
  for (const ch of name.trim().toLowerCase()) h = (h * 31 + ch.charCodeAt(0)) >>> 0;
  return FALLBACK[h % FALLBACK.length] ?? "#4dabf7";
}

interface Hsl {
  h: number;
  s: number;
  l: number;
}

function toHsl(hex: string): Hsl {
  const n = Number.parseInt(hex.slice(1), 16);
  const r = ((n >> 16) & 255) / 255;
  const g = ((n >> 8) & 255) / 255;
  const b = (n & 255) / 255;
  const max = Math.max(r, g, b);
  const min = Math.min(r, g, b);
  const l = (max + min) / 2;
  if (max === min) return { h: 0, s: 0, l };
  const d = max - min;
  const s = l > 0.5 ? d / (2 - max - min) : d / (max + min);
  const h = max === r ? (g - b) / d + (g < b ? 6 : 0) : max === g ? (b - r) / d + 2 : (r - g) / d + 4;
  return { h: h * 60, s, l };
}

const hsl = ({ h, s, l }: Hsl, a = 1) =>
  `hsl(${Math.round(h)} ${Math.round(s * 100)}% ${Math.round(l * 100)}%${a < 1 ? ` / ${a}` : ""})`;

export interface ChipColors {
  /** the status colour itself, for the dot */
  dot: string;
  /** text: same hue, light enough to read on a dark panel */
  ink: string;
  /** soft tint behind the text */
  tint: string;
}

/**
 * Keep the hue, move the lightness so the label reads on the panel: dark UI lifts dark colours
 * (e.g. #000000 "To Do"), light UI darkens pale ones (e.g. #f3f0ff "Cancelled").
 */
export function chipColors(name: string, hex: string | undefined, mode: "light" | "dark" = "dark"): ChipColors {
  const base = statusColor(name, hex);
  const c = toHsl(base);
  const s = Math.min(c.s, 0.85);
  if (mode === "light") {
    return {
      dot: c.l > 0.85 ? hsl({ h: c.h, s, l: 0.7 }) : base,
      ink: hsl({ h: c.h, s, l: Math.min(Math.max(c.l, 0.22), 0.36) }),
      tint: hsl({ h: c.h, s, l: Math.min(Math.max(c.l, 0.45), 0.6) }, 0.14),
    };
  }
  return {
    dot: c.l < 0.2 ? hsl({ h: c.h, s, l: 0.55 }) : base,
    ink: hsl({ h: c.h, s, l: Math.min(Math.max(c.l, 0.7), 0.85) }),
    tint: hsl({ h: c.h, s, l: Math.max(c.l, 0.5) }, 0.16),
  };
}
