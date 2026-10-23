#[cfg(test)]
mod tests {
    use rust_order_book_mirror::level3::mirror;
    use std::fs::File;
    use std::io::{BufRead, BufReader};
    use std::path::{Path, PathBuf};

    const PRICE_SCALE: u8 = 1;
    const QTY_SCALE: u8 = 8;

    fn test_file_path(fname: &str) -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/data")
            .join(fname)
    }

    fn read_test_file(fname: &str) -> String {
        let path = test_file_path(fname);
        std::fs::read_to_string(path).unwrap()
    }

    fn open_test_file(fname: &str) -> BufReader<File> {
        let path = test_file_path(fname);
        let file = File::open(path).unwrap();
        BufReader::new(file)
    }

    #[test]
    fn mirror_processing_messages() {
        let mut mirror = mirror::OrderBookMirror::new(10, PRICE_SCALE, QTY_SCALE);
        let file_names = [
            "level3_snapshot.json",
            "level3_update_1.json",
            "level3_update_2.json",
        ];

        for file_name in file_names {
            mirror.on_message(&read_test_file(file_name)).unwrap();
        }
    }

    #[test]
    fn mirror_processing_fails_with_snapshot_expected() {
        let mut mirror = mirror::OrderBookMirror::new(10, PRICE_SCALE, QTY_SCALE);

        assert_eq!(
            mirror
                .on_message(&read_test_file("level3_update_1.json"))
                .expect_err("Error expected")
                .to_string(),
            "Expected snapshot but received update"
        );
    }

    #[test]
    fn mirror_processing_fails_with_update_expected() {
        let mut mirror = mirror::OrderBookMirror::new(10, PRICE_SCALE, QTY_SCALE);

        mirror
            .on_message(&read_test_file("level3_snapshot.json"))
            .unwrap();
        assert_eq!(
            mirror
                .on_message(&read_test_file("level3_snapshot.json"))
                .expect_err("Error expected")
                .to_string(),
            "Expected update but received snapshot"
        );
    }

    #[test]
    fn mirror_checksum_mismatch_after_update() {
        let mut mirror = mirror::OrderBookMirror::new(10, PRICE_SCALE, QTY_SCALE);
        let mut reader = open_test_file("btc_usd_stream.log");
        let mut line = String::new();

        while reader.read_line(&mut line).unwrap() > 0 {
            if line.len() > 0 {
                mirror.on_message(&line).unwrap();
            }
            line.clear();
        }

        // Check that we're actually on the last message
        let obook = mirror.get_order_book().expect("No order book");
        assert_eq!(
            obook.timestamp.to_string(),
            "2026-10-07 16:31:44.979328334 UTC"
        );
    }
}
