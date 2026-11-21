//! New versions: a background check shortly after start, then every `update.everyHours` (one GET
//! to the GitHub Releases API of the app's public repo). When a newer version has an installer,
//! it is downloaded in the background (`update.autoDownload`, on by default) into the user's
//! Downloads folder and checked against the release's `SHA256SUMS.txt`; "Cài" then runs it the
//! way Tauri's own updater does (`/P /R /UPDATE`: passive, no questions, the app opens again)
//! and the app quits so the installer can replace it.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use opw_config::UpdateConfig;
use opw_domain::view::{ApiError, DownloadState, UpdateDownload, UpdateView};
use opw_update::{
    Release, Version, download, expected_sha256, fetch_releases, fetch_text, file_sha256, newest,
};

/// Let the window open and the first snapshots load before the check.
const FIRST_CHECK_AFTER: Duration = Duration::from_secs(10);
/// What the NSIS installer Tauri builds takes for an update (tauri-plugin-updater passes the
/// same): `/P` passive (a progress bar, no questions; it closes the running app itself), `/R`
/// start the app again when done, `/UPDATE` keep the existing install (no uninstall first).
const INSTALLER_ARGS: [&str; 3] = ["/P", "/R", "/UPDATE"];

#[derive(Debug, Clone)]
struct Download {
    version: String,
    state: DownloadState,
    done: u64,
    total: Option<u64>,
    error: Option<String>,
    /// The checked installer, once `state` is ready.
    file: Option<PathBuf>,
    /// What `SHA256SUMS.txt` says the installer hashes to.
    sha256: Option<String>,
}

#[derive(Debug, Clone, Default)]
struct State {
    latest: Option<Release>,
    checked_at: Option<String>,
    error: Option<String>,
    download: Option<Download>,
}

#[derive(Debug)]
pub struct Updater {
    config: UpdateConfig,
    current: String,
    /// Where installers go when the system has no Downloads folder.
    fallback_dir: Option<PathBuf>,
    /// `update.autoDownload`, changed from Settings while the app runs.
    auto: Arc<AtomicBool>,
    state: Arc<Mutex<State>>,
}

impl Updater {
    /// Starts the background thread unless checks are turned off in config.
    pub fn start(config: UpdateConfig, current: &str, fallback_dir: Option<PathBuf>) -> Self {
        let updater = Self {
            auto: Arc::new(AtomicBool::new(config.auto_download)),
            config,
            current: current.to_owned(),
            fallback_dir,
            state: Arc::default(),
        };
        if updater.config.check {
            let worker = updater.worker();
            let every = Duration::from_secs(u64::from(updater.config.every_hours.max(1)) * 3600);
            let _ = std::thread::Builder::new()
                .name("opw-update".into())
                .spawn(move || {
                    std::thread::sleep(FIRST_CHECK_AFTER);
                    loop {
                        worker.check();
                        std::thread::sleep(every);
                    }
                });
        }
        updater
    }

    /// What a background thread needs (the state is shared).
    fn worker(&self) -> Worker {
        Worker {
            config: self.config.clone(),
            current: self.current.clone(),
            dir: download_dir(self.fallback_dir.as_deref()),
            auto: self.auto.clone(),
            state: self.state.clone(),
        }
    }

    /// Settings → "Tự tải bản mới"; turned on with a newer version known, it downloads now.
    pub fn set_auto_download(&self, on: bool) -> UpdateView {
        self.auto.store(on, Ordering::Release);
        if on { self.download_now() } else { self.view() }
    }

    /// Check now (blocking network call; run it off the UI thread).
    pub fn check_now(&self) -> UpdateView {
        self.worker().check();
        self.view()
    }

    /// Download the newest version now (again after a failure), in the background.
    pub fn download_now(&self) -> UpdateView {
        let worker = self.worker();
        if worker.claim_download() {
            let _ = std::thread::Builder::new()
                .name("opw-update-download".into())
                .spawn(move || worker.download());
        }
        self.view()
    }

    pub fn view(&self) -> UpdateView {
        let s = self.state.lock().map(|s| s.clone()).unwrap_or_default();
        let available = newer(s.latest.as_ref(), &self.current);
        UpdateView {
            current: self.current.clone(),
            enabled: self.config.check,
            latest: s.latest.as_ref().map(|r| r.version.to_string()),
            available,
            url: s.latest.as_ref().map(|r| r.url.clone()),
            published_at: s.latest.as_ref().and_then(|r| r.published_at.clone()),
            checked_at: s.checked_at,
            error: s.error,
            can_install: available
                && s.latest
                    .as_ref()
                    .is_some_and(|r| r.installer().is_some() && r.sums().is_some()),
            auto_download: self.auto.load(Ordering::Acquire),
            download: s
                .download
                .filter(|d| {
                    available
                        && s.latest
                            .as_ref()
                            .is_some_and(|r| r.version.to_string() == d.version)
                })
                .map(|d| UpdateDownload {
                    version: d.version,
                    state: d.state,
                    done: d.done,
                    total: d.total,
                    error: d.error,
                    file: d.file.map(|f| f.to_string_lossy().into_owned()),
                }),
        }
    }

