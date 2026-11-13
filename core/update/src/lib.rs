//! New-version check: one GET to the GitHub Releases API of the app's public repo, compared with
//! the running version. Nothing is downloaded or installed; the UI links to the release page and
//! the user installs by hand.

use std::cmp::Ordering;
use std::time::Duration;

use serde_json::Value;

const TIMEOUT: Duration = Duration::from_secs(15);
/// Releases fetched per check; enough to skip a few drafts or odd tags.
const PAGE_SIZE: u32 = 20;

#[derive(Debug, thiserror::Error)]
pub enum UpdateError {
    #[error("không kết nối được GitHub: {0}")]
    Network(String),
    #[error("GitHub trả về HTTP {0}")]
    Http(u16),
    #[error("dữ liệu GitHub không đúng dạng: {0}")]
    Shape(String),
}

pub type UpdateResult<T> = Result<T, UpdateError>;

/// `major.minor.patch[-pre]`, compared the semver way (a pre-release sorts before its release).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Version {
    pub major: u64,
    pub minor: u64,
    pub patch: u64,
    pub pre: Option<String>,
}

impl Version {
    /// Accepts `v0.2.0`, `0.2.0`, `0.2.0-beta.1`, `0.2` (patch 0); build metadata is ignored.
    pub fn parse(text: &str) -> Option<Self> {
        let text = text.trim().trim_start_matches(['v', 'V']);
        let text = text.split('+').next()?;
        let (core, pre) = match text.split_once('-') {
            Some((c, p)) if !p.is_empty() => (c, Some(p.to_owned())),
            Some(_) => return None,
            None => (text, None),
        };
        let mut nums = core.split('.').map(|n| n.parse::<u64>().ok());
        let major = nums.next()??;
        let minor = nums.next().unwrap_or(Some(0))?;
        let patch = nums.next().unwrap_or(Some(0))?;
        if nums.next().is_some() {
            return None;
        }
        Some(Self {
            major,
            minor,
            patch,
            pre,
        })
    }
}

impl std::fmt::Display for Version {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}.{}.{}", self.major, self.minor, self.patch)?;
        if let Some(pre) = &self.pre {
            write!(f, "-{pre}")?;
        }
        Ok(())
    }
}

