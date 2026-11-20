import { useRef, useState } from "react";
import { vi } from "../../i18n/vi";
import { errorMessage, flowAction } from "../../ipc/client";
import { useDismiss } from "../../shared/hooks";
import { Icon } from "../../shared/ui/Icon";
import "./startOver.css";

const t = vi.workspace.steps;

/** "Làm lại từ đầu" with its confirmation in a small box over the page (nothing below moves):
 *  the task goes back to Phân tích in a new session. Used in the task window, on the Phiên
 *  cards and (inline) above a task's chat in Workspace. */
export function StartOver({
  wpId, disabled, onDone, small,
}: {
  wpId: number;
  disabled?: boolean;
  onDone: () => void;
  /** the cards' size */
  small?: boolean;
}) {
  const [open, setOpen] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string>();
  const ref = useRef<HTMLDivElement>(null);
  useDismiss(ref, open, () => setOpen(false));

  const confirm = async () => {
    setBusy(true);
    setError(undefined);
    try {
      await flowAction(wpId, { type: "startOver" });
      setOpen(false);
      onDone();
    } catch (e) {
      setError(errorMessage(e));
    } finally {
      setBusy(false);
    }
  };

  return (
    <div className="start-over" ref={ref}>
      <button
        type="button"
        className={small ? "btn btn--sm btn--ghost" : "btn btn--sm"}
        aria-expanded={open}
        aria-haspopup="dialog"
        disabled={disabled}
        title={t.startOverAbout}
        onClick={() => setOpen((v) => !v)}
      >
        <Icon name="refresh" size={12} />
        {t.startOver}
      </button>
      {open && (
        <div className="start-over-pop" role="dialog" aria-label={t.startOver}>
          <p>{t.startOverAbout}</p>
          {error && <p className="note note--error">{error}</p>}
          <div className="start-over-actions">
            <button type="button" className="btn btn--sm btn--ghost" onClick={() => setOpen(false)}>
              {t.cancel}
            </button>
            <button type="button" className="btn btn--sm btn--danger" disabled={busy} onClick={() => void confirm()}>
              {t.startOverConfirm}
            </button>
          </div>
        </div>
      )}
    </div>
  );
}
