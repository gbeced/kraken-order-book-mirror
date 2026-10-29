pub struct JsonScanner<'a> {
    bytes: &'a [u8],
    i: usize,
}

impl<'a> JsonScanner<'a> {
    pub fn new(bytes: &'a [u8]) -> Self {
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
    /// `None` means the cursor is not a finished JSON string.
    /// The bool is true when the raw contents contain no backslash.
    /// The `&str` is those raw contents, a subslice of the input.
    fn scan_string(&mut self) -> Option<(bool, &'a str)> {
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
                    let end = self.i;
                    self.i += 1;
                    let content = &self.bytes[start..end];
                    let plain = !content.contains(&b'\\');
                    let text = std::str::from_utf8(content).ok()?;
                    return Some((plain, text));
                }
                _ => self.i += 1,
            }
        }
        None
    }

    fn match_plain_string(&mut self, expected: &[u8]) -> Option<bool> {
        self.scan_string()
            .map(|(plain, content)| plain && content.as_bytes() == expected)
    }

    fn skip_string(&mut self) -> Option<()> {
        self.scan_string().map(|_| ())
    }

    fn plain_string(&mut self) -> Option<&'a str> {
        self.scan_string()
            .and_then(|(plain, content)| plain.then_some(content))
    }

    pub fn top_level_plain_string(&mut self, key: &[u8]) -> Option<&'a str> {
        self.skip_ws();
        if !self.eat(b'{') {
            return None;
        }

        loop {
            self.skip_ws();
            if self.i >= self.bytes.len() || self.bytes[self.i] == b'}' {
                return None;
            }

            let key_matches = self.match_plain_string(key)?;
            self.skip_ws();
            if !self.eat(b':') {
                return None;
            }
            self.skip_ws();

            if key_matches {
                return self.plain_string();
            }
            self.skip_value()?;

            self.skip_ws();
            if self.i >= self.bytes.len() || self.bytes[self.i] != b',' {
                return None;
            }
            self.i += 1;
        }
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
