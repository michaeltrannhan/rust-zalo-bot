//! Loopback HTTP contract tests for the Zalo Bot API adapter.

use std::net::TcpListener;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use chrono::{TimeZone, Utc};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use zl_expense::error::ErrorClass;
use zl_expense::provider::{InboundEventKind, ZaloHttpAdapter, ZaloHttpConfig};

type LoopbackHandler = Arc<dyn Fn(&str, &str, &str) -> (u16, String) + Send + Sync>;

fn fixture(name: &str) -> Vec<u8> {
    let path = format!("tests/fixtures/zalo/{}", name);
    std::fs::read(&path).unwrap_or_else(|e| panic!("read fixture {}: {}", path, e))
}

fn test_config(api_base: &str, token: &str, secret: &str) -> ZaloHttpConfig {
    ZaloHttpConfig {
        api_base: api_base.to_string(),
        bot_token: token.to_string(),
        webhook_secret: secret.to_string(),
        provider_scope: "zalo_bot".to_string(),
        request_timeout: Duration::from_secs(2),
    }
}

#[derive(Clone)]
struct RecordedRequest {
    method: String,
    path: String,
    body: String,
}

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
                            Duration::from_secs(5),
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

    fn spawn_recording(
        response_status: u16,
        response_body: &str,
    ) -> (Self, Arc<Mutex<Option<RecordedRequest>>>) {
        let recorded = Arc::new(Mutex::new(None));
        let rec = Arc::clone(&recorded);
        let body = response_body.to_string();
        let server = Self::spawn(Arc::new(move |method, path, req_body| {
            *rec.lock().expect("lock") = Some(RecordedRequest {
                method: method.to_string(),
                path: path.to_string(),
                body: req_body.to_string(),
            });
            (response_status, body.clone())
        }));
        (server, recorded)
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
        if buf.windows(4).any(|w| w == b"\r\n\r\n") {
            break;
        }
    }
    let header_end = buf
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
        .map(|position| position + 4)
        .unwrap_or(buf.len());
    let headers = String::from_utf8_lossy(&buf[..header_end]);
    let content_length = headers
        .lines()
        .find_map(|line| {
            let (name, value) = line.split_once(':')?;
            name.eq_ignore_ascii_case("content-length")
                .then(|| value.trim().parse::<usize>().ok())
                .flatten()
        })
        .unwrap_or(0);
    while buf.len() < header_end + content_length {
        let n = stream.read(&mut chunk).await.unwrap_or(0);
        if n == 0 {
            break;
        }
        buf.extend_from_slice(&chunk[..n]);
    }
    let request = String::from_utf8_lossy(&buf[..header_end]);
    let mut lines = request.lines();
    let request_line = lines.next().unwrap_or_default();
    let parts: Vec<&str> = request_line.split_whitespace().collect();
    let method = parts.first().copied().unwrap_or_default();
    let path = parts.get(1).copied().unwrap_or_default();
    let body = String::from_utf8_lossy(&buf[header_end..]).to_string();
    let (status, response_body) = handler(method, path, &body);
    let response = format!(
        "HTTP/1.1 {} OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
        status,
        response_body.len(),
        response_body
    );
    let _ = stream.write_all(response.as_bytes()).await;
}

impl LoopbackServer {
    fn url(&self) -> &str {
        &self.addr
    }
}

impl Drop for LoopbackServer {
    fn drop(&mut self) {
        if let Some(shutdown) = self.shutdown.take() {
            let _ = shutdown.send(());
        }
    }
}

#[test]
fn parse_text_flat_fixture() {
    let adapter =
        ZaloHttpAdapter::new(test_config("http://unused", "tok", "sec")).expect("adapter");
    let body = fixture("text_flat.json");
    let event = adapter.parse_text_webhook(&body).expect("parse");
    assert_eq!(event.provider_scope, "zalo_bot");
    assert_eq!(event.event_id, "a1b2c3d4e5f60718293a4b5c");
    assert_eq!(event.sender_id, "8891234567890123456");
    assert_eq!(event.chat_id, "7009876543210987654");
    assert_eq!(event.text, "cà phê 35k");
    assert_eq!(event.received_at, Utc.timestamp_opt(1752854400, 0).unwrap());
    assert_eq!(event.kind, InboundEventKind::TextReceived);
}

