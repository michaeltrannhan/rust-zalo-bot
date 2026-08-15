//! Image webhook parsing contract tests.

use chrono::{TimeZone, Utc};
use zl_expense::error::ErrorClass;
use zl_expense::provider::{InboundEventKind, ZaloHttpAdapter, ZaloHttpConfig};

fn fixture(name: &str) -> Vec<u8> {
    let path = format!("tests/fixtures/zalo/{}", name);
    std::fs::read(&path).unwrap_or_else(|e| panic!("read fixture {}: {}", path, e))
}

fn test_config() -> ZaloHttpConfig {
    ZaloHttpConfig {
        api_base: "http://unused".to_string(),
        bot_token: "tok".to_string(),
        webhook_secret: "sec".to_string(),
        provider_scope: "zalo_bot".to_string(),
        request_timeout: std::time::Duration::from_secs(2),
    }
}

#[test]
fn parse_image_flat_fixture_photo_field() {
    let adapter = ZaloHttpAdapter::new(test_config()).expect("adapter");
    let event = adapter
        .parse_image_webhook(&fixture("image_flat.json"))
        .expect("parse");
    assert_eq!(event.provider_scope, "zalo_bot");
    assert_eq!(event.event_id, "img-msg-001");
    assert_eq!(event.sender_id, "8891234567890123456");
    assert_eq!(event.chat_id, "7009876543210987654");
    assert_eq!(event.image_url, "https://s120.zdn.vn/redacted/receipt.jpg");
    assert_eq!(event.caption, "lunch receipt");
    assert_eq!(event.received_at, Utc.timestamp_opt(1752854400, 0).unwrap());
    assert_eq!(event.kind, InboundEventKind::ImageReceived);
}

#[test]
fn parse_image_photo_url_legacy_shape() {
    let adapter = ZaloHttpAdapter::new(test_config()).expect("adapter");
    let event = adapter
        .parse_image_webhook(&fixture("image_photo_url.json"))
        .expect("parse");
    assert_eq!(event.image_url, "https://s120.zadn.vn/legacy/photo.jpg");
    assert_eq!(event.kind, InboundEventKind::ImageReceived);
}

#[test]
fn parse_image_envelope_with_photos_array() {
    let adapter = ZaloHttpAdapter::new(test_config()).expect("adapter");
    let event = adapter
        .parse_image_webhook(&fixture("image_envelope.json"))
        .expect("parse");
    assert_eq!(event.event_id, "img-envelope-001");
    assert_eq!(event.image_url, "https://s120.zdn.vn/envelope/photo.jpg");
    assert_eq!(event.caption, "from envelope");
    assert_eq!(
        event.received_at,
        Utc.timestamp_millis_opt(1750316131602).unwrap()
    );
}

#[test]
fn parse_image_tolerates_image_and_attachments_shapes() {
    let adapter = ZaloHttpAdapter::new(test_config()).expect("adapter");

    let body = serde_json::json!({
        "event_name": "message.image.received",
        "message": {
            "message_id": "img-shape-1",
            "from": { "id": "s1" },
            "chat": { "id": "c1" },
            "date": 1752854400_i64,
            "image": { "url": "https://s120.zdn.vn/image-object.jpg" }
        }
    });
    let event = adapter
        .parse_image_webhook(&serde_json::to_vec(&body).expect("json"))
        .expect("image object");
    assert_eq!(event.image_url, "https://s120.zdn.vn/image-object.jpg");

    let body = serde_json::json!({
        "event_name": "message.image.received",
        "message": {
            "message_id": "img-shape-2",
            "from": { "id": "s1" },
            "chat": { "id": "c1" },
            "date": 1752854400_i64,
            "attachments": "https://s120.zdn.vn/attachment-string.jpg"
        }
    });
    let event = adapter
        .parse_image_webhook(&serde_json::to_vec(&body).expect("json"))
        .expect("attachment string");
    assert_eq!(event.image_url, "https://s120.zdn.vn/attachment-string.jpg");

    let body = serde_json::json!({
        "event_name": "message.image.received",
        "message": {
            "message_id": "img-shape-3",
            "from": { "id": "s1" },
            "chat": { "id": "c1" },
            "date": 1752854400_i64,
            "attachments": [
                { "photo": "https://s120.zdn.vn/attachment-array.jpg" }
            ]
        }
    });
    let event = adapter
        .parse_image_webhook(&serde_json::to_vec(&body).expect("json"))
        .expect("attachment array");
    assert_eq!(event.image_url, "https://s120.zdn.vn/attachment-array.jpg");
}

#[test]
fn parse_image_missing_url_is_validation() {
    let adapter = ZaloHttpAdapter::new(test_config()).expect("adapter");
    let err = adapter
        .parse_image_webhook(&fixture("image_missing_url.json"))
        .expect_err("missing url");
    assert_eq!(err.class, ErrorClass::Validation);
}