    /// The release page to open, only if it belongs to the configured repo.
    pub fn release_page(&self) -> Option<String> {
        let url = self
            .state
            .lock()
            .ok()?
            .latest
            .as_ref()
            .map(|r| r.url.clone())
            .unwrap_or_else(|| format!("https://github.com/{}/releases", self.config.repo));
        opw_update::is_release_page(&url, &self.config.repo).then_some(url)
    }

    /// Start the downloaded installer (checked once more, the file sat in Downloads meanwhile).
    /// The caller quits the app right after, so the installer can replace it.
    pub fn run_installer(&self) -> Result<(), ApiError> {
        let (file, want) = {
            let s = self
                .state
                .lock()
                .map_err(|_| ApiError::new("update.state", "Không đọc được trạng thái cập nhật"))?;
            let d = s
                .download
                .as_ref()
                .filter(|d| d.state == DownloadState::Ready)
                .ok_or_else(|| ApiError::new("update.notReady", "Bản mới chưa tải xong"))?;
            (d.file.clone(), d.sha256.clone())
        };
        let (Some(file), Some(want)) = (file, want) else {
            return Err(ApiError::new("update.notReady", "Bản mới chưa tải xong"));
        };
        let got = file_sha256(&file).map_err(|e| {
            ApiError::new(
                "update.file",
                format!("Không đọc được file cài {}: {e}", file.display()),
            )
        })?;
        if got != want {
            let _ = std::fs::remove_file(&file);
            self.fail("File cài đã bị đổi sau khi tải (SHA-256 không khớp). Tải lại.");
            return Err(ApiError::new(
                "update.hash",
                "File cài đã bị đổi sau khi tải (SHA-256 không khớp). App đã xoá nó; bấm tải lại.",
            ));
        }
        Command::new(&file)
            .args(INSTALLER_ARGS)
            .spawn()
            .map(drop)
            .map_err(|e| ApiError::new("update.run", format!("Không chạy được trình cài: {e}")))
    }

    fn fail(&self, error: &str) {
        if let Ok(mut s) = self.state.lock() {
            if let Some(d) = s.download.as_mut() {
                d.state = DownloadState::Failed;
                d.error = Some(error.to_owned());
                d.file = None;
            }
        }
    }
}

/// The user's Downloads folder (where a browser would have put the installer), else the app's
/// data folder.
fn download_dir(fallback: Option<&Path>) -> Option<PathBuf> {
    dirs::download_dir().or_else(|| fallback.map(|d| d.join("updates")))
}

fn newer(latest: Option<&Release>, current: &str) -> bool {
    match (latest, Version::parse(current)) {
        (Some(r), Some(cur)) => r.version > cur,
        _ => false,
    }
}

/// The part of [`Updater`] the background threads use.
#[derive(Debug, Clone)]
struct Worker {
    config: UpdateConfig,
    current: String,
    dir: Option<PathBuf>,
    auto: Arc<AtomicBool>,
    state: Arc<Mutex<State>>,
}

impl Worker {
    fn check(&self) {
        let result = fetch_releases(&self.config.repo, &self.current);
        let now = opw_quota::now_rfc3339();
        if let Ok(mut s) = self.state.lock() {
            s.checked_at = Some(now);
            match result {
                Ok(releases) => {
                    s.latest = newest(&releases, self.config.include_prerelease).cloned();
                    s.error = None;
                }
                Err(e) => s.error = Some(e.to_string()),
            }
        }
        if self.auto.load(Ordering::Acquire) && self.claim_download() {
            self.download();
        }
    }

    /// Mark a download of the newest version as started, unless one runs or is done already
    /// (or there is nothing newer with an installer). `true` = the caller downloads.
    fn claim_download(&self) -> bool {
        let Ok(mut s) = self.state.lock() else {
            return false;
        };
        let Some(r) = s.latest.clone().filter(|r| newer(Some(r), &self.current)) else {
            return false;
        };
        if r.installer().is_none() || r.sums().is_none() {
            return false;
        }
        let version = r.version.to_string();
        let busy = s.download.as_ref().is_some_and(|d| {
            d.version == version && matches!(d.state, DownloadState::Downloading | DownloadState::Ready)
        });
        if busy {
            return false;
        }
        s.download = Some(Download {
            version,
            state: DownloadState::Downloading,
            done: 0,
            total: r.installer().map(|a| a.size).filter(|n| *n > 0),
            error: None,
            file: None,
            sha256: None,
        });
        true
    }