#[test]
fn parse_text_envelope_fixture() {
    let adapter =
        ZaloHttpAdapter::new(test_config("http://unused", "tok", "sec")).expect("adapter");
    let body = fixture("text_envelope.json");
    let event = adapter.parse_text_webhook(&body).expect("parse");
    assert_eq!(event.event_id, "envelope-msg-001");
    assert_eq!(event.sender_id, "sender-envelope-42");
    assert_eq!(event.chat_id, "chat-envelope-99");
    assert_eq!(event.text, "hello from envelope");
    assert_eq!(
        event.received_at,
        Utc.timestamp_millis_opt(1750316131602).unwrap()
    );
}

#[test]
fn parse_numeric_ids_and_seconds_date() {
    let adapter =
        ZaloHttpAdapter::new(test_config("http://unused", "tok", "sec")).expect("adapter");
    let body = r#"{"event_name":"message.text.received","message":{"message_id":12345,"from":{"id":678},"chat":{"id":901},"date":1752854400,"text":"xin chào"}}"#;
    let event = adapter.parse_text_webhook(body.as_bytes()).expect("parse");
    assert_eq!(event.event_id, "12345");
    assert_eq!(event.sender_id, "678");
    assert_eq!(event.chat_id, "901");
}

#[test]
fn unsupported_event_is_observable() {
    let adapter =
        ZaloHttpAdapter::new(test_config("http://unused", "tok", "sec")).expect("adapter");
    let body = fixture("unsupported.json");
    let event = adapter.parse_text_webhook(&body).expect("parse");
    assert_eq!(
        event.kind,
        InboundEventKind::Unsupported("message.sticker.received".to_string())
    );
    assert_eq!(event.event_id, "c3d4e5f60718293a4b5c6d7e");
}

#[test]
fn zalo_platform_webhook_test_probe_is_unsupported() {
    let adapter =
        ZaloHttpAdapter::new(test_config("http://unused", "tok", "sec")).expect("adapter");
    let body = fixture("webhook_test.json");
    let event = adapter.parse_text_webhook(&body).expect("parse");
    assert_eq!(
        event.kind,
        InboundEventKind::Unsupported("webhook.test".to_string())
    );
    assert_eq!(event.event_id, "webhook-test");
    assert!(event.sender_id.is_empty());
    assert!(event.chat_id.is_empty());
}

#[test]
fn malformed_known_event_is_validation() {
    let adapter =
        ZaloHttpAdapter::new(test_config("http://unused", "tok", "sec")).expect("adapter");
    let body = fixture("missing_fields.json");
    let err = adapter
        .parse_text_webhook(&body)
        .expect_err("missing fields");
    assert_eq!(err.class, ErrorClass::Validation);
}

#[test]
fn text_event_without_text_is_validation() {
    let adapter =
        ZaloHttpAdapter::new(test_config("http://unused", "tok", "sec")).expect("adapter");
    let body = br#"{"event_name":"message.text.received","message":{"message_id":"m1","from":{"id":"s1"},"chat":{"id":"c1"},"date":1752854400}}"#;
    let err = adapter
        .parse_text_webhook(body)
        .expect_err("text is required");
    assert_eq!(err.class, ErrorClass::Validation);
}

#[test]
fn inbound_text_and_identifiers_are_bounded() {
    let adapter =
        ZaloHttpAdapter::new(test_config("http://unused", "tok", "sec")).expect("adapter");
    let long_text = "x".repeat(2001);
    let body = serde_json::json!({
        "event_name": "message.text.received",
        "message": {
            "message_id": "m1",
            "from": { "id": "s1" },
            "chat": { "id": "c1" },
            "date": 1752854400_i64,
            "text": long_text,
        }
    });
    let err = adapter
        .parse_text_webhook(&serde_json::to_vec(&body).expect("JSON"))
        .expect_err("oversized text");
    assert_eq!(err.class, ErrorClass::Validation);

    let body = serde_json::json!({
        "event_name": "message.text.received",
        "message": {
            "message_id": "m".repeat(257),
            "from": { "id": "s1" },
            "chat": { "id": "c1" },
            "date": 1752854400_i64,
            "text": "hello",
        }
    });
    let err = adapter
        .parse_text_webhook(&serde_json::to_vec(&body).expect("JSON"))
        .expect_err("oversized identifier");
    assert_eq!(err.class, ErrorClass::Validation);
}

