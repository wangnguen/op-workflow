//! Read-only HTTP client for OpenProject (OpenProject API v3). Only GET requests exist here: the
//! app never writes to OpenProject.
//!
//! Auth: `Authorization: Bearer <token>` (used in Phase 0). If the server answers 401, the
//! client switches once to OpenProject's API-key form (`Basic base64("apikey:<token>")`).

use std::sync::Mutex;
use std::time::Duration;

use serde_json::Value;

use crate::{
    Comment, Me, TrackerApi, TrackerError, TrackerResult, WorkPackage, comments_from_activities, inbox_url,
    parse_me, parse_work_package, parse_work_package_collection,
};

const TIMEOUT: Duration = Duration::from_secs(20);
const INBOX_PAGE_SIZE: u32 = 100;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AuthMode {
    Bearer,
    ApiKeyBasic,
}

pub struct HttpTracker {
    base_url: String,
    token: String,
    agent: ureq::Agent,
    mode: Mutex<AuthMode>,
}

impl std::fmt::Debug for HttpTracker {
    // Never print the token.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HttpTracker")
            .field("base_url", &self.base_url)
            .finish_non_exhaustive()
    }
}

impl HttpTracker {
    pub fn new(base_url: &str, token: &str) -> Self {
        let tls = ureq::tls::TlsConfig::builder()
            .provider(ureq::tls::TlsProvider::NativeTls)
            .root_certs(ureq::tls::RootCerts::PlatformVerifier)
            .build();
        let agent: ureq::Agent = ureq::Agent::config_builder()
            .timeout_global(Some(TIMEOUT))
            .http_status_as_error(false)
            .tls_config(tls)
            .user_agent(concat!("op-workflow/", env!("CARGO_PKG_VERSION")))
            .build()
            .into();
        Self {
            base_url: base_url.trim_end_matches('/').to_owned(),
            token: token.trim().to_owned(),
            agent,
            mode: Mutex::new(AuthMode::Bearer),
        }
    }

    fn auth_header(&self, mode: AuthMode) -> String {
        match mode {
            AuthMode::Bearer => format!("Bearer {}", self.token),
            AuthMode::ApiKeyBasic => format!("Basic {}", base64(format!("apikey:{}", self.token).as_bytes())),
        }
    }

    fn send(&self, url: &str, mode: AuthMode) -> TrackerResult<(u16, String)> {
        let mut resp = self
            .agent
            .get(url)
            .header("Authorization", &self.auth_header(mode))
            .header("Accept", "application/hal+json, application/json")
            .call()
            .map_err(|e| TrackerError::Network(network_problem(&e)))?;
        let status = resp.status().as_u16();
        let body = resp
            .body_mut()
            .read_to_string()
            .map_err(|e| TrackerError::Network(network_problem(&e)))?;
        Ok((status, body))
    }

    /// GET a path under the base URL (or a full URL) and parse the JSON body.
    fn get_json(&self, path_or_url: &str) -> TrackerResult<Value> {
        let url = if path_or_url.starts_with("http") {
            path_or_url.to_owned()
        } else {
            format!("{}{}", self.base_url, path_or_url)
        };
        let mode = *self
            .mode
            .lock()
            .map_err(|e| TrackerError::Network(format!("lỗi nội bộ của app ({e})")))?;
        let (mut status, mut body) = self.send(&url, mode)?;
        if status == 401 && mode == AuthMode::Bearer {
            let (s, b) = self.send(&url, AuthMode::ApiKeyBasic)?;
            if s != 401 {
                if let Ok(mut m) = self.mode.lock() {
                    *m = AuthMode::ApiKeyBasic;
                }
            }
            (status, body) = (s, b);
        }
        if !(200..300).contains(&status) {
            return Err(TrackerError::Http {
                status,
                message: error_message(&body),
            });
        }
        serde_json::from_str(&body).map_err(|e| {
            if body.trim_start().starts_with('<') {
                TrackerError::NotApi
            } else {
                TrackerError::Shape(e.to_string())
            }
        })
    }

    /// Priorities (`GET /api/v3/priorities`): the admin's colours and OpenProject's order.
    pub fn priorities(&self) -> TrackerResult<crate::Priorities> {
        Ok(crate::parse_priorities(&self.get_json("/api/v3/priorities")?))
    }

    /// Status colours set by the OpenProject admin (`GET /api/v3/statuses`).
    pub fn status_colors(&self) -> TrackerResult<Vec<(String, String)>> {
        Ok(crate::parse_status_colors(&self.get_json("/api/v3/statuses")?))
    }

    /// The token owner (`GET /api/v3/users/me`): checks a new token, and its id tells the
    /// user's own comments from other people's.
    pub fn me(&self) -> TrackerResult<Me> {
        parse_me(&self.get_json("/api/v3/users/me")?)
    }
}

