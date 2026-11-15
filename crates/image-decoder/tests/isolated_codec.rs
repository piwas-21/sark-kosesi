#![cfg(target_os = "linux")]

use commerce_image_codec::{Kind, Operation, Response, wire};
use image::{DynamicImage, ImageFormat};
use std::{
    io::{Cursor, Write},
    process::{Command, Stdio},
};

fn decode(operation: Operation, bytes: &[u8]) -> Response {
    let mut child = Command::new(env!("CARGO_BIN_EXE_commerce-image-decoder"))
        .arg("decode")
        .env_clear()
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("decoder child");
    let mut stdin = child.stdin.take().expect("stdin");
    stdin
        .write_all(&wire::request_header(operation, bytes.len()).expect("header"))
        .expect("header write");
    stdin.write_all(bytes).expect("input write");
    drop(stdin);
    let output = child.wait_with_output().expect("child result");
    assert!(
        output.status.success(),
        "isolated codec child failed: {:?}",
        output.status
    );
    wire::parse_response(&output.stdout).expect("isolated response")
}

#[test]
fn filtered_child_processes_valid_png_jpeg_and_webp_and_rejects_corruption() {
    for (kind, format) in [
        (Kind::Png, ImageFormat::Png),
        (Kind::Jpeg, ImageFormat::Jpeg),
        (Kind::Webp, ImageFormat::WebP),
    ] {
        let mut original = Vec::new();
        DynamicImage::new_rgb8(2, 3)
            .write_to(&mut Cursor::new(&mut original), format)
            .expect("image fixture");
        let response = decode(
            Operation::Prepare {
                kind,
                width: 2,
                height: 3,
            },
            &original,
        );
        let Response::Prepared(prepared) = response else {
            panic!("valid isolated image");
        };
        assert_eq!((prepared.width, prepared.height), (2, 3));
        assert_eq!(prepared.renditions.len(), 1);
        let rendition = &prepared.renditions[0];
        assert_eq!(
            decode(
                Operation::Validate {
                    kind: rendition.kind,
                    width: 2,
                    height: 3
                },
                &rendition.bytes
            ),
            Response::Valid
        );
    }
    let result = decode(
        Operation::Prepare {
            kind: Kind::Png,
            width: 2,
            height: 3,
        },
        b"damaged image",
    );
    assert!(matches!(result, Response::Rejected(_)));
}
