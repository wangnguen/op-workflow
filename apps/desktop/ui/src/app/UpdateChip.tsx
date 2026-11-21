import { useRef, useState } from "react";
import { vi } from "../i18n/vi";
import type { UpdateView } from "../ipc/types";
import { useDismiss } from "../shared/hooks";
import { UpdateActions, downloadPct } from "../features/update/UpdateActions";

interface Props {
  update: UpdateView;
  onChanged: () => void;
  onOpenRelease: () => void;
}

/** "Bản mới" in the top bar: says how the download goes ("Đang tải 45%", "Cài bản 0.5.0") and
 *  opens the box that installs it. */
export function UpdateChip({ update, onChanged, onOpenRelease }: Props) {
  const [open, setOpen] = useState(false);
  const ref = useRef<HTMLDivElement>(null);
  useDismiss(ref, open, () => setOpen(false));
  const v = update.latest ?? "";
  const state = update.download?.state;
  const label =
    state === "ready"
      ? vi.topbar.updateReady(v)
      : state === "downloading"
        ? vi.topbar.updateDownloading(v, downloadPct(update))
        : vi.topbar.update(v);
  return (
    <div className="update-chip" ref={ref}>
      <button
        className={`chip chip--update${state === "ready" ? " is-ready" : ""}`}
        title={vi.topbar.updateHint}
        aria-expanded={open}
        onClick={() => setOpen((o) => !o)}
      >
        <svg viewBox="0 0 24 24" width={12} height={12} aria-hidden="true">
          <path d="M12 4v11M7 10l5 5 5-5M5 20h14" className="icon-stroke" />
        </svg>
        {label}
      </button>
      {open && (
        <div className="update-pop" role="dialog" aria-label={label}>
          <UpdateActions update={update} onChanged={onChanged} onOpenRelease={onOpenRelease} />
        </div>
      )}
    </div>
  );
}
