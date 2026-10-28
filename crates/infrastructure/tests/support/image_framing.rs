use super::*;

fn chunk(kind: &[u8; 4], payload: &[u8]) -> Vec<u8> {
    let mut result = kind.to_vec();
    result.extend(
        u32::try_from(payload.len())
            .expect("small fixture")
            .to_le_bytes(),
    );
    result.extend(payload);
    if payload.len() % 2 == 1 {
        result.push(0);
    }
    result
}
fn container(chunks: &[Vec<u8>]) -> Vec<u8> {
    let mut bytes = b"RIFF\0\0\0\0WEBP".to_vec();
    for chunk in chunks {
        bytes.extend(chunk);
    }
    let size = u32::try_from(bytes.len() - 8).expect("small fixture");
    bytes[4..8].copy_from_slice(&size.to_le_bytes());
    bytes
}
const LOSSLESS: &[u8] = &[0x2f, 1, 0x80, 0, 0]; // Native width2, height3.
const EXTENDED: &[u8] = &[0, 0, 0, 0, 1, 0, 0, 2, 0, 0];
const LOSSY: &[u8] = &[0, 0, 0, 0x9d, 1, 0x2a, 2, 0, 3, 0];

#[test]
fn declared_ranges_and_padding_fit_the_actual_container_without_allocating_payloads() {
    assert!(webp(&container(&[chunk(b"VP8L", LOSSLESS)])).is_ok());
    assert!(webp(&container(&[chunk(b"VP8 ", LOSSY)])).is_ok());
    assert!(
        webp(&container(&[
            chunk(b"VP8X", EXTENDED),
            chunk(b"VP8L", LOSSLESS),
            chunk(b"EXIF", b"bounded metadata"),
            chunk(b"TEST", b"odd")
        ]))
        .is_ok()
    );
    for invalid in [
        vec![],
        b"RIFF1234WEB".to_vec(),
        b"FAIL\x04\0\0\0WEBP".to_vec(),
        b"RIFF\x04\0\0\0FAIL".to_vec(),
    ] {
        assert!(matches!(webp(&invalid), Err(ImageError::InvalidImage)));
    }
    let mut wrong_size = container(&[chunk(b"VP8L", LOSSLESS)]);
    wrong_size[4..8].copy_from_slice(&u32::MAX.to_le_bytes());
    assert!(matches!(webp(&wrong_size), Err(ImageError::ResourceLimit)));
    for size in [65536, 135266304, u32::MAX] {
        let mut exif = chunk(b"EXIF", &[0; 16]);
        exif[4..8].copy_from_slice(&size.to_le_bytes());
        // Outer RIFF length is honest; only the nested metadata range is false.
        let bytes = container(&[chunk(b"VP8X", EXTENDED), chunk(b"VP8L", LOSSLESS), exif]);
        assert!(bytes.len() < 100);
        assert!(matches!(webp(&bytes), Err(ImageError::ResourceLimit)));
    }
    let truncated = container(&[chunk(b"VP8L", LOSSLESS), b"EXIF\x01".to_vec()]);
    assert!(matches!(webp(&truncated), Err(ImageError::InvalidImage)));
    let mut missing_pad = chunk(b"TEST", b"odd");
    missing_pad.pop();
    assert!(matches!(
        webp(&container(&[chunk(b"VP8L", LOSSLESS), missing_pad])),
        Err(ImageError::ResourceLimit)
    ));
}

#[test]
fn static_canvas_and_native_bitstream_agree_before_any_decoder_allocation() {
    for chunks in [
        vec![],
        vec![chunk(b"EXIF", b"unused")],
        vec![chunk(b"VP8L", LOSSLESS), chunk(b"VP8L", LOSSLESS)],
        vec![
            chunk(b"VP8X", EXTENDED),
            chunk(b"VP8X", EXTENDED),
            chunk(b"VP8L", LOSSLESS),
        ],
        vec![
            chunk(b"TEST", b"first"),
            chunk(b"VP8X", EXTENDED),
            chunk(b"VP8L", LOSSLESS),
        ],
        vec![chunk(b"VP8X", &EXTENDED[..9]), chunk(b"VP8L", LOSSLESS)],
        vec![chunk(b"VP8L", &LOSSLESS[..4])],
        vec![chunk(b"VP8L", &[0, 1, 0x80, 0, 0])],
        vec![chunk(b"VP8 ", &LOSSY[..9])],
        vec![chunk(b"VP8 ", &[1, 0, 0, 0x9d, 1, 0x2a, 2, 0, 3, 0])],
        vec![chunk(b"VP8 ", &[0, 0, 0, 0, 1, 0x2a, 2, 0, 3, 0])],
        vec![chunk(b"VP8X", &[0; 10]), chunk(b"VP8L", LOSSLESS)],
    ] {
        assert!(matches!(
            webp(&container(&chunks)),
            Err(ImageError::InvalidImage)
        ));
    }
    let mut animated = EXTENDED.to_vec();
    animated[0] = 2;
    assert!(matches!(
        webp(&container(&[chunk(b"VP8X", &animated)])),
        Err(ImageError::UnsupportedImage)
    ));
    let mut nested = vec![0; 16];
    nested.extend(b"EXIF");
    nested.extend(u32::MAX.to_le_bytes());
    nested.extend([0; 16]);
    for animation_chunk in [chunk(b"ANIM", &[0; 6]), chunk(b"ANMF", &nested)] {
        assert!(matches!(
            webp(&container(&[
                chunk(b"VP8X", EXTENDED),
                chunk(b"VP8L", LOSSLESS),
                animation_chunk
            ])),
            Err(ImageError::UnsupportedImage)
        ));
    }
}
