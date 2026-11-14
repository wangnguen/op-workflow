# op-workflow

App desktop (Tauri v2) chạy Claude Code và Codex theo quy trình làm task OpenProject: nhận task → phân tích → bạn duyệt plan → code → `/review-local` → Claude sửa → Codex review ⇄ Claude sửa → `/review-local` chốt → `/push-summary`. App chạy hoàn toàn trên máy, **không bao giờ tự commit hay push**.

Kiến trúc: [docs/architecture.md](docs/architecture.md). Muốn sửa phần nào thì xem [docs/backend-guide.md](docs/backend-guide.md).

## Yêu cầu

- Rust stable (đã chạy với 1.98, toolchain `x86_64-pc-windows-gnu`). Với bộ GNU, linker báo `.rsrc merge failure: multiple non-default manifests` vì MinGW tự gắn manifest mặc định. Thông báo này vô hại (manifest Common-Controls v6 vẫn có trong exe) nên đã tắt trong `apps/desktop/src-tauri/src/main.rs`. Bản phát hành nên build bằng toolchain MSVC.
- Node 24 + pnpm 12
- WebView2 (có sẵn trên Windows 10/11)
- Claude Code CLI và Codex CLI đã đăng nhập (quota thật và các bước AI)

## Chạy

```bash
pnpm install
pnpm dev            # mở app Tauri (tự chạy Vite ở :5173)
pnpm ui:dev         # chỉ UI trong trình duyệt, dữ liệu mock
pnpm build          # bản cài NSIS: target/release/bundle/nsis/op-workflow_<ver>_x64-setup.exe
```

Lần đầu mở app: bấm biểu tượng bánh răng → **Cài đặt** → nhập URL OpenProject và dán token (My account → Access tokens) → **Kiểm tra & lưu**. Token lưu trong Windows Credential Manager. App chỉ **đọc** OpenProject: task giao cho bạn và comment. Chưa có token thì bảng task hiện dữ liệu demo. Quota Claude và Codex lúc nào cũng là số thật. Bên nào mất kết nối hoặc bị đăng xuất thì vòng quota hiện "!"; mở popover Hạn mức (hoặc Cài đặt → Hạn mức) để bấm **Kết nối lại** hoặc **Đăng nhập** (app mở cửa sổ chạy `claude auth login` / `codex login`, đăng nhập xong đóng cửa sổ là app tự kết nối lại).

**Bắt đầu làm việc** trên một task: app tìm repo git trong các thư mục `workspaces` (mặc định `~/Desktop`, sâu 2 cấp), gợi ý repo có remote khớp link issue trong mô tả task, liệt kê nhánh local + `origin/*` (nút *Lấy nhánh mới* chạy `git fetch --prune origin`) và điền sẵn tên branch theo mẫu của project (lần đầu đoán từ các nhánh có sẵn của repo, ví dụ `f/WP-{taskId}`). Bấm **Bắt đầu** thì app tạo branch ở máy (`git switch -c <branch> --no-track <base>`; đã có thì chỉ switch), ghi `brief.md` vào `<thư mục brief>/<id>/` và chuyển task sang cột *Đang làm* (làm song song được nhiều task; hai task chung một repo thì app nhắc switch nhánh). Working tree chưa sạch thì app chặn lại để bạn tự commit hoặc stash. App nhớ repo, base và mẫu branch cho lần sau trong `config.json`. Không commit, không push. Trong danh sách nhánh có nút xoá: nhánh local (còn commit chưa merge thì hỏi thêm) và nhánh trên origin (gõ đúng tên nhánh để xác nhận); nhánh chính, nhánh đang đứng và nhánh của task đang làm thì không xoá được.

Ở màn **Phiên** cũng bắt đầu được: nút *Bắt đầu task* trên thanh trên (hoặc chọn một task Inbox trên bản đồ) mở cùng hộp đó; bắt đầu xong, bản đồ đưa task vào giữa. Tab *Bảng* của màn Phiên hiện các phiên theo Cần bạn / Đang chạy / Rảnh, kèm nút *Trên bản đồ*, *Mở task*, *Lấy issue*.

