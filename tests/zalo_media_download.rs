//! Loopback contract tests for bounded media downloads.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr, TcpListener};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use zl_expense::error::ErrorClass;
use zl_expense::provider::{
    InjectedMediaResolver, MediaDownloadPolicy, MediaDownloadResult, ZaloMediaDownloader,
};

const TEST_HOST: &str = "s120.zdn.vn";
/// Globally unicast fixture. Used only on validation paths that never open a socket.
const PUBLIC_RESOLVED_IP: IpAddr = IpAddr::V4(Ipv4Addr::new(8, 8, 8, 8));
const LOOPBACK_IP: IpAddr = IpAddr::V4(Ipv4Addr::LOCALHOST);

#[derive(Clone)]
struct MediaResponse {
    status: u16,
    headers: Vec<(String, String)>,
    body: Vec<u8>,
    delay_ms: u64,
}

struct MediaLoopbackServer {
    addr: SocketAddr,
    shutdown: Option<tokio::sync::oneshot::Sender<()>>,
}

type MediaHandler = Arc<dyn Fn(&str) -> MediaResponse + Send + Sync>;

impl MediaLoopbackServer {
    fn spawn(handler: MediaHandler) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
        listener.set_nonblocking(true).expect("nonblocking");
        let addr = listener.local_addr().expect("addr");
        let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel();

