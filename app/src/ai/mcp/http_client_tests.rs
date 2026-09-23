use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use super::build_client_with_headers;

/// Serves every connection with `response`, recording that it was contacted.
fn spawn_http_server(response: String, contacted: Arc<AtomicBool>) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind test listener");
    let address = listener.local_addr().expect("listener address");
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else {
                break;
            };
            contacted.store(true, Ordering::SeqCst);
            let mut request = [0u8; 4096];
            let _ = stream.read(&mut request);
            let _ = stream.write_all(response.as_bytes());
        }
    });
    format!("http://{address}")
}

#[tokio::test]
async fn custom_headers_are_not_replayed_to_a_redirect_target() {
    let target_contacted = Arc::new(AtomicBool::new(false));
    let target = spawn_http_server(
        "HTTP/1.1 200 OK\r\ncontent-length: 0\r\nconnection: close\r\n\r\n".to_string(),
        target_contacted.clone(),
    );
    let redirecting_server = spawn_http_server(
        format!(
            "HTTP/1.1 307 Temporary Redirect\r\nlocation: {target}/capture\r\n\
             content-length: 0\r\nconnection: close\r\n\r\n"
        ),
        Arc::new(AtomicBool::new(false)),
    );

    let client = build_client_with_headers(&HashMap::from([(
        "x-api-key".to_string(),
        "synthetic-test-key".to_string(),
    )]))
    .expect("client builds");
    let response = client
        .post(format!("{redirecting_server}/mcp"))
        .send()
        .await
        .expect("request completes");

    assert_eq!(response.status(), reqwest::StatusCode::TEMPORARY_REDIRECT);
    assert!(
        !target_contacted.load(Ordering::SeqCst),
        "the redirect target must not receive the request (or its custom headers)"
    );
}
