//! New versions from the GitHub Releases of the app's public repo: one GET to the API compared
//! with the running version, then the release's installer downloaded and checked against the
//! `SHA256SUMS.txt` attached to the same release (both written by release.yml). Running the
//! installer is the caller's part. No token, no personal data.

use std::cmp::Ordering;
use std::path::Path;
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
    #[error("không ghi được file: {0}")]
    File(String),
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
    /// The files attached to the release (the installer, `SHA256SUMS.txt`).
    pub assets: Vec<Asset>,
}

/// One file of a release.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Asset {
    pub name: String,
    /// `browser_download_url`: `https://github.com/{repo}/releases/download/{tag}/{name}`.
    pub url: String,
    pub size: u64,
}

/// The checksums file release.yml attaches (`sha256sum *.exe`).
pub const SUMS_FILE: &str = "SHA256SUMS.txt";
/// Largest installer the app downloads (v0.4.1 is 5.6 MB).
const MAX_DOWNLOAD: u64 = 200 * 1024 * 1024;
/// A download may take a while on a slow line; the check keeps its short timeout.
const DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(15 * 60);

impl Release {
    /// The Windows installer (`op-workflow_<v>_x64-setup.exe`, NSIS).
    pub fn installer(&self) -> Option<&Asset> {
        let setup = |a: &&Asset| a.name.to_ascii_lowercase().ends_with("-setup.exe");
        self.assets
            .iter()
            .filter(setup)
            .find(|a| a.name.contains("x64"))
            .or_else(|| self.assets.iter().find(setup))
    }

    pub fn sums(&self) -> Option<&Asset> {
        self.assets.iter().find(|a| a.name == SUMS_FILE)
    }
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
                assets: r
                    .get("assets")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                    .filter_map(|a| {
                        Some(Asset {
                            name: a.get("name")?.as_str()?.to_owned(),
                            url: a.get("browser_download_url")?.as_str()?.to_owned(),
                            size: a.get("size").and_then(Value::as_u64).unwrap_or(0),
                        })
                    })
                    .collect(),
            })
        })
        .collect())
}

/// Download links the app follows: only files of the configured repo's releases, with a plain
/// file name (no folder parts).
pub fn is_release_download(url: &str, repo: &str) -> bool {
    url.strip_prefix(&format!("https://github.com/{repo}/releases/download/"))
        .is_some_and(|rest| {
            let mut parts = rest.split('/');
            let (tag, name) = (parts.next(), parts.next());
            parts.next().is_none()
                && tag.is_some_and(|t| !t.is_empty())
                && name.is_some_and(is_plain_file_name)
        })
        && !url.contains(char::is_whitespace)
}

/// A file name the app may write into its download folder.
pub fn is_plain_file_name(name: &str) -> bool {
    !name.is_empty()
        && name != "."
        && name != ".."
        && !name.contains(['/', '\\', ':', '*', '?', '"', '<', '>', '|'])
}

/// The SHA-256 (lower-case hex) `SHA256SUMS.txt` gives for `file`: lines `<hex>  <name>` (or
/// `<hex> *<name>`, binary mode).
pub fn expected_sha256(sums: &str, file: &str) -> Option<String> {
    sums.lines().find_map(|line| {
        let (hash, name) = line.trim().split_once(char::is_whitespace)?;
        let name = name.trim().trim_start_matches('*');
        (name == file && hash.len() == 64 && hash.chars().all(|c| c.is_ascii_hexdigit()))
            .then(|| hash.to_ascii_lowercase())
    })
}

/// SHA-256 of a file, lower-case hex.
pub fn file_sha256(path: &Path) -> std::io::Result<String> {
    use sha2::{Digest, Sha256};
    let mut file = std::fs::File::open(path)?;
    let mut hasher = Sha256::new();
    std::io::copy(&mut file, &mut hasher)?;
    Ok(hasher.finalize().iter().map(|b| format!("{b:02x}")).collect())
}

fn agent(app_version: &str, timeout: Duration) -> ureq::Agent {
    let tls = ureq::tls::TlsConfig::builder()
        .provider(ureq::tls::TlsProvider::NativeTls)
        .root_certs(ureq::tls::RootCerts::PlatformVerifier)
        .build();
    ureq::Agent::config_builder()
        .timeout_global(Some(timeout))
        .http_status_as_error(false)
        .tls_config(tls)
        .user_agent(format!("op-workflow/{app_version}"))
        .build()
        .into()
}

/// A small text file of a release (`SHA256SUMS.txt`).
pub fn fetch_text(url: &str, app_version: &str) -> UpdateResult<String> {
    let mut resp = agent(app_version, TIMEOUT)
        .get(url)
        .call()
        .map_err(|e| UpdateError::Network(e.to_string()))?;
    let status = resp.status().as_u16();
    if !(200..300).contains(&status) {
        return Err(UpdateError::Http(status));
    }
    resp.body_mut()
        .read_to_string()
        .map_err(|e| UpdateError::Network(e.to_string()))
}

