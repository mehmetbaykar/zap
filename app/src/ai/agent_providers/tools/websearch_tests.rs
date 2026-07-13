//! Unit tests for `web_runtime::run_websearch` (no external network).

use super::*;

fn build_client() -> reqwest::Client {
    reqwest::Client::builder().build().expect("client")
}

fn search_args(query: &str) -> SearchToolArgs {
    SearchToolArgs {
        query: query.to_owned(),
        num_results: None,
        livecrawl: None,
        search_type: None,
        context_max_characters: None,
    }
}

fn sse_body(text: &str) -> String {
    format!(
        "event: message\ndata: {{\"result\":{{\"content\":[{{\"type\":\"text\",\"text\":{}}}]}}}}\n\n",
        serde_json::to_string(text).unwrap()
    )
}

fn build_test_request(args: SearchToolArgs) -> (String, reqwest::Request) {
    build_websearch_request(
        &build_client(),
        args,
        None,
        Some("https://example.test/mcp"),
    )
    .expect("request")
}

fn request_json(request: &reqwest::Request) -> serde_json::Value {
    let body = request
        .body()
        .and_then(reqwest::Body::as_bytes)
        .expect("JSON request body");
    serde_json::from_slice(body).expect("valid JSON request body")
}

// ---------------------------------------------------------------------------
// Endpoint routing / API key injection
// ---------------------------------------------------------------------------

#[test]
fn anonymous_endpoint_no_querystring() {
    let (query, request) = build_test_request(search_args("q"));
    assert_eq!(query, "q");
    assert_eq!(request.url().as_str(), "https://example.test/mcp");
    assert!(request.url().query().is_none());
}

#[test]
fn passes_api_key_via_querystring() {
    let url = exa::endpoint_url(Some("k1+k2"));
    assert!(url.contains("?exaApiKey="));
    assert!(url.contains("k1%2Bk2"), "should percent-encode: {url}");
}

// ---------------------------------------------------------------------------
// Request body shape
// ---------------------------------------------------------------------------

#[test]
fn request_body_is_jsonrpc_with_default_args() {
    let (_, request) = build_test_request(search_args("rust"));
    let body = request_json(&request);
    assert_eq!(body["jsonrpc"], "2.0");
    assert_eq!(body["method"], "tools/call");
    assert_eq!(body["params"]["name"], "web_search_exa");
    assert_eq!(body["params"]["arguments"]["query"], "rust");
    assert_eq!(body["params"]["arguments"]["numResults"], 8);
    assert_eq!(body["params"]["arguments"]["type"], "auto");
    assert_eq!(body["params"]["arguments"]["livecrawl"], "fallback");
}

#[test]
fn all_optional_args_passthrough() {
    let args = SearchToolArgs {
        query: "deep".into(),
        num_results: Some(20),
        livecrawl: Some("preferred".into()),
        search_type: Some("deep".into()),
        context_max_characters: Some(15000),
    };
    let (_, request) = build_test_request(args);
    let body = request_json(&request);
    let arguments = &body["params"]["arguments"];
    assert_eq!(arguments["query"], "deep");
    assert_eq!(arguments["numResults"], 20);
    assert_eq!(arguments["type"], "deep");
    assert_eq!(arguments["livecrawl"], "preferred");
    assert_eq!(arguments["contextMaxCharacters"], 15000);
}

#[test]
fn sends_correct_accept_header() {
    let (_, request) = build_test_request(search_args("q"));
    assert_eq!(
        request.headers().get(ACCEPT).and_then(|v| v.to_str().ok()),
        Some("application/json, text/event-stream")
    );
    assert_eq!(
        request
            .headers()
            .get(CONTENT_TYPE)
            .and_then(|v| v.to_str().ok()),
        Some("application/json")
    );
}

// ---------------------------------------------------------------------------
// SSE parsing / errors
// ---------------------------------------------------------------------------

#[test]
fn empty_results_returns_fallback() {
    let out = search_output_from_response(
        "q".to_owned(),
        StatusCode::OK,
        "event: message\ndata: {\"result\":{\"content\":[]}}\n\n",
    )
    .expect("ok");
    assert!(
        out.results.contains("No search results found"),
        "got: {}",
        out.results
    );
}

#[test]
fn http_error_propagates() {
    let err = search_output_from_response(
        "q".to_owned(),
        StatusCode::INTERNAL_SERVER_ERROR,
        "internal err",
    )
    .unwrap_err();
    assert!(err.to_string().contains("500"), "got: {err}");
}

#[test]
fn invalid_sse_payload_returns_err() {
    let err = search_output_from_response("q".to_owned(), StatusCode::OK, "data: not_json\n")
        .unwrap_err();
    let msg = format!("{err:#}");
    assert!(
        msg.contains("Exa SSE") || msg.contains("invalid"),
        "got: {msg}"
    );
}

