//! Literal transport policy and decoding controls independent of provider authority.
use super::*;
use axum::http::HeaderValue;

fn headers(value: &str) -> HeaderMap {
    let mut headers = HeaderMap::new();
    headers.insert(
        CONTENT_TYPE,
        HeaderValue::from_str(value).expect("fixture header"),
    );
    headers
}
fn form() -> HeaderMap {
    headers("application/x-www-form-urlencoded")
}

#[test]
fn callback_form_decodes_exactly_one_id_without_narrowing_the_opaque_suffix() {
    for (body, expected) in [
        ("id=tr_SYNTHETIC", "tr_SYNTHETIC"),
        ("%69%64=%74%72%5fSYNTHETIC", "tr_SYNTHETIC"),
        ("i%64=tr_a%2Bb+c", "tr_a+b c"),
        ("id=tr_%C3%A9", "tr_é"),
        ("id=tr_é", "tr_é"),
        ("id=tr_a%26b%3Dc", "tr_a&b=c"),
        ("id=tr_a=b", "tr_a=b"),
        ("id=tr_%252B", "tr_%2B"),
        ("id=tr_%3a%2F%3F%23", "tr_:/?#"),
    ] {
        assert_eq!(
            parse(&form(), body.as_bytes()),
            Ok(expected.into()),
            "{body}"
        );
    }
    assert_eq!(
        decode(b"%00%09%0A%0a%ff").expect_err("strict UTF-8"),
        FormError::InvalidBody
    );
    assert_eq!(
        decode(b"%00%09%0A%0a").expect("decode before domain controls"),
        "\0\t\n\n"
    );
}

#[test]
fn callback_form_rejects_duplicates_unknown_names_empty_segments_and_incomplete_pairs() {
    for body in [
        "",
        "id",
        "id=",
        "=tr_a",
        "status=paid",
        "ID=tr_a",
        " id=tr_a",
        "i+d=tr_a",
        "id=tr_a&id=tr_b",
        "id=tr_a&%69%64=tr_b",
        "%69d=tr_a&i%64=tr_b",
        "id=tr_a&status=paid",
        "status=paid&id=tr_a",
        "&id=tr_a",
        "id=tr_a&",
        "id=tr_a&&",
        "&",
        "=",
        "id%3D=tr_a",
        "%2569d=tr_a",
        "id=tr_",
        "id=wrong",
        "id=+tr_a",
        "id=tr_a+",
        "id=tr_a%00",
        "id=tr_a%09",
        "id=tr_a%7F",
        "id=tr_a%C2%85",
        "id=tr_a%C2%A0",
    ] {
        assert_eq!(
            parse(&form(), body.as_bytes()),
            Err(FormError::InvalidBody),
            "{body}"
        );
    }
}

#[test]
fn callback_form_percent_and_utf8_decoding_fail_closed_without_replacement_characters() {
    for body in [
        b"%=tr_a".as_slice(),
        b"%6=tr_a",
        b"%GG=tr_a",
        b"%6G=tr_a",
        b"id=tr_%",
        b"id=tr_%2",
        b"id=tr_%G1",
        b"id=tr_%1G",
        b"id=tr_%ff",
        b"id=tr_%C3",
        b"id=tr_%C0%AF",
        b"id=tr_%ED%A0%80",
        b"id=tr_%F4%90%80%80",
        b"%ff=tr_a",
        b"id=tr_\xff",
    ] {
        assert_eq!(
            parse(&form(), body),
            Err(FormError::InvalidBody),
            "{body:?}"
        );
    }
}

