//! The stored decoder accepts closed object tuples, never positional substitutes.
use super::*;
use serde_json::{Value, json};

fn literal() -> Value {
    json!({"schema_version":1,"members":[{"hold_id":"11111111111111111111111111111111","sku":"SET-COVERS","revision":1,"units":1,"components":[{"component_code":"SEAT-COVER","quantity":2}]}],"components":[{"component_code":"SEAT-COVER","quantity":2,"before":{"on_hand":10,"quarantine":1,"reserved":2},"after":{"on_hand":8,"quarantine":1,"reserved":0}}]})
}
fn accepted(value: Value) {
    let decoded = decode_footprint(value).expect("literal closed footprint");
    assert_eq!(decoded.schema_version, 1);
    assert_eq!(decoded.validate(), Ok(()));
}
fn rejected(value: Value) {
    assert!(matches!(
        decode_footprint(value),
        Err(SettlementError::InvalidStoredData)
    ));
    accepted(literal());
}
#[test]
fn decoder_requires_objects_and_nonempty_bounded_collections_at_every_level() {
    accepted(literal());
    for value in [
        Value::Null,
        json!([]),
        json!({}),
        json!({"members":[]}),
        json!({"members":{},"components":[]}),
    ] {
        rejected(value);
    }
    let mutations: [fn(&mut Value); 15] = [
        |v| v["members"] = json!([]),
        |v| v["components"] = json!([]),
        |v| v["members"] = json!({}),
        |v| v["components"] = json!({}),
        |v| v["members"][0] = json!([]),
        |v| {
            v["members"][0]
                .as_object_mut()
                .expect("literal member")
                .remove("components");
        },
        |v| v["members"][0]["components"] = Value::Null,
        |v| v["members"][0]["components"] = json!({}),
        |v| v["members"][0]["components"] = json!([]),
        |v| v["members"][0]["components"][0] = json!([]),
        |v| v["components"][0] = json!([]),
        |v| {
            v["components"][0]
                .as_object_mut()
                .expect("literal component")
                .remove("before");
        },
        |v| v["components"][0]["before"] = json!([]),
        |v| {
            v["components"][0]
                .as_object_mut()
                .expect("literal component")
                .remove("after");
        },
        |v| v["components"][0]["after"] = json!([]),
    ];
    for change in mutations {
        let mut damaged = literal();
        change(&mut damaged);
        rejected(damaged);
    }
    let mut too_many_members = literal();
    too_many_members["members"] = Value::Array(vec![literal()["members"][0].clone(); 21]);
    rejected(too_many_members);
    let mut too_many_lines = literal();
    too_many_lines["members"][0]["components"] = Value::Array(vec![
        json!({"component_code":"SEAT-COVER","quantity":2});
        65
    ]);
    rejected(too_many_lines);
    let mut too_many_components = literal();
    too_many_components["components"] =
        Value::Array(vec![literal()["components"][0].clone(); 1281]);
    rejected(too_many_components);
}
#[test]
fn decoder_preserves_closed_metadata_types_and_unknown_fields_are_rejected() {
    for pointer in [
        "",
        "/members/0",
        "/members/0/components/0",
        "/components/0",
        "/components/0/before",
        "/components/0/after",
    ] {
        let mut extra = literal();
        extra
            .pointer_mut(pointer)
            .expect("literal tuple")
            .as_object_mut()
            .expect("literal object")
            .insert("unknown".into(), true.into());
        rejected(extra);
    }
    for pointer in [
        "/schema_version",
        "/members/0/hold_id",
        "/members/0/sku",
        "/members/0/revision",
        "/members/0/units",
        "/members/0/components/0/component_code",
        "/members/0/components/0/quantity",
        "/components/0/component_code",
        "/components/0/quantity",
        "/components/0/before/on_hand",
        "/components/0/before/quarantine",
        "/components/0/before/reserved",
        "/components/0/after/on_hand",
        "/components/0/after/quarantine",
        "/components/0/after/reserved",
    ] {
        let mut missing = literal();
        *missing
            .pointer_mut(pointer)
            .expect("literal required value") = Value::Null;
        rejected(missing);
    }
    let mut version = literal();
    version["schema_version"] = 2.into();
    let decoded = decode_footprint(version).expect("numeric schema is structurally typed");
    assert!(
        decoded.validate().is_err(),
        "separate semantic validation rejects unsupported schema"
    );
}
#[test]
fn decoder_accepts_the_full_twenty_by_sixty_four_and_1280_union_object_boundary() {
    let mut members = vec![];
    let mut components = vec![];
    for member in 0..20 {
        let mut lines = vec![];
        for line in 0..64 {
            let code = format!("COVER-{member:02}-{line:02}");
            lines.push(json!({"component_code":code,"quantity":1}));
            components.push(json!({"component_code":code,"quantity":1,"before":{"on_hand":2,"quarantine":0,"reserved":1},"after":{"on_hand":1,"quarantine":0,"reserved":0}}));
        }
        members.push(json!({"hold_id":format!("{:032x}",member+1),"sku":format!("SET-{member:02}"),"revision":1,"units":1,"components":lines}));
    }
    let decoded =
        decode_footprint(json!({"schema_version":1,"members":members,"components":components}))
            .expect("literal maximum closed object footprint");
    assert_eq!(decoded.members.len(), 20);
    assert_eq!(decoded.components.len(), 1280);
    assert!(
        decoded
            .members
            .iter()
            .all(|member| member.components.len() == 64)
    );
    assert_eq!(decoded.validate(), Ok(()));
}

#[path = "order_stock_rows_postgres.rs"]
mod postgres;
