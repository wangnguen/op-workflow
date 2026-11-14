// Pure formatting helpers (Vietnamese). No React here, so they are easy to test.

export function clamp(x: number, lo: number, hi: number): number {
  return Math.min(hi, Math.max(lo, x));
}

export function formatTokens(n: number): string {
  if (n >= 1_000_000) return `${trimNumber(n / 1_000_000)}M`;
  if (n >= 1_000) return `${trimNumber(n / 1_000)}K`;
  return String(Math.round(n));
}

function trimNumber(x: number): string {
  return x >= 100 ? x.toFixed(0) : x.toFixed(1).replace(/\.0$/, "");
}

export function formatUsd(n: number): string {
  return `$${n.toFixed(2)}`;
}

export function formatPct(n: number): string {
  return `${Math.round(n)}%`;
}

/** "3 giờ 5 phút", "12 phút", "40 giây" */
export function formatDuration(ms: number): string {
  const totalSec = Math.max(0, Math.round(ms / 1000));
  const min = Math.floor(totalSec / 60);
  const hours = Math.floor(min / 60);
  const days = Math.floor(hours / 24);
  if (days > 0) return `${days} ngày ${hours % 24} giờ`;
  if (hours > 0) return `${hours} giờ ${min % 60} phút`;
  if (min > 0) return `${min} phút`;
  return `${totalSec} giây`;
}

/** "vừa xong", "5 phút trước", "2 giờ trước", "hôm qua", "3 ngày trước" */
export function formatRelative(iso: string | undefined, now: number): string {
  if (!iso) return "—";
  const t = Date.parse(iso);
  if (Number.isNaN(t)) return "—";
  const sec = Math.round((now - t) / 1000);
  if (sec < 45) return "vừa xong";
  const min = Math.round(sec / 60);
  if (min < 60) return `${min} phút trước`;
  const hours = Math.round(min / 60);
  if (hours < 24) return `${hours} giờ trước`;
  const days = Math.round(hours / 24);
  return days === 1 ? "hôm qua" : `${days} ngày trước`;
}

/** Countdown to a reset time: "còn 2 giờ 14 phút". */
export function formatCountdown(iso: string | undefined, now: number, passedText: string): string {
  if (!iso) return "—";
  const t = Date.parse(iso);
  if (Number.isNaN(t)) return "—";
  const ms = t - now;
  return ms <= 0 ? passedText : `còn ${formatDuration(ms)}`;
}

/** "14/11 09:32" in local time. */
export function formatDateTime(iso: string): string {
  const d = new Date(iso);
  if (Number.isNaN(d.getTime())) return "—";
  const two = (n: number) => String(n).padStart(2, "0");
  return `${two(d.getDate())}/${two(d.getMonth() + 1)} ${two(d.getHours())}:${two(d.getMinutes())}`;
}

export function formatClock(iso: string): string {
  const d = new Date(iso);
  if (Number.isNaN(d.getTime())) return "—";
  const hh = String(d.getHours()).padStart(2, "0");
  const mm = String(d.getMinutes()).padStart(2, "0");
  const ss = String(d.getSeconds()).padStart(2, "0");
  return `${hh}:${mm}:${ss}`;
}
