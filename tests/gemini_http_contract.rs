//! Loopback HTTP contract tests for the Gemini generateContent extractor.

use std::net::TcpListener;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use base64::Engine;
use base64::engine::general_purpose::STANDARD as BASE64;
use image::{DynamicImage, ImageBuffer, ImageFormat, Rgba};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use zl_expense::error::ErrorClass;
use zl_expense::receipt::{
    GeminiExtractorConfig, GeminiHttpExtractor, ReceiptExtractor, downscale_to_jpeg,
};

type LoopbackHandler = Arc<dyn Fn(&str, &str, &str, &[u8]) -> (u16, String) + Send + Sync>;

struct LoopbackServer {
    addr: String,
    shutdown: Option<tokio::sync::oneshot::Sender<()>>,
}

impl LoopbackServer {
    fn spawn(handler: LoopbackHandler) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind loopback");
        listener.set_nonblocking(true).expect("nonblocking");
        let addr = format!("http://{}", listener.local_addr().expect("addr"));
        let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel();

        std::thread::spawn(move || {
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("runtime");
            rt.block_on(async {
                let listener = tokio::net::TcpListener::from_std(listener).expect("tokio listener");
                let mut shutdown_rx = shutdown_rx;
                loop {
                    let accept = listener.accept();
                    let shutdown = &mut shutdown_rx;
                    let (stream, _) = tokio::select! {
                        res = accept => res.expect("accept"),
                        _ = &mut *shutdown => return,
                    };
                    let handler = Arc::clone(&handler);
                    tokio::spawn(async move {
                        let _ = tokio::time::timeout(
                            Duration::from_secs(8),
                            handle_connection(stream, handler),
                        )
                        .await;
                    });
                }
            });
        });

        Self {
            addr,
            shutdown: Some(shutdown_tx),
        }
    }
}

impl Drop for LoopbackServer {
    fn drop(&mut self) {
        if let Some(shutdown) = self.shutdown.take() {
            let _ = shutdown.send(());
        }
    }
}

async fn handle_connection(mut stream: tokio::net::TcpStream, handler: LoopbackHandler) {
    let mut buf = Vec::new();
    let mut chunk = [0u8; 4096];
    loop {
        let n = stream.read(&mut chunk).await.unwrap_or(0);
        if n == 0 {
            break;
        }
        buf.extend_from_slice(&chunk[..n]);
        if buf.windows(4).any(|window| window == b"\r\n\r\n") {
            break;
        }
    }
    let header_end = buf
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
        .map(|position| position + 4)
        .unwrap_or(buf.len());
    let headers = String::from_utf8_lossy(&buf[..header_end]).into_owned();
    let content_length = headers
        .lines()
        .find_map(|line| {
            let (name, value) = line.split_once(':')?;
            name.trim()
                .eq_ignore_ascii_case("content-length")
                .then(|| value.trim().parse::<usize>().unwrap_or(0))
        })
        .unwrap_or(0);
    while buf.len() < header_end + content_length {
        let n = stream.read(&mut chunk).await.unwrap_or(0);
        if n == 0 {
            break;
        }
        buf.extend_from_slice(&chunk[..n]);
    }
    let body_end = header_end + content_length.min(buf.len().saturating_sub(header_end));
    let body = &buf[header_end..body_end];
    let request_line = headers.lines().next().unwrap_or("");
    let mut parts = request_line.split_whitespace();
    let method = parts.next().unwrap_or("");
    let path = parts.next().unwrap_or("");
    let (status, response_body) = handler(method, path, &headers, body);
    let response = format!(
        "HTTP/1.1 {status} {}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        status_text(status),
        response_body.len()
    );
    let _ = stream.write_all(response.as_bytes()).await;
    let _ = stream.write_all(response_body.as_bytes()).await;
}

