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

Lần đầu mở app: bấm biểu tượng bánh răng → **Cài đặt** → nhập URL OpenProject và dán token (My account → Access tokens) → **Kiểm tra & lưu**. Token lưu trong Windows Credential Manager. App chỉ **đọc** OpenProject: task giao cho bạn và comment. Chưa có token thì bảng task hiện dữ liệu demo. Quota Claude và Codex lúc nào cũng là số thật. Bên nào mất kết nối hoặc bị đăng xuất thì vòng quota hiện "!"; mở popover Hạn mức (hoặc Cài đặt → Hạn mức) để bấm **Kết nối lại** hoặc **Đăng nhập** (app mở cửa sổ chạy `claude auth login` / `codex login`, đăng nhập xong đóng cửa sổ là app tự kết nối lại).

**Bắt đầu làm việc** trên một task: app tìm repo git trong các thư mục `workspaces` (mặc định `~/Desktop`, sâu 2 cấp), gợi ý repo có remote khớp link issue trong mô tả task, liệt kê nhánh local + `origin/*` (nút *Lấy nhánh mới* chạy `git fetch --prune origin`) và điền sẵn tên branch theo mẫu của project (lần đầu đoán từ các nhánh có sẵn của repo, ví dụ `f/WP-{taskId}`). Bấm **Bắt đầu** thì app tạo branch ở máy (`git switch -c <branch> --no-track <base>`; đã có thì chỉ switch), ghi `brief.md` vào `<thư mục brief>/<id>/` và chuyển task sang cột *Đang làm* (làm song song được nhiều task; hai task chung một repo thì app nhắc switch nhánh). Working tree chưa sạch thì app chặn lại để bạn tự commit hoặc stash. App nhớ repo, base và mẫu branch cho lần sau trong `config.json`. Không commit, không push.

**Thư mục brief:** mặc định nằm trong thư mục dữ liệu (`~/.op-workflow/tasks`). Lần đầu app hỏi có chuyển ra `Desktop/op-workflow-tasks` cho dễ tìm không; đổi lại lúc nào cũng được ở Cài đặt → Giới thiệu → *Thư mục brief* (app chuyển các thư mục task cũ sang, lưu vào `tasksDir` trong `config.json`).

**Đọc issue GitHub** gắn trong task (nút *Lấy issue* trên thẻ, và tự chạy sau *Bắt đầu*): app nối vào Chrome bạn đang dùng (đã đăng nhập GitHub), mở issue trong một tab nền, chỉ lấy tiêu đề, nội dung và comment rồi đóng tab, ghi vào mục *Từ issue* của brief. Không token, không `gh`, không bấm hay sửa gì trên GitHub; tên người và `@mention` bị bỏ. Cần Chrome 144 trở lên và bật một lần ở `chrome://inspect/#remote-debugging`; mỗi lần đọc, Chrome hỏi **Allow**.

**Bản mới**: vài giờ một lần app hỏi GitHub Releases của repo này (một GET, không gửi thông tin cá nhân). Có bản mới thì thanh trên hiện chip *Bản mới vX.Y.Z*, bấm để mở trang tải; tắt bằng `"update": { "check": false }` trong `config.json`.

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
  update                kiểm tra bản mới trên GitHub Releases
  browser               đọc issue GitHub qua Chrome của bạn (DevTools, chỉ đọc)
  pty                   terminal tương tác (portable-pty, ADR 0003); --pty-selftest
  monitor               Phase 3
  orchestrator          nơi duy nhất ghép các crate
tools/opw-hook          hook + statusline bridge, chỉ ghi số liệu
tools/fake-agent        phát lại stream-json để test không tốn token
```

Font giao diện: Inter và JetBrains Mono (giấy phép OFL-1.1, gói `@fontsource-variable/*`), đóng gói kèm app, có bộ ký tự tiếng Việt; app không tải font từ mạng.

Dữ liệu và cấu hình nằm ở `~/.op-workflow/` (bản debug dùng `~/.op-workflow-dev/`). Token OpenProject lưu trong Windows Credential Manager, không nằm trong repo.
