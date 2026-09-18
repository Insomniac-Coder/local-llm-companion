//! Asking a headless browser questions (Chrome DevTools Protocol).
//!
//! The command-line flags can dump a page and take a picture of it, and that
//! is all: they cannot say how big anything is or where it ended up. That
//! matters, because the way a local model's page fails is usually geometric —
//! one project's dashboard rendered perfectly and sat entirely below the fold
//! behind a sidebar that was told to be a full screen tall, which reading the
//! page's text would never catch (owner request, 2026-09-18).
//!
//! The protocol is JSON over a WebSocket, so this speaks just enough WebSocket
//! to send a request and read the reply: the handshake, masked text frames out,
//! unmasked frames in, pings answered. Nothing here talks to anything but a
//! browser this process started on the loopback interface.

use base64::Engine;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

/// A browser this process started, with its debugging port.
pub struct Browser {
    child: Child,
    port: u16,
    profile: PathBuf,
}

impl Drop for Browser {
    fn drop(&mut self) {
        let pid = self.child.id();
        #[cfg(windows)]
        let _ = Command::new("taskkill")
            .args(["/T", "/F", "/PID", &pid.to_string()])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
        #[cfg(not(windows))]
        let _ = Command::new("kill")
            .args(["-KILL", &pid.to_string()])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = std::fs::remove_dir_all(&self.profile);
    }
}

impl Browser {
    /// Start a browser with no window and a debugging port of its choosing,
    /// and wait for it to say which port that is.
    pub fn start(exe: &Path, wait: Duration) -> Result<Browser, String> {
        let profile = std::env::temp_dir().join(format!(
            "companion-cdp-{}-{}",
            std::process::id(),
            uuid::Uuid::new_v4().simple()
        ));
        std::fs::create_dir_all(&profile).map_err(|e| format!("cannot make a browser profile: {e}"))?;
        let mut child = Command::new(exe)
            .arg("--headless=new")
            .arg("--disable-gpu")
            .arg("--no-first-run")
            .arg("--no-default-browser-check")
            .arg("--disable-extensions")
            .arg("--hide-scrollbars")
            .arg("--window-size=1280,800")
            .arg("--remote-debugging-port=0")
            .arg(format!("--user-data-dir={}", profile.display()))
            .arg("about:blank")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| format!("the browser did not start: {e}"))?;
        let stderr = child.stderr.take().ok_or("the browser gave no error stream")?;
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            for line in BufReader::new(stderr).lines().map_while(Result::ok) {
                if let Some(at) = line.find("ws://127.0.0.1:") {
                    let rest = &line[at + "ws://127.0.0.1:".len()..];
                    let port: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
                    if let Ok(port) = port.parse::<u16>() {
                        let _ = tx.send(port);
                        break;
                    }
                }
            }
        });
        match rx.recv_timeout(wait) {
            Ok(port) => Ok(Browser { child, port, profile }),
            Err(_) => {
                let _ = child.kill();
                let _ = std::fs::remove_dir_all(&profile);
                Err("the browser did not open a debugging port".into())
            }
        }
    }

    /// The address of the page target to talk to.
    fn page_socket(&self) -> Result<String, String> {
        let body = http_get(self.port, "/json/list")?;
        let targets: serde_json::Value =
            serde_json::from_str(&body).map_err(|e| format!("the browser's target list was unreadable: {e}"))?;
        targets
            .as_array()
            .and_then(|targets| {
                targets
                    .iter()
                    .find(|target| target.get("type").and_then(|kind| kind.as_str()) == Some("page"))
                    .and_then(|target| target.get("webSocketDebuggerUrl"))
                    .and_then(|url| url.as_str())
                    .map(|url| url.to_string())
            })
            .ok_or_else(|| "the browser has no page to talk to".into())
    }

    pub fn page(&self) -> Result<Session, String> {
        Session::connect(&self.page_socket()?)
    }
}

/// One plain HTTP GET against the browser's own port.
///
/// The answer is read by its length rather than by waiting for the connection
/// to end: the browser holds it open whatever the request asks for, so reading
/// to the end of the stream simply times out (measured, 2026-09-18).
fn http_get(port: u16, path: &str) -> Result<String, String> {
    let mut stream = TcpStream::connect(("127.0.0.1", port)).map_err(|e| format!("cannot reach the browser: {e}"))?;
    stream
        .set_read_timeout(Some(Duration::from_secs(10)))
        .map_err(|e| e.to_string())?;
    write!(stream, "GET {path} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nConnection: close\r\n\r\n")
        .map_err(|e| format!("cannot ask the browser: {e}"))?;
    let mut reader = BufReader::new(stream);
    let mut length = 0usize;
    loop {
        let mut line = String::new();
        let read = reader
            .read_line(&mut line)
            .map_err(|e| format!("the browser said nothing: {e}"))?;
        if read == 0 {
            return Err("the browser closed before answering".into());
        }
        let line = line.trim_end();
        if line.is_empty() {
            break;
        }
        if let Some(value) = line.to_ascii_lowercase().strip_prefix("content-length:") {
            length = value.trim().parse().unwrap_or(0);
        }
    }
    if length == 0 {
        return Err("the browser's answer had no body".into());
    }
    let mut body = vec![0u8; length];
    reader
        .read_exact(&mut body)
        .map_err(|e| format!("the browser's answer stopped early: {e}"))?;
    String::from_utf8(body).map_err(|_| "the browser's answer was unreadable".to_string())
}

