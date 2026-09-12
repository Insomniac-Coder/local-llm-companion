//! Opt-in web search (§§116–129).
//!
//! Local-first boundary (§129): inference, history, and files never leave
//! the machine. Only an explicit Search-toggle request performs HTTP — and
//! only to the configured search provider plus pages the model/agent opens.
//! Search runs through tools (`web_search`), never inside the engine (§118).

use serde::{Deserialize, Serialize};
use std::time::Duration;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchResult {
    pub title: String,
    pub url: String,
    pub snippet: String,
    pub source: String,
}

#[derive(Debug, Clone)]
pub struct SearchConfig {
    pub provider: String, // duckduckgo | brave | custom
    pub brave_key: String,
    pub custom_url: String,
    pub max_results: usize,
    pub timeout_secs: u64,
}

impl Default for SearchConfig {
    fn default() -> Self {
        Self {
            provider: "duckduckgo".into(),
            brave_key: String::new(),
            custom_url: String::new(),
            max_results: 5,
            timeout_secs: 15,
        }
    }
}

fn http_client(timeout_secs: u64) -> Result<reqwest::Client, String> {
    reqwest::Client::builder()
        .timeout(Duration::from_secs(timeout_secs.clamp(5, 120)))
        .user_agent("Mozilla/5.0 (Windows NT 10.0; Win64; x64) LocalCompanion/0.1")
        .build()
        .map_err(|e| format!("http client failed: {e}"))
}

fn domain_of(url: &str) -> String {
    url.split("://")
        .nth(1)
        .unwrap_or(url)
        .split('/')
        .next()
        .unwrap_or(url)
        .to_string()
}

/// Search via DuckDuckGo's keyless HTML endpoint. No account, no key.
async fn search_duckduckgo(
    query: &str,
    max: usize,
    timeout: u64,
) -> Result<Vec<SearchResult>, String> {
    let client = http_client(timeout)?;
    let html = client
        .get("https://html.duckduckgo.com/html/")
        .query(&[("q", query)])
        .send()
        .await
        .map_err(|e| format!("provider unreachable: {e}"))?
        .error_for_status()
        .map_err(|e| format!("provider error: {e}"))?
        .text()
        .await
        .map_err(|e| format!("read failed: {e}"))?;
    Ok(parse_ddg(&html, max))
}

fn parse_ddg(html: &str, max: usize) -> Vec<SearchResult> {
    // Result anchors look like:
    // <a rel="nofollow" class="result__a" href="URL">Title</a> ... class="result__snippet">Snippet
    let anchor =
        regex::Regex::new(r#"(?s)class="result__a" href="([^"]+)">(.*?)</a>"#).expect("regex");
    let snippet = regex::Regex::new(r#"(?s)class="result__snippet"[^>]*>(.*?)</"#).expect("regex");
    let tag = regex::Regex::new(r"<[^>]+>").expect("regex");
    let mut out = vec![];
    for cap in anchor.captures_iter(html).take(max) {
        let url = html_unescape(&cap[1]);
        // DDG wraps outbound links in a redirect: //duckduckgo.com/l/?uddg=<target>
        let url = redirect_target(&url);
        let title = tag.replace_all(&cap[2], "").trim().to_string();
        let after = &html[cap.get(0).unwrap().end()..];
        let snip = snippet
            .captures(after)
            .map(|c| tag.replace_all(&c[1], "").trim().to_string())
            .unwrap_or_default();
        if url.is_empty() || title.is_empty() {
            continue;
        }
        let source = domain_of(&url);
        out.push(SearchResult {
            title,
            url,
            snippet: html_unescape(&snip),
            source,
        });
    }
    out
}

fn redirect_target(url: &str) -> String {
    if let Some(q) = url.split("uddg=").nth(1) {
        let enc = q.split('&').next().unwrap_or(q);
        if let Ok(decoded) = urlencoding_like(enc) {
            return decoded;
        }
    }
    url.to_string()
}

fn urlencoding_like(s: &str) -> Result<String, ()> {
    let mut out = String::with_capacity(s.len());
    let bytes = s.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'%' if i + 2 < bytes.len() => {
                let hex = std::str::from_utf8(&bytes[i + 1..i + 3]).map_err(|_| ())?;
                let b = u8::from_str_radix(hex, 16).map_err(|_| ())?;
                out.push(b as char);
                i += 3;
            }
            b'+' => {
                out.push(' ');
                i += 1;
            }
            b => {
                out.push(b as char);
                i += 1;
            }
        }
    }
    Ok(out)
}

fn html_unescape(s: &str) -> String {
    s.replace("&amp;", "&")
        .replace("&quot;", "\"")
        .replace("&#x27;", "'")
        .replace("&#x2F;", "/")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&#39;", "'")
}

