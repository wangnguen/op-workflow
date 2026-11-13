# ADR 0002: App và AI không bao giờ tự commit hay push

- Trạng thái: chấp nhận (2026-11-13)

## Bối cảnh

Người dùng review mọi thứ trước khi rời máy. Ở một số repo, một tiền tố đặc biệt trong commit message còn kích hoạt deploy. Quy tắc của người dùng: không bao giờ tự push code, branch mới tạo cũng không push; chỉ push khi người dùng bảo.

## Quyết định

1. `opw-vcs` không có hàm commit hay push. Branch tạo bằng `switch -c <name> --no-track <base>`, chỉ ở máy.
2. Mọi tiến trình Claude/Codex do app chạy nhận môi trường từ `guard_env()`:
   - `url.no-push://…/.pushInsteadOf` cho `https://`, `http://`, `ssh://`, `git://`, `git@`, `file://` → mọi push lỗi, kể cả `--no-verify`; fetch vẫn chạy.
   - `core.hooksPath` trỏ tới hook của app: `pre-commit` và `pre-push` luôn từ chối.
   - Không ghi gì vào `.git/config` của repo.
3. Claude còn bị `--disallowedTools` chặn `git push/commit/merge/rebase/reset/clean/tag` và `gh pr`.
4. Sau mỗi bước, app so `HEAD` và danh sách nhánh/remote trước–sau; có thay đổi lạ thì dừng và báo.
5. Pane `shell` không bị chặn: người dùng tự push ở đó khi muốn.

## Kiểm chứng

Test `core/vcs/src/guard.rs::guard_blocks_commit_and_push_but_not_fetch` dựng repo tạm và kiểm tra commit, push nhánh mới, push `--no-verify` đều lỗi, fetch chạy, remote không đổi.
