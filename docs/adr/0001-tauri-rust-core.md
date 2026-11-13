# ADR 0001: Tauri v2 + core Rust tách khỏi UI

- Trạng thái: chấp nhận (2026-11-13)

## Bối cảnh

Cần app desktop trên Windows, nhẹ, chạy local, có terminal tương tác và đọc file của Claude Code / Codex. Người dùng chọn Tauri; dự án dùng pnpm + TypeScript cho UI.

## Quyết định

- Tauri v2, UI React + TypeScript (pnpm workspace), logic trong workspace Rust `core/`.
- `core/` không phụ thuộc Tauri (kiểm tra bằng `scripts/check-core-deps.*`), để có thể chạy core thành tiến trình riêng nếu ConPTY gây crash, hoặc dựng CLI dev.
- IPC bằng Tauri command + Channel, không mở port.
- Học cấu trúc từ repo look (GPL-3.0): chỉ học quy ước, tự viết code. Học thiết kế từ Orrery (không có license): không chép code. Có thể dùng lại code OpenQuota (MIT) kèm ghi nguồn.

## Hệ quả

- Build lần đầu lâu (Tauri ~400 crate). CI chạy trên `windows-latest`.
- Hợp đồng IPC viết tay ở hai phía cho tới khi sinh tự động (ts-rs/specta) ở phase sau.