fn status_text(status: u16) -> &'static str {
    match status {
        200 => "OK",
        401 => "Unauthorized",
        429 => "Too Many Requests",
        500 => "Internal Server Error",
        _ => "OK",
    }
}

fn extractor(api_base: &str, timeout: Duration) -> GeminiHttpExtractor {
    GeminiHttpExtractor::new(GeminiExtractorConfig {
        api_base: api_base.to_string(),
        api_key: "sk-test-secret".to_string(),
        model: "gemini-2.5-flash".to_string(),
        profile_name: "receipt-fast".to_string(),
        timeout,
        max_input_bytes: 4_194_304,
        max_output_tokens: 2048,
        thinking_effort: "none".to_string(),
        schema_version: "v1".to_string(),
    })
    .expect("extractor")
}

fn success_body() -> String {
    serde_json::json!({
        "candidates": [{
            "finishReason": "STOP",
            "content": {
                "parts": [{
                    "text": serde_json::json!({
                        "merchant": "Co.opmart",
                        "amount_minor": 325000,
                        "currency": "VND",
                        "category_key": "thuc-pham",
                        "transaction_type": "expense",
                        "occurred_at": "2026-07-15T09:24:00Z",
                        "confidence": 0.95,
                        "unsupported": false
                    }).to_string()
                }]
            }
        }],
        "usageMetadata": {
            "promptTokenCount": 100,
            "candidatesTokenCount": 40
        }
    })
    .to_string()
}

fn tiny_png() -> Vec<u8> {
    include_bytes!("../src/receipt/testdata/tiny.png").to_vec()
}

fn wide_png() -> Vec<u8> {
    let buffer = ImageBuffer::from_pixel(3000, 1000, Rgba([12_u8, 34, 56, 255]));
    let image = DynamicImage::ImageRgba8(buffer);
    let mut png = Vec::new();
    image
        .write_to(&mut std::io::Cursor::new(&mut png), ImageFormat::Png)
        .expect("png");
    png
}

#[tokio::test(flavor = "multi_thread")]
async fn success_maps_json_and_records_tokens() {
    let recorded = Arc::new(Mutex::new(None::<(String, String, String)>));
    let recorded_clone = Arc::clone(&recorded);
    let server = LoopbackServer::spawn(Arc::new(move |method, path, headers, body| {
        *recorded_clone.lock().expect("lock") = Some((
            method.to_string(),
            path.to_string(),
            String::from_utf8_lossy(body).into_owned(),
        ));
        assert!(
            headers
                .to_ascii_lowercase()
                .contains("x-goog-api-key: sk-test-secret")
        );
        (200, success_body())
    }));
    let extractor = extractor(&server.addr, Duration::from_secs(2));
    let attempt = extractor.extract(&tiny_png()).expect("extract");
    assert_eq!(attempt.result.merchant, "Co.opmart");
    assert_eq!(attempt.result.amount_minor, 325_000);
    assert_eq!(attempt.meta.input_tokens, Some(100));
    assert_eq!(attempt.meta.output_tokens, Some(40));
    assert_eq!(attempt.meta.provider, "gemini");
    assert_eq!(attempt.meta.prompt_version, "extraction-json-v2");

    let (method, path, body) = recorded.lock().expect("lock").clone().expect("recorded");
    assert_eq!(method, "POST");
    assert_eq!(path, "/v1beta/models/gemini-2.5-flash:generateContent");
    assert!(body.contains("inlineData"));
    assert!(body.contains("application/json"));
    assert!(body.contains("\"thinkingBudget\":0"));
    assert!(!body.contains("thinkingLevel"));
    let debug = format!("{extractor:?} {attempt:?}");
    assert!(!debug.contains("sk-test-secret"));
}

#[tokio::test(flavor = "multi_thread")]
async fn http_429_is_rate_limited() {
    let server = LoopbackServer::spawn(Arc::new(|_, _, _, _| (429, "{}".to_string())));
    let extractor = extractor(&server.addr, Duration::from_secs(2));
    let error = extractor.extract(&tiny_png()).unwrap_err();
    assert_eq!(error.class, ErrorClass::RateLimited);
    assert!(!format!("{error}").contains("sk-test-secret"));
}

