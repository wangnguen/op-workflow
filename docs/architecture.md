# Kiến trúc

```text
+----------------------------------------------------------+
| Tauri shell (apps/desktop/src-tauri)                     |
|  windows, tray, notifications, commands  <-->  React UI  |
+----------------------------------------------------------+
          |  Rust calls; Tauri Channel streams; no open port
          v
+----------------------------------------------------------+
| core/ (Rust workspace, no Tauri deps)                    |
|  orchestrator : workflow state machine + gates           |
|  agents       : claude / codex adapters (headless)       |
|  pty          : interactive terminals (ConPTY)           |
|  monitor      : session registry, transcripts, rollouts  |
|  quota        : rate_limit_event, app-server, statusline |
|  tracker      : OpenProject API v3                       |
|  vcs          : git (branches, diff), optional gh        |
|  context      : brief, redact, handoff                   |
|  update       : new-version check (GitHub Releases)      |
|  storage      : SQLite (WAL, user_version)               |
+----------------------------------------------------------+
     |          |            |             |           |
     v          v            v             v           v
  claude      codex      ~/.claude/     ~/.codex/   git
   (CLI)      (CLI)       (files)        (files)    OpenProject
```

## Quy tắc phụ thuộc

- `core/*` không phụ thuộc `tauri`, `wry`, `tao`. `scripts/check-core-deps.*` kiểm tra điều này trong CI.
- `domain` không có I/O. Các crate năng lực (`tracker`, `vcs`, `quota`, …) chỉ phụ thuộc `domain` và thư viện ngoài.
- Chỉ `orchestrator` được ghép các crate năng lực với nhau. Vỏ Tauri chỉ gọi `orchestrator`.
- UI chỉ gọi Tauri trong `apps/desktop/ui/src/ipc/`. Hợp đồng dữ liệu: `opw_domain::view` (Rust) ↔ `ipc/types.ts` (TS), test `view::tests` khoá format JSON.

## Luồng dữ liệu của bản base

UI hỏi `board_snapshot` / `orbit_snapshot` mỗi 1–2 giây → `AppService` → `demo` (chạy máy trạng thái thật của `domain::workflow` theo thời gian). Phase 1 thay `demo` bằng dữ liệu từ OpenProject, git, agent và quota.

## Gọi mạng

Chỉ các đích sau, đều do code trong `core/` gọi: OpenProject (`tracker`, chỉ GET), API của Anthropic/OpenAI qua CLI `claude`/`codex`, remote git của repo khi bạn bấm *Lấy nhánh mới* (`git fetch`, chỉ đọc), và `api.github.com/repos/<repo>/releases` để kiểm tra bản mới (`update`, tắt được bằng `update.check`).

## Bảo vệ

- App không commit, không push. Mọi tiến trình AI chạy kèm môi trường git chặn commit/push (`opw_vcs::guard_env`), xem [ADR 0002](adr/0002-no-commit-no-push.md).
- App chỉ đọc OpenProject, không ghi gì lên đó. Bạn tự đổi trạng thái task trên OpenProject.
- Không mở port; CSP chặt; secret trong Windows Credential Manager.
