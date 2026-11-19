import { createElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { afterEach, describe, expect, it } from "vitest";
import { mockTaskDetail, resetMockFlows } from "../../ipc/mockFlow";
import type { TaskDetail } from "../../ipc/types";
import { TalkPanel } from "./TalkPanel";

afterEach(() => resetMockFlows());

const detail = (over: Partial<TaskDetail>): TaskDetail => ({ ...mockTaskDetail(9), ...over });
const render = (d: TaskDetail) =>
  renderToStaticMarkup(createElement(TalkPanel, { detail: d, disabled: false, onAction: async () => true }));
/** whether the "Chạy tiếp theo hướng đã bàn" button is off */
const goOnOff = (html: string) => {
  const button = [...html.matchAll(/<button([^>]*)>(.*?)<\/button>/g)].find((m) => m[2]?.includes("Chạy tiếp theo hướng đã bàn"));
  if (!button) throw new Error("no Chạy tiếp button");
  return (button[1] ?? "").includes("disabled");
};

describe("Trao đổi với AI", () => {
  it("shows the conversation and explains every button", () => {
    const html = render(
      detail({
        talk: [
          { fromUser: true, text: "Vì sao lỗi?", at: "2026-11-19T01:00:00Z" },
          { fromUser: false, text: "Vì thiếu quyền đọc thư mục", at: "2026-11-19T01:00:05Z", costUsd: 0.04 },
        ],
        canGoOn: false,
      }),
    );
    expect(html).toContain("Trao đổi với AI");
    expect(html).toContain("Vì sao lỗi?");
    expect(html).toContain("Vì thiếu quyền đọc thư mục");
    // the panel's hint and one under each button, always on screen
    expect(html.match(/<small/g)).toHaveLength(3);
    // no session to go on in: Chạy tiếp is off and says why
    expect(html).toContain("không còn phiên để tiếp");
    expect(goOnOff(html)).toBe(true);
  });

  it("offers Chạy tiếp when the step's session can go on", () => {
    const html = render(detail({ talk: [], canGoOn: true }));
    expect(html).toContain("AI làm tiếp trong đúng phiên của bước");
    expect(goOnOff(html)).toBe(false);
  });

  it("lets the user stop an answer on its way", () => {
    const html = render(detail({ talk: [{ fromUser: true, text: "Giải thích giúp", at: "2026-11-19T01:00:00Z" }], talkBusy: true }));
    expect(html).toContain("AI đang đọc và trả lời");
    expect(html).toContain("Dừng");
    expect(html).not.toContain(">Hỏi AI<");
  });
});
