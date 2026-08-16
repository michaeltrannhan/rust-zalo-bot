//! S3-compatible object store tests against a loopback HTTP server.

use std::collections::HashMap;
use std::net::TcpListener;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use zl_expense::error::ErrorClass;
use zl_expense::receipt::{ReceiptObjectStore, S3CompatibleObjectStore, S3ObjectStoreConfig};

type LoopbackHandler = Arc<dyn Fn(&str, &str, &[u8]) -> (u16, Vec<u8>) + Send + Sync>;

struct LoopbackServer {
    addr: String,
}

impl LoopbackServer {
    fn spawn(handler: LoopbackHandler) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind loopback");
        listener.set_nonblocking(true).expect("nonblocking");
        let addr = format!("http://{}", listener.local_addr().expect("addr"));
        std::thread::spawn(move || {
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("runtime");
            rt.block_on(async {
                let listener = tokio::net::TcpListener::from_std(listener).expect("tokio listener");
                loop {
                    let (stream, _) = listener.accept().await.expect("accept");
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

        Self { addr }
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
    let body_start = header_end;
    while buf.len() < body_start + content_length {
        let n = stream.read(&mut chunk).await.unwrap_or(0);
        if n == 0 {
            break;
        }
        buf.extend_from_slice(&chunk[..n]);
    }
    let body_end = body_start + content_length.min(buf.len().saturating_sub(body_start));
    let body = &buf[body_start..body_end];

    let request_line = headers.lines().next().unwrap_or("");
    let mut parts = request_line.split_whitespace();
    let method = parts.next().unwrap_or("");
    let path = parts.next().unwrap_or("");

    let (status, response_body) = handler(method, path, body);
    let response = format!(
        "HTTP/1.1 {} {}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        status,
        status_text(status),
        response_body.len()
    );
    let _ = stream.write_all(response.as_bytes()).await;
    let _ = stream.write_all(&response_body).await;
}

fn status_text(status: u16) -> &'static str {
    match status {
        200 => "OK",
        204 => "No Content",
        404 => "Not Found",
        500 => "Internal Server Error",
        _ => "OK",
    }
}

fn s3_store(endpoint: &str) -> S3CompatibleObjectStore {
    S3CompatibleObjectStore::new(S3ObjectStoreConfig {
        endpoint: endpoint.to_string(),
        bucket: "receipts".to_string(),
        region: "us-east-1".to_string(),
        access_key: "test-access-key".to_string(),
        secret_key: "test-secret-key".to_string(),
        force_path_style: true,
    })
    .expect("store")
}

fn path_style_handler(
    objects: Arc<Mutex<HashMap<String, Vec<u8>>>>,
    fail_with: Option<u16>,
) -> LoopbackHandler {
    Arc::new(move |method, path, body| {
        if fail_with == Some(500) {
            return (500, Vec::new());
        }
        let key = path.strip_prefix("/receipts/").unwrap_or(path);
        match method {
            "PUT" => {
                objects
                    .lock()
                    .expect("lock")
                    .insert(key.to_string(), body.to_vec());
                (200, Vec::new())
            }
            "GET" => {
                let guard = objects.lock().expect("lock");
                if let Some(bytes) = guard.get(key) {
                    (200, bytes.clone())
                } else {
                    (404, Vec::new())
                }
            }
            "DELETE" => {
                objects.lock().expect("lock").remove(key);
                (204, Vec::new())
            }
            _ => (404, Vec::new()),
        }
    })
}

#[tokio::test(flavor = "multi_thread")]
async fn put_get_delete_success() {
    let objects = Arc::new(Mutex::new(HashMap::new()));
    let server = LoopbackServer::spawn(path_style_handler(Arc::clone(&objects), None));
    let store = s3_store(&server.addr);
    let key = "receipts/account/submission/hash";
    let bytes = b"s3-payload";

    store.put(key, bytes).expect("put");
    let loaded = store.get(key).expect("get").expect("object");
    assert_eq!(loaded, bytes);
    store.delete(key).expect("delete");
    assert!(store.get(key).expect("get after delete").is_none());
}

#[tokio::test(flavor = "multi_thread")]
async fn missing_get_returns_none() {
    let objects = Arc::new(Mutex::new(HashMap::new()));
    let server = LoopbackServer::spawn(path_style_handler(Arc::clone(&objects), None));
    let store = s3_store(&server.addr);
    assert!(
        store
            .get("receipts/missing/submission/hash")
            .expect("get")
            .is_none()
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn upstream_5xx_maps_to_dependency() {
    let objects = Arc::new(Mutex::new(HashMap::new()));
    let server = LoopbackServer::spawn(path_style_handler(Arc::clone(&objects), Some(500)));
    let store = s3_store(&server.addr);
    let error = store.get("receipts/account/submission/hash").unwrap_err();
    assert_eq!(error.class, ErrorClass::Dependency);
}

#[tokio::test(flavor = "multi_thread")]
async fn put_missing_object_is_not_success() {
    let server = LoopbackServer::spawn(Arc::new(|method, _path, _body| {
        if method == "PUT" {
            (404, Vec::new())
        } else {
            (500, Vec::new())
        }
    }));
    let store = s3_store(&server.addr);
    let error = store
        .put("receipts/account/submission/hash", b"payload")
        .unwrap_err();
    assert_eq!(error.class, ErrorClass::Dependency);
}
