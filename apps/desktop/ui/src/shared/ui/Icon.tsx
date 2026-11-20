import type { ReactNode } from "react";

/** Small stroke icons on a 24-unit grid, drawn here (no icon font, nothing to download). */
const PATHS = {
  branch: (
    <>
      <circle cx={6} cy={5} r={2} />
      <circle cx={6} cy={19} r={2} />
      <circle cx={18} cy={7} r={2} />
      <path d="M6 7v10M18 9c0 5-12 3-12 8" />
    </>
  ),
  laptop: <path d="M5 6h14v9H5zM3 18.5h18" />,
  cloud: <path d="M7.5 18h9.5a4 4 0 0 0 .6-7.96A6 6 0 0 0 6.2 9.3 4.4 4.4 0 0 0 7.5 18z" />,
  search: <path d="M10.5 4a6.5 6.5 0 1 0 0 13a6.5 6.5 0 1 0 0-13M15.5 15.5 20 20" />,
  close: <path d="M6 6l12 12M18 6 6 18" />,
  trash: <path d="M4.5 7h15M9.5 7V4.5h5V7M6.5 7l1 12.5h9l1-12.5M10.5 11v5M13.5 11v5" />,
  refresh: <path d="M19.5 12a7.5 7.5 0 1 1-2.2-5.3M19.5 4.5v4.6h-4.6" />,
  folder: <path d="M3.5 7.5a2 2 0 0 1 2-2h3.8l2 2h7.2a2 2 0 0 1 2 2v7a2 2 0 0 1-2 2h-13a2 2 0 0 1-2-2z" />,
  file: <path d="M7 3.5h7l4 4v13H7zM14 3.5v4h4M9.5 12.5h6M9.5 16h6" />,
  check: <path d="M5 12.5l4.5 4.5L19 7.5" />,
  chat: <path d="M5 5.5h14a1 1 0 0 1 1 1v9a1 1 0 0 1-1 1h-8l-4.5 3.5v-3.5H5a1 1 0 0 1-1-1v-9a1 1 0 0 1 1-1z" />,
  chevron: <path d="M9.5 6l6 6-6 6" />,
  external: <path d="M13.5 4.5h6v6M19.5 4.5l-8 8M17.5 14v5a1 1 0 0 1-1 1h-11a1 1 0 0 1-1-1V8a1 1 0 0 1 1-1h5" />,
  pause: <path d="M9 6.5v11M15 6.5v11" />,
  play: <path d="M8.5 6v12l9.5-6z" />,
  arrowUp: <path d="M12 19V5.5M6 11.5l6-6 6 6" />,
  stop: <rect x={7} y={7} width={10} height={10} rx={1.5} />,
  dot: <circle cx={12} cy={12} r={3} />,
  alert: <path d="M12 4.5l8.5 15h-17zM12 10v4M12 16.8v.2" />,
  info: (
    <>
      <circle cx={12} cy={12} r={8.5} />
      <path d="M12 11v5.5M12 7.8v.2" />
    </>
  ),
  layers: <path d="M12 4l8.5 4.5L12 13 3.5 8.5zM3.5 12.5 12 17l8.5-4.5M3.5 16.5 12 21l8.5-4.5" />,
  sun: (
    <>
      <circle cx={12} cy={12} r={4} />
      <path d="M12 2.5v2M12 19.5v2M2.5 12h2M19.5 12h2M5.3 5.3l1.4 1.4M17.3 17.3l1.4 1.4M5.3 18.7l1.4-1.4M17.3 6.7l1.4-1.4" />
    </>
  ),
  moon: <path d="M19.5 14.5A7.5 7.5 0 0 1 9.5 4.5a7.5 7.5 0 1 0 10 10z" />,
  palette: (
    <>
      <path d="M12 3.5a8.5 8.5 0 0 0 0 17c1.2 0 1.8-.9 1.4-1.9-.5-1.2.3-2.3 1.6-2.3h1.8a3.7 3.7 0 0 0 3.7-3.7C20.5 7.6 16.7 3.5 12 3.5z" />
      <circle cx={8} cy={11} r={1} />
      <circle cx={11} cy={7.5} r={1} />
      <circle cx={15.5} cy={8.5} r={1} />
    </>
  ),
} satisfies Record<string, ReactNode>;

export type IconName = keyof typeof PATHS;

interface Props {
  name: IconName;
  size?: number;
  className?: string;
  /** set when the icon is the only content of a control */
  label?: string;
}

export function Icon({ name, size = 14, className, label }: Props) {
  return (
    <svg
      viewBox="0 0 24 24"
      width={size}
      height={size}
      className={className ? `icon ${className}` : "icon"}
      role={label ? "img" : undefined}
      aria-label={label}
      aria-hidden={label ? undefined : true}
    >
      {PATHS[name]}
    </svg>
  );
}
