//! Look at a page the project serves (§119, night decision 61).
//!
//! Until now a coding run had no way to find out whether what it built runs:
//! a dev server could not be started (it outlived every timeout and came back
//! empty), and nothing could open the page. Models filled the gap by assuming
//! — one wrote "the development server is running successfully" about a
//! command that had failed — and the task's own final step, "review it
//! yourself and fix what is broken", could not be carried out at all.
//!
//! This loads the page the way a browser does: the server answers over the
//! loopback interface, an installed browser renders it without a window, and
//! what comes back is the rendered DOM plus whatever the page logged. Nothing
//! leaves the machine; the address is checked before anything is opened.

use std::io::{Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// Characters of page text handed back: enough to see the headings, the
/// labels and the empty places, short enough for a small window.
const TEXT_CHARS: usize = 2_000;
const MAX_CONSOLE_LINES: usize = 20;

#[derive(Debug, Clone, Default)]
pub struct Layout {
    pub viewport: (i64, i64),
    pub page: (i64, i64),
    pub elements: i64,
    /// Pixels the page is wider than the window; 0 when it fits.
    pub overflow_x: i64,
    pub wide: Vec<String>,
    pub off_screen: Vec<String>,
    pub below_fold: Vec<String>,
    pub empty_boxes: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct PageReport {
    pub url: String,
    pub status_line: Option<String>,
    pub title: Option<String>,
    pub text: String,
    pub console: Vec<String>,
    pub dom_bytes: usize,
    pub screenshot: Option<PathBuf>,
    pub browser: Option<String>,
    pub note: Option<String>,
    /// Where things actually ended up, when the browser could be asked.
    pub layout: Option<Layout>,
}

/// An address this may open: loopback only unless the user's own page is
/// somewhere else on the machine. The local-first boundary (§129) holds —
/// a preview never reaches the internet.
pub fn parse_local_url(raw: &str) -> Result<(String, String, String), String> {
    let raw = raw.trim();
    let raw = if raw.contains("://") {
        raw.to_string()
    } else if raw.starts_with(':') {
        format!("http://localhost{raw}")
    } else if raw.chars().all(|c| c.is_ascii_digit()) {
        format!("http://localhost:{raw}")
    } else {
        format!("http://{raw}")
    };
    let rest = raw
        .strip_prefix("http://")
        .ok_or("only http:// addresses on this machine can be previewed")?;
    let (authority, path) = match rest.find('/') {
        Some(at) => (&rest[..at], &rest[at..]),
        None => (rest, "/"),
    };
    let (host, port) = match authority.rsplit_once(':') {
        Some((host, port)) => (
            host.to_string(),
            port.parse::<u16>().map_err(|_| "the port is not a number")?,
        ),
        None => (authority.to_string(), 80),
    };
    if !matches!(host.as_str(), "localhost" | "127.0.0.1" | "0.0.0.0" | "[::1]" | "::1") {
        return Err(format!(
            "{host} is not this machine; a preview only opens a page this project serves (localhost)"
        ));
    }
    let path = path.to_string();
    Ok((raw, format!("{host}:{port}"), path))
}

/// Wait for the server to answer, then return its status line. A dev server
/// takes a few seconds to come up after it is started.
pub fn wait_for_server(authority: &str, path: &str, wait: Duration) -> Result<String, String> {
    let deadline = Instant::now() + wait;
    loop {
        match request_status(authority, path) {
            Ok(status) => return Ok(status),
            Err(error) if Instant::now() >= deadline => return Err(error),
            Err(_) => std::thread::sleep(Duration::from_millis(250)),
        }
    }
}

/// One plain GET, enough to learn whether the page is served and with what
/// status. The rendering below fetches it properly.
fn request_status(authority: &str, path: &str) -> Result<String, String> {
    let address = authority
        .to_socket_addrs()
        .map_err(|e| format!("address unusable: {e}"))?
        .next()
        .ok_or("address unusable")?;
    let mut stream = TcpStream::connect_timeout(&address, Duration::from_millis(800))
        .map_err(|e| format!("not answering yet: {e}"))?;
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .map_err(|e| e.to_string())?;
    let request = format!(
        "GET {path} HTTP/1.1\r\nHost: {authority}\r\nConnection: close\r\nAccept: text/html\r\n\r\n"
    );
    stream
        .write_all(request.as_bytes())
        .map_err(|e| format!("request failed: {e}"))?;
    let mut head = Vec::new();
    let mut chunk = [0u8; 1024];
    while head.len() < 4096 {
        match stream.read(&mut chunk) {
            Ok(0) | Err(_) => break,
            Ok(read) => {
                head.extend_from_slice(&chunk[..read]);
                if head.windows(4).any(|w| w == b"\r\n\r\n") {
                    break;
                }
            }
        }
    }
    let text = String::from_utf8_lossy(&head);
    text.lines()
        .next()
        .map(|line| line.trim().to_string())
        .ok_or_else(|| "the server answered with nothing".to_string())
}

/// The browser this machine has, preferring one that is already installed for
/// the user. Nothing is downloaded and no path is assumed: the executables are
/// looked for where this system says programs live.
pub fn find_browser() -> Option<PathBuf> {
    #[cfg(windows)]
    let (names, roots) = (
        ["msedge.exe", "chrome.exe", "brave.exe", "chromium.exe"],
        ["PROGRAMFILES", "PROGRAMFILES(X86)", "LOCALAPPDATA"],
    );
    #[cfg(not(windows))]
    let (names, roots) = (
        [
            "google-chrome",
            "chromium",
            "chromium-browser",
            "microsoft-edge",
        ],
        ["HOME"],
    );
    if let Ok(path) = std::env::var("PATH") {
        for directory in std::env::split_paths(&path) {
            for name in names {
                let candidate = directory.join(name);
                if candidate.is_file() {
                    return Some(candidate);
                }
            }
        }
    }
    #[cfg(windows)]
    let suffixes = [
        "Microsoft\\Edge\\Application\\msedge.exe",
        "Google\\Chrome\\Application\\chrome.exe",
        "BraveSoftware\\Brave-Browser\\Application\\brave.exe",
        "Chromium\\Application\\chrome.exe",
    ];
    #[cfg(not(windows))]
    let suffixes = [
        "../../opt/google/chrome/chrome",
        "../../usr/bin/google-chrome",
        "../../usr/bin/chromium",
        "../../Applications/Google Chrome.app/Contents/MacOS/Google Chrome",
    ];
    for root in roots {
        let Ok(base) = std::env::var(root) else {
            continue;
        };
        for suffix in suffixes {
            let candidate = PathBuf::from(&base).join(suffix);
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }
    None
}


/// What the browser is asked about the page it has just drawn. Plain DOM
/// questions: how big the window is, how big the page turned out, and which
/// visible things ended up outside it.
const MEASURE: &str = r##"(() => {
  const vw = innerWidth, vh = innerHeight;
  const root = document.documentElement;
  const seen = document.body ? Array.from(document.body.querySelectorAll('*')) : [];
  const shown = (el) => {
    const style = getComputedStyle(el);
    return style.display !== 'none' && style.visibility !== 'hidden' && style.opacity !== '0';
  };
  const name = (el) => {
    const id = el.id ? '#' + el.id : '';
    const classes = typeof el.className === 'string' && el.className.trim()
      ? '.' + el.className.trim().split(/\s+/).slice(0, 2).join('.')
      : '';
    return el.tagName.toLowerCase() + id + classes;
  };
  const say = (el, box) => name(el) + ' ' + Math.round(box.width) + 'x' + Math.round(box.height)
    + ' at ' + Math.round(box.left) + ',' + Math.round(box.top);
  const boxes = seen.filter(shown).map((el) => [el, el.getBoundingClientRect()]);
  const wordy = (el) => (el.innerText || '').trim().length > 40;
  const pick = (list) => list.slice(0, 5).map(([el, box]) => say(el, box));
  return {
    viewport: [vw, vh],
    page: [root.scrollWidth, root.scrollHeight],
    elements: seen.length,
    title: document.title || '',
    text: (document.body ? document.body.innerText : '').replace(/\n{3,}/g, '\n\n').slice(0, 2000),
    html: root.outerHTML.length,
    overlay: !!document.querySelector('vite-error-overlay, .error-overlay, #webpack-dev-server-client-overlay'),
    overflowX: Math.max(0, root.scrollWidth - vw),
    wide: pick(boxes.filter(([, box]) => box.width > vw + 1 && box.height > 0)),
    offScreen: pick(boxes.filter(([, box]) => box.height > 0 && box.width > 0 && (box.right <= 0 || box.left >= vw))),
    belowFold: pick(boxes.filter(([el, box]) => box.top >= vh && box.height > 40 && wordy(el))),
    emptyBoxes: pick(boxes.filter(([el, box]) => el.children.length > 0 && (box.width < 1 || box.height < 1))),
  };
})()"##;

fn strings(value: Option<&serde_json::Value>) -> Vec<String> {
    value
        .and_then(|value| value.as_array())
        .map(|items| items.iter().filter_map(|item| item.as_str().map(str::to_string)).collect())
        .unwrap_or_default()
}

fn pair(value: Option<&serde_json::Value>) -> (i64, i64) {
    let numbers = value.and_then(|value| value.as_array());
    let at = |index: usize| {
        numbers
            .and_then(|numbers| numbers.get(index))
            .and_then(|number| number.as_f64())
            .unwrap_or(0.0)
            .round() as i64
    };
    (at(0), at(1))
}

/// What the page said while it loaded, out of the browser's own events.
fn console_from_events(events: &[serde_json::Value]) -> Vec<String> {
    let mut lines = Vec::new();
    for event in events {
        let method = event.get("method").and_then(|method| method.as_str()).unwrap_or("");
        let params = event.get("params").cloned().unwrap_or_default();
        let line = match method {
            "Runtime.consoleAPICalled" => {
                let level = params.get("type").and_then(|kind| kind.as_str()).unwrap_or("log");
                let text: Vec<String> = params
                    .get("args")
                    .and_then(|args| args.as_array())
                    .map(|args| {
                        args.iter()
                            .map(|arg| {
                                arg.get("value")
                                    .map(|value| match value.as_str() {
                                        Some(text) => text.to_string(),
                                        None => value.to_string(),
                                    })
                                    .or_else(|| arg.get("description").and_then(|d| d.as_str()).map(str::to_string))
                                    .unwrap_or_default()
                            })
                            .collect()
                    })
                    .unwrap_or_default();
                let text = text.join(" ");
                (!text.trim().is_empty()).then(|| format!("{}: {}", if level == "warning" { "warning" } else { level }, text))
            }
            "Runtime.exceptionThrown" => params
                .get("exceptionDetails")
                .map(|details| {
                    let description = details
                        .get("exception")
                        .and_then(|exception| exception.get("description"))
                        .and_then(|text| text.as_str())
                        .or_else(|| details.get("text").and_then(|text| text.as_str()))
                        .unwrap_or("an exception");
                    format!("error: {}", description.lines().next().unwrap_or(description))
                }),
            "Log.entryAdded" => params.get("entry").and_then(|entry| {
                let level = entry.get("level").and_then(|level| level.as_str()).unwrap_or("log");
                let text = entry.get("text").and_then(|text| text.as_str()).unwrap_or("");
                (!text.is_empty()).then(|| format!("{level}: {text}"))
            }),
            _ => None,
        };
        if let Some(line) = line {
            let line: String = line.chars().take(220).collect();
            if !lines.contains(&line) {
                lines.push(line);
            }
        }
        if lines.len() >= MAX_CONSOLE_LINES {
            break;
        }
    }
    lines
}

/// Load the page in a browser this process drives, and ask it where everything
/// ended up. Returns None when the browser cannot be driven, so the caller can
/// fall back to simply dumping the page.
pub struct Measured {
    pub title: String,
    pub text: String,
    pub html_bytes: usize,
    pub overlay: bool,
    pub console: Vec<String>,
    pub layout: Layout,
}

fn look_with_browser(
    url: &str,
    browser: &Path,
    wait: Duration,
    screenshot: Option<&PathBuf>,
) -> Result<Measured, String> {
    let running = crate::cdp::Browser::start(browser, Duration::from_secs(20))?;
    let mut page = running.page()?;
    let short = Duration::from_secs(20);
    page.call("Runtime.enable", serde_json::json!({}), short)?;
    page.call("Log.enable", serde_json::json!({}), short)?;
    page.call("Page.enable", serde_json::json!({}), short)?;
    page.call("Page.navigate", serde_json::json!({ "url": url }), short)?;
    page.collect("Page.loadEventFired", wait);
    // An application draws itself after the load event; give it a moment.
    std::thread::sleep(Duration::from_millis(1_200));
    page.collect("Runtime.consoleAPICalled", Duration::from_millis(400));
    let measured = page.call(
        "Runtime.evaluate",
        serde_json::json!({"expression": MEASURE, "returnByValue": true, "awaitPromise": true}),
        short,
    )?;
    let value = measured
        .get("result")
        .and_then(|result| result.get("value"))
        .cloned()
        .ok_or("the page did not answer the measurement")?;
    if let Some(path) = screenshot {
        if let Ok(shot) = page.call("Page.captureScreenshot", serde_json::json!({"format": "png"}), short) {
            if let Some(data) = shot.get("data").and_then(|data| data.as_str()) {
                if let Ok(bytes) = base64::Engine::decode(&base64::engine::general_purpose::STANDARD, data) {
                    let _ = std::fs::write(path, bytes);
                }
            }
        }
    }
    let layout = Layout {
        viewport: pair(value.get("viewport")),
        page: pair(value.get("page")),
        elements: value.get("elements").and_then(|count| count.as_i64()).unwrap_or(0),
        overflow_x: value.get("overflowX").and_then(|pixels| pixels.as_i64()).unwrap_or(0),
        wide: strings(value.get("wide")),
        off_screen: strings(value.get("offScreen")),
        below_fold: strings(value.get("belowFold")),
        empty_boxes: strings(value.get("emptyBoxes")),
    };
    let console = console_from_events(&page.events);
    Ok(Measured {
        title: value.get("title").and_then(|title| title.as_str()).unwrap_or("").to_string(),
        text: value.get("text").and_then(|text| text.as_str()).unwrap_or("").to_string(),
        html_bytes: value.get("html").and_then(|size| size.as_i64()).unwrap_or(0) as usize,
        overlay: value.get("overlay").and_then(|overlay| overlay.as_bool()).unwrap_or(false),
        console,
        layout,
    })
}

/// Console lines a headless browser writes to its error stream, cleaned of
/// the timestamps and process ids that say nothing to the reader.
///
/// A browser reports a page's own exception at its INFO level, so what makes
/// a line an error is what the page said, not how the browser filed it. The
/// browser's own extensions talk on the same channel and are left out.
fn console_lines(stderr: &str) -> Vec<String> {
    let mut lines = Vec::new();
    for line in stderr.lines() {
        let Some(at) = line.find(":CONSOLE") else {
            continue;
        };
        let Some((_, rest)) = line[at..].split_once(']') else {
            continue;
        };
        let rest = rest.trim();
        if rest.contains("source: chrome-extension://") || rest.contains("source: edge-extension://") {
            continue;
        }
        let quoted = rest.strip_prefix('"');
        let (message, source) = match quoted.and_then(|rest| rest.rsplit_once("\", source: ")) {
            Some((message, source)) => (message.to_string(), Some(source.trim().to_string())),
            None => (rest.trim_matches('"').to_string(), None),
        };
        if message.is_empty() {
            continue;
        }
        let severity = if line[..at].contains(":ERROR")
            || message.starts_with("Uncaught")
            || message.contains("Error:")
            || message.contains("Failed to load")
        {
            "error"
        } else if line[..at].contains(":WARNING") || message.starts_with("Warning") {
            "warning"
        } else {
            "log"
        };
        lines.push(match source {
            Some(source) => format!("{severity}: {message} [{source}]"),
            None => format!("{severity}: {message}"),
        });
        if lines.len() >= MAX_CONSOLE_LINES {
            break;
        }
    }
    lines
}

fn title_of(html: &str) -> Option<String> {
    let lower = html.to_lowercase();
    let start = lower.find("<title")?;
    let open = lower[start..].find('>')? + start + 1;
    let end = lower[open..].find("</title>")? + open;
    let title = html[open..end].trim();
    (!title.is_empty()).then(|| title.chars().take(120).collect())
}

/// Load the page in a browser with no window and report what it became.
///
/// `--virtual-time-budget` lets the page's own scripts run before the DOM is
/// taken, which is the whole point: an application that renders itself is an
/// empty shell in the served HTML.
pub fn render(
    url: &str,
    browser: &PathBuf,
    budget: Duration,
    screenshot: Option<&PathBuf>,
) -> Result<(String, Vec<String>), String> {
    let profile = std::env::temp_dir().join(format!("companion-preview-{}", std::process::id()));
    let _ = std::fs::create_dir_all(&profile);
    let mut command = std::process::Command::new(browser);
    command
        .arg("--headless=new")
        .arg("--disable-gpu")
        .arg("--no-first-run")
        .arg("--no-default-browser-check")
        .arg("--hide-scrollbars")
        .arg("--window-size=1280,900")
        .arg(format!("--user-data-dir={}", profile.display()))
        .arg(format!("--virtual-time-budget={}", budget.as_millis()))
        .arg("--enable-logging=stderr")
        .arg("--log-level=0")
        .arg("--dump-dom");
    if let Some(path) = screenshot {
        command.arg(format!("--screenshot={}", path.display()));
    }
    command.arg(url);
    let output = command
        .output()
        .map_err(|e| format!("the browser did not start: {e}"))?;
    let _ = std::fs::remove_dir_all(&profile);
    let dom = String::from_utf8_lossy(&output.stdout).into_owned();
    let console = console_lines(&String::from_utf8_lossy(&output.stderr));
    if dom.trim().is_empty() {
        return Err(format!(
            "the browser returned no page{}",
            if console.is_empty() {
                String::new()
            } else {
                format!(" ({})", console.join("; "))
            }
        ));
    }
    Ok((dom, console))
}

/// Open the page and report it: status, title, the text a reader would see,
/// and everything the page logged while it loaded.
pub fn look(url: &str, wait: Duration, want_screenshot: bool) -> Result<PageReport, String> {
    let (url, authority, path) = parse_local_url(url)?;
    let status_line = wait_for_server(&authority, &path, wait).map_err(|error| {
        format!(
            "{url} is not being served ({error}). Start the project's server first in the background, for example execute_command {{\"command\": \"npm run dev\", \"background\": true}}, then preview the address it prints."
        )
    })?;
    let Some(browser) = find_browser() else {
        return Ok(PageReport {
            url: url.clone(),
            status_line: Some(status_line),
            title: None,
            text: String::new(),
            console: Vec::new(),
            dom_bytes: 0,
            screenshot: None,
            browser: None,
            layout: None,
            note: Some(
                "The server answers, but this machine has no browser to render the page, so only the status could be checked.".into(),
            ),
        });
    };
    let screenshot = want_screenshot.then(|| {
        static NEXT: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(1);
        std::env::temp_dir().join(format!(
            "companion-preview-{}-{}.png",
            std::process::id(),
            NEXT.fetch_add(1, std::sync::atomic::Ordering::SeqCst)
        ))
    });
    // Driving the browser answers where things ended up; dumping the page is
    // the fallback when it cannot be driven on this PC.
    match look_with_browser(&url, &browser, Duration::from_secs(15), screenshot.as_ref()) {
        Ok(measured) => {
            let note = measured.overlay.then(|| {
                "The dev server is showing its build-error overlay, so the page itself did not render. The message is in the server's own output: execute_command with the background id it was started under.".to_string()
            });
            return Ok(PageReport {
                url,
                status_line: Some(status_line),
                title: (!measured.title.trim().is_empty()).then(|| measured.title.chars().take(120).collect()),
                text: measured.text.chars().take(TEXT_CHARS).collect(),
                console: measured.console,
                dom_bytes: measured.html_bytes,
                screenshot: screenshot.clone().filter(|path| path.is_file()),
                browser: browser.file_stem().and_then(|name| name.to_str()).map(|name| name.to_string()),
                note,
                layout: Some(measured.layout),
            });
        }
        Err(error) => tracing::warn!("the browser could not be driven ({error}); dumping the page instead"),
    }
    let (dom, console) = render(&url, &browser, Duration::from_secs(5), screenshot.as_ref())?;
    let text = crate::search::html_to_text(&dom);
    // A dev server that cannot build shows its error in an overlay the page
    // keeps to itself (a shadow root), so the rendered text is empty and the
    // message is only in the server's own output.
    let note = dom
        .contains("vite-error-overlay")
        .then(|| {
            "The dev server is showing its build-error overlay, so the page itself did not render. The message is in the server's own output: execute_command with the background id it was started under.".to_string()
        });
    Ok(PageReport {
        url,
        status_line: Some(status_line),
        title: title_of(&dom),
        text: text.chars().take(TEXT_CHARS).collect(),
        console,
        dom_bytes: dom.len(),
        screenshot: screenshot.filter(|path| path.is_file()),
        browser: browser
            .file_stem()
            .and_then(|name| name.to_str())
            .map(|name| name.to_string()),
        note,
        layout: None,
    })
}

pub fn format_report(report: &PageReport) -> String {
    let mut out = format!("Page: {}\n", report.url);
    if let Some(status) = &report.status_line {
        out.push_str(&format!("Server answered: {status}\n"));
    }
    if let Some(title) = &report.title {
        out.push_str(&format!("Title: {title}\n"));
    }
    out.push_str(&format!("Rendered HTML: {} bytes\n", report.dom_bytes));
    if let Some(note) = &report.note {
        out.push_str(&format!("{note}\n"));
    }
    if let Some(path) = &report.screenshot {
        out.push_str(&format!("Screenshot saved: {}\n", path.display()));
    }
    out.push_str("\n--- what the page logged ---\n");
    if report.console.is_empty() {
        out.push_str("(nothing: no errors and no warnings)\n");
    } else {
        for line in &report.console {
            out.push_str(&format!("{line}\n"));
        }
    }
    if let Some(layout) = &report.layout {
        out.push_str("\n--- how it is laid out ---\n");
        out.push_str(&format!(
            "window {}x{}, page {}x{}, {} elements\n",
            layout.viewport.0, layout.viewport.1, layout.page.0, layout.page.1, layout.elements
        ));
        let mut said_something = false;
        let mut section = |title: &str, items: &Vec<String>| {
            if items.is_empty() {
                return;
            }
            said_something = true;
            out.push_str(&format!("{title}\n"));
            for item in items {
                out.push_str(&format!("- {item}\n"));
            }
        };
        if layout.overflow_x > 0 {
            section(
                &format!("The page is {} px wider than the window (it scrolls sideways). What is too wide:", layout.overflow_x),
                &layout.wide,
            );
        }
        section("Below the first screen (nothing of this shows until the page is scrolled):", &layout.below_fold);
        section("Outside the window altogether:", &layout.off_screen);
        section("Boxes with content but no size:", &layout.empty_boxes);
        if !said_something {
            out.push_str("Nothing is off-screen, oversized or collapsed.\n");
        }
    }
    out.push_str("\n--- what the page shows ---\n");
    if report.text.trim().is_empty() {
        out.push_str(
            "(no text at all: the page is blank, which usually means it failed while starting — check the logged errors above)\n",
        );
    } else {
        out.push_str(&report.text);
        out.push('\n');
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_this_machines_addresses_are_opened() {
        let (url, authority, path) = parse_local_url("localhost:5173").unwrap();
        assert_eq!(url, "http://localhost:5173");
        assert_eq!(authority, "localhost:5173");
        assert_eq!(path, "/");
        assert_eq!(parse_local_url("5173").unwrap().1, "localhost:5173");
        assert_eq!(parse_local_url(":4000").unwrap().1, "localhost:4000");
        let (_, authority, path) = parse_local_url("http://127.0.0.1:3000/dashboard").unwrap();
        assert_eq!(authority, "127.0.0.1:3000");
        assert_eq!(path, "/dashboard");
        let refused = parse_local_url("http://example.com/page").unwrap_err();
        assert!(refused.contains("not this machine"), "{refused}");
        assert!(parse_local_url("https://localhost:5173").is_err());
    }

    #[test]
    fn a_page_that_is_not_served_says_how_to_serve_it() {
        // Port 1 is never a dev server; the wait is short so the test is too.
        let error = look("http://127.0.0.1:1/", Duration::from_millis(300), false).unwrap_err();
        assert!(error.contains("is not being served"), "{error}");
        assert!(error.contains("\"background\": true"), "{error}");
    }

    #[test]
    fn console_errors_are_read_out_of_the_browsers_own_log() {
        // Both shapes a browser writes, including the one this machine's
        // browser writes: a page's own exception filed at INFO.
        let stderr = "[3468:41748:0918/055135.839:INFO:CONSOLE:5599] \"[ProtocolLaunch] handler constructed\", source: chrome-extension://ndcpkimcihhghdcddljkfmmjccdmcmof/background.js (5599)\n\
                      [3468:41748:0918/055146.586:INFO:CONSOLE:4] \"page script ran\", source: http://127.0.0.1:5199/ (4)\n\
                      [3468:41748:0918/055146.586:INFO:CONSOLE:6] \"Uncaught ReferenceError: undefinedFunction is not defined\", source: http://127.0.0.1:5199/ (6)\n\
                      [0918/061500.456:ERROR:CONSOLE(12)] \"stars.map is not a function\", source: http://localhost:5173/App.tsx (12)\n\
                      [0918/061500.789:INFO:headless_shell.cc(660)] Written to file screenshot.png.";
        let lines = console_lines(stderr);
        assert_eq!(lines.len(), 3, "the browser's own extensions are not the page: {lines:?}");
        assert!(lines[0].starts_with("log: page script ran"), "{lines:?}");
        assert!(lines[1].starts_with("error: Uncaught ReferenceError"), "{lines:?}");
        assert!(lines[1].contains("[http://127.0.0.1:5199/ (6)]"), "where it happened matters: {lines:?}");
        assert!(lines[2].starts_with("error: stars.map"), "{lines:?}");
    }

    /// Against a real page on a real browser: the one check that proves the
    /// measurement, since everything else here is arithmetic on its answer.
    /// Run it with `cargo test -- --ignored preview_measures` while something
    /// serves `COMPANION_PREVIEW_URL` (see docs/validation).
    #[test]
    #[ignore = "needs a browser and a page being served"]
    fn preview_measures_a_real_page() {
        let url = std::env::var("COMPANION_PREVIEW_URL").unwrap_or_else(|_| "http://127.0.0.1:5291/".into());
        let browser = find_browser().expect("a browser on this PC");
        let shot = std::env::temp_dir().join("companion-preview-live.png");
        let measured = look_with_browser(&url, &browser, Duration::from_secs(12), Some(&shot));
        let measured = measured.unwrap_or_else(|error| panic!("the browser could not be driven: {error}"));
        eprintln!("layout: {:?}", measured.layout);
        let report = look(&url, Duration::from_secs(10), true).expect("the page is served");
        let layout = report.layout.clone().expect("the browser was driven");
        eprintln!("{}", format_report(&report));
        assert!(layout.viewport.0 > 0 && layout.page.1 > layout.viewport.1, "{layout:?}");
        assert!(layout.elements > 3, "{layout:?}");
        assert!(!layout.below_fold.is_empty(), "the content under a full-height sidebar is below the fold: {layout:?}");
        assert!(layout.overflow_x > 0 && !layout.wide.is_empty(), "the wide strip makes the page scroll sideways: {layout:?}");
        assert!(
            report.console.iter().any(|line| line.starts_with("error:") && line.contains("undefinedThing")),
            "the page's own exception is reported: {:?}",
            report.console
        );
        assert!(
            report.console.iter().any(|line| line.contains("dashboard ready")),
            "and so is what it logged: {:?}",
            report.console
        );
        assert_eq!(report.title.as_deref(), Some("Below The Fold"));
        assert!(report.text.contains("nobody ever sees it"), "{}", report.text);
        let picture = report.screenshot.expect("a picture was taken");
        assert!(std::fs::metadata(&picture).map(|meta| meta.len() > 1_000).unwrap_or(false), "{picture:?}");
        let _ = std::fs::remove_file(picture);
    }

    #[test]
    fn the_report_names_a_blank_page_as_a_failure() {
        let blank = PageReport {
            url: "http://localhost:5173/".into(),
            status_line: Some("HTTP/1.1 200 OK".into()),
            title: Some("Vite App".into()),
            text: String::new(),
            console: vec!["error: Uncaught ReferenceError: Planet is not defined".into()],
            dom_bytes: 512,
            screenshot: None,
            browser: Some("browser".into()),
            note: None,
            layout: Some(Layout {
                viewport: (1280, 800),
                page: (1280, 2783),
                elements: 412,
                overflow_x: 0,
                below_fold: vec!["main.main-content 1018x2015 at 0,768".into()],
                ..Layout::default()
            }),
        };
        let text = format_report(&blank);
        assert!(text.contains("the page is blank"), "{text}");
        assert!(text.contains("Uncaught ReferenceError"), "{text}");
        // The measurement is what catches a page that rendered somewhere the
        // first screen does not show.
        assert!(text.contains("window 1280x800, page 1280x2783"), "{text}");
        assert!(text.contains("Below the first screen"), "{text}");
        assert!(text.contains("main.main-content 1018x2015 at 0,768"), "{text}");
        let tidy = PageReport { layout: Some(Layout { viewport: (1280, 800), page: (1280, 800), elements: 40, ..Layout::default() }), ..blank };
        assert!(format_report(&tidy).contains("Nothing is off-screen, oversized or collapsed"), "{}", format_report(&tidy));
        let title_from_dom = title_of("<html><head><TITLE> Neon Observatory </TITLE></head></html>");
        assert_eq!(title_from_dom.as_deref(), Some("Neon Observatory"));
    }
}
