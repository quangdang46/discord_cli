//! `discord fetch-links` — download images linked in messages via Discord's
//! CDN proxy (the same `media.discordapp.net` route the in-app Download
//! button uses).
//!
//! Ported from langkurt `internal/discord/fetchlinks.go` + `ogimage.go`
//! (MIT, `.tmp/`). Differs from `download`: attachments (Discord-hosted) are
//! handled by `download` from the archive — this command scans *external*
//! URLs in message text live over REST and pulls them through Discord's proxy
//! so the images are embeddable by Discord clients (proxy strips hotlink
//! protection). Fetch is live, not from the DB, so it sees new messages.

use std::process::ExitCode;
use std::time::Duration;

use anyhow::Result;
use discord_core::config;
use discord_core::output::{self, exit};
use discord_core::stealth::browser_user_agent;

use super::dc::{classify, resolve_channel_id, DcCtx};
use super::download::{parse_since, sanitise_name, time_to_snowflake};

/// Hosts we know how to extract an image from via og:image (langkurt
/// ogimage.go:14-24). fxtwitter/vxtwitter/pxiv are the "better embed"
/// mirrors for x.com/pixiv — same content, better og tags.
const SUPPORTED_DOMAINS: &[&str] = &[
    "x.com",
    "twitter.com",
    "fxtwitter.com",
    "fixupx.com",
    "vxtwitter.com",
    "fixvx.com",
    "pixiv.net",
    "phixiv.net",
    "artstation.com",
    "imgur.com",
    "i.imgur.com",
    "i.redd.it",
];

/// Image extensions that make a bare URL directly fetchable without a page
/// scrape (langkurt treats these implicitly; kept explicit here so the
/// direct-image path also goes through the Discord proxy).
const IMAGE_EXTS: &[&str] = &["png", "jpg", "jpeg", "gif", "webp", "avif", "bmp"];

/// Timeout per HTTP request (langkurt uses 20s; 10s per the design).
const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);
/// Politeness delay between external-site fetches (langkurt ogimage.go:33-38).
const DEFAULT_DELAY: Duration = Duration::from_millis(800);
/// Long-form domains that get a gentler pace (scrape-heavy, bot-hostile).
const SLOW_DOMAINS: &[&str] = &["pixiv.net", "phixiv.net"];
const SLOW_DELAY: Duration = Duration::from_millis(1500);

/// Lowercased host of an http(s) URL, minus port (`https://www.x.com:8443/a`
/// → `www.x.com`). Returns None for non-URL strings.
fn host_of(raw_url: &str) -> Option<String> {
    let rest = raw_url
        .strip_prefix("https://")
        .or_else(|| raw_url.strip_prefix("http://"))?;
    let host = rest.split(['/', '?', '#']).next().unwrap_or("");
    let host = host.split(':').next().unwrap_or(host); // strip port
    let host = host.to_lowercase();
    if host.is_empty() {
        None
    } else {
        Some(host)
    }
}

/// Lowercased path of an http(s) URL, without query/fragment
/// (`https://a.com/x/img.png?w=1` → `/x/img.png`).
fn path_of(raw_url: &str) -> String {
    let Some(rest) = raw_url
        .strip_prefix("https://")
        .or_else(|| raw_url.strip_prefix("http://"))
    else {
        return String::new();
    };
    let path = rest.split(['?', '#']).next().unwrap_or("");
    path.to_lowercase()
}

/// True if `raw_url` points at a host we know how to extract an image from
/// (exact host or subdomain of a supported domain).
fn is_supported_link_domain(raw_url: &str) -> bool {
    let Some(host) = host_of(raw_url) else {
        return false;
    };
    SUPPORTED_DOMAINS
        .iter()
        .any(|d| host == *d || host.ends_with(&format!(".{d}")))
}

/// True if the URL path ends in a known image extension (stripping query
/// strings, like `?width=800`).
fn is_direct_image_url(raw_url: &str) -> bool {
    let path = path_of(raw_url);
    IMAGE_EXTS
        .iter()
        .any(|ext| path.ends_with(&format!(".{ext}")))
}

