# op-workflow

App desktop (Tauri v2) chạy Claude Code và Codex theo quy trình làm task OpenProject: nhận task → phân tích → bạn duyệt plan → code → `/review-local` → Claude sửa → Codex review ⇄ Claude sửa → `/review-local` chốt → `/push-summary`. App chạy hoàn toàn trên máy, **không bao giờ tự commit hay push**.

Kiến trúc: [docs/architecture.md](docs/architecture.md). Muốn sửa phần nào thì xem [docs/backend-guide.md](docs/backend-guide.md).

## Yêu cầu

- Rust stable (đã chạy với 1.98, toolchain `x86_64-pc-windows-gnu`). Với bộ GNU, linker báo `.rsrc merge failure: multiple non-default manifests` vì MinGW tự gắn manifest mặc định. Thông báo này vô hại (manifest Common-Controls v6 vẫn có trong exe) nên đã tắt trong `apps/desktop/src-tauri/src/main.rs`. Bản phát hành nên build bằng toolchain MSVC.
- Node 24 + pnpm 12
- WebView2 (có sẵn trên Windows 10/11)
- Claude Code CLI và Codex CLI đã đăng nhập (để có quota thật; sau này để chạy các bước)

## Chạy

```bash
pnpm install
pnpm dev            # mở app Tauri (tự chạy Vite ở :5173)
pnpm ui:dev         # chỉ UI trong trình duyệt, dữ liệu mock
pnpm build          # bản cài NSIS: target/release/bundle/nsis/op-workflow_<ver>_x64-setup.exe
```

Lần đầu mở app: bấm biểu tượng bánh răng → **Cài đặt** → nhập URL OpenProject và dán token (My account → Access tokens) → **Kiểm tra & lưu**. Token lưu trong Windows Credential Manager. App chỉ **đọc** OpenProject: task giao cho bạn và comment. Chưa có token thì bảng task hiện dữ liệu demo. Quota Claude và Codex lúc nào cũng là số thật.

Chẩn đoán (không mở cửa sổ):

```bash
target/debug/op-workflow-desktop.exe --live-check    # quota thật + OpenProject có kết nối được không
target/debug/op-workflow-desktop.exe --pty-selftest  # TUI claude/codex trong ConPTY (ADR 0003)
```

## Kiểm tra

```bash
pnpm check                      # typecheck UI + fmt + clippy + test Rust
pnpm --filter @opw/ui test      # test UI
scripts/check-core-deps.sh      # core/ không được phụ thuộc tauri (có bản .ps1)
```

## Cấu trúc

```text
apps/desktop/ui         React + TypeScript; chỉ src/ipc/ được gọi Tauri
apps/desktop/src-tauri  vỏ mỏng: cửa sổ, lệnh IPC (*_api.rs)
core/                   Rust, không phụ thuộc Tauri
  domain                task, quy trình 8 bước (state machine), finding, DTO
  config                đường dẫn, ~/.op-workflow/config.json
  storage               SQLite + migration theo user_version
  quota                 quota Claude/Codex từ các nguồn không cần token
  tracker               OpenProject API v3, chỉ đọc
  vcs                   git: nhánh, tạo branch local, chặn commit/push của AI
  agents                dựng lệnh claude/codex, đọc stream-json
  context               redact, brief, handoff
  pty                   terminal tương tác (portable-pty, ADR 0003); --pty-selftest
  monitor               Phase 3
  orchestrator          nơi duy nhất ghép các crate
tools/opw-hook          hook + statusline bridge, chỉ ghi số liệu
tools/fake-agent        phát lại stream-json để test không tốn token
```

Dữ liệu và cấu hình nằm ở `~/.op-workflow/` (bản debug dùng `~/.op-workflow-dev/`). Token OpenProject lưu trong Windows Credential Manager, không nằm trong repo.
