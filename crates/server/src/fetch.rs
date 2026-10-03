//! Job-description fetcher with an SSRF guard applied to every redirect hop (DNS result is pinned to the connection).
use resume_core::jd::parse_jd;
use scraper::{Html, Selector};
use serde_json::{json, Value};
use std::{net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr}, time::Duration};

pub const MAX_BYTES: usize = 2 * 1024 * 1024;
const MAX_REDIRECTS: usize = 5;

#[derive(Debug)]
pub enum FetchErr {
    /// Rejected input (scheme, private address): 400.
    Bad(String),
    /// Page fetched but no usable JD: 422.
    Unusable(String),
    /// Network / upstream: 502.
    Upstream(String),
}

/// Decides whether (host, resolved ip) may be connected to.
pub type Guard<'a> = &'a (dyn Fn(&str, IpAddr) -> bool + Sync);

pub fn is_public(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v) => {
            let o = v.octets();
            !(v.is_loopback() || v.is_private() || v.is_link_local() || v.is_unspecified() || v.is_broadcast() || v.is_multicast() || v.is_documentation()
                || o[0] == 0 || (o[0] == 100 && (64..128).contains(&o[1])) || (o[0] == 192 && o[1] == 0 && o[2] == 0) || o[0] >= 240)
        }
        IpAddr::V6(v) => {
            if let Some(m) = v.to_ipv4_mapped() { return is_public(IpAddr::V4(m)); }
            let s = v.segments();
            // NAT64 64:ff9b::/96 embeds a v4 address
            if s[..6] == [0x64, 0xff9b, 0, 0, 0, 0] { return is_public(IpAddr::V4(Ipv4Addr::new((s[6] >> 8) as u8, s[6] as u8, (s[7] >> 8) as u8, s[7] as u8))); }
            !(v.is_loopback() || v.is_unspecified() || v.is_multicast() || (s[0] & 0xfe00) == 0xfc00 || (s[0] & 0xffc0) == 0xfe80 || v == Ipv6Addr::LOCALHOST)
        }
    }
}

/// Production guard: public addresses only, no `localhost` names.
pub fn public_only(host: &str, ip: IpAddr) -> bool {
    let h = host.trim_end_matches('.').to_lowercase();
    h != "localhost" && !h.ends_with(".localhost") && is_public(ip)
}

pub struct Fetched {
    pub text: String,
    pub title: Option<String>,
    pub company: Option<String>,
    pub source: &'static str,
}

impl Fetched {
    pub fn json(&self) -> Value {
        json!({"text": self.text, "title": self.title, "company": self.company, "source": self.source})
    }
}

async fn get_capped(start: &str, guard: Guard<'_>) -> Result<String, FetchErr> {
    let mut url = reqwest::Url::parse(start).map_err(|_| FetchErr::Bad("not a valid URL".into()))?;
    for _ in 0..=MAX_REDIRECTS {
        if !matches!(url.scheme(), "http" | "https") { return Err(FetchErr::Bad("only http and https URLs are allowed".into())); }
        let host = url.host_str().ok_or_else(|| FetchErr::Bad("URL has no host".into()))?.trim_matches(|c| c == '[' || c == ']').to_string();
        let port = url.port_or_known_default().unwrap_or(80);
        let addrs: Vec<SocketAddr> = tokio::net::lookup_host((host.as_str(), port)).await.map_err(|_| FetchErr::Upstream("could not resolve host".into()))?.collect();
        if addrs.is_empty() { return Err(FetchErr::Upstream("could not resolve host".into())); }
        if addrs.iter().any(|a| !guard(&host, a.ip())) { return Err(FetchErr::Bad("that address is not allowed (private or local network)".into())); }
        let client = reqwest::Client::builder().redirect(reqwest::redirect::Policy::none()).no_proxy().timeout(Duration::from_secs(15))
            .resolve(&host, addrs[0]).user_agent("Mozilla/5.0 (compatible; resume-tailor)").build().map_err(|_| FetchErr::Upstream("http client".into()))?;
        let mut resp = client.get(url.clone()).send().await.map_err(|e| FetchErr::Upstream(if e.is_timeout() { "timed out".into() } else { "could not connect".into() }))?;
        if resp.status().is_redirection() {
            let loc = resp.headers().get("location").and_then(|v| v.to_str().ok()).ok_or_else(|| FetchErr::Upstream("redirect without location".into()))?;
            url = url.join(loc).map_err(|_| FetchErr::Upstream("bad redirect".into()))?;
            continue;
        }
        if !resp.status().is_success() { return Err(FetchErr::Upstream(format!("the site answered HTTP {}", resp.status().as_u16()))); }
        let mut buf = Vec::new();
        while let Some(chunk) = resp.chunk().await.map_err(|_| FetchErr::Upstream("download failed".into()))? {
            if buf.len() + chunk.len() > MAX_BYTES { return Err(FetchErr::Upstream("page is larger than 2 MB".into())); }
            buf.extend_from_slice(&chunk);
        }
        return Ok(String::from_utf8_lossy(&buf).into_owned());
    }
    Err(FetchErr::Upstream("too many redirects".into()))
}

