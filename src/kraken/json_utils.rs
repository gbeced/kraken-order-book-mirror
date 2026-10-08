// Faster, but error prone.
// pub fn is_channel_level3(msg: &str) -> bool {
//     const PREFIX: &str = r#"{"channel":"level3","type":""#;
//     msg.starts_with(PREFIX)
// }

// Top-level "channel" must be the plain string "level3".
// A byte scan stops at that key, so a depth-100 snapshot is not parsed twice.
// Nested "channel" (subscribe acks) does not match. Key order and whitespace do.
pub fn is_channel_level3(msg: &str) -> bool {
    let mut scanner = JsonScanner::new(msg.as_bytes());
    scanner.skip_ws();
    if !scanner.eat(b'{') {
        return false;
    }

    loop {
        scanner.skip_ws();
        if scanner.i >= scanner.bytes.len() || scanner.bytes[scanner.i] == b'}' {
            return false;
        }

        let Some(key_is_channel) = scanner.match_plain_string(b"channel") else {
            return false;
        };
        scanner.skip_ws();
        if !scanner.eat(b':') {
            return false;
        }
        scanner.skip_ws();

        if key_is_channel {
            return scanner.match_plain_string(b"level3") == Some(true);
        }
        if scanner.skip_value().is_none() {
            return false;
        }

        scanner.skip_ws();
        if scanner.i >= scanner.bytes.len() || scanner.bytes[scanner.i] != b',' {
            return false;
        }
        scanner.i += 1;
    }
}

struct JsonScanner<'a> {
    bytes: &'a [u8],
    i: usize,
}

impl<'a> JsonScanner<'a> {
    fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, i: 0 }
    }

    fn skip_ws(&mut self) {
        while self.i < self.bytes.len()
            && matches!(self.bytes[self.i], b' ' | b'\t' | b'\n' | b'\r')
        {
            self.i += 1;
        }
    }

    fn eat(&mut self, expected: u8) -> bool {
        if self.i < self.bytes.len() && self.bytes[self.i] == expected {
            self.i += 1;
            true
        } else {
            false
        }
    }

    /// Consume a JSON string at the cursor.
    /// When `plain_eq` is set, the result says whether the raw contents match it.
    /// A backslash means the string is not plain, so it does not match.
    fn scan_string(&mut self, plain_eq: Option<&[u8]>) -> Option<bool> {
        if self.i >= self.bytes.len() || self.bytes[self.i] != b'"' {
            return None;
        }
        self.i += 1;
        let start = self.i;
        let mut escaped = false;
        while self.i < self.bytes.len() {
            let c = self.bytes[self.i];
            if escaped {
                escaped = false;
                self.i += 1;
                continue;
            }
            match c {
                b'\\' => {
                    escaped = true;
                    self.i += 1;
                }
                b'"' => {
                    let equals = match plain_eq {
                        Some(expected) => {
                            let content = &self.bytes[start..self.i];
                            !content.contains(&b'\\') && content == expected
                        }
                        None => false,
                    };
                    self.i += 1;
                    return Some(equals);
                }
                _ => self.i += 1,
            }
        }
        None
    }

    fn match_plain_string(&mut self, expected: &[u8]) -> Option<bool> {
        self.scan_string(Some(expected))
    }

    fn skip_string(&mut self) -> Option<()> {
        self.scan_string(None).map(|_| ())
    }

    fn skip_value(&mut self) -> Option<()> {
        if self.i >= self.bytes.len() {
            return None;
        }
        match self.bytes[self.i] {
            b'"' => self.skip_string(),
            b'{' => self.skip_container(b'{', b'}'),
            b'[' => self.skip_container(b'[', b']'),
            b't' => self.skip_literal(b"true"),
            b'f' => self.skip_literal(b"false"),
            b'n' => self.skip_literal(b"null"),
            b'-' | b'0'..=b'9' => self.skip_number(),
            _ => None,
        }
    }

    fn skip_container(&mut self, open: u8, close: u8) -> Option<()> {
        if self.i >= self.bytes.len() || self.bytes[self.i] != open {
            return None;
        }
        self.i += 1;
        let mut depth = 1;
        let mut escaped = false;
        let mut in_string = false;
        while self.i < self.bytes.len() {
            let c = self.bytes[self.i];
            self.i += 1;
            if in_string {
                if escaped {
                    escaped = false;
                } else if c == b'\\' {
                    escaped = true;
                } else if c == b'"' {
                    in_string = false;
                }
                continue;
            }
            if c == b'"' {
                in_string = true;
            } else if c == open {
                depth += 1;
            } else if c == close {
                depth -= 1;
                if depth == 0 {
                    return Some(());
                }
            }
        }
        None
    }

    fn skip_literal(&mut self, literal: &[u8]) -> Option<()> {
        let end = self.i.checked_add(literal.len())?;
        if self.bytes.get(self.i..end) != Some(literal) {
            return None;
        }
        if self
            .bytes
            .get(end)
            .is_some_and(|c| c.is_ascii_alphanumeric())
        {
            return None;
        }
        self.i = end;
        Some(())
    }

    fn skip_number(&mut self) -> Option<()> {
        if self.bytes.get(self.i) == Some(&b'-') {
            self.i += 1;
        }
        if !self.bytes.get(self.i).is_some_and(|c| c.is_ascii_digit()) {
            return None;
        }
        while self
            .bytes
            .get(self.i)
            .is_some_and(|c| matches!(c, b'0'..=b'9' | b'.' | b'e' | b'E' | b'+' | b'-'))
        {
            self.i += 1;
        }
        Some(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    fn fixture(name: &str) -> String {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/data")
            .join(name);
        std::fs::read_to_string(path).unwrap()
    }

    #[test]
    fn accepts_level3_frames() {
        let samples = [
            r#"{"channel":"level3","type":"snapshot","data":[]}"#,
            r#"{"channel":"level3","type":"update","data":[]}"#,
            "{\n  \"channel\" : \"level3\"\n}",
            r#"{"type":"update","channel":"level3","data":[]}"#,
            r#"{"data":[{"note":"}"}],"channel":"level3"}"#,
            r#"{"note":"\"channel\":\"level3\"","channel":"level3"}"#,
            r#"{"result":{"channel":"heartbeat"},"channel":"level3"}"#,
            // Stop at the channel key. The rest of the frame is not scanned.
            r#"{"channel":"level3","data":[}"#,
        ];
        for sample in samples {
            assert!(is_channel_level3(sample), "{sample}");
        }
        for name in [
            "level3_snapshot.json",
            "level3_update_1.json",
            "level3_update_2.json",
        ] {
            assert!(is_channel_level3(&fixture(name)), "{name}");
        }
    }

    #[test]
    fn rejects_other_frames() {
        let samples = [
            "",
            "[]",
            "{",
            "{}",
            r#"{"channel":"#,
            r#"{"channel":"heartbeat"}"#,
            r#"{"channel":"status","type":"update","data":[]}"#,
            r#"{"channel":"level3 "}"#,
            r#"{"channel":"\u006cevel3"}"#,
            r#"{"data":[{"channel":"level3"}]}"#,
            r#"{"note":"\"channel\":\"level3\""}"#,
            r#"{"method":"subscribe","result":{"channel":"level3","symbol":"BTC/USD"},"success":true}"#,
            r#"{"method":"unsubscribe","result":{"channel":"level3","symbol":"BTC/USD"},"success":true}"#,
            r#"{"method":"subscribe","params":{"channel":"level3","symbol":["BTC/USD"]},"success":true}"#,
        ];
        for sample in samples {
            assert!(!is_channel_level3(sample), "{sample}");
        }
    }
}
