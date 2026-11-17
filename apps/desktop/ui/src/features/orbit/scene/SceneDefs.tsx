import { memo } from "react";
/** Shared gradients and filters for the orbit scene. */
export const SceneDefs = memo(function SceneDefs() {
  return (
    <defs>
      <radialGradient id="g-star">
        <stop offset="0%" style={{ stopColor: "var(--star-core)" }} />
        <stop offset="55%" style={{ stopColor: "var(--star-mid)" }} />
        <stop offset="100%" style={{ stopColor: "#ff8a3d" }} />
      </radialGradient>
      <radialGradient id="g-star-wait">
        <stop offset="0%" style={{ stopColor: "#fffbe6" }} />
        <stop offset="60%" style={{ stopColor: "var(--star-wait)" }} />
        <stop offset="100%" style={{ stopColor: "#f0b400" }} />
      </radialGradient>
      <radialGradient id="g-halo">
        <stop offset="0%" style={{ stopColor: "#ffb067", stopOpacity: 0.55 }} />
        <stop offset="45%" style={{ stopColor: "#ff8a3d", stopOpacity: 0.14 }} />
        <stop offset="100%" style={{ stopColor: "#ff8a3d", stopOpacity: 0 }} />
      </radialGradient>
      <radialGradient id="g-halo-wait">
        <stop offset="0%" style={{ stopColor: "#ffe680", stopOpacity: 0.6 }} />
        <stop offset="45%" style={{ stopColor: "#ffd54f", stopOpacity: 0.16 }} />
        <stop offset="100%" style={{ stopColor: "#ffd54f", stopOpacity: 0 }} />
      </radialGradient>

      <radialGradient id="g-atmo-claude">
        <stop offset="0%" style={{ stopColor: "var(--claude)", stopOpacity: 0.04 }} />
        <stop offset="70%" style={{ stopColor: "var(--claude)", stopOpacity: 0.16 }} />
        <stop offset="92%" style={{ stopColor: "var(--claude)", stopOpacity: 0.42 }} />
        <stop offset="100%" style={{ stopColor: "var(--claude)", stopOpacity: 0 }} />
      </radialGradient>
      <radialGradient id="g-atmo-codex">
        <stop offset="0%" style={{ stopColor: "var(--codex)", stopOpacity: 0.04 }} />
        <stop offset="70%" style={{ stopColor: "var(--codex)", stopOpacity: 0.16 }} />
        <stop offset="92%" style={{ stopColor: "var(--codex)", stopOpacity: 0.42 }} />
        <stop offset="100%" style={{ stopColor: "var(--codex)", stopOpacity: 0 }} />
      </radialGradient>
      <radialGradient id="g-atmo-shell">
        <stop offset="0%" style={{ stopColor: "var(--shell)", stopOpacity: 0.04 }} />
        <stop offset="92%" style={{ stopColor: "var(--shell)", stopOpacity: 0.4 }} />
        <stop offset="100%" style={{ stopColor: "var(--shell)", stopOpacity: 0 }} />
      </radialGradient>

      <linearGradient id="g-meteor" x1="0" y1="0" x2="1" y2="0">
        <stop offset="0%" style={{ stopColor: "#ffffff", stopOpacity: 0.95 }} />
        <stop offset="100%" style={{ stopColor: "#9fd0ff", stopOpacity: 0 }} />
      </linearGradient>

      <filter id="f-glow" x="-80%" y="-80%" width="260%" height="260%">
        <feGaussianBlur stdDeviation="1.6" result="blur" />
        <feMerge>
          <feMergeNode in="blur" />
          <feMergeNode in="SourceGraphic" />
        </feMerge>
      </filter>
    </defs>
  );
});
