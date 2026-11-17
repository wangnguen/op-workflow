// Dialogs sit in the middle of the window. One whose content changes after it opens (a check
// result replacing "Đang kiểm tra…", a list that loads) would then jump by half the change; this
// eases every `.sheet` to its new place instead. Started once for the whole app.

const MOVE_MS = 200;
const EASE = "cubic-bezier(0.2, 0.8, 0.2, 1)";

const reducedMotion = () =>
  typeof window.matchMedia === "function" && window.matchMedia("(prefers-reduced-motion: reduce)").matches;

/** Follows the `.sheet` elements under `root`; returns the function that stops it. */
export function followSheets(root: HTMLElement = document.body): () => void {
  if (typeof ResizeObserver === "undefined" || typeof MutationObserver === "undefined") return () => undefined;
  // layout position (offsetTop ignores the transforms of running animations)
  const tops = new WeakMap<HTMLElement, number>();
  const sizes = new ResizeObserver((entries) => {
    for (const entry of entries) {
      const el = entry.target as HTMLElement;
      const top = el.offsetTop;
      const before = tops.get(el);
      tops.set(el, top);
      if (before === undefined || Math.abs(before - top) < 1 || reducedMotion()) continue;
      // `translate` composes with the opening animation, which uses `scale`
      el.animate([{ translate: `0 ${before - top}px` }, { translate: "0 0" }], { duration: MOVE_MS, easing: EASE });
    }
  });
  const sheetsIn = (node: Node): HTMLElement[] => {
    if (!(node instanceof HTMLElement)) return [];
    const found = Array.from(node.querySelectorAll<HTMLElement>(".sheet"));
    return node.classList.contains("sheet") ? [node, ...found] : found;
  };
  const added = new MutationObserver((records) => {
    for (const r of records) {
      r.addedNodes.forEach((n) => sheetsIn(n).forEach((el) => sizes.observe(el)));
      r.removedNodes.forEach((n) => sheetsIn(n).forEach((el) => sizes.unobserve(el)));
    }
  });
  sheetsIn(root).forEach((el) => sizes.observe(el));
  added.observe(root, { childList: true, subtree: true });
  // a window resize moves every sheet: only content changes are eased
  const forget = () => sheetsIn(root).forEach((el) => tops.set(el, el.offsetTop));
  window.addEventListener("resize", forget);
  return () => {
    added.disconnect();
    sizes.disconnect();
    window.removeEventListener("resize", forget);
  };
}
