import type { StepState } from "../../ipc/types";

interface Props {
  state: StepState;
  size?: number;
}

/** One glyph per step state, so state never rests on colour alone. */
export function StatusIcon({ state, size = 13 }: Props) {
  return (
    <svg viewBox="0 0 16 16" width={size} height={size} className={`status-icon status-icon--${state}`} aria-hidden="true">
      {state === "pending" && <circle cx={8} cy={8} r={5.6} strokeDasharray="2.2 2.2" />}
      {state === "running" && (
        <>
          <circle cx={8} cy={8} r={5.6} opacity={0.35} />
          <path className="arc" d="M8 2.4a5.6 5.6 0 0 1 5.6 5.6" strokeWidth={1.9} />
        </>
      )}
      {state === "waitingUser" && (
        <>
          <circle cx={8} cy={8} r={5.6} />
          <path className="solid" d="M8 2.4a5.6 5.6 0 0 1 0 11.2z" />
        </>
      )}
      {state === "done" && (
        <>
          <circle cx={8} cy={8} r={5.6} />
          <path d="M5.4 8.2l1.8 1.8 3.4-3.6" />
        </>
      )}
      {state === "failed" && (
        <>
          <circle cx={8} cy={8} r={5.6} />
          <path d="M6 6l4 4M10 6l-4 4" />
        </>
      )}
      {state === "skipped" && (
        <>
          <circle cx={8} cy={8} r={5.6} />
          <path d="M5.5 8h5" />
        </>
      )}
    </svg>
  );
}
