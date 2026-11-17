# op-workflow

App desktop (Tauri v2) điều phối Claude Code và Codex theo luồng 12 bước: nhận task OpenProject → phân tích → bạn duyệt plan → code → kiểm tra → harness → review và sửa lỗi bạn chọn → test trình duyệt → review chốt → Push summary. App chạy trên máy bạn, **không bao giờ tự commit hay push**.

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

**Luồng 12 bước** chạy trong app. AI dùng tiến trình `claude -p` / `codex exec` chạy nền; terminal tương tác còn ở Phase 3. Kiểm tra dùng lệnh của app; Harness và Test trình duyệt bật/tắt theo task ở cổng Duyệt plan. Cột AI là mặc định; mỗi bước (trừ Test trình duyệt) đổi sang Claude hoặc Codex được ở Cài đặt → *Các bước AI*:

| Bước | AI | Làm gì |
|---|---|---|
| Phân tích | Claude, chỉ đọc (`--restricted`, Read/Grep/Glob) | Đọc brief và code, ghi `plan.md`. Dừng ở cổng **Duyệt plan**: *Duyệt, AI viết code* / *Góp ý* (phân tích lại) / *Tự code* (bạn code, xong bấm *Tôi code xong*) |
| Kiểm tra | App chạy lệnh | Sau Viết code và trong mỗi lần Sửa lỗi; typecheck / lint / prettier theo package hoặc `repos.<repo>.checks`. Chỉ tính lỗi thuộc file task đổi; AI của bước Sửa lỗi sửa lỗi kiểm tra tối đa 2 vòng |
| Harness | Claude dựng, app chạy | Test ngoài repo, có negative control và PASS / FAIL; app chạy lại sau mỗi lần sửa; thư viện riêng cho từng repo |
| Viết code, Sửa lỗi | Claude, tiếp phiên của bước Phân tích | Sửa file trong working tree. Claude: Bash chỉ chạy lệnh trong allowlist (`repos.<repo>.allowBash` và `.claude/settings.json` của repo); `git commit/push/merge/rebase/reset/clean/tag` bị chặn. Codex: `--sandbox workspace-write`, tiếp phiên Codex trước đó (`codex exec resume`); AI khác với AI đã viết code thì bắt đầu phiên mới, nhận plan + brief |
| Tự review, Review chốt | Claude | `/review-local` của repo nếu có (`.claude/commands/review-local.md`), không thì prompt review của app (dải trạng thái ghi rõ cái nào đang chạy); kết quả về dạng lỗi review, viết bằng tiếng Việt. Phần reviewer đánh giá có cần cập nhật docs / test hiện ở cuối tab *Lỗi review* |
| Review chéo | AI khác với Viết code (mặc định Codex `--sandbox read-only`) | Review diff so với base; lỗi chờ bạn chọn. Sau Sửa lỗi bạn quyết có review lại không, không giới hạn số vòng |
| Test trình duyệt | Claude + Chrome DevTools MCP | Chạy trước Review chốt, để lần review cuối xem cả phần sửa sau test. Thử kịch bản trong plan bằng Chrome riêng, ghi OK / NG và ảnh; NG chờ bạn chọn có sửa không. Sửa lỗi của Review chốt xong có thể *Chạy lại Test trình duyệt* rồi Review chốt lại |
| Push summary | Claude | `/push-summary` của repo nếu có, không thì prompt của app: commit message + mô tả PR (giữ ngôn ngữ repo yêu cầu). App không push. Nút *Copy commit* / *Copy PR* trên thẻ và trong cửa sổ task copy thẳng vào clipboard |

Bấm **Mở task** để xem cửa sổ task: các bước, nút cho bước kế tiếp, và các tab *Brief* (sửa được trước khi AI đọc), *Comment*, *Plan*, *Lỗi review* (mỗi lỗi: Sửa / Chấp nhận + lý do / Để sau), *Diff*, *Tiến trình*, *Chat*, *Push summary*, *Log*. Dưới tên mỗi bước là model · effort của lần chạy gần nhất. Mặc định app **dừng sau mỗi bước** để bạn xem kết quả rồi bấm chạy bước tiếp; bật *Tự chạy* trong cửa sổ task (công tắc riêng của từng task; mặc định lấy từ `"steps": { "autoAdvance": true }`) thì app chạy liền tới chỗ cần bạn quyết, rồi dừng và báo (chuông + thông báo Windows). Mỗi lần review (Tự review, Review chéo, Review chốt) báo lỗi, app dừng để bạn chọn lỗi nào cho AI sửa (mỗi lỗi ghi nguồn: /review-local, prompt của app hay Codex, bước và vòng); app không tự sửa lỗi nào chỉ vì mức độ. Sau mỗi lần Sửa lỗi app hỏi *Review chéo lại?*: có thì Review chéo thêm một vòng, không thì sang Review chốt (chạy lại Review chốt / Test trình duyệt nếu vừa sửa lỗi của bước đó), hoặc chọn thêm lỗi để sửa. Không giới hạn số vòng. Bước lỗi có nút *Thử lại*, và *Đăng nhập* khi Claude/Codex bị đăng xuất; *Dừng* kill cả cây tiến trình.

