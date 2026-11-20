// Browser preview of the Workspace: sessions that answer a little later, write their answer
// word by word and ask for a permission, a choice or a plan approval when the message hints at
// one ("chạy" / "npm", "hỏi", "plan"). Nothing here runs an AI.
import type {
  AskAnswer,
  ChatItem,
  NewSession,
  SessionAsk,
  SessionChat,
  SessionPatch,
  SessionView,
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
  {
    wpId: 188002,
    taskId: "WP-188002",
    title: "Thêm bộ lọc theo ngày ở trang báo cáo",
    project: "Web Platform",
    repo: REPO_WEB,
    branch: "feature/188002",
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
  }),
  [
    { kind: "user", text: "Đọc brief.md rồi tìm nguyên nhân đơn hàng không hiện khi user chưa có tổ chức." },
    { kind: "text", text: "Mình đọc brief trước, rồi tìm chỗ lọc đơn hàng theo tổ chức." },
    { kind: "tool", tool: "Read", text: "brief.md", output: "# [WP-187716] Đơn hàng không hiện…" },
    { kind: "tool", tool: "Grep", text: "orgId  ·  src/orders", output: "src/orders/order.repository.ts:42" },
    {
      kind: "text",
      text: "Nguyên nhân: `order.repository.ts:42` lọc `orgId = user.orgId`, mà user chưa có tổ chức thì `orgId` là `null`, nên câu truy vấn không ra dòng nào.\n\nĐề xuất: khi `orgId` rỗng thì lọc theo `userId`.",
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
  return structuredClone({ tasks, sessions: list });
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

export function mockSessionOpenVscode(id: string): null {
  const s = get(id);
  s.view = { ...s.view, live: false, state: "outside", outside: "VS Code" };
  push(s, s.items.length);
  return null;
}

export function mockSessionHide(id: string): null {
  sessions.delete(id);
  return null;
}