fn collapse(s: &str) -> String {
    let mut out: Vec<String> = vec![];
    for l in s.lines() {
        let l = l.split_whitespace().collect::<Vec<_>>().join(" ");
        if !l.is_empty() || out.last().is_some_and(|p| !p.is_empty() && !p.starts_with("- ")) { out.push(l); }
    }
    out.join("\n").trim().to_string()
}

fn walk(el: scraper::ElementRef, out: &mut String) {
    let name = el.value().name();
    if matches!(name, "script" | "style" | "nav" | "footer" | "noscript" | "svg" | "template" | "head" | "form") { return; }
    let block = matches!(name, "p" | "div" | "br" | "li" | "ul" | "ol" | "tr" | "section" | "article" | "main" | "h1" | "h2" | "h3" | "h4" | "h5" | "h6" | "header");
    if block { out.push('\n'); }
    if name == "li" { out.push_str("- "); }
    for c in el.children() {
        if let Some(t) = c.value().as_text() { out.push_str(t); } else if let Some(e) = scraper::ElementRef::wrap(c) { walk(e, out); }
    }
    if block { out.push('\n'); }
}

fn html_text(html: &str) -> String {
    let mut out = String::new();
    walk(Html::parse_fragment(html).root_element(), &mut out);
    let t = collapse(&out);
    // descriptions are sometimes entity-escaped HTML: decode once more
    if html.contains("&lt;") && t.contains('<') && t.contains('>') { html_text(&t) } else { t }
}

fn sel(s: &str) -> Selector {
    Selector::parse(s).unwrap()
}

/// First JobPosting in any JSON-LD block (handles arrays and @graph).
fn job_posting(doc: &Html) -> Option<Value> {
    fn find(v: &Value) -> Option<Value> {
        match v {
            Value::Array(a) => a.iter().find_map(find),
            Value::Object(o) => {
                let t = o.get("@type").unwrap_or(&Value::Null);
                if t == "JobPosting" || t.as_array().is_some_and(|a| a.iter().any(|x| x == "JobPosting")) { return Some(v.clone()); }
                o.get("@graph").and_then(find)
            }
            _ => None,
        }
    }
    doc.select(&sel("script[type=\"application/ld+json\"]")).find_map(|s| find(&serde_json::from_str(s.text().collect::<String>().trim()).ok()?))
}

/// Parse a fetched page into JD text. Prefers JSON-LD JobPosting, else main/article/body text.
pub fn extract(html: &str) -> Result<Fetched, FetchErr> {
    let doc = Html::parse_document(html);
    let first_text = |q: &str| doc.select(&sel(q)).next().map(|e| e.text().collect::<String>().split_whitespace().collect::<Vec<_>>().join(" ")).filter(|t| !t.is_empty());
    let f = if let Some(jp) = job_posting(&doc) {
        let title = jp["title"].as_str().map(|t| html_text(t));
        let company = jp["hiringOrganization"]["name"].as_str().or(jp["hiringOrganization"].as_str()).map(String::from);
        let desc = html_text(jp["description"].as_str().unwrap_or(""));
        let mut text = String::new();
        if let Some(t) = &title { text.push_str(&format!("{t}\n")); }
        if let Some(c) = &company { text.push_str(&format!("Company: {c}\n")); }
        text.push('\n');
        text.push_str(&desc);
        Fetched { text: text.trim().to_string(), title, company, source: "jsonld" }
    } else {
        let root = ["main", "article", "[role=main]", "body"].iter().find_map(|q| doc.select(&sel(q)).next());
        let mut out = String::new();
        if let Some(r) = root { walk(r, &mut out); }
        let text = collapse(&out);
        let title = first_text("h1").or_else(|| first_text("title"));
        let company = parse_jd(&format!("{}\n{text}", title.clone().unwrap_or_default())).company
            .or_else(|| doc.select(&sel("meta[property=\"og:site_name\"]")).next().and_then(|m| m.value().attr("content").map(String::from)));
        Fetched { text, title, company, source: "html" }
    };
    if f.text.chars().count() < 200 {
        return Err(FetchErr::Unusable("That page has too little readable text (it may need a login or load its content with JavaScript). Open the job in your browser and paste the description text instead.".into()));
    }
    Ok(f)
}

pub async fn fetch_jd(url: &str, guard: Guard<'_>) -> Result<Fetched, FetchErr> {
    extract(&get_capped(url, guard).await?)
}