impl TrackerApi for HttpTracker {
    fn inbox(&self) -> TrackerResult<Vec<WorkPackage>> {
        parse_work_package_collection(&self.get_json(&inbox_url(&self.base_url, INBOX_PAGE_SIZE))?)
    }

    fn work_package(&self, id: u64) -> TrackerResult<WorkPackage> {
        parse_work_package(&self.get_json(&format!("/api/v3/work_packages/{id}"))?)
    }

    fn comments(&self, id: u64) -> TrackerResult<Vec<Comment>> {
        Ok(comments_from_activities(
            &self.get_json(&format!("/api/v3/work_packages/{id}/activities"))?,
        ))
    }
}

/// OpenProject error bodies look like `{"_type":"Error","message":"..."}`.
/// Why the request never got an answer, in words the user can act on.
fn network_problem(e: &ureq::Error) -> String {
    use std::io::ErrorKind;
    match e {
        ureq::Error::HostNotFound => "không tìm thấy máy chủ. Kiểm tra URL OpenProject, mạng hoặc VPN".into(),
        ureq::Error::Timeout(_) => "máy chủ không trả lời kịp. Kiểm tra mạng / VPN rồi thử lại".into(),
        ureq::Error::ConnectionFailed => "không mở được kết nối. Kiểm tra mạng / VPN".into(),
        ureq::Error::BadUri(_) => "URL OpenProject không hợp lệ".into(),
        ureq::Error::TooManyRedirects | ureq::Error::RedirectFailed => {
            "URL OpenProject chuyển hướng vòng vòng. Kiểm tra lại URL".into()
        }
        ureq::Error::Io(io) => match io.kind() {
            ErrorKind::ConnectionRefused => {
                "máy chủ từ chối kết nối (sai cổng trong URL, hoặc OpenProject đang tắt)".into()
            }
            ErrorKind::TimedOut => "hết thời gian chờ. Kiểm tra mạng / VPN rồi thử lại".into(),
            ErrorKind::ConnectionReset | ErrorKind::ConnectionAborted | ErrorKind::UnexpectedEof => {
                "kết nối bị ngắt giữa chừng (mạng chập chờn, hoặc VPN / proxy chặn)".into()
            }
            _ => format!("lỗi mạng ({io})"),
        },
        other => {
            let text = other.to_string();
            let low = text.to_lowercase();
            if low.contains("tls") || low.contains("certificate") || low.contains("ssl") {
                "lỗi chứng chỉ HTTPS (proxy công ty chặn, hoặc chứng chỉ máy chủ không hợp lệ)".into()
            } else {
                format!("lỗi mạng ({text})")
            }
        }
    }
}

fn error_message(body: &str) -> String {
    serde_json::from_str::<Value>(body)
        .ok()
        .and_then(|v| v.get("message").and_then(Value::as_str).map(str::to_owned))
        .unwrap_or_else(|| body.chars().take(200).collect())
}

fn base64(input: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(input.len().div_ceil(3) * 4);
    for chunk in input.chunks(3) {
        let b = [chunk[0], *chunk.get(1).unwrap_or(&0), *chunk.get(2).unwrap_or(&0)];
        let n = (u32::from(b[0]) << 16) | (u32::from(b[1]) << 8) | u32::from(b[2]);
        for i in 0..4 {
            if i <= chunk.len() {
                out.push(char::from(ALPHABET[((n >> (18 - 6 * i)) & 63) as usize]));
            } else {
                out.push('=');
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64_matches_rfc4648_vectors() {
        for (input, want) in [
            ("", ""),
            ("f", "Zg=="),
            ("fo", "Zm8="),
            ("foo", "Zm9v"),
            ("foobar", "Zm9vYmFy"),
            ("apikey:abc", "YXBpa2V5OmFiYw=="),
        ] {
            assert_eq!(base64(input.as_bytes()), want, "{input}");
        }
    }

    #[test]
    fn debug_hides_token_and_auth_forms_are_right() {
        let t = HttpTracker::new("https://project.example/", " secret-token ");
        assert!(!format!("{t:?}").contains("secret"));
        assert_eq!(t.auth_header(AuthMode::Bearer), "Bearer secret-token");
        assert_eq!(
            t.auth_header(AuthMode::ApiKeyBasic),
            format!("Basic {}", base64(b"apikey:secret-token"))
        );
    }

    #[test]
    fn error_message_prefers_openproject_message() {
        assert_eq!(
            error_message(r#"{"_type":"Error","message":"Unauthenticated"}"#),
            "Unauthenticated"
        );
        assert_eq!(error_message("plain"), "plain");
    }
}