An toàn: trước và sau mỗi bước app so HEAD, nhánh và ref; bước chỉ đọc mà working tree đổi thì app dừng. Giới hạn chi phí và thời gian đặt ở `steps.<bước>.maxBudgetUsd` / `timeoutMin`. Sau bước 12 bạn tự review diff, commit, push rồi bấm *Đã push xong*. `plan.md`, `push.md`, kết quả Codex nằm cạnh brief; log AI ở `<dữ liệu>/runs` (xoá sau 14 ngày). App dọn tiến trình con và cổng AI để lại sau mỗi lần chạy. Bấm X hỏi **Thu xuống khay** (AI tiếp tục) hay **Thoát hẳn** (dừng AI cùng cây tiến trình), có ô *Nhớ lựa chọn*; đổi ở Cài đặt → Thông báo (`onClose`).

**Hạn mức trước bước AI:** khi cửa sổ 5 giờ hoặc tuần vượt `quota.warnAt` (mặc định 80 / 95%), cạnh nút chạy hiện provider, % đã dùng, giờ reset và chi phí lần chạy có số liệu gần nhất của bước đó trên cùng provider. Mức thấp chỉ nhắc; mức cao nhất dừng Tự chạy trước bước AI tiếp theo, bạn vẫn bấm Chạy được. Lệnh Kiểm tra và chạy lại harness không bị chặn. Hết quota giữa bước được ghi thành `quota.hit`; Thử lại giữ đúng bước và phiên code đang dở. Sau reset, task bật Tự chạy tiếp tục, task tắt Tự chạy nhận chuông/toast một lần; chưa biết giờ reset thì chờ gauge có chỗ trống. Lịch chờ được lưu qua lần mở app lại, chỉ áp dụng task đang hoạt động.

**AI, model, effort và quyền theo bước:** Cài đặt → *Các bước AI*: mỗi bước chọn Claude hoặc Codex (Test trình duyệt giữ Claude tới khi Codex qua một lần chạy thử thật), rồi model, effort và chế độ quyền trong danh sách CLI trên máy đưa ra (model đọc qua `initialize` / `model/list`, chế độ quyền đọc từ `claude --help` và `codex exec --help`; không tốn token). Mặc định *Theo máy* / *Theo app*: app không truyền cờ model / effort (riêng Push summary của Claude vẫn `--effort low`) và giữ cờ quyền như trước (Claude `acceptEdits` + allowlist ở bước viết code; Codex `workspace-write` ở bước viết code, `read-only` ở bước còn lại). Chọn rồi thì app truyền cờ rõ ràng: Claude `--model` / `--effort` / `--permission-mode`, Codex `-m` / `-c model_reasoning_effort=…` / `--sandbox` (hoặc `--approve-for-me`, `--dangerously-bypass-approvals-and-sandbox`); lưu ở `steps.<bước>.agent` / `model` / `effort` / `permissionMode` trong `config.json`. Ở mọi chế độ app vẫn chặn commit / push bằng git guard và so HEAD trước-sau bước. Codex ở Tự review / Review chốt / Push summary dùng prompt của app (không chạy `/review-local`, `/push-summary` của repo) và không báo chi phí USD. Cạnh mỗi bước có chi phí và thời gian trung bình của các lần chạy trước.

**Comment:** tab *Comment* đọc comment của task trên OpenProject lúc mở tab và issue GitHub của lần *Lấy issue* gần nhất (*Lấy lại issue* đọc lại qua Chrome). Comment của người khác viết sau lần xem trước có dấu *cần xem*. Comment của bạn (OpenProject: tác giả trùng chủ token, `users/me`; GitHub: cờ `viewerDidAuthor` của trang, không lấy tên) thu gọn một dòng, bấm để đọc hết, và không bao giờ tính là mới hay báo chuông. Mỗi comment có nút *Mở trên OpenProject* (mở đúng comment đó trong trình duyệt; comment issue thì *Mở trên GitHub*, mở trang issue), nhãn *OpenProject #id* ở đầu tab mở task. App chỉ mở link do chính nó dựng cho task đó. App không dịch comment. Có comment của người khác mới hơn phần comment trong brief thì *Cập nhật brief* ghi lại riêng mục đó (đã che email, token), phần bạn sửa ở chỗ khác giữ nguyên. Thẻ task hiện "N comment mới" (chỉ đếm comment của người khác); bấm vào là mở tab này. Task ở Inbox (chưa bắt đầu) thì bấm "N comment mới" mở hộp *Comment* ngay trên bảng (cũng có link mở từng comment), không cần bắt đầu task; dòng chuông của task Inbox cũng mở hộp này. Mỗi thẻ hiện độ ưu tiên của task đúng như trên OpenProject (tên và màu admin đặt; không có màu thì chữ trơn). Bấm mã task trên thẻ (hoặc ở đầu cửa sổ task) để mở task trên OpenProject.

