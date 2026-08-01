//! Unit tests for `web_runtime::run_webfetch` (no external network).

use super::*;

fn build_client() -> reqwest::Client {
    reqwest::Client::builder()
        .build()
        .expect("reqwest client build")
}

fn args(url: &str) -> FetchArgs {
    FetchArgs {
        url: url.to_owned(),
        format: None,
        timeout: None,
    }
}

// ---------------------------------------------------------------------------
// URL validation (pure logic, no HTTP)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn rejects_non_https_scheme() {
    let client = build_client();
    for bad in [
        "ftp://example.com",
        "file:///etc/passwd",
        "javascript:alert(1)",
        "http://example.com",
        "",
    ] {
        let err = run_webfetch(&client, args(bad)).await.unwrap_err();
        assert!(err.to_string().contains("HTTPS"), "bad={bad} err={err}");
    }
}

#[tokio::test]
async fn rejects_http_urls() {
    let client = build_client();
    let err = run_webfetch(&client, args("http://example.com"))
        .await
        .unwrap_err();
    assert!(
        err.to_string().contains("HTTPS"),
        "HTTP should be rejected: {err}"
    );
}

// ---------------------------------------------------------------------------
// Content-type branches
// ---------------------------------------------------------------------------

fn fetch_output_from_test_parts(
    body: &[u8],
    content_type: &str,
    format: Option<FetchFormat>,
) -> Result<FetchOutput> {
    let fmt = format.unwrap_or_default();
    fetch_output_from_bytes(
        StatusCode::OK,
        content_type.to_owned(),
        body,
        "https://example.test/resource",
        &fmt,
    )
}

#[test]
fn html_to_markdown() {
    let out = fetch_output_from_test_parts(
        b"<html><body><h1>Hello</h1><p>World</p></body></html>",
        "text/html; charset=utf-8",
        None,
    )
    .expect("ok");
    assert!(
        out.output.contains("Hello"),
        "missing Hello: {}",
        out.output
    );
    assert!(
        out.output.contains("World"),
        "missing World: {}",
        out.output
    );
    assert!(
        out.output.contains('#') || !out.output.contains("<h1>"),
        "should be markdown not HTML: {}",
        out.output
    );
    assert_eq!(out.format, "markdown");
    assert!(out.attachments.is_empty());
}

#[test]
fn text_plain_passthrough() {
    let out = fetch_output_from_test_parts(b"just some text", "text/plain", None).expect("ok");
    assert_eq!(out.output, "just some text");
}

