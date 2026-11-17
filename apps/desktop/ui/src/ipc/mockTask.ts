// Browser preview data for the Comment and Diff tabs, the bell and the notification switches.
// Example names only (the repo is public).
import type {
  AlertView,
  CommentView,
  CommentsView,
  DiffFile,
  DiffLine,
  DiffView,
  IssueThread,
  NotifyKind,
  NotifySettings,
} from "./types";

const iso = (minutesAgo: number) => new Date(Date.now() - minutesAgo * 60_000).toISOString();

// ---------- comments ----------

interface MockComments {
  /** OpenProject comment ids the user has seen */
  seen: number;
  /** issue comments seen (by index of ISSUE_COMMENTS) */
  issueSeen: number;
  briefStale: boolean;
}

const commentState = new Map<number, MockComments>();
const stateOf = (wpId: number): MockComments => {
  let s = commentState.get(wpId);
  if (!s) {
    s = { seen: 2, issueSeen: 1, briefStale: true };
    commentState.set(wpId, s);
  }
  return s;
};

/** Oldest first; `mine` = written by the user (shown folded, never new). */
const OP_COMMENTS = [
  { id: 1, minutes: 60 * 26, author: "Tester A", text: "Bấm Lưu đơn khi chưa chọn tổ chức thì trang trắng." },
  { id: 2, minutes: 60 * 20, author: "PM B", text: "Ưu tiên cao, khách đang chờ bản sửa trước thứ Sáu." },
  {
    id: 3,
    minutes: 60 * 2,
    author: "Dev C",
    mine: true,
    text: "Đã sửa: kiểm tra tổ chức trống trước khi lưu, thêm thông báo lỗi.\nNhờ chị test lại trên staging giúp em. Bản build 2026.11.17-1.",
  },
  {
    id: 4,
    minutes: 35,
    author: "Tester A",
    text: "Test failed trên staging: vẫn trắng trang khi user mới tạo tài khoản.\nLog: TypeError: Cannot read properties of null (reading 'id')",
  },
];

/** Oldest first. */
const ISSUE_COMMENTS = [
  { minutes: 60 * 50, text: "Steps: create account → open Orders → Save. Page goes blank." },
  { minutes: 60 * 4, mine: true, text: "Thanks, looking into it. Which browser version are you on?" },
  { minutes: 60 * 3, text: "Same on mobile Safari. Attaching console log in the next comment." },
];

/** Other people's new OpenProject + issue comments of a task in the preview (the card badge). */
export function mockNewComments(wpId: number): number {
  const s = stateOf(wpId);
  return (
    OP_COMMENTS.filter((c) => !c.mine && c.id > s.seen).length +
    ISSUE_COMMENTS.filter((c, i) => !c.mine && i >= s.issueSeen).length
  );
}

export function mockTaskComments(wpId: number, markSeen: boolean): CommentsView {
  const s = stateOf(wpId);
  const taskUrl = `https://openproject.example.com/work_packages/${wpId}`;
  const comments: CommentView[] = [...OP_COMMENTS].reverse().map((c) => ({
    id: String(c.id),
    createdAt: iso(c.minutes),
    author: c.author,
    text: c.text,
    mine: c.mine === true,
    isNew: c.mine !== true && c.id > s.seen,
    url: `${taskUrl}/activity#activity-${c.id + 1}`,
  }));
  const issueUrl = "https://github.com/acme/web/issues/456";
  const issue: IssueThread = {
    url: issueUrl,
    title: "Orders: blank page after saving",
    comments: ISSUE_COMMENTS.map((c, i) => ({
      id: `issue#${i}`,
      createdAt: iso(c.minutes),
      text: c.text,
      mine: c.mine === true,
      isNew: c.mine !== true && i >= s.issueSeen,
      url: issueUrl,
    })).reverse(),
    unread: 1,
  };
  const view: CommentsView = {
    comments,
    taskUrl,
    issues: [issue],
    issuesReadAt: iso(5),
    briefStale: s.briefStale,
    fetchedAt: new Date().toISOString(),
  };
  if (markSeen) {
    s.seen = OP_COMMENTS.length;
    s.issueSeen = ISSUE_COMMENTS.length;
  }
  return view;
}

export function mockRefreshBriefComments(wpId: number): CommentsView {
  stateOf(wpId).briefStale = false;
  return mockTaskComments(wpId, true);
}

// ---------- diff ----------

const ctx = (old: number, nw: number, text: string): DiffLine => ({ kind: "context", old, new: nw, text });
const add = (nw: number, text: string): DiffLine => ({ kind: "added", new: nw, text });
const del = (old: number, text: string): DiffLine => ({ kind: "removed", old, text });