/// An open conversation with one page.
pub struct Session {
    stream: TcpStream,
    next_id: u64,
    /// Everything the page said that was not an answer to a request.
    pub events: Vec<serde_json::Value>,
}

impl Session {
    fn connect(url: &str) -> Result<Session, String> {
        let rest = url.strip_prefix("ws://").ok_or("the browser gave an address this cannot open")?;
        let (authority, path) = rest.split_once('/').unwrap_or((rest, ""));
        let mut stream = TcpStream::connect(authority).map_err(|e| format!("cannot reach the page: {e}"))?;
        stream.set_read_timeout(Some(Duration::from_secs(30))).map_err(|e| e.to_string())?;
        let key = base64::engine::general_purpose::STANDARD.encode(uuid::Uuid::new_v4().as_bytes());
        write!(
            stream,
            "GET /{path} HTTP/1.1\r\nHost: {authority}\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Key: {key}\r\nSec-WebSocket-Version: 13\r\n\r\n"
        )
        .map_err(|e| format!("cannot open the page channel: {e}"))?;
        // The handshake's reply ends at the blank line; the frames follow it.
        let mut head = Vec::new();
        let mut byte = [0u8; 1];
        while !head.ends_with(b"\r\n\r\n") {
            let read = stream.read(&mut byte).map_err(|e| format!("the page channel closed: {e}"))?;
            if read == 0 {
                return Err("the page channel closed during the handshake".into());
            }
            head.push(byte[0]);
            if head.len() > 4096 {
                return Err("the page channel said too much before saying yes".into());
            }
        }
        let head = String::from_utf8_lossy(&head);
        if !head.starts_with("HTTP/1.1 101") {
            return Err(format!("the page refused the channel: {}", head.lines().next().unwrap_or("")));
        }
        Ok(Session { stream, next_id: 0, events: Vec::new() })
    }

    fn write_frame(&mut self, text: &str) -> Result<(), String> {
        let payload = text.as_bytes();
        let mut frame = vec![0x81u8]; // final frame, text
        let mask = uuid::Uuid::new_v4().as_bytes()[..4].to_vec();
        match payload.len() {
            length if length < 126 => frame.push(0x80 | length as u8),
            length if length <= u16::MAX as usize => {
                frame.push(0x80 | 126);
                frame.extend_from_slice(&(length as u16).to_be_bytes());
            }
            length => {
                frame.push(0x80 | 127);
                frame.extend_from_slice(&(length as u64).to_be_bytes());
            }
        }
        frame.extend_from_slice(&mask);
        frame.extend(payload.iter().enumerate().map(|(index, byte)| byte ^ mask[index % 4]));
        self.stream.write_all(&frame).map_err(|e| format!("cannot send to the page: {e}"))?;
        self.stream.flush().map_err(|e| e.to_string())
    }

    fn read_exact(&mut self, count: usize) -> Result<Vec<u8>, String> {
        let mut buffer = vec![0u8; count];
        self.stream
            .read_exact(&mut buffer)
            .map_err(|e| format!("the page stopped answering: {e}"))?;
        Ok(buffer)
    }

    /// One whole message, joining continuation frames and answering pings.
    fn read_message(&mut self) -> Result<String, String> {
        let mut message = Vec::new();
        loop {
            let header = self.read_exact(2)?;
            let final_frame = header[0] & 0x80 != 0;
            let opcode = header[0] & 0x0f;
            let length = match header[1] & 0x7f {
                126 => {
                    let bytes = self.read_exact(2)?;
                    u16::from_be_bytes([bytes[0], bytes[1]]) as usize
                }
                127 => {
                    let bytes = self.read_exact(8)?;
                    u64::from_be_bytes(bytes.try_into().expect("eight bytes")) as usize
                }
                short => short as usize,
            };
            let payload = if length > 0 { self.read_exact(length)? } else { Vec::new() };
            match opcode {
                0x8 => return Err("the page closed the channel".into()),
                0x9 => {
                    // A ping is answered with its own payload, as a pong.
                    let mut pong = vec![0x8au8, 0x80 | payload.len() as u8];
                    let mask = [0u8; 4];
                    pong.extend_from_slice(&mask);
                    pong.extend_from_slice(&payload);
                    let _ = self.stream.write_all(&pong);
                    continue;
                }
                0xa => continue,
                _ => message.extend_from_slice(&payload),
            }
            if final_frame {
                return String::from_utf8(message).map_err(|_| "the page said something unreadable".to_string());
            }
        }
    }

