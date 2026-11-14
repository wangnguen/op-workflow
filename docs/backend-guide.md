# Muốn đổi X thì sửa ở đâu

| Muốn đổi | Sửa ở | Ghi chú |
|---|---|---|
| Thứ tự bước, cổng duyệt, số vòng review | `core/domain/src/workflow.rs` | Có test cho từng nhánh; thêm test khi đổi luật |
| Mức lỗi nào chặn / hỏi / chỉ hiện | `core/domain/src/finding.rs` (`GatePolicy`), config `gate` | |
| Mẫu tên branch | `core/domain/src/task.rs`, config `projects.*.branch` | App không đổi trạng thái OpenProject; bạn tự đổi. `infer_branch_template` đoán mẫu từ nhánh có sẵn |
| Hộp "Bắt đầu làm việc" (tìm repo, nhánh, tạo branch, brief) | `core/orchestrator/src/start.rs`, `core/vcs/src/discover.rs`, `apps/desktop/ui/src/features/board/StartWorkDialog.tsx` | Test dùng repo git tạm; không push |
| Mẫu `brief.md` | `core/context/src/brief.rs` | Luôn đi qua `redact` |
| Chạy các bước 2–8 (thứ tự, cổng, dừng sau mỗi bước, kiểm tra git trước/sau) | `core/orchestrator/src/flow.rs` (`Flow`, `run_step`, `git_surprise`) | Test dùng `Launcher` giả trên repo git tạm, không gọi AI thật. Trạng thái lưu ở bảng `flow` (JSON của `Saved`) |
| Prompt từng bước (plan, code, fix, review, push summary) | `core/context/src/prompts.rs` | Brief/issue bọc trong thẻ và gọi là dữ liệu (chống prompt injection) |
| Cờ dòng lệnh của từng bước | `core/agents/src/claude.rs` (`analyze_args`, `code_args`, `review_args`, …), `core/agents/src/codex.rs` | Bước nào cũng có `--tools`, `--strict-mcp-config`, `--max-budget-usd`; test khoá danh sách cờ |
| Đọc lỗi review từ câu trả lời, khối commit/PR | `core/agents/src/text.rs` | Ưu tiên khối JSON, sau đó mốc 🔴/🟠/🟡/🔵 và `[High]`; không đọc được thì bước lỗi (không coi là sạch) |
| Chạy tiến trình AI (stdin, đọc từng dòng, Dừng, timeout) | `core/agents/src/run.rs`, `CliLauncher` trong `flow.rs` | Kill cả cây tiến trình (`taskkill /T /F`); luôn kèm `guard_env` |
| Giới hạn tiền/thời gian, tự chạy bước tiếp | config `steps` (`opw_config::StepsConfig`), `flow::limits` | Không ghi thì dùng mặc định trong `limits` |
| Cửa sổ task (tab Brief/Comment/Plan/Lỗi review/Diff/Push/Log) | `apps/desktop/ui/src/features/task/` | Bản xem trước dùng `ipc/mockFlow.ts` (các bước giả, cùng luật với `flow.rs`) và `ipc/mockTask.ts` (comment, diff, chuông) |
| Tab Comment, "N comment mới", cập nhật mục comment của brief | `core/orchestrator/src/comments.rs`, `AppService::task_comments` / `refresh_brief_comments`, `core/context/src/brief.rs` (`replace_comment_section`) | Đã xem / đã báo lưu ở bảng `task_kv` (`comments.seen`, `comments.notified`, `issues.read`, `issues.seen`) |
| Tab Diff | `core/vcs/src/diff.rs` (đọc `git diff`), `Git::task_diff`, `core/orchestrator/src/diffview.rs` | Chỉ đọc: `--no-optional-locks`, `diff.autoRefreshIndex=false`; dấu vân tay từng file (`diff.seen`) cho "đổi từ lần xem trước" |
| Thông báo (chuông, toast Windows) | `core/orchestrator/src/alerts.rs`, `AppService::poll_alerts`, `Flow::set_alert_sink`, `apps/desktop/src-tauri/src/tray.rs` (`spawn_watcher`) | Core quyết báo gì; vỏ Tauri chỉ hiện toast (`tauri-plugin-notification`) theo config `notify` |
| Icon khay hạn mức | `core/orchestrator/src/tray.rs` (vẽ RGBA + tooltip), `apps/desktop/src-tauri/src/tray.rs` | Vỏ chỉ đưa pixel cho Windows; tooltip dưới 128 ký tự |
| Comment OpenProject theo nhịp | `core/orchestrator/src/live.rs` (`fetch_changed_comments`) | Chỉ đọc lại task có `updatedAt` đổi |
| Kiểm tra bản mới | `core/update/src/lib.rs`, `core/orchestrator/src/updater.rs`, config `update` | Chỉ GET GitHub Releases; không tự tải, không tự cài |
| Field gửi qua IPC | `core/domain/src/view.rs` **và** `apps/desktop/ui/src/ipc/types.ts` | Sửa cả hai; test `view::tests` khoá JSON |
| Lệnh IPC mới | `apps/desktop/src-tauri/src/*_api.rs` + đăng ký trong `main.rs` + `Commands` trong `types.ts` | |
| Nguồn quota mới | `core/quota/src/lib.rs` | Trả về `QuotaSnapshot`; thêm fixture thật đã xoá thông tin cá nhân |
| Kết nối quota, Kết nối lại, Đăng nhập Claude/Codex | `core/orchestrator/src/live.rs` (`ProviderLink`, `reconnect`, `open_login`) | Đăng nhập chỉ mở lệnh login của CLI trong cửa sổ console; app không đọc hay lưu thông tin đăng nhập |
| Đọc issue GitHub qua Chrome ("Lấy issue") | `core/browser/src/github.rs` (script đọc trang, `strip_mentions`), `core/browser/src/cdp.rs`, `core/orchestrator/src/issues.rs` | Chỉ mở `github.com/<org>/<repo>/issues/<n>`; không lấy tên người; thử thật bằng test `live_read_issues` (cần bấm Allow) |
| Xoá nhánh local / trên origin | `core/vcs/src/lib.rs` (`delete_local_branch`, `delete_remote_branch`, `PROTECTED_BRANCHES`), `start::delete_branch` | `delete_remote_branch` là lần ghi duy nhất lên remote; chỉ gọi khi người dùng bấm và gõ tên nhánh. Test chạy với một bare repo tạm |
| Thư mục brief (`tasksDir`) | `core/orchestrator/src/folders.rs`, `AppService::set_tasks_dir` | Ghi thử trước khi chuyển; không ghi đè thư mục trùng tên |
| Cách gọi `claude` / `codex` | `core/agents/src/claude.rs` | Tham số dạng mảng, prompt qua stdin, luôn kèm `DISALLOWED_GIT` cho bước sửa code |
| Chặn commit/push | `core/vcs/src/guard.rs` | Test `guard_blocks_commit_and_push_but_not_fetch` phải luôn xanh |
| Nội dung AI được đọc, redact | `core/context/src/lib.rs` | |
| Schema SQLite | `core/storage/src/lib.rs` (`MIGRATIONS`) | Chỉ thêm migration mới, không sửa migration cũ |
| Config mặc định, đường dẫn | `core/config/src/lib.rs` | Section sai → về mặc định + cảnh báo |
| Dữ liệu demo | `core/orchestrator/src/demo.rs` (Tauri), `apps/desktop/ui/src/ipc/mock.ts` (trình duyệt) | Giữ hai bên giống nhau |

## Quy ước

- Mỗi crate: `XError` + `XResult<T>`; ở ranh giới UI trả `ApiError { code, message }`.
- Hằng số có tên, không magic number. Code riêng hệ điều hành nằm sau `#[cfg(target_os = ...)]`.
- Test tích hợp dùng `tools/fake-agent` thay cho CLI thật để không tốn token.