#[test]
fn verify_webhook_secret_accepts_trimmed_match() {
    let adapter =
        ZaloHttpAdapter::new(test_config("http://unused", "tok", "s3cret-token")).expect("adapter");
    adapter
        .verify_webhook_secret(Some("  s3cret-token  "))
        .expect("valid secret");
}

#[test]
fn verify_webhook_secret_rejects_wrong_secret() {
    let adapter =
        ZaloHttpAdapter::new(test_config("http://unused", "tok", "s3cret-token")).expect("adapter");
    let err = adapter
        .verify_webhook_secret(Some("nope"))
        .expect_err("wrong secret");
    assert_eq!(err.class, ErrorClass::Auth);
}

#[tokio::test]
async fn send_message_success_records_request() {
    const TOKEN: &str = "T0K3N";
    let (server, recorded) =
        LoopbackServer::spawn_recording(200, r#"{"ok":true,"result":{"message_id":12345}}"#);
    let adapter = ZaloHttpAdapter::new(test_config(server.url(), TOKEN, "sec")).expect("adapter");
    let result = adapter
        .send_message("c1", "đã ghi nhận")
        .await
        .expect("send");
    assert_eq!(result.provider_message_id, "12345");

    let req = recorded.lock().expect("lock").clone().expect("recorded");
    assert_eq!(req.method, "POST");
    assert_eq!(req.path, format!("/bot{TOKEN}/sendMessage"));
    assert!(req.body.contains("\"chat_id\":\"c1\""));
    assert!(req.body.contains("\"text\":\"đã ghi nhận\""));
}

#[tokio::test]
async fn send_message_accepts_success_without_provider_message_id() {
    let (server, _) = LoopbackServer::spawn_recording(200, r#"{"ok":true,"result":{}}"#);
    let adapter = ZaloHttpAdapter::new(test_config(server.url(), "token", "sec")).expect("adapter");
    let result = adapter.send_message("c1", "x").await.expect("send");
    assert_eq!(result.provider_message_id, "");
}

#[tokio::test]
async fn send_message_encodes_token_as_one_path_segment() {
    let (server, recorded) = LoopbackServer::spawn_recording(200, r#"{"ok":true,"result":{}}"#);
    let adapter =
        ZaloHttpAdapter::new(test_config(server.url(), "tok/part?query", "sec")).expect("adapter");
    adapter.send_message("c1", "x").await.expect("send");
    let path = recorded
        .lock()
        .expect("lock")
        .clone()
        .expect("request")
        .path;
    assert_eq!(path, "/bottok%2Fpart%3Fquery/sendMessage");
}

#[tokio::test]
async fn send_message_malformed_2xx_is_ambiguous() {
    const TOKEN: &str = "T0K3N";
    let (server, _) = LoopbackServer::spawn_recording(200, "{not-json");
    let adapter = ZaloHttpAdapter::new(test_config(server.url(), TOKEN, "sec")).expect("adapter");
    let err = adapter
        .send_message("c1", "x")
        .await
        .expect_err("malformed");
    assert_eq!(err.class, ErrorClass::ProviderAmbiguous);
}

#[tokio::test]
async fn send_message_ok_false_2xx_is_ambiguous() {
    const TOKEN: &str = "T0K3N";
    let (server, _) =
        LoopbackServer::spawn_recording(200, r#"{"ok":false,"description":"rejected"}"#);
    let adapter = ZaloHttpAdapter::new(test_config(server.url(), TOKEN, "sec")).expect("adapter");
    let err = adapter.send_message("c1", "x").await.expect_err("ok false");
    assert_eq!(err.class, ErrorClass::ProviderAmbiguous);
}

#[tokio::test]
async fn send_message_429_is_rate_limited() {
    const TOKEN: &str = "T0K3N";
    let (server, _) =
        LoopbackServer::spawn_recording(429, r#"{"ok":false,"description":"too many"}"#);
    let adapter = ZaloHttpAdapter::new(test_config(server.url(), TOKEN, "sec")).expect("adapter");
    let err = adapter.send_message("c1", "x").await.expect_err("429");
    assert_eq!(err.class, ErrorClass::RateLimited);
}

#[tokio::test]
async fn send_message_5xx_is_provider_error() {
    const TOKEN: &str = "T0K3N";
    let (server, _) = LoopbackServer::spawn_recording(500, r#"{"ok":false,"description":"boom"}"#);
    let adapter = ZaloHttpAdapter::new(test_config(server.url(), TOKEN, "sec")).expect("adapter");
    let err = adapter.send_message("c1", "x").await.expect_err("500");
    assert_eq!(err.class, ErrorClass::ProviderError);
}

#[tokio::test]
async fn provider_error_never_includes_response_body() {
    let (server, _) = LoopbackServer::spawn_recording(
        500,
        r#"{"description":"unrelated-private-provider-payload"}"#,
    );
    let adapter = ZaloHttpAdapter::new(test_config(server.url(), "token", "sec")).expect("adapter");
    let err = adapter.send_message("c1", "x").await.expect_err("500");
    let rendered = format!("{err} {err:?}");
    assert!(!rendered.contains("unrelated-private-provider-payload"));
}

#[tokio::test]
async fn send_message_4xx_is_provider_error() {
    const TOKEN: &str = "T0K3N";
    let (server, _) =
        LoopbackServer::spawn_recording(400, r#"{"ok":false,"description":"bad request"}"#);
    let adapter = ZaloHttpAdapter::new(test_config(server.url(), TOKEN, "sec")).expect("adapter");
    let err = adapter.send_message("c1", "x").await.expect_err("400");
    assert_eq!(err.class, ErrorClass::ProviderError);
}

#[tokio::test]
async fn send_message_timeout_is_ambiguous() {
    const TOKEN: &str = "T0K3N";
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let addr = format!("http://{}", listener.local_addr().expect("addr"));
    let cfg = ZaloHttpConfig {
        api_base: addr,
        bot_token: TOKEN.to_string(),
        webhook_secret: "sec".to_string(),
        provider_scope: "zalo_bot".to_string(),
        request_timeout: Duration::from_millis(50),
    };
    let adapter = ZaloHttpAdapter::new(cfg).expect("adapter");

    std::thread::spawn(move || {
        if let Ok((mut stream, _)) = listener.accept() {
            let mut buf = [0u8; 1024];
            let _ = std::io::Read::read(&mut stream, &mut buf);
            std::thread::sleep(Duration::from_millis(200));
        }
    });

    let err = adapter.send_message("c1", "x").await.expect_err("timeout");
    assert_eq!(err.class, ErrorClass::ProviderAmbiguous);
}

#[test]
fn send_validation_rejects_empty_and_long_text() {
    let adapter =
        ZaloHttpAdapter::new(test_config("http://unused", "tok", "sec")).expect("adapter");
    for text in ["", "   ", &"x".repeat(2001)] {
        let err = adapter
            .validate_outbound("c1", text)
            .expect_err("validation");
        assert_eq!(err.class, ErrorClass::Validation);
    }
}

#[test]
fn debug_output_redacts_config_and_normalized_event() {
    const TOKEN: &str = "secret:bot-token";
    const SECRET: &str = "webhook-secret-value";
    const CHAT: &str = "chat-sensitive-42";
    const TEXT: &str = "sensitive message body";
    let config = test_config("http://unused", TOKEN, SECRET);
    let config_debug = format!("{config:?}");
    assert!(!config_debug.contains(TOKEN));
    assert!(!config_debug.contains(SECRET));

    let adapter = ZaloHttpAdapter::new(config).expect("adapter");
    let body = format!(
        r#"{{"event_name":"message.text.received","message":{{"message_id":"event-private","from":{{"id":"sender-private"}},"chat":{{"id":"{CHAT}"}},"date":1752854400,"text":"{TEXT}"}}}}"#
    );
    let event = adapter.parse_text_webhook(body.as_bytes()).expect("event");
    let event_debug = format!("{event:?}");
    for secret in ["event-private", "sender-private", CHAT, TEXT] {
        assert!(!event_debug.contains(secret));
    }
}

#[tokio::test]
async fn transport_error_redacts_token_from_url() {
    const TOKEN: &str = "secret:bot-token";
    let cfg = ZaloHttpConfig {
        api_base: "http://127.0.0.1:1".to_string(),
        bot_token: TOKEN.to_string(),
        webhook_secret: "sec".to_string(),
        provider_scope: "zalo_bot".to_string(),
        request_timeout: Duration::from_millis(100),
    };
    let adapter = ZaloHttpAdapter::new(cfg).expect("adapter");
    let err = adapter
        .send_message("c1", "hello")
        .await
        .expect_err("transport");
    let combined = format!("{} {:?}", err, err);
    assert!(!combined.contains(TOKEN));
    assert!(!combined.contains("secret%3Abot-token"));
}