#[test]
fn callback_form_raw_and_decoded_byte_limits_are_distinct_literal_boundaries() {
    assert_eq!(MAX_BODY, 1024);
    let id = format!("tr_{}", "x".repeat(253));
    assert_eq!(
        parse(&form(), format!("id={id}").as_bytes()),
        Ok(id.clone())
    );
    assert_eq!(
        parse(&form(), format!("id={id}x").as_bytes()),
        Err(FormError::InvalidBody)
    );
    let unicode = format!("tr_{}x", "é".repeat(126));
    assert_eq!(
        parse(&form(), format!("id={unicode}").as_bytes()),
        Ok(unicode.clone())
    );
    assert_eq!(
        parse(&form(), format!("id={unicode}x").as_bytes()),
        Err(FormError::InvalidBody)
    );
    let encoded = format!("%69%64=%74%72%5F{}", "%78".repeat(253));
    assert_eq!(encoded.len(), 775);
    assert_eq!(parse(&form(), encoded.as_bytes()), Ok(id));
    let at_limit = format!("id=tr_{}", "x".repeat(1018));
    assert_eq!(at_limit.len(), 1024);
    assert_eq!(
        parse(&form(), at_limit.as_bytes()),
        Err(FormError::InvalidBody)
    );
    assert_eq!(
        parse(&form(), format!("{at_limit}x").as_bytes()),
        Err(FormError::TooLarge)
    );
}

#[test]
fn callback_form_accepts_case_insensitive_media_type_and_only_one_optional_utf8_charset() {
    for value in [
        "application/x-www-form-urlencoded",
        "APPLICATION/X-WWW-FORM-URLENCODED",
        " \tapplication/x-www-form-urlencoded\t ",
        "application/x-www-form-urlencoded;charset=utf-8",
        "application/x-www-form-urlencoded; CHARSET = UTF-8 ",
        "application/x-www-form-urlencoded;\tcharset\t=\t\"Utf-8\"\t",
    ] {
        assert_eq!(
            parse(&headers(value), b"id=tr_a"),
            Ok("tr_a".into()),
            "{value}"
        );
    }
    let padded = format!("application/x-www-form-urlencoded{}", " ".repeat(95));
    assert_eq!(padded.len(), 128);
    assert_eq!(parse(&headers(&padded), b"id=tr_a"), Ok("tr_a".into()));
    assert_eq!(
        parse(&headers(&format!("{padded} ")), b"id=tr_a"),
        Err(FormError::UnsupportedMediaType)
    );
}

#[test]
fn callback_form_rejects_missing_duplicate_encoded_or_unsupported_media_headers() {
    assert_eq!(
        parse(&HeaderMap::new(), b"id=tr_a"),
        Err(FormError::UnsupportedMediaType)
    );
    for value in [
        "",
        "application/json",
        "text/plain",
        "application/x-www-form-urlencoded, application/x-www-form-urlencoded",
        "application/x-www-form-urlencoded;",
        "application/x-www-form-urlencoded;charset",
        "application/x-www-form-urlencoded;encoding=utf-8",
        "application/x-www-form-urlencoded;charset=ascii",
        "application/x-www-form-urlencoded;charset=",
        "application/x-www-form-urlencoded;charset='utf-8'",
        "application/x-www-form-urlencoded;charset=\" utf-8\"",
        "application/x-www-form-urlencoded;charset=\"utf-8",
        "application/x-www-form-urlencoded;charset=utf-8;",
        "application/x-www-form-urlencoded;charset=utf-8;charset=utf-8",
        "application/x-www-form-urlencoded;charset=utf-8;other=x",
        "application/x-www-form-urlencoded;charset=\"utf-8\"junk",
    ] {
        assert_eq!(
            parse(&headers(value), b"id=tr_a"),
            Err(FormError::UnsupportedMediaType),
            "{value}"
        );
    }
    let mut duplicate = form();
    duplicate.append(
        CONTENT_TYPE,
        HeaderValue::from_static("application/x-www-form-urlencoded"),
    );
    assert_eq!(
        parse(&duplicate, b"id=tr_a"),
        Err(FormError::UnsupportedMediaType)
    );
    let mut opaque = form();
    opaque.insert(
        CONTENT_TYPE,
        HeaderValue::from_bytes(&[255]).expect("opaque fixture"),
    );
    assert_eq!(
        parse(&opaque, b"id=tr_a"),
        Err(FormError::UnsupportedMediaType)
    );
    for encoding in ["", "identity", "gzip"] {
        let mut encoded = form();
        encoded.insert(
            CONTENT_ENCODING,
            HeaderValue::from_str(encoding).expect("fixture encoding"),
        );
        assert_eq!(
            parse(&encoded, b"id=tr_a"),
            Err(FormError::UnsupportedMediaType)
        );
    }
}
