import { useEffect, useState } from "react";
import { vi } from "../i18n/vi";
import {
  errorMessage,
  minimizeWindow,
  onWindowMaximized,
  requestWindowClose,
  toggleMaximizeWindow,
} from "../ipc/client";

/** Minimize / maximize / close at the right end of the top bar: the window has no frame of its own,
 *  so the top bar is the title bar (drag it, double-click it to maximize). */
export function WindowControls() {
  const [maximized, setMaximized] = useState(false);
  const [error, setError] = useState<string>();
  useEffect(() => onWindowMaximized(setMaximized), []);

  const actions = [
    { name: "minimize", label: vi.window.minimize, run: minimizeWindow },
    {
      name: maximized ? "restore" : "maximize",
      label: maximized ? vi.window.restore : vi.window.maximize,
      run: toggleMaximizeWindow,
    },
    { name: "close", label: vi.window.close, run: requestWindowClose },
  ];

  return (
    <div className="window-controls" role="group" aria-label={vi.window.controls}>
      {error && (
        <span className="window-controls-error" role="status" title={error}>
          {error}
        </span>
      )}
      {actions.map(({ name, label, run }) => (
        <button
          type="button"
          key={name === "restore" ? "maximize" : name}
          className={`window-button${name === "close" ? " window-button--close" : ""}`}
          aria-label={label}
          title={label}
          onClick={() => {
            setError(undefined);
            void run().catch((e: unknown) => setError(errorMessage(e)));
          }}
        >
          <span className={`window-glyph window-glyph--${name}`} aria-hidden="true" />
        </button>
      ))}
    </div>
  );
}