#[test]
fn json_pretty_print() {
    let out = fetch_output_from_test_parts(br#"{"a":1,"b":[2,3]}"#, "application/json", None)
        .expect("ok");
    assert!(
        out.output.starts_with("```json\n"),
        "missing fence: {}",
        out.output
    );
    assert!(
        out.output.contains("\"a\": 1"),
        "not pretty: {}",
        out.output
    );
    assert!(out.output.ends_with("\n```"));
}

#[test]
fn image_attachment_base64() {
    // 1x1 transparent PNG
    let png_bytes: Vec<u8> = vec![
        0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0x00, 0x00, 0x00, 0x0D, 0x49, 0x48, 0x44,
        0x52, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x06, 0x00, 0x00, 0x00, 0x1F,
        0x15, 0xC4, 0x89, 0x00, 0x00, 0x00, 0x0A, 0x49, 0x44, 0x41, 0x54, 0x78, 0x9C, 0x63, 0x00,
        0x01, 0x00, 0x00, 0x05, 0x00, 0x01, 0x0D, 0x0A, 0x2D, 0xB4, 0x00, 0x00, 0x00, 0x00, 0x49,
        0x45, 0x4E, 0x44, 0xAE, 0x42, 0x60, 0x82,
    ];
    let out = fetch_output_from_test_parts(&png_bytes, "image/png", None).expect("ok");
    assert_eq!(out.attachments.len(), 1);
    let att = &out.attachments[0];
    assert_eq!(att.mime, "image/png");
    assert!(att.url.starts_with("data:image/png;base64,"));
    let b64 = att.url.trim_start_matches("data:image/png;base64,");
    let decoded = BASE64.decode(b64).expect("decode");
    assert_eq!(decoded, png_bytes);
}

// ---------------------------------------------------------------------------
// format parameter
// ---------------------------------------------------------------------------

#[test]
fn format_html_returns_raw() {
    let raw = "<html><body><h1>Raw</h1></body></html>";
    let out = fetch_output_from_test_parts(raw.as_bytes(), "text/html", Some(FetchFormat::Html))
        .expect("ok");
    assert_eq!(out.output, raw);
    assert_eq!(out.format, "html");
}

#[test]
fn format_text_strips_html() {
    let out = fetch_output_from_test_parts(
        b"<html><body><p>One</p><p>Two</p><script>alert(1)</script></body></html>",
        "text/html",
        Some(FetchFormat::Text),
    )
    .expect("ok");
    assert!(out.output.contains("One"));
    assert!(out.output.contains("Two"));
    assert!(
        !out.output.contains("alert(1)"),
        "script content should be stripped: {}",
        out.output
    );
    assert_eq!(out.format, "text");
}

#[test]
fn default_format_is_markdown() {
    let out =
        fetch_output_from_test_parts(b"<html><body><h2>x</h2></body></html>", "text/html", None)
            .unwrap();
    assert_eq!(out.format, "markdown");
}

#[test]
fn accept_header_negotiation_for_markdown() {
    assert!(
        FetchFormat::Markdown
            .accept_header()
            .starts_with("text/markdown;q=1.0")
    );
}

// ---------------------------------------------------------------------------
// Size / status
// ---------------------------------------------------------------------------

#[test]
fn rejects_oversized_content_length() {
    let err = validate_fetch_response_metadata(
        StatusCode::OK,
        Some(MAX_RESPONSE_SIZE + 1024),
        "https://example.test/big",
    )
    .unwrap_err();
    let msg = err.to_string();
    assert!(msg.contains("too large"), "got: {msg}");
}

#[test]
fn http_error_status_propagates() {
    let err =
        validate_fetch_response_metadata(StatusCode::NOT_FOUND, None, "https://example.test/404")
            .unwrap_err();
    assert!(err.to_string().contains("404"), "got: {err}");
}

// ---------------------------------------------------------------------------
// SSRF: is_blocked_ip coverage tests
// ---------------------------------------------------------------------------

#[test]
fn blocked_ip_ipv4_basics() {
    use std::net::IpAddr;
    for blocked in [
        "127.0.0.1",
        "10.0.0.1",
        "172.16.0.1",
        "192.168.1.1",
        "169.254.1.1",
        "0.0.0.0",
        "0.0.0.1",       // 0.0.0.0/8 this-host range
        "0.255.255.255", // 0.0.0.0/8 upper bound
        "255.255.255.255",
        "100.64.0.1",      // CGNAT
        "192.0.2.1",       // TEST-NET-1
        "198.51.100.1",    // TEST-NET-2
        "203.0.113.1",     // TEST-NET-3
        "198.18.0.1",      // benchmarking address
        "224.0.0.1",       // multicast
        "239.255.255.255", // multicast upper bound
        "240.0.0.1",       // reserved address
    ] {
        let ip: IpAddr = blocked.parse().unwrap();
        assert!(is_blocked_ip(ip), "should block {blocked}");
    }
    // Public IPs must not be blocked by mistake.
    for allowed in ["8.8.8.8", "1.1.1.1", "93.184.216.34"] {
        let ip: IpAddr = allowed.parse().unwrap();
        assert!(!is_blocked_ip(ip), "should allow {allowed}");
    }
}

#[test]
fn blocked_ip_ipv4_mapped_ipv6() {
    use std::net::IpAddr;
    // ::ffff:127.0.0.1 is IPv4-mapped IPv6 and must be blocked as IPv4 loopback.
    let mapped_loopback: IpAddr = "::ffff:127.0.0.1".parse().unwrap();
    assert!(
        is_blocked_ip(mapped_loopback),
        "::ffff:127.0.0.1 must be blocked"
    );

    let mapped_private: IpAddr = "::ffff:10.0.0.1".parse().unwrap();
    assert!(
        is_blocked_ip(mapped_private),
        "::ffff:10.0.0.1 must be blocked"
    );

    let mapped_link_local: IpAddr = "::ffff:169.254.1.1".parse().unwrap();
    assert!(
        is_blocked_ip(mapped_link_local),
        "::ffff:169.254.1.1 must be blocked"
    );

    // ::ffff:8.8.8.8 corresponds to public IPv4 and must not be blocked by mistake.
    let mapped_public: IpAddr = "::ffff:8.8.8.8".parse().unwrap();
    assert!(
        !is_blocked_ip(mapped_public),
        "::ffff:8.8.8.8 should be allowed"
    );
}

#[test]
fn blocked_ip_ipv6_ranges() {
    use std::net::IpAddr;
    for blocked in [
        "::1",         // loopback
        "::",          // unspecified
        "fc00::1",     // unique-local
        "fe80::1",     // link-local
        "ff00::1",     // multicast
        "2001:db8::1", // documentation example address
    ] {
        let ip: IpAddr = blocked.parse().unwrap();
        assert!(is_blocked_ip(ip), "should block {blocked}");
    }
    // Public IPv6 must not be blocked by mistake.
    let public: IpAddr = "2606:4700:4700::1111".parse().unwrap();
    assert!(!is_blocked_ip(public), "public IPv6 should be allowed");
}

// ---------------------------------------------------------------------------
// SSRF redirect protection
// ---------------------------------------------------------------------------

#[tokio::test]
async fn ssrf_safe_client_builds_with_redirect_policy() {
    let client = build_ssrf_safe_client().expect("build client");
    // Verify the client builds successfully with the custom SSRF redirect policy and DNS resolver.
    // Redirect behavior is covered through the pure policy inputs; no local socket is needed here.
    assert!(client.get("https://example.invalid").build().is_ok());
}

// ---------------------------------------------------------------------------
// FetchOutput serialization tests
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// Real-endpoint smoke tests are skipped by default, to avoid CI or offline dev environments depending on the external network.
// Set WARP_RUN_WEB_INTEGRATION=1 to manually verify against real endpoints.
// ---------------------------------------------------------------------------

fn skip_real() -> bool {
    std::env::var("WARP_RUN_WEB_INTEGRATION").is_err()
}

#[tokio::test]
async fn real_example_com_markdown() {
    if skip_real() {
        return;
    }
    let client = build_ssrf_safe_client().expect("build client");
    let out = run_webfetch(&client, args("https://example.com"))
        .await
        .expect("real example.com");
    assert!(
        out.output.to_lowercase().contains("example domain"),
        "got: {}",
        out.output
    );
}

#[tokio::test]
async fn real_httpbin_html_to_markdown() {
    if skip_real() {
        return;
    }
    let client = build_ssrf_safe_client().expect("build client");
    let out = run_webfetch(&client, args("https://httpbin.org/html"))
        .await
        .expect("real httpbin html");
    assert!(!out.output.trim().is_empty());
    assert_eq!(out.format, "markdown");
}

#[tokio::test]
async fn real_httpbin_json_pretty() {
    if skip_real() {
        return;
    }
    let client = build_ssrf_safe_client().expect("build client");
    let out = run_webfetch(&client, args("https://httpbin.org/json"))
        .await
        .expect("real httpbin json");
    assert!(out.output.contains("```json"), "got: {}", out.output);
}

#[tokio::test]
async fn real_httpbin_image_attachment() {
    if skip_real() {
        return;
    }
    let client = build_ssrf_safe_client().expect("build client");
    let out = run_webfetch(&client, args("https://httpbin.org/image/png"))
        .await
        .expect("real png");
    assert_eq!(out.attachments.len(), 1);
    assert_eq!(out.attachments[0].mime, "image/png");
}

#[tokio::test]
async fn real_httpbin_404_errors() {
    if skip_real() {
        return;
    }
    let client = build_ssrf_safe_client().expect("build client");
    let err = run_webfetch(&client, args("https://httpbin.org/status/404"))
        .await
        .unwrap_err();
    assert!(err.to_string().contains("404"), "got: {err}");
}

// ---------------------------------------------------------------------------
// Description doc / byte-level alignment with opencode regression
// ---------------------------------------------------------------------------

/// Locks webfetch.md to be byte-level identical with opencode `packages/opencode/src/tool/webfetch.txt`.
/// When modifying, both sides need to be kept in sync.
#[test]
fn webfetch_description_matches_opencode_verbatim() {
    use super::super::webfetch::WEBFETCH;
    let expected = "- Fetches content from a specified URL\n\
                    - Takes a URL and optional format as input\n\
                    - Fetches the URL content, converts to requested format (markdown by default)\n\
                    - Returns the content in the specified format\n\
                    - Use this tool when you need to retrieve and analyze web content\n\
                    \n\
                    Usage notes:\n\
                    \x20\x20- IMPORTANT: if another tool is present that offers better web fetching capabilities, is more targeted to the task, or has fewer restrictions, prefer using that tool instead of this one.\n\
                    \x20\x20- The URL must be a fully-formed valid URL\n\
                    \x20\x20- The URL must use HTTPS (http:// URLs are rejected)\n\
                    \x20\x20- Format options: \"markdown\" (default), \"text\", or \"html\"\n\
                    \x20\x20- This tool is read-only and does not modify any files\n\
                    \x20\x20- Results may be summarized if the content is very large\n";
    assert_eq!(WEBFETCH.description, expected);
}

#[test]
fn fetch_output_omits_empty_attachments_in_json() {
    let out = FetchOutput {
        url: "https://x".into(),
        status: 200,
        content_type: "text/plain".into(),
        format: "markdown".into(),
        output: "hi".into(),
        attachments: vec![],
    };
    let v = fetch_output_to_json(&out);
    assert!(
        v.get("attachments").is_none(),
        "empty attachments should be skipped: {v}"
    );
    assert_eq!(v["output"], "hi");
}

/// The `_byop_intercepted` sentinel must be present in all web tool results (including errors),
/// otherwise the controller (`controller.rs::needs_byop_local_resume`) won't trigger auto-resume,
/// and the model gets stuck waiting for a result, with the UI showing a silent failure.
#[test]
fn fetch_output_carries_byop_sentinel() {
    let out = FetchOutput {
        url: "https://x".into(),
        status: 200,
        content_type: "text/plain".into(),
        format: "markdown".into(),
        output: "hi".into(),
        attachments: vec![],
    };
    let v = fetch_output_to_json(&out);
    assert_eq!(v["_byop_intercepted"], true);

    let err = error_to_json("webfetch", &anyhow::anyhow!("boom"));
    assert_eq!(err["_byop_intercepted"], true);
    assert_eq!(err["status"], "error");
}