#[test]
fn parse_image_oversized_caption_is_validation() {
    let adapter = ZaloHttpAdapter::new(test_config()).expect("adapter");
    let body = serde_json::json!({
        "event_name": "message.image.received",
        "message": {
            "message_id": "m1",
            "from": { "id": "s1" },
            "chat": { "id": "c1" },
            "date": 1752854400_i64,
            "photo": "https://s120.zdn.vn/photo.jpg",
            "caption": "x".repeat(2001),
        }
    });
    let err = adapter
        .parse_image_webhook(&serde_json::to_vec(&body).expect("json"))
        .expect_err("caption");
    assert_eq!(err.class, ErrorClass::Validation);
}

#[test]
fn parse_image_bounded_identifiers() {
    let adapter = ZaloHttpAdapter::new(test_config()).expect("adapter");
    let body = serde_json::json!({
        "event_name": "message.image.received",
        "message": {
            "message_id": "m".repeat(257),
            "from": { "id": "s1" },
            "chat": { "id": "c1" },
            "date": 1752854400_i64,
            "photo": "https://s120.zdn.vn/photo.jpg",
        }
    });
    let err = adapter
        .parse_image_webhook(&serde_json::to_vec(&body).expect("json"))
        .expect_err("id");
    assert_eq!(err.class, ErrorClass::Validation);
}

#[test]
fn parse_unsupported_image_event_is_observable() {
    let adapter = ZaloHttpAdapter::new(test_config()).expect("adapter");
    let body = serde_json::json!({
        "event_name": "message.sticker.received",
        "message": {
            "message_id": "sticker-1",
            "from": { "id": "s1" },
            "chat": { "id": "c1" },
            "date": 1752854400_i64,
            "photo": "https://s120.zdn.vn/sticker.jpg"
        }
    });
    let event = adapter
        .parse_image_webhook(&serde_json::to_vec(&body).expect("json"))
        .expect("parse");
    assert_eq!(
        event.kind,
        InboundEventKind::Unsupported("message.sticker.received".to_string())
    );
}

#[test]
fn debug_output_redacts_normalized_image() {
    const EVENT_ID: &str = "event-private-img";
    const SENDER: &str = "sender-private-img";
    const CHAT: &str = "chat-sensitive-img";
    const URL: &str = "https://s120.zdn.vn/private/secret.jpg";
    const CAPTION: &str = "sensitive caption text";

    let adapter = ZaloHttpAdapter::new(test_config()).expect("adapter");
    let body = serde_json::json!({
        "event_name": "message.image.received",
        "message": {
            "message_id": EVENT_ID,
            "from": { "id": SENDER },
            "chat": { "id": CHAT },
            "date": 1752854400_i64,
            "photo": URL,
            "caption": CAPTION,
        }
    });
    let event = adapter
        .parse_image_webhook(&serde_json::to_vec(&body).expect("json"))
        .expect("event");
    let debug = format!("{event:?}");
    for secret in [EVENT_ID, SENDER, CHAT, URL, CAPTION] {
        assert!(!debug.contains(secret));
    }
}

#[test]
fn parse_image_trims_url_and_walks_unusable_array_entries() {
    let adapter = ZaloHttpAdapter::new(test_config()).expect("adapter");
    let body = serde_json::json!({
        "event_name": "message.image.received",
        "message": {
            "message_id": "img-trim-1",
            "from": { "id": "s1" },
            "chat": { "id": "c1" },
            "date": 1752854400_i64,
            "photos": [
                "",
                { "note": "unusable" },
                { "file_url": "  https://s120.zdn.vn/array-fallback.jpg  " }
            ]
        }
    });
    let event = adapter
        .parse_image_webhook(&serde_json::to_vec(&body).expect("json"))
        .expect("array fallback");
    assert_eq!(event.image_url, "https://s120.zdn.vn/array-fallback.jpg");
    assert_eq!(event.caption, "");
}

#[test]
fn parse_image_supports_file_url_and_nested_caption() {
    let adapter = ZaloHttpAdapter::new(test_config()).expect("adapter");
    let body = serde_json::json!({
        "event_name": "message.image.received",
        "message": {
            "message_id": "img-file-url",
            "from": { "id": "s1" },
            "chat": { "id": "c1" },
            "date": 1752854400_i64,
            "photo": {
                "file_url": "https://s120.zdn.vn/nested-file.jpg",
                "caption": "nested caption"
            }
        }
    });
    let event = adapter
        .parse_image_webhook(&serde_json::to_vec(&body).expect("json"))
        .expect("file_url");
    assert_eq!(event.image_url, "https://s120.zdn.vn/nested-file.jpg");
    assert_eq!(event.caption, "nested caption");
}

#[test]
fn parse_image_bounds_event_name() {
    let adapter = ZaloHttpAdapter::new(test_config()).expect("adapter");
    let body = serde_json::json!({
        "event_name": "e".repeat(257),
        "message": {
            "message_id": "m1",
            "from": { "id": "s1" },
            "chat": { "id": "c1" },
            "date": 1752854400_i64,
            "photo": "https://s120.zdn.vn/photo.jpg",
        }
    });
    let err = adapter
        .parse_image_webhook(&serde_json::to_vec(&body).expect("json"))
        .expect_err("event name");
    assert_eq!(err.class, ErrorClass::Validation);
}
