// Browser preview of the Workspace: sessions that answer a little later, write their answer
// word by word and ask for a permission, a choice or a plan approval when the message hints at
// one ("chạy" / "npm", "hỏi", "plan"). Nothing here runs an AI.
import type {
  ArchivedSession,
  AskAnswer,
  ChatItem,
  NewSession,
  OutsideChat,
  OutsideSession,
  SessionAsk,
  SessionChat,
  SessionPatch,
  SessionView,
  TaskOutput,
  WorkspaceSnapshot,
  WorkspaceTask,
} from "./types";

const REPO_WEB = "C:/Users/dev/Desktop/acme-web";
const REPO_API = "C:/Users/dev/Desktop/acme-api";

const tasks: WorkspaceTask[] = [
  {
    wpId: 187716,
    taskId: "WP-187716",
    title: "Đơn hàng không hiện khi user chưa có tổ chức",
    project: "Web Platform",
    repo: REPO_WEB,
    branch: "feature/187716",
  },
  // the board's second task in progress (mock.ts SECOND)
  {
    wpId: 188003,
    taskId: "WP-188003",
    title: "Tối ưu truy vấn danh sách thư mục dùng chung",
    project: "Web Platform",
    repo: REPO_API,
    branch: "f/WP-188003",
  },
];

interface MockSession {
  view: SessionView;
  items: ChatItem[];
  partial: string;
  asks: SessionAsk[];
  seq: number;
  timers: number[];
}

const iso = (minutesAgo: number) => new Date(Date.now() - minutesAgo * 60_000).toISOString();

function session(id: string, part: Partial<SessionView>): SessionView {
  return {
    id,
    kind: "free",
    title: "Phiên mới",
    repo: REPO_WEB,
    state: "idle",
    live: false,
    costUsd: 0,
    started: false,
    createdAt: iso(30),
    updatedAt: iso(30),
    ...part,
  };
}

const sessions = new Map<string, MockSession>();

function add(view: SessionView, items: ChatItem[] = []) {
  sessions.set(view.id, { view, items, partial: "", asks: [], seq: 1, timers: [] });
}

add(
  session("9f1c2a40-0000-4000-8000-000000000001", {
    kind: "main",
    wpId: 187716,
    taskId: "WP-187716",
    title: "Tìm nguyên nhân đơn hàng không hiện",
    branch: "feature/187716",
    started: true,
    costUsd: 0.42,
    model: "claude-opus-5-5",
    mode: "default",
    updatedAt: iso(4),
    // `?bg`: a subagent and a dev server still running in the background
    ...(typeof location !== "undefined" && new URLSearchParams(location.search).has("bg")
      ? {
          live: true,
          state: "running" as const,
          background: [
            {
              id: "a7f3", kind: "agent", description: "Tìm mọi chỗ lọc đơn hàng theo tổ chức", toolUseId: "toolu_mock_agent",
              startedAt: iso(3), activity: "Đọc src/orders/order.repository.ts", toolUses: 2, tokens: 18_400,
            },
            { id: "b2k9", kind: "bash", description: "npm run dev", startedAt: iso(12) },
          ],
        }
      : {}),
  }),
  [
    {
      kind: "user",
      step: "analyze",
      text: "Đọc brief.md rồi tìm nguyên nhân đơn hàng không hiện khi user chưa có tổ chức.",
    },
    { kind: "text", text: "Mình đọc brief trước, rồi tìm chỗ lọc đơn hàng theo tổ chức." },
    { kind: "tool", tool: "Read", text: "brief.md", output: "# [WP-187716] Đơn hàng không hiện…" },
    {
      kind: "tool",
      tool: "Agent",
      toolId: "toolu_mock_agent",
      text: "Tìm mọi chỗ lọc đơn hàng theo tổ chức",
      output: "Chỉ có một chỗ lọc theo orgId: src/orders/order.repository.ts:42 (findForUser).",
      children: [
        { kind: "tool", tool: "Grep", text: "orgId  ·  src", output: "src/orders/order.repository.ts:42\nsrc/users/user.service.ts:18" },
        { kind: "tool", tool: "Read", text: "src/orders/order.repository.ts", output: "export async function findForUser(user) {…" },
        { kind: "text", text: "Chỉ `findForUser` lọc theo `orgId`; `user.service.ts` chỉ đọc nó." },
      ],
    },
    { kind: "tool", tool: "Grep", text: "orgId  ·  src/orders", output: "src/orders/order.repository.ts:42" },
    {
      kind: "text",
      text: "Nguyên nhân: `order.repository.ts:42` lọc `orgId = user.orgId`, mà user chưa có tổ chức thì `orgId` là `null`, nên câu truy vấn không ra dòng nào.\n\nĐề xuất: khi `orgId` rỗng thì lọc theo `userId`.",
    },
    {
      kind: "note",
      text: "Brief đổi sau khi bạn duyệt plan: thêm 1 comment trên OpenProject. Muốn AI xem lại thì nhắn trong chat hoặc chạy lại Phân tích.",
    },
  ],
);
add(
  session("9f1c2a40-0000-4000-8000-000000000002", {
    kind: "free",
    title: "Hỏi về cấu trúc thư mục api",
    repo: REPO_API,
    started: true,
    costUsd: 0.08,
    state: "outside",
    outside: "VS Code",
    updatedAt: iso(55),
  }),
  [
    { kind: "user", text: "Repo này chia module thế nào?" },
    { kind: "text", text: "Mỗi thư mục trong `src/` là một module: `orders`, `users`, `billing`…" },
  ],
);