    /// Ask the page something and wait for that answer, keeping anything else
    /// it says on the way.
    pub fn call(&mut self, method: &str, params: serde_json::Value, wait: Duration) -> Result<serde_json::Value, String> {
        self.next_id += 1;
        let id = self.next_id;
        self.write_frame(&serde_json::json!({"id": id, "method": method, "params": params}).to_string())?;
        let deadline = Instant::now() + wait;
        loop {
            if Instant::now() > deadline {
                return Err(format!("{method} did not answer in time"));
            }
            let text = self.read_message()?;
            let message: serde_json::Value = match serde_json::from_str(&text) {
                Ok(message) => message,
                Err(_) => continue,
            };
            if message.get("id").and_then(|value| value.as_u64()) == Some(id) {
                if let Some(error) = message.get("error") {
                    return Err(format!("{method}: {error}"));
                }
                return Ok(message.get("result").cloned().unwrap_or_default());
            }
            if message.get("method").is_some() {
                self.events.push(message);
            }
        }
    }

    /// Read whatever the page says for a while: the load event, its console.
    pub fn collect(&mut self, until: &str, wait: Duration) -> bool {
        let deadline = Instant::now() + wait;
        while Instant::now() < deadline {
            let remaining = deadline.saturating_duration_since(Instant::now()).max(Duration::from_millis(50));
            let _ = self.stream.set_read_timeout(Some(remaining));
            let Ok(text) = self.read_message() else { break };
            let Ok(message) = serde_json::from_str::<serde_json::Value>(&text) else { continue };
            let method = message.get("method").and_then(|value| value.as_str()).unwrap_or("").to_string();
            if message.get("method").is_some() {
                self.events.push(message);
            }
            if method == until {
                let _ = self.stream.set_read_timeout(Some(Duration::from_secs(30)));
                return true;
            }
        }
        let _ = self.stream.set_read_timeout(Some(Duration::from_secs(30)));
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The frame this writes is the one a server expects to read: final text
    /// frame, masked, with the length in the place its size calls for.
    #[test]
    fn frames_are_masked_and_sized_the_way_the_protocol_asks() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut head = Vec::new();
            let mut byte = [0u8; 1];
            while !head.ends_with(b"\r\n\r\n") {
                stream.read_exact(&mut byte).unwrap();
                head.push(byte[0]);
            }
            let request = String::from_utf8_lossy(&head).to_string();
            stream
                .write_all(b"HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\n\r\n")
                .unwrap();
            let mut header = [0u8; 2];
            stream.read_exact(&mut header).unwrap();
            let length = (header[1] & 0x7f) as usize;
            let mut mask = [0u8; 4];
            stream.read_exact(&mut mask).unwrap();
            let mut payload = vec![0u8; length];
            stream.read_exact(&mut payload).unwrap();
            let text: String = payload
                .iter()
                .enumerate()
                .map(|(index, byte)| (byte ^ mask[index % 4]) as char)
                .collect();
            // Answer with an unmasked text frame, as a server does.
            let reply = serde_json::json!({"id": 1, "result": {"value": 42}}).to_string();
            let mut frame = vec![0x81u8, reply.len() as u8];
            frame.extend_from_slice(reply.as_bytes());
            stream.write_all(&frame).unwrap();
            (request, text, header[0], header[1] & 0x80)
        });

        let mut session = Session::connect(&format!("ws://127.0.0.1:{port}/devtools/page/x")).unwrap();
        let result = session
            .call("Runtime.evaluate", serde_json::json!({"expression": "6*7"}), Duration::from_secs(5))
            .unwrap();
        assert_eq!(result["value"], 42);

        let (request, text, first_byte, masked) = server.join().unwrap();
        assert!(request.contains("Upgrade: websocket"), "{request}");
        assert!(request.contains("Sec-WebSocket-Key: "), "{request}");
        assert_eq!(first_byte, 0x81, "one final text frame");
        assert_eq!(masked, 0x80, "a client always masks");
        let sent: serde_json::Value = serde_json::from_str(&text).unwrap();
        assert_eq!(sent["method"], "Runtime.evaluate");
        assert_eq!(sent["id"], 1);
    }

    #[test]
    fn a_browser_that_never_opens_a_port_is_given_up_on() {
        // Something that is not a browser: it exits without saying anything.
        let exe = if cfg!(windows) { "cmd" } else { "true" };
        let started = Browser::start(Path::new(exe), Duration::from_millis(400));
        assert!(started.is_err(), "a program that opens no port cannot be talked to");
    }
}
