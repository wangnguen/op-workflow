# ADR 0003: Terminal tương tác dùng portable-pty (ConPTY) và core tự trả lời `ESC[6n`

- Trạng thái: chấp nhận (2026-11-13)

## Bối cảnh

Bước Implement chạy TUI thật của `claude` và `codex` trong pane terminal của app. Trên Windows việc này đi qua ConPTY. Orrery từng gặp tiến trình con bị treo khi dùng `portable-pty`, nên Phase 0 phải thử trước: dùng `portable-pty` hay gọi thẳng API ConPTY (cần `unsafe`, mà workspace đang `forbid`).

Phần mềm diệt virus trên máy làm việc cách ly file exe thử nghiệm build trong `%TEMP%` (phát hiện kiểu heuristic), kể cả bản copy chưa chạy. Vì vậy spike được chạy từ chính file exe của app, qua cờ `--pty-selftest`.

## Thử nghiệm

1. Spike riêng với `cmd`: ConPTY gửi `ESC[6n` (hỏi vị trí con trỏ) ngay khi mở rồi đứng chờ. Không trả lời thì tiến trình con không chạy tiếp, và đóng PTY cũng treo. Trả lời `ESC[1;1R` thì chạy bình thường.
2. `op-workflow-desktop --pty-selftest` (`core/orchestrator/src/selftest.rs`). Mở TUI trong thư mục trống, không gõ prompt nên không tốn token, cũng không trust thư mục nào:

| | claude 2.1.323 | codex 0.169.0 (`codex.exe` thật, không qua `codex.cmd`) |
|---|---|---|
| Output đầu tiên | 53 ms | 57 ms |
| `ESC[6n` do core trả lời | 1 lần | 1 lần |
| Đổi kích thước 120x30 → 100x40 | ok, TUI vẽ lại (+2,2 KB) | ok, TUI vẽ lại (+2,4 KB) |
| `taskkill /PID <pid> /T /F` | thoát ngay | thoát ngay |
| Đóng console | 3 ms | 31 ms |
| Màn hình | hộp "Accessing workspace…" (trust) | "OpenAI Codex (v0.169.0) loading" |

## Quyết định

1. Dùng `portable-pty` 0.9.0, không gọi thẳng ConPTY; workspace vẫn `forbid(unsafe_code)`.
2. `opw_pty::PtySession` tự trả lời `ESC[6n` (`DsrWatcher`, bắt được cả truy vấn bị cắt giữa hai lần đọc) cho tới khi xterm.js gắn vào pane, vì xterm.js tự trả lời. Gọi `set_answer_dsr(false)` khi gắn xterm.js để không trả lời hai lần.
3. Kill cả cây tiến trình bằng `taskkill /T /F` với cờ `CREATE_NO_WINDOW`, rồi `Child::kill` dự phòng. Đóng console có timeout; quá hạn thì báo lỗi, không chặn UI.
4. Gọi thẳng file chạy thật của CLI (`opw_agents::cli::find_claude` / `find_codex`), không đi qua shim `codex.cmd`.
5. Lọc môi trường cho tiến trình con bằng `opw_agents::env::is_stripped`. Với preset AI, app thêm `opw_vcs::guard_env` (ADR 0002).

## Hệ quả

- Phase 3 dựng phần hiển thị bằng xterm.js và stream byte qua Tauri Channel trên nền `PtySession`. Phần này chưa thử.
- Phần mềm diệt virus có thể bắt file exe của app khi code thay đổi. Trước khi phát hành cho đồng nghiệp cần ký số, hoặc nhờ IT cho phép.
- Mỗi lần nâng version CLI, chạy lại `op-workflow-desktop --pty-selftest`. Kết quả ghi vào `<data_dir>/pty-selftest.txt`.