/// All http(s) URLs in `content` worth processing: supported embed domains
/// plus any direct image URL (langkurt ExtractURLs ogimage.go:54-64, widened
/// to bare image links per the design). media.discordapp.net is excluded —
/// those are already-discord-hosted attachments covered by `download`.
fn extract_urls(content: &str) -> Vec<String> {
    let mut seen = std::collections::HashSet::new();
    let mut out = Vec::new();
    let bytes = content.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        // Scan for the next scheme prefix.
        let scheme = if bytes[i..].starts_with(b"https://") {
            "https://"
        } else if bytes[i..].starts_with(b"http://") {
            "http://"
        } else {
            i += 1;
            continue;
        };
        // Extend to the URL terminator: whitespace or <>"' chars.
        let start = i;
        let mut end = i + scheme.len();
        while end < bytes.len()
            && !matches!(
                bytes[end],
                b' ' | b'\t' | b'\n' | b'\r' | b'<' | b'>' | b'"' | b'\''
            )
        {
            end += 1;
        }
        i = end;
        let raw = &content[start..end];
        // Trim markdown-ish trailing punctuation from linkified URLs.
        let raw = raw.trim_end_matches([')', ']', '.', ',', ';', '!', '?']);
        if raw.contains("discordapp.net") || raw.contains("discord.com") {
            continue;
        }
        if seen.contains(raw) {
            continue;
        }
        if is_supported_link_domain(raw) || is_direct_image_url(raw) {
            seen.insert(raw.to_string());
            out.push(raw.to_string());
        }
    }
    out
}

/// Value of an HTML attribute like `property="..."` or `content='...'`
/// (case-insensitive name, either quote style). Returns None if absent.
fn html_attr(tag: &str, name: &str) -> Option<String> {
    let lower = tag.to_lowercase();
    let needle = format!("{name}=");
    let rest = lower.as_str();
    let mut from = 0;
    while let Some(pos) = rest[from..].find(&needle) {
        let eq = from + pos + needle.len();
        let quote = rest[eq..].chars().next()?;
        if quote != '"' && quote != '\'' {
            from = eq;
            continue;
        }
        let open = eq + quote.len_utf8();
        let close = rest[open..].find(quote)? + open;
        return Some(tag[open..close].to_string());
    }
    None
}

/// og:image extraction from an HTML head (langkurt ogimage.go:27-28, 92-105).
/// Hand-rolled scanner: splits on `<meta`, then checks the tag for the
/// og:image property and pulls its `content` attr (attribute order varies
/// between sites).
fn extract_og_image(html: &str) -> Option<String> {
    for part in html.split("<meta") {
        let tag = part.split('>').next().unwrap_or(part);
        let lower = tag.to_lowercase();
        if !(lower.contains("og:image")) {
            continue;
        }
        let url = html_attr(tag, "content")?;
        let url = url.trim();
        if url.is_empty() {
            return None;
        }
        return Some(url.to_string());
    }
    None
}

/// Politeness delay for a given URL's host: slow pace for bot-hostile
/// scrape targets, default pace otherwise (langkurt DelayForURL
/// ogimage.go:108-124).
fn delay_for_url(raw_url: &str) -> Duration {
    let Some(host) = host_of(raw_url) else {
        return DEFAULT_DELAY;
    };
    if SLOW_DOMAINS
        .iter()
        .any(|d| host == *d || host.ends_with(&format!(".{d}")))
    {
        SLOW_DELAY
    } else {
        DEFAULT_DELAY
    }
}

/// Browser-ish request headers so hotlink-protected hosts don't block us
/// (langkurt ogimage.go:72-74).
async fn fetch_head(client: &reqwest::Client, url: &str) -> Result<reqwest::Response> {
    Ok(client
        .get(url)
        .header(reqwest::header::USER_AGENT, browser_user_agent())
        .header(
            reqwest::header::ACCEPT,
            "text/html,application/xhtml+xml,application/xml;q=0.9,*/*;q=0.8",
        )
        .send()
        .await?)
}