**Các bước AI (2–8)** chạy ngay trong app, mỗi bước là một tiến trình `claude -p` / `codex exec` chạy nền trong repo của task (chưa có terminal; terminal tương tác là Phase 3):

| Bước | AI | Làm gì |
|---|---|---|
| Phân tích | Claude, chỉ đọc (`--restricted`, Read/Grep/Glob) | Đọc brief và code, ghi `plan.md`. Dừng ở cổng **Duyệt plan**: *Duyệt, AI viết code* / *Góp ý* (phân tích lại) / *Tự code* (bạn code, xong bấm *Tôi code xong*) |
| Viết code, Sửa lỗi | Claude, tiếp phiên của bước Phân tích | Sửa file trong working tree. Bash chỉ chạy lệnh trong allowlist (`repos.<repo>.allowBash` và `.claude/settings.json` của repo); `git commit/push/merge/rebase/reset/clean/tag` bị chặn |
| Tự review, Review chốt | Claude | `/review-local` của repo nếu có (`.claude/commands/review-local.md`), không thì prompt review của app; kết quả về dạng lỗi review |
| Review chéo | Codex (`--sandbox read-only`) | Review diff so với base; còn lỗi thì Claude sửa rồi Codex review lại, tối đa 3 vòng |
| Chuẩn bị push | Claude | `/push-summary` của repo nếu có, không thì prompt của app: commit message + mô tả PR, mỗi khối một nút Copy |

Bấm **Mở task** để xem cửa sổ task: các bước, nút cho bước kế tiếp, và các tab *Brief* (sửa được trước khi AI đọc), *Plan*, *Lỗi review* (mỗi lỗi: Sửa / Chấp nhận + lý do / Để sau), *Push*, *Log*. Mặc định app **dừng sau mỗi bước** để bạn xem kết quả rồi bấm chạy bước tiếp; bật *Tự chạy bước tiếp* trong cửa sổ task (hoặc `"steps": { "autoAdvance": true }`) thì app chạy liền tới khi cần bạn. Lỗi medium ở Review chốt luôn chờ bạn quyết. Bước lỗi có nút *Thử lại*, và *Đăng nhập* khi Claude/Codex bị đăng xuất; *Dừng* kill cả cây tiến trình.

An toàn: trước và sau mỗi bước app so HEAD, nhánh hiện tại và các ref; bước chỉ đọc mà file trong working tree đổi thì app cũng dừng. Thấy gì lạ là app dừng và báo, không tự sửa. Giới hạn chi phí và thời gian của từng bước nằm ở `steps.<bước>.maxBudgetUsd` / `timeoutMin` (mặc định: Phân tích $1.5, Viết code $5, review $3, Sửa lỗi $2, Chuẩn bị push $1). App không commit, không push: xong bước 8 bạn tự review diff, commit, push, rồi bấm *Đã push xong*. `plan.md`, `push.md` và kết quả Codex nằm cạnh `brief.md` trong thư mục task; output thô của từng lần chạy ở `<thư mục dữ liệu>/runs` (tự xoá sau 14 ngày).

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
  agents                dựng lệnh claude/codex, chạy tiến trình, đọc stream-json, tách lỗi review
  context               redact, brief, prompt cho từng bước, handoff
  update                kiểm tra bản mới trên GitHub Releases
  browser               đọc issue GitHub qua Chrome của bạn (DevTools, chỉ đọc)
  pty                   terminal tương tác (portable-pty, ADR 0003); --pty-selftest
  monitor               Phase 3
  orchestrator          nơi duy nhất ghép các crate; flow.rs chạy các bước 2–8
tools/opw-hook          hook + statusline bridge, chỉ ghi số liệu
tools/fake-agent        phát lại stream-json để test không tốn token
```

Font giao diện: Inter và JetBrains Mono (giấy phép OFL-1.1, gói `@fontsource-variable/*`), đóng gói kèm app, có bộ ký tự tiếng Việt; app không tải font từ mạng.

Dữ liệu và cấu hình nằm ở `~/.op-workflow/` (bản debug dùng `~/.op-workflow-dev/`). Token OpenProject lưu trong Windows Credential Manager, không nằm trong repo.