        std::thread::spawn(move || {
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("runtime");
            rt.block_on(async {
                let listener = tokio::net::TcpListener::from_std(listener).expect("listener");
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
                            Duration::from_secs(10),
                            handle_media_connection(stream, handler),
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

async fn handle_media_connection(mut stream: tokio::net::TcpStream, handler: MediaHandler) {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

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
    let path = request
        .lines()
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .unwrap_or("/");

    let response = handler(path);
    if response.delay_ms > 0 {
        tokio::time::sleep(Duration::from_millis(response.delay_ms)).await;
    }

    let mut header_lines = format!(
        "HTTP/1.1 {} {}\r\nConnection: close\r\n",
        response.status,
        if response.status < 400 { "OK" } else { "Error" }
    );
    for (name, value) in &response.headers {
        header_lines.push_str(&format!("{}: {}\r\n", name, value));
    }
    let has_content_length = response
        .headers
        .iter()
        .any(|(name, _)| name.eq_ignore_ascii_case("content-length"));
    if response.body.is_empty() && !has_content_length {
        header_lines.push_str("Content-Length: 0\r\n");
    } else if !response.body.is_empty() && !has_content_length {
        header_lines.push_str(&format!("Content-Length: {}\r\n", response.body.len()));
    }
    header_lines.push_str("\r\n");
    let _ = stream.write_all(header_lines.as_bytes()).await;
    if !response.body.is_empty() {
        let _ = stream.write_all(&response.body).await;
    }
}

impl Drop for MediaLoopbackServer {
    fn drop(&mut self) {
        if let Some(shutdown) = self.shutdown.take() {
            let _ = shutdown.send(());
        }
    }
}

fn loopback_policy(port: u16) -> MediaDownloadPolicy {
    let mut policy = MediaDownloadPolicy::production_default();
    policy.require_https = false;
    policy.permit_private_resolved_addresses = true;
    policy.allowed_explicit_port = Some(port);
    policy
}

fn loopback_policy_without_private(port: u16) -> MediaDownloadPolicy {
    let mut policy = loopback_policy(port);
    policy.permit_private_resolved_addresses = false;
    policy
}

fn test_resolver(host: &str, ip: IpAddr) -> InjectedMediaResolver {
    let mut resolver = InjectedMediaResolver::new();
    resolver.insert(host, vec![ip]);
    resolver
}

fn media_url(server: &MediaLoopbackServer, path: &str) -> String {
    format!("http://{TEST_HOST}:{}{}", server.addr.port(), path)
}

fn downloader(
    server: &MediaLoopbackServer,
    resolver: InjectedMediaResolver,
) -> ZaloMediaDownloader<InjectedMediaResolver> {
    ZaloMediaDownloader::new(loopback_policy(server.addr.port()), resolver)
}

#[test]
fn production_default_never_permits_private_or_nondefault_port() {
    let policy = MediaDownloadPolicy::production_default();
    assert!(!policy.permit_private_resolved_addresses);
    assert_eq!(policy.allowed_explicit_port, None);
    assert!(policy.require_https);
    assert_eq!(policy.total_timeout, Duration::from_secs(15));
}

#[tokio::test]
async fn download_success_returns_bytes_and_content_type() {
    let server = MediaLoopbackServer::spawn(Arc::new(|path| {
        assert_eq!(path, "/receipt.jpg");
        MediaResponse {
            status: 200,
            headers: vec![(
                "Content-Type".to_string(),
                "image/jpeg; charset=binary".to_string(),
            )],
            body: b"jpeg-bytes".to_vec(),
            delay_ms: 0,
        }
    }));
    let resolver = test_resolver(TEST_HOST, LOOPBACK_IP);
    let url = media_url(&server, "/receipt.jpg");
    let result = downloader(&server, resolver)
        .download(&url)
        .await
        .expect("download");
    assert_eq!(result.bytes, b"jpeg-bytes");
    assert_eq!(
        result.content_type.as_deref(),
        Some("image/jpeg; charset=binary")
    );
}

#[tokio::test]
async fn download_rejects_host_suffix_spoof() {
    let server = MediaLoopbackServer::spawn(Arc::new(|_| MediaResponse {
        status: 200,
        headers: vec![],
        body: b"ok".to_vec(),
        delay_ms: 0,
    }));
    let resolver = test_resolver("evilzaloplatforms.com", LOOPBACK_IP);
    let url = format!(
        "http://evilzaloplatforms.com:{}/spoof.jpg",
        server.addr.port()
    );
    let err = ZaloMediaDownloader::new(loopback_policy(server.addr.port()), resolver)
        .download(&url)
        .await
        .expect_err("spoof");
    assert_eq!(err.class, ErrorClass::Validation);
}

#[tokio::test]
async fn download_rejects_private_resolved_addresses() {
    let server = MediaLoopbackServer::spawn(Arc::new(|_| MediaResponse {
        status: 200,
        headers: vec![],
        body: b"ok".to_vec(),
        delay_ms: 0,
    }));
    let resolver = test_resolver(TEST_HOST, IpAddr::V4(Ipv4Addr::new(10, 0, 0, 1)));
    let url = media_url(&server, "/private-dns.jpg");
    let err = ZaloMediaDownloader::new(
        loopback_policy_without_private(server.addr.port()),
        resolver,
    )
    .download(&url)
    .await
    .expect_err("private dns");
    assert_eq!(err.class, ErrorClass::Validation);
}

#[tokio::test]
async fn download_rejects_mixed_public_and_private_dns() {
    let mut resolver = InjectedMediaResolver::new();
    resolver.insert(
        TEST_HOST,
        vec![PUBLIC_RESOLVED_IP, IpAddr::V4(Ipv4Addr::new(10, 0, 0, 1))],
    );
    let err = ZaloMediaDownloader::new(MediaDownloadPolicy::production_default(), resolver)
        .download("https://s120.zdn.vn/mixed.jpg")
        .await
        .expect_err("mixed dns");
    assert_eq!(err.class, ErrorClass::Validation);
}

#[tokio::test]
async fn download_rejects_forbidden_resolved_address_families() {
    let cases: [(&str, IpAddr); 27] = [
        ("loopback", IpAddr::V4(Ipv4Addr::LOCALHOST)),
        ("private", IpAddr::V4(Ipv4Addr::new(10, 0, 0, 1))),
        (
            "link-local-metadata",
            IpAddr::V4(Ipv4Addr::new(169, 254, 169, 254)),
        ),
        ("multicast", IpAddr::V4(Ipv4Addr::new(224, 0, 0, 1))),
        ("unspecified", IpAddr::V4(Ipv4Addr::UNSPECIFIED)),
        ("this-network-0-8", IpAddr::V4(Ipv4Addr::new(0, 0, 0, 1))),
        ("cgnat-100-64", IpAddr::V4(Ipv4Addr::new(100, 64, 0, 1))),
        (
            "ietf-protocol-192-0-0",
            IpAddr::V4(Ipv4Addr::new(192, 0, 0, 1)),
        ),
        (
            "documentation-test-net-1",
            IpAddr::V4(Ipv4Addr::new(192, 0, 2, 1)),
        ),
        (
            "documentation-test-net-2",
            IpAddr::V4(Ipv4Addr::new(198, 51, 100, 1)),
        ),
        (
            "documentation-test-net-3",
            IpAddr::V4(Ipv4Addr::new(203, 0, 113, 1)),
        ),
        (
            "benchmarking-198-18",
            IpAddr::V4(Ipv4Addr::new(198, 18, 0, 1)),
        ),
        ("reserved-240-4", IpAddr::V4(Ipv4Addr::new(240, 0, 0, 1))),
        ("v6-loopback", IpAddr::V6(Ipv6Addr::LOCALHOST)),
        (
            "v6-link-local",
            IpAddr::V6(Ipv6Addr::new(0xfe80, 0, 0, 0, 0, 0, 0, 1)),
        ),
        (
            "v6-unique-local",
            IpAddr::V6(Ipv6Addr::new(0xfc00, 0, 0, 0, 0, 0, 0, 1)),
        ),
        (
            "v6-site-local",
            IpAddr::V6(Ipv6Addr::new(0xfec0, 0, 0, 0, 0, 0, 0, 1)),
        ),
        (
            "v6-documentation",
            IpAddr::V6(Ipv6Addr::new(0x2001, 0xdb8, 0, 0, 0, 0, 0, 1)),
        ),
        (
            "v6-documentation-3fff",
            IpAddr::V6(Ipv6Addr::new(0x3fff, 0, 0, 0, 0, 0, 0, 1)),
        ),
        (
            "v6-ietf-protocol-2001",
            IpAddr::V6(Ipv6Addr::new(0x2001, 0, 0, 0, 0, 0, 0, 1)),
        ),
        (
            "v6-discard-0100",
            IpAddr::V6(Ipv6Addr::new(0x100, 0, 0, 0, 0, 0, 0, 1)),
        ),
        (
            "v6-future-reserved-4000",
            IpAddr::V6(Ipv6Addr::new(0x4000, 0, 0, 0, 0, 0, 0, 1)),
        ),
        (
            "v6-multicast",
            IpAddr::V6(Ipv6Addr::new(0xff02, 0, 0, 0, 0, 0, 0, 1)),
        ),
        ("v6-unspecified", IpAddr::V6(Ipv6Addr::UNSPECIFIED)),
        (
            "v6-mapped-private",
            IpAddr::V6(Ipv6Addr::new(0, 0, 0, 0, 0, 0xffff, 0xa00, 1)),
        ),
        (
            "v6-mapped-cgnat",
            IpAddr::V6(Ipv6Addr::new(0, 0, 0, 0, 0, 0xffff, 0x6440, 1)),
        ),
        (
            "v6-mapped-documentation",
            IpAddr::V6(Ipv6Addr::new(0, 0, 0, 0, 0, 0xffff, 0xcb00, 0x7101)),
        ),
    ];
    for (label, ip) in cases {
        let resolver = test_resolver(TEST_HOST, ip);
        let err = ZaloMediaDownloader::new(MediaDownloadPolicy::production_default(), resolver)
            .download("https://s120.zdn.vn/forbidden.jpg")
            .await
            .expect_err(label);
        assert_eq!(err.class, ErrorClass::Validation, "{label}");
    }
}

#[tokio::test]
async fn download_rejects_ipv6_link_local_resolution() {
    let resolver = test_resolver(
        TEST_HOST,
        IpAddr::V6(Ipv6Addr::new(0xfe80, 0, 0, 0, 0, 0, 0, 1)),
    );
    let err = ZaloMediaDownloader::new(MediaDownloadPolicy::production_default(), resolver)
        .download("https://s120.zdn.vn/link-local.jpg")
        .await
        .expect_err("ipv6 link-local");
    assert_eq!(err.class, ErrorClass::Validation);
}

#[tokio::test]
async fn download_rejects_redirect_to_private_host() {
    let server = MediaLoopbackServer::spawn(Arc::new(|path| {
        if path == "/start.jpg" {
            MediaResponse {
                status: 302,
                headers: vec![(
                    "Location".to_string(),
                    "https://127.0.0.1/private.jpg".to_string(),
                )],
                body: vec![],
                delay_ms: 0,
            }
        } else {
            MediaResponse {
                status: 200,
                headers: vec![],
                body: b"never".to_vec(),
                delay_ms: 0,
            }
        }
    }));
    let resolver = test_resolver(TEST_HOST, LOOPBACK_IP);
    let url = media_url(&server, "/start.jpg");
    let err = downloader(&server, resolver)
        .download(&url)
        .await
        .expect_err("redirect private");
    assert_eq!(err.class, ErrorClass::Validation);
}

#[tokio::test]
async fn download_rejects_literal_private_redirect_under_test_policy() {
    let server = MediaLoopbackServer::spawn(Arc::new(|path| {
        if path == "/start.jpg" {
            MediaResponse {
                status: 302,
                headers: vec![(
                    "Location".to_string(),
                    "http://127.0.0.1/private.jpg".to_string(),
                )],
                body: vec![],
                delay_ms: 0,
            }
        } else {
            MediaResponse {
                status: 200,
                headers: vec![],
                body: b"never".to_vec(),
                delay_ms: 0,
            }
        }
    }));
    let resolver = test_resolver(TEST_HOST, LOOPBACK_IP);
    let url = media_url(&server, "/start.jpg");
    let err = downloader(&server, resolver)
        .download(&url)
        .await
        .expect_err("literal private redirect");
    assert_eq!(err.class, ErrorClass::Validation);
}

#[tokio::test]
async fn download_rejects_redirect_cap() {
    let server = MediaLoopbackServer::spawn(Arc::new(|path| {
        if path.starts_with("/hop") {
            let next = path
                .strip_prefix("/hop")
                .and_then(|n| n.parse::<u32>().ok())
                .unwrap_or(0);
            MediaResponse {
                status: 302,
                headers: vec![("Location".to_string(), format!("/hop{}", next + 1))],
                body: vec![],
                delay_ms: 0,
            }
        } else {
            MediaResponse {
                status: 200,
                headers: vec![],
                body: b"done".to_vec(),
                delay_ms: 0,
            }
        }
    }));
    let resolver = test_resolver(TEST_HOST, LOOPBACK_IP);
    let url = media_url(&server, "/hop0");
    let err = downloader(&server, resolver)
        .download(&url)
        .await
        .expect_err("redirect cap");
    assert_eq!(err.class, ErrorClass::Validation);
}

#[tokio::test]
async fn download_timeout_is_timeout_class() {
    let server = MediaLoopbackServer::spawn(Arc::new(|_| MediaResponse {
        status: 200,
        headers: vec![],
        body: b"slow".to_vec(),
        delay_ms: 500,
    }));
    let resolver = test_resolver(TEST_HOST, LOOPBACK_IP);
    let url = media_url(&server, "/slow.jpg");
    let mut policy = loopback_policy(server.addr.port());
    policy.total_timeout = Duration::from_millis(50);
    let err = ZaloMediaDownloader::new(policy, resolver)
        .download(&url)
        .await
        .expect_err("timeout");
    assert_eq!(err.class, ErrorClass::Timeout);
}

#[tokio::test]
async fn download_multi_hop_shares_total_deadline() {
    let server = MediaLoopbackServer::spawn(Arc::new(|path| {
        let delay_ms = if path == "/first.jpg" || path == "/second.jpg" {
            80
        } else {
            0
        };
        if path == "/first.jpg" {
            MediaResponse {
                status: 302,
                headers: vec![("Location".to_string(), "/second.jpg".to_string())],
                body: vec![],
                delay_ms,
            }
        } else {
            MediaResponse {
                status: 200,
                headers: vec![],
                body: b"late".to_vec(),
                delay_ms,
            }
        }
    }));
    let resolver = test_resolver(TEST_HOST, LOOPBACK_IP);
    let url = media_url(&server, "/first.jpg");
    let mut policy = loopback_policy(server.addr.port());
    policy.total_timeout = Duration::from_millis(120);
    let err = ZaloMediaDownloader::new(policy, resolver)
        .download(&url)
        .await
        .expect_err("multi-hop deadline");
    assert_eq!(err.class, ErrorClass::Timeout);
}

#[tokio::test]
async fn download_status_classification() {
    for (status, expected) in [
        (429_u16, ErrorClass::RateLimited),
        (500, ErrorClass::ProviderError),
        (400, ErrorClass::Validation),
    ] {
        let server = MediaLoopbackServer::spawn(Arc::new(move |_| MediaResponse {
            status,
            headers: vec![],
            body: b"err".to_vec(),
            delay_ms: 0,
        }));
        let resolver = test_resolver(TEST_HOST, LOOPBACK_IP);
        let url = media_url(&server, "/status.jpg");
        let err = downloader(&server, resolver)
            .download(&url)
            .await
            .expect_err("status");
        assert_eq!(err.class, expected, "status {}", status);
    }
}

#[tokio::test]
async fn download_rejects_content_length_over_cap() {
    let server = MediaLoopbackServer::spawn(Arc::new(|_| MediaResponse {
        status: 200,
        headers: vec![("Content-Length".to_string(), "10485761".to_string())],
        body: vec![],
        delay_ms: 0,
    }));
    let resolver = test_resolver(TEST_HOST, LOOPBACK_IP);
    let url = media_url(&server, "/huge.jpg");
    let err = downloader(&server, resolver)
        .download(&url)
        .await
        .expect_err("content-length");
    assert_eq!(err.class, ErrorClass::Validation);
}

#[tokio::test]
async fn download_aborts_streaming_body_over_cap() {
    let server = MediaLoopbackServer::spawn(Arc::new(|_| MediaResponse {
        status: 200,
        headers: vec![],
        body: vec![0_u8; 1024],
        delay_ms: 0,
    }));
    let resolver = test_resolver(TEST_HOST, LOOPBACK_IP);
    let url = media_url(&server, "/stream.jpg");
    let mut policy = loopback_policy(server.addr.port());
    policy.max_bytes = 512;
    let err = ZaloMediaDownloader::new(policy, resolver)
        .download(&url)
        .await
        .expect_err("stream cap");
    assert_eq!(err.class, ErrorClass::Validation);
}

#[tokio::test]
async fn download_accepts_small_body_when_policy_max_bytes_is_u64_max() {
    let server = MediaLoopbackServer::spawn(Arc::new(|_| MediaResponse {
        status: 200,
        headers: vec![],
        body: b"ok".to_vec(),
        delay_ms: 0,
    }));
    let resolver = test_resolver(TEST_HOST, LOOPBACK_IP);
    let url = media_url(&server, "/tiny.jpg");
    let mut policy = loopback_policy(server.addr.port());
    policy.max_bytes = u64::MAX;
    let result = ZaloMediaDownloader::new(policy, resolver)
        .download(&url)
        .await
        .expect("u64::MAX policy");
    assert_eq!(result.bytes, b"ok");
}

#[tokio::test]
async fn download_error_never_includes_url_or_body() {
    const SECRET_URL: &str = "https://s120.zdn.vn/secret-token-path/photo.jpg";
    let server = MediaLoopbackServer::spawn(Arc::new(|_| MediaResponse {
        status: 500,
        headers: vec![],
        body: b"unrelated-private-provider-payload".to_vec(),
        delay_ms: 0,
    }));
    let resolver = test_resolver(TEST_HOST, LOOPBACK_IP);
    let url = format!(
        "http://{TEST_HOST}:{}/secret-token-path/photo.jpg",
        server.addr.port()
    );
    let err = downloader(&server, resolver)
        .download(&url)
        .await
        .expect_err("500");
    let rendered = format!("{err} {err:?}");
    assert!(!rendered.contains(SECRET_URL));
    assert!(!rendered.contains("secret-token-path"));
    assert!(!rendered.contains("unrelated-private-provider-payload"));
}

#[tokio::test]
async fn download_follows_allowlisted_redirect() {
    let hops: Arc<Mutex<u32>> = Arc::new(Mutex::new(0));
    let server = MediaLoopbackServer::spawn({
        let hops = Arc::clone(&hops);
        Arc::new(move |path| {
            if path == "/first.jpg" {
                MediaResponse {
                    status: 302,
                    headers: vec![("Location".to_string(), "/second.jpg".to_string())],
                    body: vec![],
                    delay_ms: 0,
                }
            } else {
                *hops.lock().expect("lock") += 1;
                MediaResponse {
                    status: 200,
                    headers: vec![("Content-Type".to_string(), "image/png".to_string())],
                    body: b"png-bytes".to_vec(),
                    delay_ms: 0,
                }
            }
        })
    });
    let resolver = test_resolver(TEST_HOST, LOOPBACK_IP);
    let url = media_url(&server, "/first.jpg");
    let result = downloader(&server, resolver)
        .download(&url)
        .await
        .expect("redirect ok");
    assert_eq!(result.bytes, b"png-bytes");
    assert_eq!(result.content_type.as_deref(), Some("image/png"));
    assert_eq!(*hops.lock().expect("lock"), 1);
}

#[tokio::test]
async fn download_rejects_production_nondefault_port_and_userinfo() {
    let resolver = test_resolver(TEST_HOST, PUBLIC_RESOLVED_IP);
    let policy = MediaDownloadPolicy::production_default();

    let port_err = ZaloMediaDownloader::new(policy.clone(), resolver.clone())
        .download("https://s120.zdn.vn:8443/photo.jpg")
        .await
        .expect_err("port");
    assert_eq!(port_err.class, ErrorClass::Validation);
    let port_rendered = format!("{port_err} {port_err:?}");
    assert!(!port_rendered.contains("8443"));
    assert!(!port_rendered.contains("s120.zdn.vn"));

    let userinfo_err = ZaloMediaDownloader::new(policy, resolver)
        .download("https://user:pass@s120.zdn.vn/photo.jpg")
        .await
        .expect_err("userinfo");
    assert_eq!(userinfo_err.class, ErrorClass::Validation);
    let userinfo_rendered = format!("{userinfo_err} {userinfo_err:?}");
    assert!(!userinfo_rendered.contains("user:pass"));
    assert!(!userinfo_rendered.contains("s120.zdn.vn"));
}

#[test]
fn media_download_result_debug_redacts_bytes_and_content_type() {
    const SECRET_BYTES: &[u8] = b"SECRET-RECEIPT-BYTES";
    const SECRET_TYPE: &str = "image/jpeg; x-secret=HEADER-SECRET-VALUE";
    let result = MediaDownloadResult {
        bytes: SECRET_BYTES.to_vec(),
        content_type: Some(SECRET_TYPE.to_string()),
    };
    let debug = format!("{result:?}");
    assert!(!debug.contains("SECRET-RECEIPT-BYTES"));
    assert!(!debug.contains("HEADER-SECRET-VALUE"));
    assert!(!debug.contains(SECRET_TYPE));
    assert!(!debug.contains("image/jpeg"));
    assert!(debug.contains("bytes_len"));
    assert!(debug.contains(&SECRET_BYTES.len().to_string()));
    assert!(debug.contains("content_type_present"));
}
