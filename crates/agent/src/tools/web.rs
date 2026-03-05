use std::pin::Pin;
use std::future::Future;
use std::sync::Arc;
use serde_json::json;
use reqwest::Client;

use super::Tool;

pub struct WebSearchTool {
    client: Arc<Client>,
    api_key: Option<String>,
}

impl WebSearchTool {
    pub fn new(api_key: Option<String>) -> Self {
        Self {
            client: Arc::new(Client::new()),
            api_key,
        }
    }
}

impl Tool for WebSearchTool {
    fn name(&self) -> &str { "web_search" }
    fn description(&self) -> &str {
        "Search the web using Brave Search API. Returns top results with titles, URLs, and snippets."
    }
    fn parameters(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "query": { "type": "string", "description": "The search query" },
                "count": { "type": "integer", "description": "Number of results (default: 5, max: 10)" }
            },
            "required": ["query"]
        })
    }
    fn execute<'a>(&'a self, args: serde_json::Value) -> Pin<Box<dyn Future<Output = String> + Send + 'a>> {
        let client = Arc::clone(&self.client);
        let api_key = self.api_key.clone();
        Box::pin(async move {
            let query = match args.get("query").and_then(|v| v.as_str()) {
                Some(q) => q.to_string(),
                None => return "Error: missing 'query' argument".to_string(),
            };
            let count = args.get("count").and_then(|v| v.as_u64()).unwrap_or(5).min(10);

            let key = match api_key {
                Some(k) if !k.is_empty() => k,
                _ => return "Error: BRAVE_API_KEY not configured. Set it in ~/.panther/config.toml".to_string(),
            };

            let url = format!("https://api.search.brave.com/res/v1/web/search?q={}&count={}",
                urlencoding(&query), count);

            let resp = match client.get(&url)
                .header("Accept", "application/json")
                .header("Accept-Encoding", "gzip")
                .header("X-Subscription-Token", &key)
                .send()
                .await {
                Ok(r) => r,
                Err(e) => return format!("Search request failed: {}", e),
            };

            if !resp.status().is_success() {
                let status = resp.status();
                return format!("Brave Search API error: {}", status);
            }

            let data: serde_json::Value = match resp.json().await {
                Ok(d) => d,
                Err(e) => return format!("Failed to parse search results: {}", e),
            };

            let mut results = Vec::new();
            if let Some(web) = data.get("web").and_then(|w| w.get("results")).and_then(|r| r.as_array()) {
                for (i, item) in web.iter().enumerate().take(count as usize) {
                    let title = item.get("title").and_then(|v| v.as_str()).unwrap_or("No title");
                    let url = item.get("url").and_then(|v| v.as_str()).unwrap_or("");
                    let desc = item.get("description").and_then(|v| v.as_str()).unwrap_or("");
                    results.push(format!("{}. **{}**\n   URL: {}\n   {}", i+1, title, url, desc));
                }
            }

            if results.is_empty() {
                return "No results found.".to_string();
            }
            format!("Search results for \"{}\":\n\n{}", query, results.join("\n\n"))
        })
    }
}

fn urlencoding(s: &str) -> String {
    let mut encoded = String::new();
    for c in s.chars() {
        match c {
            'A'..='Z' | 'a'..='z' | '0'..='9' | '-' | '_' | '.' | '~' => encoded.push(c),
            ' ' => encoded.push('+'),
            c => {
                for byte in c.to_string().as_bytes() {
                    encoded.push_str(&format!("%{:02X}", byte));
                }
            }
        }
    }
    encoded
}

pub struct WebFetchTool {
    client: Arc<Client>,
}

impl WebFetchTool {
    pub fn new() -> Self {
        Self { client: Arc::new(Client::builder()
            .user_agent("Mozilla/5.0 (compatible; Panther/1.0)")
            .build()
            .expect("Failed to build HTTP client")) }
    }
}

impl Tool for WebFetchTool {
    fn name(&self) -> &str { "web_fetch" }
    fn description(&self) -> &str {
        "Fetch the content of a URL. Returns the text content (HTML stripped to readable text)."
    }
    fn parameters(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "url": { "type": "string", "description": "The URL to fetch" },
                "max_chars": { "type": "integer", "description": "Max characters to return (default: 8000)" }
            },
            "required": ["url"]
        })
    }
    fn execute<'a>(&'a self, args: serde_json::Value) -> Pin<Box<dyn Future<Output = String> + Send + 'a>> {
        let client = Arc::clone(&self.client);
        Box::pin(async move {
            let url = match args.get("url").and_then(|v| v.as_str()) {
                Some(u) => u.to_string(),
                None => return "Error: missing 'url' argument".to_string(),
            };
            let max_chars = args.get("max_chars").and_then(|v| v.as_u64()).unwrap_or(8000) as usize;

            let resp = match client.get(&url).send().await {
                Ok(r) => r,
                Err(e) => return format!("Fetch failed: {}", e),
            };

            if !resp.status().is_success() {
                return format!("HTTP {}: {}", resp.status(), url);
            }

            let body = match resp.text().await {
                Ok(t) => t,
                Err(e) => return format!("Failed to read response: {}", e),
            };

            let text = strip_html(&body);
            if text.chars().count() > max_chars {
                let safe: String = text.chars().take(max_chars).collect();
                format!("{}... (truncated at {} chars)", safe, max_chars)
            } else {
                text
            }
        })
    }
}

fn strip_html(html: &str) -> String {
    let mut result = String::new();
    let mut in_tag = false;
    let mut in_script = false;
    let mut in_style = false;
    let mut buf = String::new();

    let lower = html.to_lowercase();
    let chars: Vec<char> = html.chars().collect();
    let lower_chars: Vec<char> = lower.chars().collect();
    let mut i = 0;

    while i < chars.len() {
        if !in_tag && i + 7 < chars.len() {
            let seg: String = lower_chars[i..i+7].iter().collect();
            if seg.starts_with("<script") { in_script = true; }
            if seg.starts_with("<style") { in_style = true; }
        }
        if !in_tag && i + 9 < chars.len() {
            let seg: String = lower_chars[i..i+9].iter().collect();
            if seg == "</script>" { in_script = false; i += 9; continue; }
            if seg == "</style>" { in_style = false; i += 8; continue; }
        }
        match chars[i] {
            '<' => { in_tag = true; buf.clear(); }
            '>' => {
                in_tag = false;
                let tag = buf.trim().to_lowercase();
                let tag = tag.trim_start_matches('/');
                if ["p","div","br","h1","h2","h3","h4","li","tr"].contains(&tag) {
                    result.push('\n');
                }
            }
            c if !in_tag && !in_script && !in_style => result.push(c),
            c if in_tag => buf.push(c),
            _ => {}
        }
        i += 1;
    }

    let result = result
        .replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&nbsp;", " ")
        .replace("&quot;", "\"")
        .replace("&#39;", "'");

    let mut cleaned = String::new();
    let mut last_newline = false;
    let mut last_space = false;
    for c in result.chars() {
        match c {
            '\n' | '\r' => {
                if !last_newline { cleaned.push('\n'); }
                last_newline = true;
                last_space = false;
            }
            ' ' | '\t' => {
                if !last_space && !last_newline { cleaned.push(' '); }
                last_space = true;
            }
            c => {
                cleaned.push(c);
                last_newline = false;
                last_space = false;
            }
        }
    }
    cleaned.trim().to_string()
}