#[test]
fn handles_multiple_data_lines() {
    let body = "data: {\"result\":{\"content\":[]}}\n\
                data: {\"result\":{\"content\":[{\"type\":\"text\",\"text\":\"second\"}]}}\n\n";
    let out = search_output_from_response("q".to_owned(), StatusCode::OK, body).expect("ok");
    assert_eq!(out.results, "second");
}

// ---------------------------------------------------------------------------
// SearchToolArgs → SearchArgs default filling
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// Real-endpoint smoke test (opt in with WARP_RUN_WEB_INTEGRATION=1)
// ---------------------------------------------------------------------------

fn skip_real() -> bool {
    std::env::var("WARP_RUN_WEB_INTEGRATION").is_err()
}

#[tokio::test]
async fn real_exa_anonymous_search() {
    if skip_real() {
        return;
    }
    let client = build_client();
    let out = run_websearch(
        &client,
        search_args("rust async runtime tutorial"),
        None,
        None,
    )
    .await
    .expect("real Exa anonymous");
    assert!(!out.results.trim().is_empty(), "empty Exa output");
    assert_eq!(out.query, "rust async runtime tutorial");
}

// ---------------------------------------------------------------------------
// Description doc / byte-level alignment with opencode + {{year}} placeholder regression
// ---------------------------------------------------------------------------

/// websearch.md must contain the `{{year}}` placeholder — `chat_stream::build_tools_array`
/// replaces it with the current year at build time (aligned with opencode `websearch.ts:30-32`). Removing the placeholder
/// would make the model use an old year from its training data for time-sensitive searches.
#[test]
fn websearch_description_contains_year_placeholder() {
    use super::super::websearch::WEBSEARCH;
    assert!(
        WEBSEARCH.description.contains("{{year}}"),
        "websearch description must contain the {{{{year}}}} placeholder, replaced at build time"
    );
}

/// Locks websearch.md to be byte-level identical with opencode `packages/opencode/src/tool/websearch.txt`.
/// When modifying, both sides need to be kept in sync.
#[test]
fn websearch_description_matches_opencode_verbatim() {
    use super::super::websearch::WEBSEARCH;
    let expected = "- Search the web using Exa AI - performs real-time web searches and can scrape content from specific URLs\n\
                    - Provides up-to-date information for current events and recent data\n\
                    - Supports configurable result counts and returns the content from the most relevant websites\n\
                    - Use this tool for accessing information beyond knowledge cutoff\n\
                    - Searches are performed automatically within a single API call\n\
                    \n\
                    Usage notes:\n\
                    \x20\x20- Supports live crawling modes: 'fallback' (backup if cached unavailable) or 'preferred' (prioritize live crawling)\n\
                    \x20\x20- Search types: 'auto' (balanced), 'fast' (quick results), 'deep' (comprehensive search)\n\
                    \x20\x20- Configurable context length for optimal LLM integration\n\
                    \x20\x20- Domain filtering and advanced search options available\n\
                    \n\
                    The current year is {{year}}. You MUST use this year when searching for recent information or current events\n\
                    - Example: If the current year is 2026 and the user asks for \"latest AI news\", search for \"AI news 2026\", NOT \"AI news 2025\"\n";
    assert_eq!(WEBSEARCH.description, expected);
}

#[test]
fn search_tool_args_into_exa_uses_defaults() {
    let a = SearchToolArgs {
        query: "z".into(),
        num_results: None,
        livecrawl: None,
        search_type: None,
        context_max_characters: None,
    };
    let exa = a.into_exa_args();
    assert_eq!(exa.query, "z");
    assert_eq!(exa.num_results, 8);
    assert_eq!(exa.search_type, "auto");
    assert_eq!(exa.livecrawl, "fallback");
    assert!(exa.context_max_characters.is_none());
}

/// The `_byop_intercepted` sentinel must be present in the search result (same as webfetch),
/// so the controller knows to trigger auto-resume, otherwise the model gets stuck waiting for a result.
#[test]
fn search_output_carries_byop_sentinel() {
    let out = SearchOutput {
        query: "q".into(),
        results: "r".into(),
    };
    let v = search_output_to_json(&out);
    assert_eq!(v["_byop_intercepted"], true);
}

#[test]
fn search_tool_args_overrides_defaults() {
    let a = SearchToolArgs {
        query: "z".into(),
        num_results: Some(2),
        livecrawl: Some("preferred".into()),
        search_type: Some("fast".into()),
        context_max_characters: Some(500),
    };
    let exa = a.into_exa_args();
    assert_eq!(exa.num_results, 2);
    assert_eq!(exa.livecrawl, "preferred");
    assert_eq!(exa.search_type, "fast");
    assert_eq!(exa.context_max_characters, Some(500));
}