type Listener = (p: SessionPatch) => void;
const listeners = new Set<Listener>();

/** Browser preview: patches the mock sessions push (the real app pushes `opw://session`). */
export function onMockSessionPatch(handler: Listener): () => void {
  listeners.add(handler);
  return () => listeners.delete(handler);
}

function state(s: MockSession): SessionView["state"] {
  if (s.asks.length > 0) return "waiting";
  if (s.view.state === "running") return "running";
  return s.view.outside ? "outside" : "idle";
}

function push(s: MockSession, from: number) {
  s.seq += 1;
  s.view = { ...s.view, state: state(s), updatedAt: new Date().toISOString() };
  const patch: SessionPatch = {
    id: s.view.id,
    seq: s.seq,
    from,
    items: s.items.slice(from),
    dropped: 0,
    partial: s.partial,
    asks: s.asks,
    session: s.view,
  };
  for (const l of listeners) l(structuredClone(patch));
}

function get(id: string): MockSession {
  const s = sessions.get(id);
  if (!s) throw { code: "session.missing", message: "Không còn phiên này" };
  return s;
}

function chatOf(s: MockSession): SessionChat {
  return structuredClone({
    session: { ...s.view, state: state(s) },
    items: s.items,
    dropped: 0,
    partial: s.partial,
    asks: s.asks,
    seq: s.seq,
  });
}

export function mockWorkspace(): WorkspaceSnapshot {
  const list = [...sessions.values()]
    .map((s) => ({ ...s.view, state: state(s) }))
    .sort((a, b) => b.updatedAt.localeCompare(a.updatedAt));
  // what the sessions table shows of each: the last thing it did
  for (const v of list) {
    const s = sessions.get(v.id);
    const last = s ? [...s.items].reverse().find((i) => i.kind === "text" || i.kind === "tool" || i.kind === "user") : undefined;
    if (last && !v.activity) {
      const line = last.text.split("\n").find((l) => l.trim()) ?? "";
      v.activity = last.kind === "tool" ? `${last.tool ?? "Tool"}: ${line}` : last.kind === "user" ? `Bạn: ${line}` : line;
    }
    if (s?.asks.length) v.activity = "Đang chờ bạn trả lời";
  }
  return structuredClone({ tasks, sessions: list, outside: mockOutside(), archived });
}

// ---------- "Đã lưu trữ" ----------

const archived: ArchivedSession[] = [
  {
    id: "9f1c2a40-0000-4000-8000-0000000000a1",
    title: "Thử cấu hình ESLint mới",
    repo: REPO_WEB,
    archivedAt: iso(60 * 26),
  },
];
const archivedSessions = new Map<string, MockSession>();
const archivedOutside = new Map<string, (typeof OUTSIDE)[number]>();

export function mockArchiveOutside(id: string): null {
  const i = OUTSIDE.findIndex((o) => o.view.id === id);
  const o = OUTSIDE[i];
  if (!o) throw { code: "session.closed", message: "Phiên này không còn mở ở ngoài app" };
  OUTSIDE.splice(i, 1);
  archivedOutside.set(id, o);
  archived.unshift({ id, title: o.view.title, repo: o.view.cwd, outside: true, archivedAt: new Date().toISOString() });
  return null;
}

