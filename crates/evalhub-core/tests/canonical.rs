#![allow(clippy::unwrap_used, clippy::expect_used)]

//! RFC 8785 test vector, key-order independence, and the fixed-point property.

use std::path::PathBuf;

use evalhub_core::canonical::{ContentHash, canonicalize, content_hash, hash_value};
use proptest::prelude::*;
use serde_json::{Value, json};

fn fixture(name: &str) -> Vec<u8> {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("fixtures/canonical")
        .join(name);
    std::fs::read(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

/// Expected canonical bytes are stored as files; an editor may add one
/// trailing newline, which is not part of the value.
fn expected(name: &str) -> Vec<u8> {
    let mut bytes = fixture(name);
    if bytes.last() == Some(&b'\n') {
        bytes.pop();
    }
    bytes
}

fn parse(bytes: &[u8]) -> Value {
    serde_json::from_slice(bytes).unwrap()
}

/// The worked example of RFC 8785 §3.2.3. The input is built with Rust
/// escapes rather than kept as a JSON file because it contains U+000F,
/// which JSON text can only carry escaped; the value is the same one the
/// RFC's input text parses to. The expected output is the RFC's, verbatim.
#[test]
// The RFC's input deliberately carries more digits than a double holds;
// that the excess is dropped is part of what the vector checks.
#[allow(clippy::excessive_precision)]
fn rfc8785_section_3_2_3_example() {
    let value = json!({
        "numbers": [333333333.33333329_f64, 1E30_f64, 4.50_f64, 2e-3_f64, 0.000000000000000000000000001_f64],
        "string": "\u{20ac}$\u{000F}\nA'B\"\\\\\"/",
        "literals": [null, true, false]
    });
    let expected = "{\"literals\":[null,true,false],\
        \"numbers\":[333333333.3333333,1e+30,4.5,0.002,1e-27],\
        \"string\":\"\u{20ac}$\\u000f\\nA'B\\\"\\\\\\\\\\\"/\"}";
    let bytes = canonicalize(&value).unwrap();
    assert_eq!(String::from_utf8(bytes).unwrap(), expected);
}

#[test]
fn key_order_and_number_spelling_do_not_change_the_bytes_or_the_hash() {
    let a = parse(&fixture("key-order-a.json"));
    let b = parse(&fixture("key-order-b.json"));
    let (bytes_a, hash_a) = hash_value(&a).unwrap();
    let (bytes_b, hash_b) = hash_value(&b).unwrap();
    assert_eq!(bytes_a, expected("key-order.canonical"));
    assert_eq!(bytes_a, bytes_b);
    assert_eq!(hash_a, hash_b);
}

#[test]
fn content_hash_is_sha256_hex_and_round_trips() {
    // sha256("") is the well-known digest; canonical bytes of `{}` are "{}".
    let empty = content_hash(b"");
    assert_eq!(
        empty.as_hex(),
        "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
    );
    let obj = content_hash(&canonicalize(&json!({})).unwrap());
    assert_eq!(
        obj.as_hex(),
        "44136fa355b3678a1146ad16f7e8649e94fb4fc21fe77e8310c060f61caaff8a"
    );
    let parsed: ContentHash = obj.as_hex().parse().unwrap();
    assert_eq!(parsed, obj);
    assert_eq!(parsed.to_string(), obj.as_hex());
    assert!("abc".parse::<ContentHash>().is_err());
    assert!("zz".repeat(32).parse::<ContentHash>().is_err());
}

#[test]
fn absent_and_null_are_different_values() {
    let recorded_unknown = json!({"task": {"id": "t", "seed": null}});
    let not_recorded = json!({"task": {"id": "t"}});
    assert_ne!(
        hash_value(&recorded_unknown).unwrap().1,
        hash_value(&not_recorded).unwrap().1
    );
}

fn arb_json() -> impl Strategy<Value = Value> {
    let leaf = prop_oneof![
        Just(Value::Null),
        any::<bool>().prop_map(Value::Bool),
        // Finite doubles only: JSON has no NaN or infinity.
        any::<f64>()
            .prop_filter("finite", |f| f.is_finite())
            .prop_map(|f| serde_json::Number::from_f64(f)
                .map(Value::Number)
                .unwrap_or(Value::Null)),
        any::<i64>().prop_map(Value::from),
        "[\\PC\\n\\t\"\\\\]{0,16}".prop_map(Value::String),
    ];
    leaf.prop_recursive(4, 64, 8, |inner| {
        prop_oneof![
            prop::collection::vec(inner.clone(), 0..8).prop_map(Value::Array),
            prop::collection::btree_map("[a-zA-Z0-9 _\\-\\p{Greek}]{0,8}", inner, 0..8)
                .prop_map(|m| Value::Object(m.into_iter().collect())),
        ]
    })
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(512))]

    #[test]
    fn canonical_form_is_a_fixed_point(value in arb_json()) {
        let once = canonicalize(&value).unwrap();
        let reparsed: Value = serde_json::from_slice(&once).unwrap();
        let twice = canonicalize(&reparsed).unwrap();
        prop_assert_eq!(&once, &twice);
        prop_assert_eq!(content_hash(&once), content_hash(&twice));
    }
}