impl PartialOrd for Version {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Version {
    fn cmp(&self, other: &Self) -> Ordering {
        (self.major, self.minor, self.patch)
            .cmp(&(other.major, other.minor, other.patch))
            .then_with(|| match (&self.pre, &other.pre) {
                (None, None) => Ordering::Equal,
                (None, Some(_)) => Ordering::Greater,
                (Some(_), None) => Ordering::Less,
                (Some(a), Some(b)) => cmp_pre(a, b),
            })
    }
}

/// Dot-separated identifiers; numeric ones compare as numbers and sort before text ones.
fn cmp_pre(a: &str, b: &str) -> Ordering {
    let mut ai = a.split('.');
    let mut bi = b.split('.');
    loop {
        match (ai.next(), bi.next()) {
            (None, None) => return Ordering::Equal,
            (None, Some(_)) => return Ordering::Less,
            (Some(_), None) => return Ordering::Greater,
            (Some(x), Some(y)) => {
                let ord = match (x.parse::<u64>(), y.parse::<u64>()) {
                    (Ok(m), Ok(n)) => m.cmp(&n),
                    (Ok(_), Err(_)) => Ordering::Less,
                    (Err(_), Ok(_)) => Ordering::Greater,
                    (Err(_), Err(_)) => x.cmp(y),
                };
                if ord != Ordering::Equal {
                    return ord;
                }
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Release {
    pub tag: String,
    pub version: Version,
    /// Release page on github.com.
    pub url: String,
    pub published_at: Option<String>,
    pub prerelease: bool,
}

/// `GET /repos/{repo}/releases` → releases with a version-like tag; drafts are dropped.
pub fn parse_releases(v: &Value) -> UpdateResult<Vec<Release>> {
    let list = v
        .as_array()
        .ok_or_else(|| UpdateError::Shape("expected an array".into()))?;
    Ok(list
        .iter()
        .filter(|r| !r.get("draft").and_then(Value::as_bool).unwrap_or(false))
        .filter_map(|r| {
            let tag = r.get("tag_name")?.as_str()?;
            Some(Release {
                tag: tag.to_owned(),
                version: Version::parse(tag)?,
                url: r.get("html_url")?.as_str()?.to_owned(),
                published_at: r.get("published_at").and_then(Value::as_str).map(str::to_owned),
                prerelease: r.get("prerelease").and_then(Value::as_bool).unwrap_or(false),
            })
        })
        .collect())
}

/// Highest version among `releases` (pre-releases only when allowed).
pub fn newest(releases: &[Release], include_prerelease: bool) -> Option<&Release> {
    releases
        .iter()
        .filter(|r| include_prerelease || !r.prerelease)
        .max_by(|a, b| a.version.cmp(&b.version))
}

/// Release page URLs the app is willing to open: only pages of the configured repo.
pub fn is_release_page(url: &str, repo: &str) -> bool {
    url.strip_prefix(&format!("https://github.com/{repo}/releases"))
        .is_some_and(|rest| rest.is_empty() || rest.starts_with('/'))
        && !url.contains(char::is_whitespace)
}

/// Ask GitHub for the releases of `repo` (`owner/name`). No token, no personal data: the only
/// header that names anything is `User-Agent: op-workflow/<version>`.
pub fn fetch_releases(repo: &str, app_version: &str) -> UpdateResult<Vec<Release>> {
    let tls = ureq::tls::TlsConfig::builder()
        .provider(ureq::tls::TlsProvider::NativeTls)
        .root_certs(ureq::tls::RootCerts::PlatformVerifier)
        .build();
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(TIMEOUT))
        .http_status_as_error(false)
        .tls_config(tls)
        .user_agent(format!("op-workflow/{app_version}"))
        .build()
        .into();
    let url = format!("https://api.github.com/repos/{repo}/releases?per_page={PAGE_SIZE}");
    let mut resp = agent
        .get(&url)
        .header("Accept", "application/vnd.github+json")
        .header("X-GitHub-Api-Version", "2022-11-28")
        .call()
        .map_err(|e| UpdateError::Network(e.to_string()))?;
    let status = resp.status().as_u16();
    if !(200..300).contains(&status) {
        return Err(UpdateError::Http(status));
    }
    let body = resp
        .body_mut()
        .read_to_string()
        .map_err(|e| UpdateError::Network(e.to_string()))?;
    let v: Value = serde_json::from_str(&body).map_err(|e| UpdateError::Shape(e.to_string()))?;
    parse_releases(&v)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn v(s: &str) -> Version {
        Version::parse(s).unwrap()
    }

    #[test]
    fn parses_and_orders_versions() {
        assert_eq!(v("v0.1.0"), v("0.1.0"));
        assert_eq!(v("0.2").to_string(), "0.2.0");
        assert!(v("0.1.1") > v("0.1.0"));
        assert!(v("0.10.0") > v("0.9.9"));
        assert!(v("1.0.0") > v("1.0.0-rc.1"));
        assert!(v("1.0.0-rc.2") > v("1.0.0-rc.1"));
        assert!(v("1.0.0-rc.10") > v("1.0.0-rc.9"));
        assert!(v("1.0.0-beta") > v("1.0.0-alpha.5"));
        assert_eq!(v("1.2.3+build.5"), v("1.2.3"));
        for bad in ["", "v", "latest", "1.x", "1.2.3.4", "1.2.3-"] {
            assert!(Version::parse(bad).is_none(), "{bad}");
        }
    }

    #[test]
    fn picks_the_newest_release_and_skips_drafts() {
        let body = json!([
            {"tag_name": "v0.3.0", "html_url": "https://github.com/o/r/releases/tag/v0.3.0",
             "draft": true, "prerelease": false},
            {"tag_name": "v0.2.0-beta.1", "html_url": "https://github.com/o/r/releases/tag/v0.2.0-beta.1",
             "draft": false, "prerelease": true, "published_at": "2026-11-20T00:00:00Z"},
            {"tag_name": "nightly", "html_url": "https://github.com/o/r/releases/tag/nightly",
             "draft": false, "prerelease": true},
            {"tag_name": "v0.1.0", "html_url": "https://github.com/o/r/releases/tag/v0.1.0",
             "draft": false, "prerelease": true}
        ]);
        let releases = parse_releases(&body).unwrap();
        assert_eq!(releases.len(), 2);
        let best = newest(&releases, true).unwrap();
        assert_eq!(best.tag, "v0.2.0-beta.1");
        assert!(best.version > v("0.1.0"));
        assert!(newest(&releases, false).is_none());
        assert!(parse_releases(&json!({"message": "Not Found"})).is_err());
    }

    /// Real GitHub call: `cargo test -p opw-update -- --ignored`.
    #[test]
    #[ignore = "network"]
    fn live_github_releases() {
        let releases = fetch_releases("wangnguen/op-workflow", "0.1.0").unwrap();
        println!("{releases:#?}");
        if let Some(r) = newest(&releases, true) {
            assert!(is_release_page(&r.url, "wangnguen/op-workflow"), "{}", r.url);
        }
    }

    #[test]
    fn only_release_pages_of_the_repo_open() {
        assert!(is_release_page(
            "https://github.com/o/r/releases/tag/v0.2.0",
            "o/r"
        ));
        assert!(is_release_page("https://github.com/o/r/releases", "o/r"));
        assert!(!is_release_page("https://github.com/o/r2/releases", "o/r"));
        assert!(!is_release_page("https://github.com/o/r/releasesX", "o/r"));
        assert!(!is_release_page("https://evil.example/o/r/releases", "o/r"));
        assert!(!is_release_page("https://github.com/o/r/releases/x y", "o/r"));
    }
}