export function mockRestoreSession(id: string): null {
  const i = archived.findIndex((a) => a.id === id);
  if (i < 0) throw { code: "session.missing", message: "Không còn phiên này trong mục Đã lưu trữ" };
  const [a] = archived.splice(i, 1);
  const s = archivedSessions.get(id);
  const o = archivedOutside.get(id);
  if (s) {
    sessions.set(id, s);
    archivedSessions.delete(id);
  } else if (o) {
    OUTSIDE.push(o);
    archivedOutside.delete(id);
  } else if (a) {
    add(session(id, { title: a.title, repo: a.repo, started: true, updatedAt: a.archivedAt }));
  }
  return null;
}

/** "Tiếp tục trong app": the outside session joins the app's list; still open in VS Code, so
 *  the app only shows it until it is closed there. */
export function mockAdoptOutside(id: string): SessionView {
  const i = OUTSIDE.findIndex((o) => o.view.id === id);
  const o = OUTSIDE[i];
  if (!o) throw { code: "session.closed", message: "Phiên này không còn mở ở ngoài app" };
  OUTSIDE.splice(i, 1);
  const view = session(id, {
    title: o.view.title,
    repo: o.view.cwd,
    started: true,
    state: "outside",
    outside: o.view.openIn,
    updatedAt: o.view.updatedAt ?? iso(1),
  });
  add(view, structuredClone(o.items));
  return structuredClone(view);
}

/** "Tách bản sao": a copy you can type into now; the original stays where it is open. */
export function mockForkOutside(id: string): SessionView {
  const o = OUTSIDE.find((x) => x.view.id === id);
  if (!o) throw { code: "session.closed", message: "Phiên này không còn mở ở ngoài app" };
  const copy = `9f1c2a40-0000-4000-8000-${String(counter++).padStart(12, "0")}`;
  const view = session(copy, {
    title: `${o.view.title} (bản sao)`,
    repo: o.view.cwd,
    forkOf: id,
    updatedAt: new Date().toISOString(),
  });
  add(view, structuredClone(o.items));
  return structuredClone(view);
}

// ---------- sessions opened outside the app (the map's far stars ext-1..3) ----------

const OUTSIDE: { view: OutsideSession; items: ChatItem[] }[] = [
  {
    view: {
      id: "ext-1",
      cwd: "C:/Users/dev/Desktop/acme-desktop",
      title: "Sửa lỗi build trên Windows",
      state: "idle",
      openIn: "VS Code",
      updatedAt: iso(12),
    },
    items: [
      { kind: "user", text: "Build trên Windows báo thiếu `link.exe`, xem giúp." },
      { kind: "tool", tool: "Bash", text: "cargo build", output: "error: linker `link.exe` not found" },
      {
        kind: "text",
        text: "Máy chưa có Build Tools của Visual Studio. Cài gói **Desktop development with C++** rồi build lại.",
      },
    ],
  },
  {
    view: {
      id: "ext-2",
      cwd: "C:/Users/dev/code/misfile-buster",
      title: "Viết test cho bộ phân loại file",
      state: "running",
      openIn: "terminal",
      updatedAt: iso(1),
    },
    items: [
      { kind: "user", text: "Viết test cho `classify()` với các đuôi file lạ." },
      { kind: "tool", tool: "Read", text: "src/classify.ts" },
      { kind: "text", text: "Mình thêm test cho `.tar.gz`, file không đuôi và tên có dấu chấm ở đầu." },
    ],
  },
  {
    view: {
      id: "ext-3",
      cwd: "C:/Users/dev/Desktop/study-workspace",
      title: "Tóm tắt chương 3",
      state: "waiting",
      openIn: "VS Code",
      updatedAt: iso(3),
    },
    items: [
      { kind: "user", text: "Tóm tắt chương 3 vào notes/ch3.md." },
      { kind: "text", text: "Mình sẽ ghi bản tóm tắt vào `notes/ch3.md`." },
    ],
  },
];

let outsideHidden = false;

/** Browser preview of Settings → "Hiện phiên mở ngoài app". */
export const mockOutsideHidden = (): boolean => outsideHidden;
export function mockSetOutsideHidden(hide: boolean): void {
  outsideHidden = hide;
}

/** Sessions open outside the app, newest first (none when hidden). */
export function mockOutside(): OutsideSession[] {
  if (outsideHidden) return [];
  return structuredClone(OUTSIDE.map((o) => o.view)).sort((a, b) =>
    (b.updatedAt ?? "").localeCompare(a.updatedAt ?? ""),
  );
}

export function mockOutsideChat(id: string): OutsideChat {
  const o = outsideHidden ? undefined : OUTSIDE.find((x) => x.view.id === id);
  if (!o) throw { code: "session.closed", message: "Phiên này không còn mở ở ngoài app" };
  return structuredClone({ session: o.view, items: o.items, dropped: 0 });
}

