#![allow(clippy::unwrap_used, clippy::expect_used)]

//! Identifier round-trips and ordering.

use evalhub_core::id::{RecordId, VersionId};

#[test]
fn text_form_round_trips_and_is_26_chars() {
    let id = RecordId::new();
    let text = id.to_string();
    assert_eq!(text.len(), 26);
    assert_eq!(text.parse::<RecordId>().unwrap(), id);
    assert_eq!(text.to_lowercase().parse::<RecordId>().unwrap(), id);
    assert!("not-a-ulid".parse::<RecordId>().is_err());
}

#[test]
fn uuid_form_round_trips_and_keeps_the_bits() {
    let id = VersionId::from_parts(1_700_000_000_000, 0x1234_5678_9abc_def0_1234);
    let uuid = id.as_uuid();
    assert_eq!(VersionId::from_uuid(uuid), id);
    assert_eq!(uuid.as_u128() >> 80, 1_700_000_000_000);
}

#[test]
fn serde_uses_the_text_form() {
    let id = VersionId::new();
    let json = serde_json::to_string(&id).unwrap();
    assert_eq!(json, format!("\"{id}\""));
    let back: VersionId = serde_json::from_str(&json).unwrap();
    assert_eq!(back, id);
}

#[test]
fn later_ids_sort_higher_and_new_ids_are_distinct() {
    let earlier = RecordId::from_parts(1_000, u128::MAX >> 48);
    let later = RecordId::from_parts(1_001, 0);
    assert!(earlier < later);

    let a = VersionId::new();
    std::thread::sleep(std::time::Duration::from_millis(2));
    let b = VersionId::new();
    assert_ne!(a, b);
    assert!(a < b);
    assert!(a.timestamp_ms() < b.timestamp_ms());
}

#[test]
fn debug_names_the_type() {
    let id = RecordId::from_parts(0, 0);
    assert_eq!(format!("{id:?}"), "RecordId(00000000000000000000000000)");
}
