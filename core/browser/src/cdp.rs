//! A tiny Chrome DevTools Protocol client over one blocking websocket: send a command, read
//! until its answer arrives, keep events that came in between.

use std::collections::VecDeque;
use std::io::ErrorKind;
use std::net::{SocketAddr, TcpStream};
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use tungstenite::handshake::HandshakeError;
use tungstenite::{Message, WebSocket};

use crate::BrowserError;

const CONNECT_TIMEOUT: Duration = Duration::from_secs(3);
const WRITE_TIMEOUT: Duration = Duration::from_secs(10);
/// Events we keep while waiting for an answer; older ones are dropped.
const MAX_EVENTS: usize = 500;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum CdpError {
    Timeout,
    Closed,
    /// Chrome answered the command with an error.
    Remote(String),
    Protocol(String),
}

pub(crate) struct Cdp {
    ws: WebSocket<TcpStream>,
    next_id: u64,
    events: VecDeque<Value>,
}

fn time_left(deadline: Instant) -> Option<Duration> {
    let left = deadline.saturating_duration_since(Instant::now());
    (!left.is_zero()).then_some(left)
}

impl Cdp {
    /// Connect to the browser websocket. Chrome may hold the handshake until the user clicks
    /// Allow, so it gets the whole `deadline`.
    pub fn connect(port: u16, path: &str, deadline: Instant) -> Result<Self, BrowserError> {
        let addr = SocketAddr::from(([127, 0, 0, 1], port));
        let stream =
            TcpStream::connect_timeout(&addr, CONNECT_TIMEOUT).map_err(|_| BrowserError::NotRunning)?;
        let protocol = |e: std::io::Error| BrowserError::Protocol(e.to_string());
        stream.set_write_timeout(Some(WRITE_TIMEOUT)).map_err(protocol)?;
        stream
            .set_read_timeout(Some(time_left(deadline).ok_or(BrowserError::Denied)?))
            .map_err(protocol)?;
        let url = format!("ws://127.0.0.1:{port}{path}");
        match tungstenite::client(url.as_str(), stream) {
            Ok((ws, _)) => Ok(Self {
                ws,
                next_id: 1,
                events: VecDeque::new(),
            }),
            // timed out, refused (HTTP 403) or dropped while the dialog was up
            Err(HandshakeError::Interrupted(_)) => Err(BrowserError::Denied),
            Err(HandshakeError::Failure(e)) => Err(match e {
                tungstenite::Error::Io(_)
                | tungstenite::Error::Http(_)
                | tungstenite::Error::ConnectionClosed
                | tungstenite::Error::AlreadyClosed => BrowserError::Denied,
                other => BrowserError::Protocol(other.to_string()),
            }),
        }
    }

    /// Send `method` (to a tab when `session` is set) and wait for its result.
    pub fn call(
        &mut self,
        method: &str,
        params: Value,
        session: Option<&str>,
        deadline: Instant,
    ) -> Result<Value, CdpError> {
        let id = self.next_id;
        self.next_id += 1;
        let mut msg = json!({ "id": id, "method": method, "params": params });
        if let Some(s) = session {
            msg["sessionId"] = json!(s);
        }
        self.ws.send(Message::text(msg.to_string())).map_err(ws_error)?;
        loop {
            let v = self.next_message(deadline)?;
            if v.get("id").and_then(Value::as_u64) == Some(id) {
                if let Some(err) = v.get("error") {
                    let text = err.get("message").and_then(Value::as_str).unwrap_or("lỗi");
                    return Err(CdpError::Remote(text.to_owned()));
                }
                return Ok(v.get("result").cloned().unwrap_or(Value::Null));
            }
            self.keep_event(v);
        }
    }

    /// Wait for an event (`Page.loadEventFired`, …) of one tab; it may already have arrived.
    pub fn wait_event(&mut self, method: &str, session: &str, deadline: Instant) -> Result<Value, CdpError> {
        let is_it = |v: &Value| {
            v.get("method").and_then(Value::as_str) == Some(method)
                && v.get("sessionId").and_then(Value::as_str) == Some(session)
        };
        if let Some(at) = self.events.iter().position(is_it) {
            return Ok(self.events.remove(at).unwrap_or_default());
        }
        loop {
            let v = self.next_message(deadline)?;
            if is_it(&v) {
                return Ok(v);
            }
            self.keep_event(v);
        }
    }

    fn keep_event(&mut self, v: Value) {
        if v.get("method").is_some() {
            if self.events.len() >= MAX_EVENTS {
                self.events.pop_front();
            }
            self.events.push_back(v);
        }
    }

    fn next_message(&mut self, deadline: Instant) -> Result<Value, CdpError> {
        loop {
            let left = time_left(deadline).ok_or(CdpError::Timeout)?;
            self.ws
                .get_ref()
                .set_read_timeout(Some(left))
                .map_err(|e| CdpError::Protocol(e.to_string()))?;
            match self.ws.read() {
                Ok(Message::Text(text)) => {
                    if let Ok(v) = serde_json::from_str(text.as_str()) {
                        return Ok(v);
                    }
                }
                Ok(Message::Close(_)) => return Err(CdpError::Closed),
                Ok(_) => {}
                Err(e) => return Err(ws_error(e)),
            }
        }
    }

    pub fn close(mut self) {
        let _ = self.ws.close(None);
        let _ = self.ws.flush();
    }
}

fn ws_error(e: tungstenite::Error) -> CdpError {
    match e {
        tungstenite::Error::Io(io) if matches!(io.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut) => {
            CdpError::Timeout
        }
        tungstenite::Error::ConnectionClosed | tungstenite::Error::AlreadyClosed => CdpError::Closed,
        tungstenite::Error::Io(io)
            if matches!(
                io.kind(),
                ErrorKind::ConnectionReset | ErrorKind::ConnectionAborted | ErrorKind::BrokenPipe
            ) =>
        {
            CdpError::Closed
        }
        other => CdpError::Protocol(other.to_string()),
    }
}
