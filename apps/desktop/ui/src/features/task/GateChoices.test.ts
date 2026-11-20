import { createElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";
import { BudgetChoices } from "./GateChoices";

describe("workflow follow-up choices", () => {
  it("shows the current extra budget and explains why continuation may be disabled", () => {
    for (const canGoOn of [false, true]) {
      const html = renderToStaticMarkup(createElement(BudgetChoices, { disabled: false, canGoOn, budget: 7, onAction: () => {} }));
      expect(html).toContain("Chạy tiếp (thêm $7)");
      expect(html).toContain("Chạy lại từ đầu");
      expect(html.match(/<small/g)).toHaveLength(2);
      expect(html).toContain(canGoOn ? "Cài đặt → Các bước AI" : "Phiên vừa dừng không còn");
    }
  });
});