/// Resolve `page_url` to a downloadable image URL:
/// 1. Discord CDN proxy (`media.discordapp.net/{url}?width=800` — the same
///    route the client's Download button hits). Only valid when the upstream
///    answers with an image content-type, otherwise the proxy refuses.
/// 2. Fallback: scrape the page's og:image meta tag and fetch that directly.
async fn resolve_image_url(client: &reqwest::Client, page_url: &str) -> Result<String> {
    // Direct image URLs (bare .png/.jpg/.webp/…) are already the final asset —
    // proxying them via media.discordapp.net 401s for cdn.discordapp.com and
    // other hosts, and there's no og:image to scrape. Download them as-is.
    if is_direct_image_url(page_url) {
        return Ok(page_url.to_string());
    }
    // Fast path: try Discord's proxy first (discordapp.net excluded earlier,
    // so this is always an external host being proxied). Strip the scheme so
    // the proxy URL is `https://media.discordapp.net/<host>/<path>?width=800`.
    let rest = page_url
        .strip_prefix("https://")
        .or_else(|| page_url.strip_prefix("http://"))
        .unwrap_or(page_url);
    let proxy = format!("https://media.discordapp.net/{rest}?width=800");
    if let Ok(resp) = fetch_head(client, &proxy).await {
        if let Some(ct) = resp
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
        {
            if ct.starts_with("image/") {
                return Ok(proxy);
            }
        }
    }
    // Slow path: scrape og:image.
    let html = fetch_head(client, page_url).await?.error_for_status()?;
    let body = html.bytes().await?;
    let body = String::from_utf8_lossy(&body[..body.len().min(128 * 1024)]);
    extract_og_image(&body).ok_or_else(|| anyhow::anyhow!("no og:image on page"))
}

/// Download `img_url` into `dest_dir`, deriving the filename from the URL
/// path with a short md5(message_id|url) prefix to avoid collisions (langkurt
/// downloadLinkImage fetchlinks.go:134-174). Returns `(path, skipped)` —
/// `skipped` is true when the file was already on disk (dedupe).
async fn download_image(
    client: &reqwest::Client,
    img_url: &str,
    message_id: &str,
    dest_dir: &std::path::Path,
) -> Result<(std::path::PathBuf, bool)> {
    let stem = img_url.split('?').next().unwrap_or("image");
    let mut name = stem.rsplit('/').next().unwrap_or("image").to_string();
    if name.is_empty() || name == "." || name == "/" {
        name = "image.jpg".into();
    }
    let name = sanitise_name(&name);
    let suffix = attachment_id(message_id, img_url);
    let local = dest_dir.join(format!("{}_{}", &suffix[..suffix.len().min(8)], name));
    if local.exists() {
        return Ok((local, true));
    }
    let resp = client.get(img_url).send().await?.error_for_status()?;
    let bytes = resp.bytes().await?;
    std::fs::write(&local, &bytes)?;
    Ok((local, false))
}

/// md5 hex of `msg_id|url` — stable filename prefix for dedupe/collision
/// avoidance (langkurt LinkID fetchlinks.go:37-39; same key the sync ledger
/// uses, sync.rs:14-23).
fn attachment_id(message_id: &str, url: &str) -> String {
    use md5::{Digest, Md5};
    let mut h = Md5::new();
    h.update(message_id.as_bytes());
    h.update(b"|");
    h.update(url.as_bytes());
    format!("{:x}", h.finalize())
}

/// Options for `dc fetch-links` (avoids clippy too_many_arguments).
pub struct FetchLinksOpts<'a> {
    pub channel: &'a str,
    pub since: Option<&'a str>,
    pub limit: usize,
    pub out: Option<&'a str>,
}