const FILES: DiffFile[] = [
  {
    path: "api/src/routes/files.ts",
    change: "modified",
    binary: false,
    untracked: false,
    added: 6,
    removed: 2,
    truncated: false,
    changed: false,
    hunks: [
      {
        section: "export async function listFiles(req: Request) {",
        lines: [
          ctx(38, 38, "  const user = await users.get(req.userId);"),
          ctx(39, 39, "  if (!user) return notFound();"),
          del(40, "  const orgId = user.org.id;"),
          add(40, "  const org = user.org;"),
          add(41, "  if (!org) {"),
          add(42, "    return ok([]);"),
          add(43, "  }"),
          add(44, "  const orgId = org.id;"),
          ctx(41, 45, "  const files = await db.files.byOrg(orgId);"),
          ctx(42, 46, "  return ok(files);"),
          ctx(43, 47, "}"),
        ],
      },
      {
        section: "function toView(f: FileRow) {",
        lines: [
          ctx(49, 53, "function toView(f: FileRow) {"),
          ctx(50, 54, "  return {"),
          del(51, "    name: f.name,"),
          add(55, "    name: f.name.trim(),"),
          ctx(52, 56, "    size: f.size,"),
        ],
      },
    ],
  },
  {
    path: "api/src/lib/util.ts",
    change: "modified",
    binary: false,
    untracked: false,
    added: 1,
    removed: 1,
    truncated: false,
    changed: false,
    hunks: [
      {
        section: "",
        lines: [ctx(9, 9, "// shared helpers"), del(10, "export const x = (a: string) => a.trim();"), add(10, "export const tidy = (a: string) => a.trim();")],
      },
    ],
  },
  {
    path: "api/test/files.test.ts",
    change: "added",
    binary: false,
    untracked: true,
    added: 7,
    removed: 0,
    truncated: false,
    changed: false,
    hunks: [
      {
        section: "",
        lines: [
          add(1, "import { listFiles } from \"../src/routes/files\";"),
          add(2, ""),
          add(3, "test(\"user without org gets an empty list\", async () => {"),
          add(4, "  const res = await listFiles(fakeRequest({ org: null }));"),
          add(5, "  expect(res.status).toBe(200);"),
          add(6, "  expect(res.body).toEqual([]);"),
          add(7, "});"),
        ],
      },
    ],
  },
  {
    path: "web/public/empty-state.png",
    change: "added",
    binary: true,
    untracked: true,
    added: 0,
    removed: 0,
    truncated: false,
    changed: false,
    hunks: [],
  },
];

const diffSeen = new Set<number>();

export function mockTaskDiff(wpId: number, markSeen: boolean): DiffView {
  const seenBefore = diffSeen.has(wpId);
  // the second look pretends the AI touched files.ts in between
  const files = FILES.map((f, i) => ({ ...f, changed: seenBefore && i === 0 }));
  if (markSeen) diffSeen.add(wpId);
  return {
    base: "origin/development",
    forkPoint: "3f9c2a1b",
    files,
    added: files.reduce((n, f) => n + f.added, 0),
    removed: files.reduce((n, f) => n + f.removed, 0),
    changedSinceSeen: files.filter((f) => f.changed).length,
    seenBefore,
    truncated: false,
  };
}

// ---------- bell ----------

let alerts: AlertView[] = [
  {
    id: 3,
    at: iso(2),
    kind: "gate",
    wpId: 187716,
    taskId: "WP-187716",
    title: "WP-187716 · plan chờ bạn duyệt",
    body: "Không lưu được đơn khi user chưa có tổ chức",
    tab: "plan",
    read: false,
  },
  {
    id: 2,
    at: iso(35),
    kind: "comment",
    wpId: 187716,
    taskId: "WP-187716",
    title: "WP-187716 · 1 comment mới",
    body: "Tester A: Test failed trên staging: vẫn trắng trang khi user mới tạo tài khoản.",
    tab: "comments",
    read: false,
  },
  {
    id: 1,
    at: iso(80),
    kind: "quota",
    title: "Claude · tuần đã dùng 88%",
    body: "Còn khoảng 12% tới lần reset. Mở Hạn mức để xem giờ reset.",
    read: true,
  },
];

const DELETED_RETENTION_MS = 30 * 24 * 60 * 60 * 1000;

export function mockAlerts(): AlertView[] {
  alerts = alerts.filter((a) => {
    const age = a.deletedAt ? Date.now() - Date.parse(a.deletedAt) : NaN;
    return !Number.isFinite(age) || age < DELETED_RETENTION_MS;
  });
  return alerts.map((a) => ({ ...a }));
}
export const mockUnreadAlerts = (): number => mockAlerts().filter((a) => !a.read && !a.deletedAt).length;

export function mockMarkAlertsRead(ids: number[]): null {
  alerts = alerts.map((a) => (!a.deletedAt && (ids.length === 0 || ids.includes(a.id)) ? { ...a, read: true } : a));
  return null;
}

export function mockDeleteAlerts(ids: number[]): AlertView[] {
  const at = new Date().toISOString();
  alerts = mockAlerts().map((a) => !a.deletedAt && (ids.length === 0 || ids.includes(a.id))
    ? { ...a, deletedAt: at, read: true } : a);
  return mockAlerts();
}

export function mockRestoreAlerts(ids: number[]): AlertView[] {
  alerts = mockAlerts().map((a) => a.deletedAt && (ids.length === 0 || ids.includes(a.id))
    ? { ...a, deletedAt: undefined, read: true } : a);
  return mockAlerts();
}

// ---------- notification switches ----------

let notify: NotifySettings = { comments: true, steps: true, quota: true };
export const mockNotify = (): NotifySettings => notify;
export function mockSetNotify(kind: NotifyKind, on: boolean): NotifySettings {
  notify = { ...notify, [kind]: on };
  return notify;
}
