//! Background new-version check: once shortly after start, then every `update.everyHours`.
//! One GET to the GitHub Releases API of the app's public repo; nothing is downloaded.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use opw_config::UpdateConfig;
use opw_domain::view::UpdateView;
use opw_update::{Release, Version, fetch_releases, newest};

/// Let the window open and the first snapshots load before the check.
const FIRST_CHECK_AFTER: Duration = Duration::from_secs(10);

#[derive(Debug, Clone, Default)]
struct State {
    latest: Option<Release>,
    checked_at: Option<String>,
    error: Option<String>,
}

#[derive(Debug)]
pub struct Updater {
    config: UpdateConfig,
    current: String,
    state: Arc<Mutex<State>>,
}

impl Updater {
    /// Starts the background thread unless checks are turned off in config.
    pub fn start(config: UpdateConfig, current: &str) -> Self {
        let updater = Self {
            config,
            current: current.to_owned(),
            state: Arc::default(),
        };
        if updater.config.check {
            let (state, config, current) = (
                updater.state.clone(),
                updater.config.clone(),
                updater.current.clone(),
            );
            let every = Duration::from_secs(u64::from(config.every_hours.max(1)) * 3600);
            let _ = std::thread::Builder::new()
                .name("opw-update".into())
                .spawn(move || {
                    std::thread::sleep(FIRST_CHECK_AFTER);
                    loop {
                        check_into(&state, &config, &current);
                        std::thread::sleep(every);
                    }
                });
        }
        updater
    }

    /// Check now (blocking network call; run it off the UI thread).
    pub fn check_now(&self) -> UpdateView {
        check_into(&self.state, &self.config, &self.current);
        self.view()
    }

    pub fn view(&self) -> UpdateView {
        let s = self.state.lock().map(|s| s.clone()).unwrap_or_default();
        let current = Version::parse(&self.current);
        let available = match (&s.latest, &current) {
            (Some(r), Some(cur)) => r.version > *cur,
            _ => false,
        };
        UpdateView {
            current: self.current.clone(),
            enabled: self.config.check,
            latest: s.latest.as_ref().map(|r| r.version.to_string()),
            available,
            url: s.latest.as_ref().map(|r| r.url.clone()),
            published_at: s.latest.and_then(|r| r.published_at),
            checked_at: s.checked_at,
            error: s.error,
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
}

fn check_into(state: &Mutex<State>, config: &UpdateConfig, current: &str) {
    let result = fetch_releases(&config.repo, current);
    let now = opw_quota::now_rfc3339();
    if let Ok(mut s) = state.lock() {
        s.checked_at = Some(now);
        match result {
            Ok(releases) => {
                s.latest = newest(&releases, config.include_prerelease).cloned();
                s.error = None;
            }
            Err(e) => s.error = Some(e.to_string()),
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
        );
        let v = u.view();
        assert_eq!(v.current, "0.1.0");
        assert!(!v.enabled && !v.available && v.latest.is_none());
        assert_eq!(
            u.release_page().as_deref(),
            Some("https://github.com/wangnguen/op-workflow/releases")
        );
    }
}
