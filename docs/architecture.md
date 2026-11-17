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
|  vcs          : git (branches, diff, guards)             |
|  browser      : GitHub issue via local Chrome DevTools  |
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

## Luồng dữ liệu hiện tại

UI hỏi `board_snapshot` / `orbit_snapshot` và chi tiết task theo nhịp → `AppService` ghép OpenProject, SQLite, git và quota. Không có token OpenProject thì bảng dùng dữ liệu demo; bản UI trình duyệt dùng `ipc/mock*.ts`. CLI giả chỉ dùng trong test/preview.

`domain::workflow` giữ máy trạng thái 12 bước. `orchestrator::flow` claim một bước, kiểm tra git, gọi launcher AI hoặc check runner, ghi kết quả và chuyển bước. Các cổng Duyệt plan / Chọn lỗi / Review chéo lại cần người dùng quyết. Hai task chung repo không chạy bước đồng thời. `Saved` nằm trong bảng `flow`; lịch sử lần chạy nằm trong `step_run`; migration 6 và `SavedWorkflow` hỗ trợ task cũ 8 bước.

`checks`, `harness`, `browsertest` trong orchestrator đọc kết quả lệnh/test thành ghi chú và finding. File task, scratch và thư viện harness ở ngoài repo. `agents::procs` theo dõi/dọn cây tiến trình AI để lại; vỏ Tauri thực hiện lựa chọn thu xuống khay/thoát sau khi hỏi người dùng.

Gauge quota cập nhật từ các nguồn trong `live`; `quotagate` dùng cùng quy tắc reset như gauge để nhắc trước bước và dừng Tự chạy. `flow` lưu bước/phiên bị hết hạn mức hoặc bước chờ trước launch; `AppService::poll_alerts` xét reset mỗi lần shell hỏi (~3 giây), chạy tiếp hoặc báo chuông theo Tự chạy. DTO `QuotaNote` và `FlowError.resetsAt` mang thông tin sang UI, không thêm lần gọi AI để hiện cảnh báo.

## Gọi mạng

App đọc OpenProject (`tracker`, GET), kiểm tra GitHub Releases (`update`, tắt được), đọc issue GitHub qua Chrome của bạn (`browser`), và gọi remote git khi bạn lấy nhánh / kiểm tra push. Xoá nhánh trên origin là thao tác ghi remote duy nhất của app, chỉ sau khi bạn bấm và gõ tên nhánh. CLI `claude`/`codex` gọi dịch vụ AI; bước Test trình duyệt có thể tải Chrome DevTools MCP qua npx và truy cập app theo kịch bản. Lệnh Kiểm tra/harness/dev server theo cấu hình repo vẫn có thể dùng mạng của chính công cụ đó.

## Bảo vệ

- App không commit, không push. Mọi tiến trình AI chạy kèm môi trường git chặn commit/push (`opw_vcs::guard_env`), xem [ADR 0002](adr/0002-no-commit-no-push.md).
- App chỉ đọc OpenProject, không ghi gì lên đó. Bạn tự đổi trạng thái task trên OpenProject.
- IPC app dùng Tauri, không mở server riêng. Chrome DevTools và dev server của bài test có thể mở cổng; tiến trình AI bật được dọn sau lần chạy. CSP chặt; secret trong Windows Credential Manager.