/// `discord fetch-links <CH> [--since S] [--limit N] [--out DIR]`
///
/// Scans recent messages live for external image links and pulls each
/// through Discord's CDN proxy (with og:image fallback), saving to
/// `<out>/<channel>/`. Emits `{fetched, skipped, failed, files}`.
pub async fn cmd_fetch_links(ctx: &DcCtx, opts: FetchLinksOpts<'_>) -> ExitCode {
    // Validate --since early (usage exit 2, mirrors download.rs).
    let since_snowflake: Option<u64> = match opts.since {
        Some(s) => match parse_since(s) {
            Some(t) => Some(time_to_snowflake(t)),
            None => {
                eprintln!("invalid --since: \"{s}\" (use YYYY-MM-DD or 30d/6m/1y)");
                return ExitCode::from(exit::USAGE);
            }
        },
        None => None,
    };

    let mut client = match ctx.client().await {
        Ok(c) => c,
        Err(code) => return code,
    };
    let channel_id = match resolve_channel_id(&mut client, opts.channel).await {
        Ok(id) => id,
        Err(code) => return code,
    };

    // Fetch messages live: after-cursor from --since, newest-first up to
    // --limit (100 default). fetch_messages returns ascending — the
    // furthest-back messages first — fine for scanning.
    let limit = if opts.limit == 0 { 100 } else { opts.limit };
    let after: Option<u64> = since_snowflake;
    let msgs = match client.fetch_messages(&channel_id, limit, None, after).await {
        Ok(m) => m,
        Err(e) => {
            return ExitCode::from(output::emit_error("ApiError", &e.to_string(), classify(&e)))
        }
    };

    // Collect (message_id, url) pairs across the batch.
    let mut links: Vec<(String, String)> = Vec::new();
    for m in &msgs {
        for u in extract_urls(&m.content) {
            links.push((m.message_id.clone(), u));
        }
    }
    if links.is_empty() {
        let data = serde_json::json!({ "fetched": 0, "skipped": 0, "failed": 0, "files": [] });
        let _ = output::emit(&data, ctx.format);
        return ExitCode::from(exit::OK);
    }

    // Output dir: <out>/<channel> (langkurt fetchlinks.go:76-86 nests a
    // guild/channel/links tree; here a channel segment is enough context).
    let out_root = opts.out.map(|s| s.to_string()).unwrap_or_else(|| {
        config::data_dir()
            .map(|p| p.join("media-links").to_string_lossy().into_owned())
            .unwrap_or_else(|_| "media-links".into())
    });
    let dest_dir = std::path::Path::new(&out_root).join(sanitise_name(opts.channel));
    if let Err(e) = std::fs::create_dir_all(&dest_dir) {
        eprintln!("cannot create dir {}: {e}", dest_dir.display());
        return ExitCode::from(exit::ERROR);
    }

    let http = match reqwest::Client::builder()
        .user_agent(browser_user_agent())
        .timeout(REQUEST_TIMEOUT)
        .build()
    {
        Ok(c) => c,
        Err(e) => {
            return ExitCode::from(output::emit_error("HttpError", &e.to_string(), exit::ERROR))
        }
    };

    let mut fetched = 0usize;
    let mut skipped = 0usize;
    let mut failed = 0usize;
    let mut files: Vec<String> = Vec::new();
    let total = links.len();

    for (i, (message_id, page_url)) in links.iter().enumerate() {
        eprint!("\rFetching {}... ({}/{})", page_url, i + 1, total);
        // Only pace when we actually hit the external site (langkurt
        // fetchlinks.go:124-127: Discord's proxy is fine with fast requests,
        // so skip the delay when the proxy path worked).
        match resolve_image_url(&http, page_url).await {
            Ok(img_url) => match download_image(&http, &img_url, message_id, &dest_dir).await {
                Ok((path, was_skipped)) => {
                    if was_skipped {
                        skipped += 1;
                    } else {
                        fetched += 1;
                    }
                    let p = path.to_string_lossy().into_owned();
                    if !files.contains(&p) {
                        files.push(p);
                    }
                }
                Err(e) => {
                    eprintln!("\rfailed to download {}: {e}", page_url);
                    failed += 1;
                }
            },
            Err(e) => {
                eprintln!("\rfailed to resolve {}: {e}", page_url);
                failed += 1;
            }
        }
        eprint!("\r\x1b[K");
        tokio::time::sleep(delay_for_url(page_url)).await;
    }

    let data = serde_json::json!({
        "fetched": fetched,
        "skipped": skipped,
        "failed": failed,
        "files": files,
    });
    let _ = output::emit(&data, ctx.format);
    ExitCode::from(exit::OK)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn supported_domain_matches_exact_and_subdomain() {
        assert!(is_supported_link_domain("https://x.com/some/status/1"));
        assert!(is_supported_link_domain("https://twitter.com/a/status/1"));
        assert!(is_supported_link_domain(
            "https://www.pixiv.net/en/artworks/1"
        ));
        assert!(is_supported_link_domain("https://i.imgur.com/abc.png"));
        assert!(is_supported_link_domain(
            "https://cdn.artstation.com/p/assets/1.png"
        ));
        // reddit.com (the page) is not a supported host — only i.redd.it
        // direct image links are.
        assert!(!is_supported_link_domain(
            "https://www.reddit.com/r/x/comments/1"
        ));
        assert!(is_supported_link_domain("https://i.redd.it/abc123.png"));
        // Non-supported hosts.
        assert!(!is_supported_link_domain("https://example.com/photo.png"));
        assert!(!is_supported_link_domain(
            "https://cdn.discordapp.net/attachments/1/x.png"
        ));
        assert!(!is_supported_link_domain("not a url"));
    }

    #[test]
    fn direct_image_url_detects_extensions() {
        assert!(is_direct_image_url("https://example.com/a/b/cat.png"));
        assert!(is_direct_image_url("https://example.com/a/cat.JPG"));
        assert!(is_direct_image_url(
            "https://example.com/cat.webp?width=800"
        ));
        assert!(!is_direct_image_url("https://example.com/cat"));
        assert!(!is_direct_image_url("https://example.com/page.html"));
    }

    #[test]
    fn extract_urls_filters_and_dedupes() {
        let content = concat!(
            "see https://x.com/a/status/1 and https://pixiv.net/en/artworks/2\n",
            "direct: https://i.redd.it/abc123.png\n",
            "attachment: https://media.discordapp.net/attachments/1/2/img.png\n",
            "plain: https://example.com/note\n",
            "again https://x.com/a/status/1"
        );
        let urls = extract_urls(content);
        assert_eq!(urls.len(), 3);
        assert!(urls.iter().any(|u| u.contains("x.com")));
        assert!(urls.iter().any(|u| u.contains("pixiv.net")));
        assert!(urls.iter().any(|u| u.contains("i.redd.it")));
        assert!(!urls.iter().any(|u| u.contains("discordapp.net")));
        assert!(!urls.iter().any(|u| u.contains("example.com/note")));
    }

    #[test]
    fn og_image_extraction_any_attribute_order() {
        let html = r#"<html><head>
            <meta property="og:image" content="https://cdn.x.com/photo.jpg">
        </head></html>"#;
        assert_eq!(
            extract_og_image(html).as_deref(),
            Some("https://cdn.x.com/photo.jpg")
        );
        // content before property (pixiv-style order).
        let html2 =
            r#"<meta content="https://i.pximg.net/img-original/1.png" property="og:image">"#;
        assert_eq!(
            extract_og_image(html2).as_deref(),
            Some("https://i.pximg.net/img-original/1.png")
        );
        // No og:image → None.
        assert_eq!(
            extract_og_image("<meta property=\"og:title\" content=\"hi\">"),
            None
        );
        // Empty content → None.
        assert_eq!(
            extract_og_image(r#"<meta property="og:image" content="">"#),
            None
        );
    }

    #[test]
    fn filename_sanitise_reused_from_download() {
        // The download.rs sanitise helper is what fetch-links uses for
        // filenames — verify hostile URL-path chars are tamed.
        let name = sanitise_name("a?b*c:d.png");
        assert_eq!(name, "a_b_c_d.png");
    }

    #[test]
    fn attachment_id_is_stable_md5() {
        // md5("42|https://x.png") — same key the sync ledger uses
        // (sync.rs:204). Stable + deterministic.
        let a = attachment_id("42", "https://x.png");
        let b = attachment_id("42", "https://x.png");
        assert_eq!(a, b);
        assert_eq!(a.len(), 32);
        assert!(a.chars().all(|c| c.is_ascii_hexdigit()));
        assert_ne!(a, attachment_id("43", "https://x.png"));
    }

    #[test]
    fn delays_are_positive() {
        assert!(delay_for_url("https://example.com/1") >= DEFAULT_DELAY);
        assert!(delay_for_url("https://pixiv.net/en/artworks/1") >= SLOW_DELAY);
    }
}
