# Muốn đổi X thì sửa ở đâu

| Muốn đổi | Sửa ở | Ghi chú |
|---|---|---|
| Thứ tự bước, cổng duyệt, số vòng review | `core/domain/src/workflow.rs` | Có test cho từng nhánh; thêm test khi đổi luật |
| Mức lỗi nào chặn / hỏi / chỉ hiện | `core/domain/src/finding.rs` (`GatePolicy`), config `gate` | |
| Mẫu tên branch | `core/domain/src/task.rs`, config `projects.*.branch` | App không đổi trạng thái OpenProject; bạn tự đổi |
| Field gửi qua IPC | `core/domain/src/view.rs` **và** `apps/desktop/ui/src/ipc/types.ts` | Sửa cả hai; test `view::tests` khoá JSON |
| Lệnh IPC mới | `apps/desktop/src-tauri/src/*_api.rs` + đăng ký trong `main.rs` + `Commands` trong `types.ts` | |
| Nguồn quota mới | `core/quota/src/lib.rs` | Trả về `QuotaSnapshot`; thêm fixture thật đã xoá thông tin cá nhân |
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