**Diff:** tab *Diff* hiện thay đổi của task so với chỗ nhánh tách khỏi base (`git merge-base`), gồm cả phần chưa commit và file chưa add; xem gộp hoặc hai cột, lỗi review hiện ngay dưới dòng của nó, và báo "N file đổi từ lần xem trước". Chỉ đọc: app không ghi gì vào repo (kể cả index). Repo đang ở nhánh khác thì chỉ hiện phần đã commit trên nhánh của task.

**Thông báo:** chuông trên thanh trên ghi lại comment mới của người khác trên OpenProject (comment của bạn không báo), task chuyển *Test failed* / *Reopened*, bước AI xong / chờ bạn / lỗi, và hạn mức vượt 80% / 95% (`quota.warnAt`); bấm một dòng là mở đúng tab của task; có ba mục *Chưa đọc* / *Tất cả* / *Đã xoá*, nút *Đánh dấu đã đọc hết*, xoá từng dòng / cả danh sách và khôi phục. Thông báo đã xoá không tính chưa đọc, không phát lại toast và tự xoá vĩnh viễn sau 30 ngày kể từ lúc xoá (giữ qua restart); mục *Đã xoá* giữ tối đa 500, quá thì những thông báo xoá lâu nhất bị xoá vĩnh viễn trước. Cảnh báo quota chỉ nhắc mỗi mức một lần trong kỳ reset, giữ mức cao nhất khi % / nguồn đọc dao động; các cửa sổ cùng provider trong một lần đọc được gộp lại. Mỗi loại có thông báo Windows riêng, bật/tắt ở Cài đặt → Thông báo (`notify.comments` / `steps` / `quota`). App đọc lại comment theo nhịp lấy task (`tracker.pollMin`, mặc định 5 phút) và chỉ đọc task có `updatedAt` đổi. Issue GitHub không được đọc tự động, vì mỗi lần đọc Chrome đều hỏi Allow.

**Khay hệ thống:** icon ở khay hiện hạn mức 5 giờ còn lại (vòng ngoài Claude, vòng trong Codex, đỏ khi dưới 10%), rê chuột xem cả 4 cửa sổ, bấm để mở Hạn mức; menu chuột phải có *Đọc lại hạn mức* và *Thoát*.

**Thư mục brief:** mặc định nằm trong thư mục dữ liệu (`~/.op-workflow/tasks`). Lần đầu app hỏi có chuyển ra `Desktop/op-workflow-tasks` cho dễ tìm không; đổi lại lúc nào cũng được ở Cài đặt → Giới thiệu → *Thư mục brief* (app chuyển các thư mục task cũ sang, lưu vào `tasksDir` trong `config.json`).

**Đọc issue GitHub** gắn trong task (nút *Lấy issue* trên thẻ, và tự chạy sau *Bắt đầu*; lấy xong thẻ ghi "Đã lấy N issue · giờ", lỗi thì ghi đỏ, nút thành *Lấy lại*): app nối vào Chrome bạn đang dùng (đã đăng nhập GitHub), mở issue trong một tab nền, chỉ lấy tiêu đề, nội dung và comment rồi đóng tab, ghi vào mục *Từ issue* của brief. Không token, không `gh`, không bấm hay sửa gì trên GitHub; tên người và `@mention` bị bỏ. Cần Chrome 144 trở lên và bật một lần ở `chrome://inspect/#remote-debugging`; mỗi lần đọc, Chrome hỏi **Allow**.

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
  domain                task, quy trình 12 bước (state machine), finding, DTO
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
  orchestrator          ghép các crate; flow.rs chạy bước 2–12 và các cổng quyết định
tools/opw-hook          hook + statusline bridge, chỉ ghi số liệu
tools/fake-agent        phát lại stream-json để test không tốn token
```

Font giao diện: Inter và JetBrains Mono (giấy phép OFL-1.1, gói `@fontsource-variable/*`), đóng gói kèm app, có bộ ký tự tiếng Việt; app không tải font từ mạng.

Dữ liệu và cấu hình nằm ở `~/.op-workflow/` (bản debug dùng `~/.op-workflow-dev/`). Token OpenProject lưu trong Windows Credential Manager, không nằm trong repo.
