use crate::{
    Kind, MAX_ORIGINAL_BYTES, MAX_RENDITION_BYTES, Operation, Prepared, Rejection, Rendition,
    Response, wire,
};

fn example() -> Prepared {
    Prepared {
        width: 2,
        height: 3,
        renditions: vec![Rendition {
            edge: 320,
            width: 2,
            height: 3,
            kind: Kind::Png,
            bytes: vec![1],
        }],
    }
}
#[test]
fn request_header_rejects_all_size_and_kind_boundaries() {
    let prepare = |kind, width, height| Operation::Prepare {
        kind,
        width,
        height,
    };
    let validate = |kind, width, height| Operation::Validate {
        kind,
        width,
        height,
    };
    for (operation, len) in [
        (prepare(Kind::Png, 0, 3), 1),
        (prepare(Kind::Png, 2, 0), 1),
        (prepare(Kind::Png, 16385, 3), 1),
        (prepare(Kind::Png, 2, 16385), 1),
        (prepare(Kind::Png, 2, 3), MAX_ORIGINAL_BYTES + 1),
        (validate(Kind::Png, 2, 3), MAX_RENDITION_BYTES + 1),
        (validate(Kind::Webp, 2, 3), 1),
    ] {
        assert!(
            wire::request_header(operation, len).is_err(),
            "{operation:?}"
        );
    }
    for (operation, len) in [
        (prepare(Kind::Webp, 16384, 16384), MAX_ORIGINAL_BYTES),
        (validate(Kind::Jpeg, 960, 960), MAX_RENDITION_BYTES),
    ] {
        let header = wire::request_header(operation, len).expect("valid boundary");
        assert_eq!(wire::parse_request_header(&header), Ok((operation, len)));
    }
}
#[test]
fn prepared_response_rejects_bad_dimensions_kind_and_payload() {
    let base = example();
    assert!(wire::encode_response(&Response::Prepared(base.clone())).is_ok());
    let mut cases = Vec::new();
    for (width, height) in [(0, 3), (2, 0), (16385, 3), (2, 16385)] {
        let mut p = base.clone();
        p.width = width;
        p.height = height;
        cases.push(p);
    }
    let mut p = base.clone();
    p.renditions.clear();
    cases.push(p);
    for (width, height) in [(0, 3), (2, 0), (3, 3), (2, 4), (1, 1)] {
        let mut p = base.clone();
        p.renditions[0].width = width;
        p.renditions[0].height = height;
        cases.push(p);
    }
    let mut p = base.clone();
    p.renditions[0].kind = Kind::Webp;
    cases.push(p);
    let mut p = base.clone();
    p.renditions[0].bytes.clear();
    cases.push(p);
    let mut p = base.clone();
    p.renditions[0].bytes = vec![0; MAX_RENDITION_BYTES + 1];
    cases.push(p);
    for case in cases {
        assert!(wire::encode_response(&Response::Prepared(case)).is_err());
    }
}
#[test]
fn closed_response_codes_and_payload_bounds() {
    let valid = wire::encode_response(&Response::Valid).expect("valid");
    assert_eq!(wire::parse_response(&valid), Ok(Response::Valid));
    for rejection in [
        Rejection::MetadataMismatch,
        Rejection::InvalidImage,
        Rejection::UnsupportedImage,
        Rejection::ResourceLimit,
        Rejection::InvalidStoredData,
        Rejection::Unavailable,
    ] {
        let response = Response::Rejected(rejection);
        assert_eq!(
            wire::parse_response(&wire::encode_response(&response).expect("encode")),
            Ok(response)
        );
    }
    assert!(wire::parse_response(&vec![0; wire::MAX_RESPONSE_BYTES + 1]).is_err());
    assert!(wire::parse_response(b"BAD!\x00\x00\x00\x00").is_err());
    let response = wire::encode_response(&Response::Prepared(example())).expect("prepared");
    for size in [
        0_u32,
        u32::try_from(MAX_RENDITION_BYTES + 1).expect("bounded"),
    ] {
        let mut corrupt = response.clone();
        corrupt[36..40].copy_from_slice(&size.to_be_bytes());
        assert!(wire::parse_response(&corrupt).is_err());
    }
    assert_eq!(wire::WireError.to_string(), "invalid image decoder frame");
}
