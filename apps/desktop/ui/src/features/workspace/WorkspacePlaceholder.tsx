import { vi } from "../../i18n/vi";

export function WorkspacePlaceholder() {
  return (
    <div className="placeholder">
      <div className="placeholder-card">
        <svg viewBox="0 0 64 48" width={64} height={48} aria-hidden="true">
          <rect x="2" y="2" width="60" height="44" rx="6" className="ph-frame" />
          <path d="M 12 16 L 20 22 L 12 28" className="ph-prompt" />
          <path d="M 24 30 L 38 30" className="ph-cursor" />
        </svg>
        <h2>{vi.workspace.title}</h2>
        <p className="dim">{vi.workspace.body}</p>
      </div>
    </div>
  );
}
