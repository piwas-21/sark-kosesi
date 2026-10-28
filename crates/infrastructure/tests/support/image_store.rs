use super::*;
#[test]
fn hostile_stored_geometry_hashes_formats_and_lengths_cannot_form_receipts() {
    let row = (320, 2, 3, "png".into(), vec![1; 32], 128);
    assert_eq!(
        assemble("asset", 2, 3, vec![row.clone()], false)
            .expect("oracle")
            .renditions
            .len(),
        1
    );
    for case in 0..15 {
        let mut width = 2;
        let mut height = 3;
        let mut rows = vec![row.clone()];
        match case {
            0 => width = 16385,
            1 => height = 16385,
            2 => {
                width = 8192;
                height = 8193;
            }
            3 => width = 0,
            4 => height = 0,
            5 => rows.clear(),
            6 => rows[0].0 = 640,
            7 => rows[0].1 = 0,
            8 => rows[0].2 = 0,
            9 => rows[0].2 = 2,
            10 => rows[0].3 = "webp".into(),
            11 => rows[0].4 = vec![1; 31],
            12 => rows[0].5 = 0,
            13 => rows[0].5 = 256001,
            14 => rows.push(row.clone()),
            _ => unreachable!(),
        }
        assert!(
            matches!(
                assemble("asset", width, height, rows, false),
                Err(ImageError::InvalidStoredData)
            ),
            "case {case}"
        );
    }
}