export const mockWorkspaceRepos = (): string[] => [REPO_WEB, REPO_API, "C:/Users/dev/Desktop/acme-docs"];

let counter = 3;

export function mockSessionNew(req: NewSession): SessionView {
  const task = tasks.find((t) => t.wpId === req.wpId);
  const id = `9f1c2a40-0000-4000-8000-${String(counter++).padStart(12, "0")}`;
  const view = session(id, {
    kind: task ? "main" : "free",
    wpId: task?.wpId,
    taskId: task?.taskId,
    branch: task?.branch,
    repo: task?.repo ?? req.repo ?? REPO_WEB,
    createdAt: new Date().toISOString(),
    updatedAt: new Date().toISOString(),
  });
  add(view);
  return structuredClone(view);
}

export const mockSessionChat = (id: string): SessionChat => chatOf(get(id));

/** A line from the app in the chat of the task's session (not sent to the AI); no session for
 *  the task = nothing. */
export function mockSessionNote(wpId: number, text: string): void {
  const s = [...sessions.values()].find((x) => x.view.kind === "main" && x.view.wpId === wpId);
  if (!s) return;
  s.items.push({ kind: "note", text });
  push(s, s.items.length - 1);
}

const ANSWER =
  "Mình đã xem chỗ này. Hàm `listOrders` nhận `orgId` có thể rỗng, nên mình thêm nhánh lọc theo `userId` khi `orgId` không có, và một test cho trường hợp đó.";

/** Writes `text` word by word into `partial`, then makes it an item. */
function stream(s: MockSession, text: string, then?: () => void) {
  const words = text.split(" ");
  let i = 0;
  const timer = window.setInterval(() => {
    i += 1;
    s.partial = words.slice(0, i).join(" ");
    if (i >= words.length) {
      window.clearInterval(timer);
      s.partial = "";
      s.items.push({ kind: "text", text });
      push(s, s.items.length - 1);
      then?.();
    } else {
      push(s, s.items.length);
    }
  }, 70);
  s.timers.push(timer);
}

function finish(s: MockSession) {
  s.view = { ...s.view, state: "idle", costUsd: Number((s.view.costUsd + 0.03).toFixed(2)) };
  push(s, s.items.length);
}

export function mockSessionSend(id: string, text: string): SessionView {
  const s = get(id);
  if (s.view.outside) throw { code: "session.outside", message: `Phiên đang mở ở ${s.view.outside}. Đóng phiên ở đó rồi gõ tiếp ở đây.` };
  s.items.push({ kind: "user", text });
  s.view = {
    ...s.view,
    state: "running",
    live: true,
    started: true,
    title: s.view.title === "Phiên mới" ? text.slice(0, 80) : s.view.title,
  };
  push(s, s.items.length - 1);
  const lower = text.toLowerCase();
  const later = window.setTimeout(() => {
    if (lower.includes("plan")) {
      stream(s, "Mình lập plan trước khi sửa.", () => {
        s.asks = [
          {
            id: "req-plan",
            kind: "plan",
            tool: "ExitPlanMode",
            summary: "",
            plan: "## Plan\n1. Thêm nhánh lọc theo `userId` khi `orgId` rỗng (`order.repository.ts:42`).\n2. Thêm test cho user chưa có tổ chức.\n3. Chạy `npm test`.",
          },
        ];
        push(s, s.items.length);
      });
    } else if (lower.includes("hỏi")) {
      s.asks = [
        {
          id: "req-question",
          kind: "question",
          tool: "AskUserQuestion",
          summary: "",
          questions: [
            {
              question: "Khi user chưa có tổ chức thì hiện đơn nào?",
              header: "Phạm vi",
              multiSelect: false,
              options: [
                { label: "Đơn của chính user", description: "Lọc theo userId" },
                { label: "Không hiện gì", description: "Giữ như bây giờ, chỉ thêm thông báo" },
              ],
            },
          ],
        },
      ];
      push(s, s.items.length);
    } else if (lower.includes("chạy") || lower.includes("npm") || lower.includes("test")) {
      s.items.push({ kind: "tool", tool: "Bash", text: "npm test -- orders" });
      s.asks = [
        {
          id: "req-bash",
          kind: "permission",
          tool: "Bash",
          summary: "npm test -- orders",
          detail: "Chạy test của module orders",
          always: "Bash(npm test:*) · trong phiên này",
        },
      ];
      push(s, s.items.length - 1);
    } else {
      stream(s, ANSWER, () => finish(s));
    }
  }, 600);
  s.timers.push(later);
  return structuredClone(s.view);
}

