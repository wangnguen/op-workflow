// Settings pages and the search index over their rows (pure, easy to test).
import { vi } from "../../i18n/vi";

export const PAGES = ["connection", "repos", "quota", "appearance", "about"] as const;
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
  { page: "quota", id: "claude", label: `${t.claudeQuota} · ${t.quotaSection}`, keywords: "quota hạn mức limit claude probe" },
  { page: "quota", id: "codex", label: `${t.codexQuota} · ${t.quotaSection}`, keywords: "quota hạn mức limit codex app-server" },
  { page: "appearance", id: "theme", label: t.theme, keywords: "giao diện theme chủ đề màu color sáng tối light dark dracula tokyo catppuccin nord gruvbox" },
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
