// Settings pages and the search index over their rows (pure, easy to test).
import { stepLabel, vi } from "../../i18n/vi";
import { AI_STEPS } from "../../shared/steps";

export const PAGES = ["connection", "repos", "steps", "quota", "notify", "appearance", "about"] as const;
export type PageKey = (typeof PAGES)[number];

export interface SettingEntry {
  page: PageKey;
  /** `data-setting-row` of the row to scroll to */
  id: string;
  label: string;
  /** extra words people may type (English too) */
  keywords: string;
}

const t = vi.settings;

export const ENTRIES: SettingEntry[] = [
  { page: "connection", id: "connection", label: t.connection, keywords: "openproject trạng thái status user" },
  { page: "connection", id: "sync", label: t.taskCount, keywords: "lấy lại refresh sync đồng bộ" },
  { page: "connection", id: "url", label: t.baseUrl, keywords: "url địa chỉ server openproject" },
  { page: "connection", id: "token", label: t.token, keywords: "token api key access credential mật khẩu" },
  { page: "connection", id: "github", label: t.githubSection, keywords: "github issue chrome đọc remote debugging allow trình duyệt browser" },
  { page: "repos", id: "workspaces", label: t.workspaces, keywords: "workspace thư mục folder desktop quét scan" },
  { page: "repos", id: "config", label: t.editConfig, keywords: "config json cấu hình repo branch mẫu template" },
  { page: "repos", id: "harnessLibs", label: t.harnessLibs, keywords: "harness thư viện library scratch test tmp" },
  { page: "repos", id: "repoChecks", label: t.repoChecks, keywords: "kiểm tra check lint eslint tsc prettier lệnh command" },
  ...AI_STEPS.map(
    (step): SettingEntry => ({
      page: "steps",
      id: `step:${step}`,
      label: stepLabel[step],
      keywords: "model effort agent claude codex opus sonnet haiku fable giá price chi phí cost bước step",
    }),
  ),
  { page: "quota", id: "claude", label: `${t.claudeQuota} · ${t.quotaSection}`, keywords: "quota hạn mức limit claude probe" },
  { page: "quota", id: "codex", label: `${t.codexQuota} · ${t.quotaSection}`, keywords: "quota hạn mức limit codex app-server" },
  { page: "quota", id: "activeProbe", label: t.activeProbe, keywords: "quota hạn mức probe haiku claude code realtime đồng bộ usage chu kỳ" },
  { page: "quota", id: "idleProbe", label: t.idleProbe, keywords: "quota hạn mức probe haiku claude chu kỳ rảnh không dùng" },
  { page: "notify", id: "notifyComments", label: vi.notify.comments, keywords: "thông báo notification toast comment test failed reopened" },
  { page: "notify", id: "notifySteps", label: vi.notify.steps, keywords: "thông báo notification toast bước step xong lỗi chờ" },
  { page: "notify", id: "notifyQuota", label: vi.notify.quota, keywords: "thông báo notification toast hạn mức quota 80 95" },
  { page: "notify", id: "tray", label: vi.notify.trayLabel, keywords: "khay tray icon taskbar hạn mức quota" },
  { page: "notify", id: "onClose", label: vi.close.setting, keywords: "đóng close thoát quit x khay tray thu nhỏ minimize cửa sổ window" },
  { page: "appearance", id: "theme", label: t.theme, keywords: "giao diện theme chủ đề màu color sáng tối light dark dracula tokyo catppuccin nord gruvbox" },
  { page: "appearance", id: "outsideSessions", label: t.outsideSessions, keywords: "phiên session ngoài outside vs code terminal sao xa ẩn hide claude" },
  { page: "about", id: "version", label: t.versionSection, keywords: "version phiên bản update cập nhật bản mới release github" },
  { page: "about", id: "tasksDir", label: t.tasksDir, keywords: "brief task thư mục folder desktop lưu chuyển" },
  { page: "about", id: "data", label: t.dataDir, keywords: "data dữ liệu folder thư mục sqlite" },
  { page: "about", id: "configFile", label: t.configFile, keywords: "config json cấu hình file" },
];

/** Lower-case and drop Vietnamese diacritics, so "phien ban" finds "Phiên bản". */
export function fold(s: string): string {
  return s
    .normalize("NFD")
    .replace(/[̀-ͯ]/g, "")
    .replace(/đ/g, "d")
    .replace(/Đ/g, "D")
    .toLowerCase();
}

/** Rows whose label or keywords contain every word of the query (accent-insensitive). */
export function searchEntries(query: string, entries: SettingEntry[] = ENTRIES): SettingEntry[] {
  const words = fold(query).split(/\s+/).filter(Boolean);
  if (words.length === 0) return [];
  return entries.filter((e) => {
    const hay = fold(`${e.label} ${e.keywords} ${t.pages[e.page]}`);
    return words.every((w) => hay.includes(w));
  });
}