export function mockSessionAnswer(id: string, askId: string, answer: AskAnswer): SessionView {
  const s = get(id);
  const ask = s.asks.find((a) => a.id === askId);
  if (!ask) throw { code: "session.ask", message: "Câu hỏi này không còn chờ trả lời" };
  s.asks = [];
  if (ask.kind === "permission") {
    const tool = s.items[s.items.length - 1];
    if (answer.kind === "allow") {
      if (tool) tool.output = "PASS src/orders/order.test.ts (4 tests)";
      push(s, s.items.length - 1);
      stream(s, "Test đã qua.", () => finish(s));
    } else {
      if (tool) {
        tool.failed = true;
        tool.denied = answer.kind === "deny" && answer.message ? answer.message : "Bạn từ chối";
      }
      push(s, s.items.length - 1);
      finish(s);
    }
  } else if (ask.kind === "question" && answer.kind === "answers") {
    stream(s, `Mình làm theo lựa chọn của bạn: ${Object.values(answer.answers).join(", ")}.`, () => finish(s));
  } else if (ask.kind === "plan") {
    if (answer.kind === "approvePlan") {
      s.view = { ...s.view, mode: answer.mode ?? s.view.mode };
      stream(s, "Plan đã duyệt, mình bắt đầu sửa.", () => finish(s));
    } else {
      stream(s, "Mình lập plan khác.", () => finish(s));
    }
  } else {
    finish(s);
  }
  return structuredClone(s.view);
}

export function mockSessionInterrupt(id: string): null {
  const s = get(id);
  for (const t of s.timers) {
    window.clearInterval(t);
    window.clearTimeout(t);
  }
  s.timers = [];
  s.partial = "";
  s.asks = [];
  s.items.push({ kind: "end", text: "", tool: "interrupted" });
  s.view = { ...s.view, state: "idle" };
  push(s, s.items.length - 1);
  return null;
}

export function mockSessionSetMode(id: string, mode: string): SessionView {
  const s = get(id);
  s.view = { ...s.view, mode };
  push(s, s.items.length);
  return structuredClone(s.view);
}

export function mockSessionStop(id: string): SessionView {
  const s = get(id);
  mockSessionInterrupt(id);
  s.view = { ...s.view, live: false };
  push(s, s.items.length);
  return structuredClone(s.view);
}

export function mockSessionStopTask(id: string, taskId: string): SessionView {
  const s = get(id);
  s.view = { ...s.view, background: (s.view.background ?? []).filter((b) => b.id !== taskId) };
  if (s.view.background?.length === 0 && s.view.state === "running") s.view = { ...s.view, state: "idle" };
  push(s, s.items.length);
  return structuredClone(s.view);
}

export function mockTaskOutput(_id: string, taskId: string): TaskOutput {
  const lines = Array.from({ length: 6 + Math.floor((Date.now() / 1000) % 20) }, (_, i) => `  ✓ ${taskId} kiểm thử ${i + 1}`);
  return { text: ["> vite --port 5174", "", "  VITE v7.1  ready in 412 ms", "  ➜  Local: http://localhost:5174/", ...lines].join("\n"), cut: 0 };
}

export function mockSessionSetModel(id: string, model: string | undefined): SessionView {
  const s = get(id);
  s.view = { ...s.view, modelPick: model };
  push(s, s.items.length);
  return structuredClone(s.view);
}

export function mockSessionOpenVscode(id: string, force = false): null {
  const s = get(id);
  if (!force && (s.view.state === "running" || s.view.state === "waiting")) {
    throw {
      code: "session.running",
      message:
        "Phiên đang chạy trong app. Mở trong VS Code sẽ dừng lượt đang chạy (cả subagent chạy nền) ở đây; VS Code chạy tiếp từ chỗ đã lưu.",
    };
  }
  if (force) mockSessionInterrupt(id);
  s.items.push({
    kind: "note",
    text: "Đã chuyển phiên sang VS Code. Từ giờ phiên chạy ở đó, app chỉ hiện lại hội thoại. Đóng phiên ở VS Code rồi gõ tiếp ở đây để chạy lại trong app.",
  });
  s.view = { ...s.view, live: false, state: "outside", outside: "VS Code" };
  push(s, s.items.length - 1);
  return null;
}

export function mockSessionHide(id: string): null {
  const s = sessions.get(id);
  sessions.delete(id);
  if (s) {
    archivedSessions.set(id, s);
    archived.unshift({ id, title: s.view.title, repo: s.view.repo, taskId: s.view.taskId, archivedAt: new Date().toISOString() });
  }
  return null;
}
