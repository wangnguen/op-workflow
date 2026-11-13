// Local mock for the "Bắt đầu làm việc" dialog. Real data comes from the vcs
// crate later (git for-each-ref); this is not part of the IPC contract yet.

export interface BranchOption {
  name: string;
  /** minutes since the last commit */
  ageMin: number;
  aheadBehind?: string;
  lastUsed?: boolean;
}

export interface RepoOption {
  name: string;
  dirtyFiles: number;
  branches: BranchOption[];
}

export const MOCK_REPOS: RepoOption[] = [
  {
    name: "acme-web",
    dirtyFiles: 0,
    branches: [
      { name: "origin/development", ageMin: 120, lastUsed: true },
      { name: "origin/production", ageMin: 3 * 24 * 60 },
      { name: "feature/187641", ageMin: 26 * 60, aheadBehind: "ahead 2" },
      { name: "feature/187716", ageMin: 40 },
      { name: "origin/feature/186196", ageMin: 2 * 24 * 60 },
      { name: "development", ageMin: 5 * 24 * 60, aheadBehind: "behind 14" },
    ],
  },
  {
    name: "acme-api",
    dirtyFiles: 2,
    branches: [
      { name: "origin/development", ageMin: 300, lastUsed: true },
      { name: "origin/production", ageMin: 6 * 24 * 60 },
      { name: "feature/186196", ageMin: 50 },
    ],
  },
  {
    name: "acme-tools",
    dirtyFiles: 0,
    branches: [{ name: "origin/production", ageMin: 9 * 24 * 60 }],
  },
  {
    name: "acme-desktop",
    dirtyFiles: 0,
    branches: [
      { name: "origin/master", ageMin: 8 * 60 },
      { name: "feature/185609", ageMin: 12 * 24 * 60 },
    ],
  },
];

/** "40 phút trước", "2 giờ trước", "3 ngày trước" from minutes. */
export function ageLabel(ageMin: number): string {
  if (ageMin < 60) return `${ageMin} phút trước`;
  const hours = Math.round(ageMin / 60);
  if (hours < 24) return `${hours} giờ trước`;
  const days = Math.round(hours / 24);
  return days === 1 ? "hôm qua" : `${days} ngày trước`;
}