#[tokio::test(flavor = "multi_thread")]
async fn http_500_is_transient() {
    let server = LoopbackServer::spawn(Arc::new(|_, _, _, _| (500, "{}".to_string())));
    let extractor = extractor(&server.addr, Duration::from_secs(2));
    let error = extractor.extract(&tiny_png()).unwrap_err();
    assert_eq!(error.class, ErrorClass::Transient);
}

#[tokio::test(flavor = "multi_thread")]
async fn timeout_is_timeout() {
    let server = LoopbackServer::spawn(Arc::new(|_, _, _, _| {
        std::thread::sleep(Duration::from_millis(1500));
        (200, success_body())
    }));
    let extractor = extractor(&server.addr, Duration::from_millis(200));
    let error = extractor.extract(&tiny_png()).unwrap_err();
    assert_eq!(error.class, ErrorClass::Timeout);
}

#[tokio::test(flavor = "multi_thread")]
async fn malformed_200_is_validation() {
    let server = LoopbackServer::spawn(Arc::new(|_, _, _, _| (200, "{}".to_string())));
    let extractor = extractor(&server.addr, Duration::from_secs(2));
    let error = extractor.extract(&tiny_png()).unwrap_err();
    assert_eq!(error.class, ErrorClass::Validation);
}

#[tokio::test(flavor = "multi_thread")]
async fn blocked_prompt_is_validation() {
    let server = LoopbackServer::spawn(Arc::new(|_, _, _, _| {
        (
            200,
            serde_json::json!({"promptFeedback": {"blockReason": "SAFETY"}}).to_string(),
        )
    }));
    let extractor = extractor(&server.addr, Duration::from_secs(2));
    let error = extractor.extract(&tiny_png()).unwrap_err();
    assert_eq!(error.class, ErrorClass::Validation);
}

#[tokio::test(flavor = "multi_thread")]
async fn http_401_is_auth_and_redacted() {
    let server = LoopbackServer::spawn(Arc::new(|_, _, _, _| (401, "{}".to_string())));
    let extractor = extractor(&server.addr, Duration::from_secs(2));
    let error = extractor.extract(&tiny_png()).unwrap_err();
    assert_eq!(error.class, ErrorClass::Auth);
    let rendered = format!("{error:?} {error}");
    assert!(!rendered.contains("sk-test-secret"));
}

#[tokio::test(flavor = "multi_thread")]
async fn extraction_input_is_downscaled_to_2048() {
    let jpeg = Arc::new(Mutex::new(None::<Vec<u8>>));
    let jpeg_clone = Arc::clone(&jpeg);
    let server = LoopbackServer::spawn(Arc::new(move |_, _, _, body| {
        let parsed: serde_json::Value = serde_json::from_slice(body).expect("json");
        let data = parsed["contents"][0]["parts"][1]["inlineData"]["data"]
            .as_str()
            .expect("data");
        *jpeg_clone.lock().expect("lock") = Some(BASE64.decode(data).expect("base64"));
        (200, success_body())
    }));
    let extractor = extractor(&server.addr, Duration::from_secs(5));
    extractor.extract(&wide_png()).expect("extract");
    let jpeg = jpeg.lock().expect("lock").clone().expect("jpeg");
    let decoded = image::load_from_memory(&jpeg).expect("decode");
    assert_eq!(decoded.width().max(decoded.height()), 2048);
}

#[test]
fn downscale_helper_caps_long_edge() {
    let jpeg = downscale_to_jpeg(&wide_png()).expect("downscale");
    let decoded = image::load_from_memory(&jpeg).expect("decode");
    assert_eq!(decoded.width().max(decoded.height()), 2048);
}
