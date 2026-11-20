import { describe, expect, it } from "vitest";
import { gateLabel } from "../../i18n/vi";
import { orbitForFrame } from "../../ipc/mock";
import type { OutsideSession, SessionView, WorkspaceSnapshot } from "../../ipc/types";
import { STEP_COUNT } from "../../shared/steps";
import { sessionTable, tally } from "./sessions";

const NOW = Date.parse("2026-11-13T08:00:00Z");

const view = (id: string, part: Partial<SessionView> = {}): SessionView => ({
  id,
  kind: "free",
  title: id,
  repo: "C:/code/web",
  state: "idle",
  live: false,
  costUsd: 0,
  started: true,
  createdAt: "2026-11-20T08:00:00Z",
  updatedAt: "2026-11-20T08:00:00Z",
  ...part,
});

const outside = (id: string, part: Partial<OutsideSession> = {}): OutsideSession => ({
  id,
  cwd: "C:\\code\\Web",
  title: id,
  state: "running",
  openIn: "VS Code",
  updatedAt: "2026-11-20T09:00:00Z",
  ...part,
});

const ws = (sessions: SessionView[], out: OutsideSession[] = []): WorkspaceSnapshot => ({ tasks: [], sessions, outside: out });
const rows = (t: ReturnType<typeof sessionTable>) => t.flatMap((g) => g.rows);

describe("sessionTable", () => {
  it("gives a task in progress without a session one row, with its step", () => {
    const snap = orbitForFrame(10, NOW);
    const tasks = [snap.active!, ...(snap.others ?? [])];
    const taskRows = rows(sessionTable(snap)).filter((r) => r.kind === "task");
    expect(taskRows.map((r) => r.taskId).sort()).toEqual(tasks.map((t) => t.taskId).sort());
    for (const r of taskRows) {
      const task = tasks.find((t) => t.taskId === r.taskId)!;
      expect(r).toMatchObject({ title: task.title, to: { to: "session", taskId: task.taskId } });
      if (task.agents[0] && !task.gate) expect(r.activity).toMatch(new RegExp(`\\d/${STEP_COUNT} `));
    }
  });

  it("puts a task's gate on its main session: Chờ bạn first in its repo", () => {
    const snap = orbitForFrame(2, NOW);
    const a = snap.active!;
    expect(a.gate).toBeDefined();
    // newest first, as Workspace lists them: "main" is the task's session now, "old" the one
    // before Làm lại từ đầu
    const table = sessionTable({ ...snap, others: [] }, ws([
      view("main", { kind: "main", wpId: 1, taskId: a.taskId, repo: `C:/code/${a.repo}` }),
      view("old", { kind: "main", wpId: 1, taskId: a.taskId, repo: `C:/code/${a.repo}`, state: "running" }),
    ]));
    const group = table.find((g) => g.name === a.repo)!;
    // the task has sessions, so no row of its own
    expect(group.rows.map((r) => r.id)).toEqual(["session:main", "session:old"]);
    expect(group.rows[0]).toMatchObject({ status: "waiting", asks: false });
    expect(group.rows[0]?.activity).toContain(gateLabel[a.gate!.kind]);
    expect(group.rows[1]?.status).toBe("running");
  });

  it("groups the app's sessions and those open outside it by repo, whatever the slashes or case", () => {
    const snap = { ...orbitForFrame(10, NOW), active: undefined, others: [], distant: [] };
    const table = sessionTable(snap, ws(
      [view("free-1", { state: "waiting", costUsd: 0.42, contextPercent: 31 }), view("api-1", { repo: "C:/code/api" })],
      [outside("ext-1")],
    ));
    expect(table.map((g) => g.name)).toEqual(["api", "web"]);
    const web = table.find((g) => g.name === "web")!;
    expect(web.rows.map((r) => r.id)).toEqual(["session:free-1", "outside:ext-1"]);
    expect(web.rows[0]).toMatchObject({ to: { to: "chat", sessionId: "free-1", ask: true }, asks: true, costUsd: 0.42, contextPct: 31 });
    expect(web.rows[1]).toMatchObject({ kind: "outside", openIn: "VS Code", to: { to: "outside", id: "ext-1" } });
    expect(tally(web.rows)).toEqual({ waiting: 1, running: 1 });
  });

  it("says why a session cannot be archived now", () => {
    const snap = { ...orbitForFrame(10, NOW), active: undefined, others: [], distant: [] };
    const all = rows(sessionTable(snap, ws([
      view("busy", { state: "running" }),
      view("vscode", { state: "outside", outside: "VS Code" }),
      view("term", { terminal: true }),
      view("free"),
    ])));
    const blocked = Object.fromEntries(all.map((r) => [r.id, r.archiveBlocked !== undefined]));
    expect(blocked).toEqual({ "session:busy": true, "session:vscode": true, "session:term": true, "session:free": false });
    expect(all.find((r) => r.id === "session:vscode")?.stateText).toContain("VS Code");
  });

  it("adds the map's far stars the Workspace list does not have", () => {
    const snap = orbitForFrame(10, NOW);
    const known = snap.distant[0]!;
    const all = rows(sessionTable(snap, ws([], [outside(known.id)])));
    const far = all.filter((r) => r.kind === "outside");
    expect(far).toHaveLength(snap.distant.length);
    expect(far.filter((r) => r.id === `outside:${known.id}`)).toHaveLength(1);
  });
});
