# op-workflow

App desktop (Windows, Tauri v2) đưa một task OpenProject qua Claude Code và Codex theo từng bước: phân tích → bạn duyệt plan → viết code → kiểm tra → review, sửa những lỗi bạn chọn → test → viết commit message và mô tả PR.

App chạy trên máy bạn, chỉ **đọc** OpenProject và **không bao giờ commit hay push**: bạn tự review diff rồi commit, push.

## Tính năng chính

- Bảng task OpenProject (Inbox / Đang làm / Chờ push / Đã xong), báo comment mới bằng chuông và thông báo Windows.
- Luồng 12 bước, mặc định dừng sau mỗi bước để bạn xem; bật *Tự chạy* thì app chỉ dừng ở chỗ cần bạn quyết.
- Mỗi bước AI chọn được Claude hoặc Codex, model, effort và chế độ quyền.
- Có công tắc xác minh kết luận AI qua MCP cục bộ trong Cài đặt → Các bước AI (mặc định bật); thiếu kết luận hoặc AI báo chưa xong thì app dừng ở bước đó.
- Hạn mức Claude / Codex thật trên khay hệ thống, nhắc trước khi chạy bước AI.
- Cửa sổ task có các tab Brief, Comment, Plan, Lỗi review, Diff, Tiến trình, Chat, Push summary.

## Luồng 12 bước

1. **Nhận task**: chọn repo, base, branch; app tạo branch ở máy và ghi `brief.md`.
2. **Phân tích**: AI đọc brief và code, viết `plan.md`; app kiểm tra đủ mục và khối JSON. Bạn duyệt, góp ý, hoặc tự code.
3. **Viết code**
4. **Kiểm tra**: typecheck / lint / prettier, chỉ tính lỗi ở phần task đổi.
5. **Harness** (khi task cần): test dựng ngoài repo theo brief và plan; AI còn lại review harness trước khi app chạy.
6. **Tự review**
7. **Sửa lỗi**: chỉ những lỗi bạn chọn.
8. **Review chéo**: AI khác AI đã viết code.
9. **Sửa lỗi**
10. **Test trình duyệt** (khi task cần): AI thử thay đổi trong một cửa sổ Chrome riêng.
11. **Review chốt**
12. **Push summary**: commit message và mô tả PR. Bạn tự commit, push rồi bấm *Đã push xong*.

## Yêu cầu

- Windows 10/11 (có sẵn WebView2)
- Rust stable (đã chạy với 1.98, toolchain `x86_64-pc-windows-gnu`; bản phát hành nên build bằng MSVC)
- Node 24 và pnpm 12
- Claude Code CLI và Codex CLI đã đăng nhập

## Cài và chạy

```bash
pnpm install
pnpm dev        # mở app (tự chạy Vite ở :5173)
pnpm ui:dev     # chỉ giao diện trong trình duyệt, dữ liệu mock
pnpm build      # bản cài NSIS trong target/release/bundle/nsis/
```

## Bắt đầu dùng

1. Bấm biểu tượng bánh răng → **Cài đặt**: nhập URL OpenProject, dán access token (My account → Access tokens) → **Kiểm tra & lưu**. Chưa có token thì bảng hiện dữ liệu demo.
2. Kéo một task từ *Inbox* sang *Đang làm*, chọn repo, base và tên branch → **Bắt đầu**. Working tree của repo phải sạch.
3. **Mở task**, bấm chạy từng bước, xem kết quả ở các tab và quyết ở mỗi cổng (duyệt plan, chọn lỗi cần sửa…).
4. Push summary xong: tự review diff, commit, push, rồi bấm *Đã push xong*.

Cấu hình nằm ở `~/.op-workflow/config.json` (bản debug: `~/.op-workflow-dev/`); phần lớn chỉnh được trong Cài đặt. Token OpenProject lưu trong Windows Credential Manager.

## Phát triển

```bash
pnpm check                    # typecheck UI + fmt + clippy + test Rust
pnpm --filter @opw/ui test    # test UI
scripts/check-core-deps.sh    # core/ không được phụ thuộc Tauri (có bản .ps1)
```

```text
apps/desktop/ui         React + TypeScript
apps/desktop/src-tauri  vỏ Tauri: cửa sổ, khay, lệnh IPC
core/                   Rust, không phụ thuộc Tauri (orchestrator ghép các crate)
tools/                  hook, statusline bridge, fake-agent cho test
```

- Kiến trúc: [docs/architecture.md](docs/architecture.md)
- Muốn sửa phần nào thì xem ở đâu: [docs/backend-guide.md](docs/backend-guide.md)
- Quyết định thiết kế: [docs/adr/](docs/adr/)

Font giao diện Inter và JetBrains Mono (OFL-1.1) được đóng gói kèm app, không tải từ mạng.
