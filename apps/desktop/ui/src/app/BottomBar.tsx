import { gateLabel, stepLabel, vi } from "../i18n/vi";
import type { BoardSnapshot, OrbitSnapshot } from "../ipc/types";
import { formatTokens, formatUsd } from "../shared/format";
import { STEP_COUNT } from "../shared/steps";

interface Props {
  board: BoardSnapshot | undefined;
  orbit: OrbitSnapshot | undefined;
  version: string | undefined;
  error: string | undefined;
}

export function BottomBar({ board, orbit, version, error }: Props) {
  const active = board?.tasks.find((t) => t.id === board.activeTaskId);
  const gate = orbit?.active?.gate;
  let status: string = vi.bottom.noTask;
  if (active?.step) {
    const step = `${active.step.index}/${STEP_COUNT} ${stepLabel[active.step.kind]}`;
    status = vi.bottom.activeTask(active.id, gate ? `${step} · ${gateLabel[gate.kind]}` : step);
  }
  return (
    <footer className="bottombar">
      <span className={`bb-status${gate ? " is-waiting" : active ? " is-running" : ""}`}>
        <span className="bb-dot" />
        {status}
      </span>
      {error && (
        <span className="bb-error">
          {vi.app.loadError}: {error}
        </span>
      )}
      <span className="topbar-spacer" />
      {board && (
        <span className="faint">
          {vi.bottom.today} <b className="bb-num">{formatTokens(board.today.tokens)}</b> {vi.bottom.tokens} ·{" "}
          <b className="bb-num">{formatUsd(board.today.costUsd)}</b>
        </span>
      )}
      {version && <span className="faint mono">v{version}</span>}
    </footer>
  );
}