/// Download `url` to `dest` (written as `<dest>.part`, renamed once complete). `progress` gets
/// the bytes so far and the total when the server gives it.
pub fn download(
    url: &str,
    dest: &Path,
    app_version: &str,
    progress: &mut dyn FnMut(u64, Option<u64>),
) -> UpdateResult<()> {
    use std::io::{Read, Write};
    let mut resp = agent(app_version, DOWNLOAD_TIMEOUT)
        .get(url)
        .call()
        .map_err(|e| UpdateError::Network(e.to_string()))?;
    let status = resp.status().as_u16();
    if !(200..300).contains(&status) {
        return Err(UpdateError::Http(status));
    }
    let total = resp.body().content_length();
    let part = dest.with_extension("part");
    let mut out = std::fs::File::create(&part).map_err(|e| UpdateError::File(e.to_string()))?;
    let mut reader = resp.body_mut().with_config().limit(MAX_DOWNLOAD).reader();
    let mut buf = vec![0_u8; 64 * 1024];
    let mut done = 0_u64;
    loop {
        let n = reader
            .read(&mut buf)
            .map_err(|e| UpdateError::Network(e.to_string()))?;
        if n == 0 {
            break;
        }
        out.write_all(&buf[..n])
            .map_err(|e| UpdateError::File(e.to_string()))?;
        done += n as u64;
        progress(done, total);
    }
    out.flush().map_err(|e| UpdateError::File(e.to_string()))?;
    drop(out);
    if total.is_some_and(|t| t != done) {
        let _ = std::fs::remove_file(&part);
        return Err(UpdateError::Network(format!(
            "tải thiếu ({done} / {} byte)",
            total.unwrap_or(0)
        )));
    }
    std::fs::rename(&part, dest).map_err(|e| UpdateError::File(e.to_string()))
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
    let url = format!("https://api.github.com/repos/{repo}/releases?per_page={PAGE_SIZE}");
    let mut resp = agent(app_version, TIMEOUT)
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

    /// The asset list of v0.4.1 as the API gave it on 2026-11-21 (other keys cut).
    #[test]
    fn finds_the_installer_and_its_checksum() {
        let body = json!([{"tag_name": "v0.4.1", "html_url": "https://github.com/o/r/releases/tag/v0.4.1",
            "draft": false, "prerelease": false, "assets": [
                {"name": "op-workflow_0.4.1_x64-setup.exe", "size": 5_647_794,
                 "browser_download_url": "https://github.com/o/r/releases/download/v0.4.1/op-workflow_0.4.1_x64-setup.exe"},
                {"name": "SHA256SUMS.txt", "size": 98,
                 "browser_download_url": "https://github.com/o/r/releases/download/v0.4.1/SHA256SUMS.txt"}]}]);
        let r = &parse_releases(&body).unwrap()[0];
        let setup = r.installer().unwrap();
        assert_eq!(setup.name, "op-workflow_0.4.1_x64-setup.exe");
        assert_eq!(setup.size, 5_647_794);
        assert!(is_release_download(&setup.url, "o/r"));
        assert!(is_release_download(&r.sums().unwrap().url, "o/r"));
        let sums = "6f1c2b0e3a4d5c6b7a8f9e0d1c2b3a4f5e6d7c8b9a0f1e2d3c4b5a6f7e8d9c0b  op-workflow_0.4.1_x64-setup.exe\n";
        assert_eq!(
            expected_sha256(sums, &setup.name).as_deref(),
            Some("6f1c2b0e3a4d5c6b7a8f9e0d1c2b3a4f5e6d7c8b9a0f1e2d3c4b5a6f7e8d9c0b")
        );
        assert_eq!(expected_sha256(sums, "other.exe"), None);
        assert_eq!(expected_sha256("xyz  op.exe", "op.exe"), None);
    }

    #[test]
    fn downloads_only_files_of_the_repo_releases() {
        let ok = "https://github.com/o/r/releases/download/v1.0.0/app-setup.exe";
        assert!(is_release_download(ok, "o/r"));
        for bad in [
            "https://github.com/o/r2/releases/download/v1.0.0/app-setup.exe",
            "https://evil.example/o/r/releases/download/v1.0.0/app-setup.exe",
            "https://github.com/o/r/releases/download/v1.0.0/sub/app-setup.exe",
            "https://github.com/o/r/releases/download/v1.0.0/..",
            "https://github.com/o/r/releases/download//app-setup.exe",
            "https://github.com/o/r/releases/download/v1.0.0/a b.exe",
            "http://github.com/o/r/releases/download/v1.0.0/app-setup.exe",
        ] {
            assert!(!is_release_download(bad, "o/r"), "{bad}");
        }
        assert!(!is_plain_file_name("..\\x.exe") && !is_plain_file_name("C:x.exe"));
    }

    #[test]
    fn hashes_a_file() {
        let dir = std::env::temp_dir().join(format!("opw-sha-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let f = dir.join("x.txt");
        std::fs::write(&f, b"abc").unwrap();
        assert_eq!(
            file_sha256(&f).unwrap(),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Real download of the newest installer: `cargo test -p opw-update -- --ignored live_download`.
    #[test]
    #[ignore = "network"]
    fn live_download_matches_its_checksum() {
        let releases = fetch_releases("wangnguen/op-workflow", "0.1.0").unwrap();
        let r = newest(&releases, true).unwrap();
        let setup = r.installer().unwrap();
        let sums = fetch_text(&r.sums().unwrap().url, "0.1.0").unwrap();
        let want = expected_sha256(&sums, &setup.name).unwrap();
        let dir = std::env::temp_dir().join(format!("opw-dl-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let dest = dir.join(&setup.name);
        let mut last = 0;
        download(&setup.url, &dest, "0.1.0", &mut |done, _| last = done).unwrap();
        assert_eq!(last, setup.size);
        assert_eq!(file_sha256(&dest).unwrap(), want);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