    /// Download the claimed installer and check it; the state says how it went.
    fn download(&self) {
        let result = self.fetch_installer();
        if let Ok(mut s) = self.state.lock() {
            if let Some(d) = s.download.as_mut() {
                match result {
                    Ok((file, sha)) => {
                        d.state = DownloadState::Ready;
                        d.file = Some(file);
                        d.sha256 = Some(sha);
                        d.error = None;
                    }
                    Err(e) => {
                        d.state = DownloadState::Failed;
                        d.error = Some(e);
                    }
                }
            }
        }
    }

    fn fetch_installer(&self) -> Result<(PathBuf, String), String> {
        let release = self
            .state
            .lock()
            .ok()
            .and_then(|s| s.latest.clone())
            .ok_or("Chưa biết bản mới nhất")?;
        let setup = release.installer().ok_or("Bản phát hành không có file cài")?;
        let sums = release.sums().ok_or("Bản phát hành không có SHA256SUMS.txt")?;
        let repo = &self.config.repo;
        if !opw_update::is_release_download(&setup.url, repo)
            || !opw_update::is_release_download(&sums.url, repo)
        {
            return Err("Link tải không thuộc repo phát hành của app".into());
        }
        let want = expected_sha256(
            &fetch_text(&sums.url, &self.current).map_err(|e| e.to_string())?,
            &setup.name,
        )
        .ok_or_else(|| format!("SHA256SUMS.txt không có {}", setup.name))?;
        let dir = self.dir.clone().ok_or("Không có thư mục Downloads")?;
        std::fs::create_dir_all(&dir).map_err(|e| format!("Không tạo được {}: {e}", dir.display()))?;
        let file = dir.join(&setup.name);
        // downloaded before (an earlier run of the app): reuse it if it is the right file
        if file_sha256(&file).is_ok_and(|got| got == want) {
            self.progress(setup.size, Some(setup.size));
            return Ok((file, want));
        }
        download(&setup.url, &file, &self.current, &mut |done, total| {
            self.progress(done, total)
        })
        .map_err(|e| e.to_string())?;
        let got = file_sha256(&file).map_err(|e| e.to_string())?;
        if got != want {
            let _ = std::fs::remove_file(&file);
            return Err("File tải về không khớp SHA256SUMS.txt (đã xoá). Thử tải lại.".into());
        }
        Ok((file, want))
    }

    fn progress(&self, done: u64, total: Option<u64>) {
        if let Ok(mut s) = self.state.lock() {
            if let Some(d) = s.download.as_mut() {
                d.done = done;
                if total.is_some() {
                    d.total = total;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn disabled_updater_reports_current_version_only() {
        let u = Updater::start(
            UpdateConfig {
                check: false,
                ..UpdateConfig::default()
            },
            "0.1.0",
            None,
        );
        let v = u.view();
        assert_eq!(v.current, "0.1.0");
        assert!(!v.enabled && !v.available && v.latest.is_none() && v.download.is_none());
        assert_eq!(
            u.release_page().as_deref(),
            Some("https://github.com/wangnguen/op-workflow/releases")
        );
        assert_eq!(u.run_installer().unwrap_err().code, "update.notReady");
    }

    fn release(version: &str, with_installer: bool) -> Release {
        let parsed = opw_update::parse_releases(&serde_json::json!([{
            "tag_name": format!("v{version}"),
            "html_url": format!("https://github.com/o/r/releases/tag/v{version}"),
            "assets": if with_installer { serde_json::json!([
                {"name": format!("op-workflow_{version}_x64-setup.exe"), "size": 10,
                 "browser_download_url": format!("https://github.com/o/r/releases/download/v{version}/op-workflow_{version}_x64-setup.exe")},
                {"name": "SHA256SUMS.txt", "size": 1,
                 "browser_download_url": format!("https://github.com/o/r/releases/download/v{version}/SHA256SUMS.txt")}
            ]) } else { serde_json::json!([]) }
        }]))
        .unwrap();
        parsed[0].clone()
    }

    #[test]
    fn a_download_is_claimed_once_per_version() {
        let u = Updater::start(
            UpdateConfig {
                check: false,
                ..UpdateConfig::default()
            },
            "0.4.1",
            None,
        );
        let w = u.worker();
        // nothing newer: nothing to download
        w.state.lock().unwrap().latest = Some(release("0.4.1", true));
        assert!(!w.claim_download());
        // newer, but no installer attached: only the release page
        w.state.lock().unwrap().latest = Some(release("0.4.2", false));
        assert!(!w.claim_download());
        assert!(!u.view().can_install);
        w.state.lock().unwrap().latest = Some(release("0.4.2", true));
        assert!(u.view().can_install);
        assert!(w.claim_download());
        assert!(!w.claim_download(), "already downloading");
        let v = u.view();
        let d = v.download.unwrap();
        assert_eq!(
            (d.version.as_str(), d.state, d.total),
            ("0.4.2", DownloadState::Downloading, Some(10))
        );
        // a failed download can be tried again
        u.fail("x");
        assert_eq!(u.view().download.unwrap().state, DownloadState::Failed);
        assert!(w.claim_download());
    }
}