/// Brave Search API (needs a key in Settings → Search).
async fn search_brave(
    query: &str,
    max: usize,
    timeout: u64,
    key: &str,
) -> Result<Vec<SearchResult>, String> {
    if key.trim().is_empty() {
        return Err("Brave provider needs an API key (Settings → Search).".into());
    }
    let client = http_client(timeout)?;
    let v: serde_json::Value = client
        .get("https://api.search.brave.com/res/v1/web/search")
        .header("Accept", "application/json")
        .header("X-Subscription-Token", key.trim())
        .query(&[("q", query), ("count", &max.min(20).to_string())])
        .send()
        .await
        .map_err(|e| format!("provider unreachable: {e}"))?
        .error_for_status()
        .map_err(|e| format!("provider error: {e}"))?
        .json()
        .await
        .map_err(|e| format!("bad provider JSON: {e}"))?;
    let mut out = vec![];
    if let Some(items) = v.pointer("/web/results").and_then(|r| r.as_array()) {
        for it in items.iter().take(max) {
            out.push(SearchResult {
                title: it["title"].as_str().unwrap_or("").into(),
                url: it["url"].as_str().unwrap_or("").into(),
                snippet: it["description"].as_str().unwrap_or("").into(),
                source: domain_of(it["url"].as_str().unwrap_or("")),
            });
        }
    }
    Ok(out)
}

pub async fn run_search(
    query: &str,
    cfg: &SearchConfig,
) -> Result<(Vec<SearchResult>, String), String> {
    let q = query.trim();
    if q.is_empty() {
        return Err("query is empty".into());
    }
    let max = cfg.max_results.clamp(1, 10);
    match cfg.provider.as_str() {
        "brave" => search_brave(q, max, cfg.timeout_secs, &cfg.brave_key)
            .await
            .map(|r| (r, "brave".into())),
        "custom" if !cfg.custom_url.trim().is_empty() => {
            Err("custom providers: point custom_url at a DDG-compatible endpoint (Phase 2)".into())
        }
        _ => search_duckduckgo(q, max, cfg.timeout_secs)
            .await
            .map(|r| (r, "duckduckgo".into())),
    }
}

/// Fetch a page and extract readable text (§119 open_page, basic form).
/// Same explicit-consent boundary as search itself.
pub async fn extract_page(url: &str, timeout_secs: u64) -> Result<String, String> {
    if !(url.starts_with("https://") || url.starts_with("http://")) {
        return Err("only http(s) pages can be opened".into());
    }
    let client = http_client(timeout_secs)?;
    let html = client
        .get(url)
        .send()
        .await
        .map_err(|e| format!("page unreachable: {e}"))?
        .error_for_status()
        .map_err(|e| format!("page error: {e}"))?
        .text()
        .await
        .map_err(|e| format!("read failed: {e}"))?;
    Ok(html_to_text(&html))
}

fn html_to_text(html: &str) -> String {
    // NB: the `regex` crate has no backreferences — close with alternation.
    let drop = regex::Regex::new(
        r"(?s)<(script|style|nav|footer|header)[^>]*>.*?</(script|style|nav|footer|header)>",
    )
    .expect("regex");
    let tags = regex::Regex::new(r"<[^>]+>").expect("regex");
    let ws = regex::Regex::new(r"[ \t ]+").expect("regex");
    let text = drop.replace_all(html, " ");
    let text = tags.replace_all(&text, " ");
    let text = html_unescape(&text);
    text.lines()
        .map(|l| ws.replace_all(l.trim(), " ").into_owned())
        .filter(|l| !l.is_empty())
        .collect::<Vec<_>>()
        .join("\n")
        .chars()
        .take(8_000)
        .collect()
}

/// Citations block appended to the answer (§121).
pub fn citations(results: &[SearchResult]) -> String {
    if results.is_empty() {
        return String::new();
    }
    let mut out = String::from("\n\nSources\n");
    for (i, r) in results.iter().enumerate() {
        out.push_str(&format!("{}. {} — {}\n", i + 1, r.title, r.url));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_ddg_results() {
        let html = r#"<a rel="nofollow" class="result__a" href="https://example.com/a">Example A</a><td class="result-snippet">x</td><a class="result__snippet">First snippet here</a>"#;
        let r = parse_ddg(html, 5);
        assert_eq!(r.len(), 1);
        assert_eq!(r[0].title, "Example A");
        assert_eq!(r[0].url, "https://example.com/a");
        assert_eq!(r[0].snippet, "First snippet here");
        assert_eq!(r[0].source, "example.com");
    }

    #[test]
    fn unwraps_ddg_redirect() {
        let html = r#"<a class="result__a" href="//duckduckgo.com/l/?uddg=https%3A%2F%2Fexample.com%2Fdoc">Doc</a>"#;
        let r = parse_ddg(html, 5);
        assert_eq!(r[0].url, "https://example.com/doc");
    }

    #[test]
    fn html_to_text_drops_scripts() {
        let t = html_to_text("<html><script>evil()</script><p>Hello <b>world</b></p></html>");
        assert!(t.contains("Hello world"), "{t}");
        assert!(!t.contains("evil"), "{t}");
    }

    #[test]
    fn citations_numbered() {
        let c = citations(&[SearchResult {
            title: "T".into(),
            url: "https://x/y".into(),
            snippet: "".into(),
            source: "x".into(),
        }]);
        assert!(c.contains("1. T — https://x/y"), "{c}");
    }

    #[test]
    fn empty_query_rejected() {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let err = rt
            .block_on(run_search("   ", &SearchConfig::default()))
            .unwrap_err();
        assert!(err.contains("empty"), "{err}");
    }
}
