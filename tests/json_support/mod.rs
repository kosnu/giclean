// Test-only JSON reader: validates the complete document without external tools or crates.
#[derive(Debug, PartialEq)]
pub enum Json {
    Null,
    Bool(bool),
    Number(u64),
    String(String),
    Array(Vec<Json>),
    Object(std::collections::BTreeMap<String, Json>),
}

impl Json {
    pub fn get(&self, key: &str) -> &Self {
        match self {
            Self::Object(fields) => fields
                .get(key)
                .unwrap_or_else(|| panic!("missing {key}: {self:?}")),
            _ => panic!("not an object: {self:?}"),
        }
    }
    pub fn array(&self) -> &[Self] {
        match self {
            Self::Array(values) => values,
            _ => panic!("not an array: {self:?}"),
        }
    }
    pub fn string(&self) -> &str {
        match self {
            Self::String(value) => value,
            _ => panic!("not a string: {self:?}"),
        }
    }
}

pub fn parse(bytes: &[u8]) -> Json {
    struct Parser<'a> {
        input: &'a [u8],
        pos: usize,
    }
    impl Parser<'_> {
        fn whitespace(&mut self) {
            while self
                .input
                .get(self.pos)
                .is_some_and(|b| b" \t\n\r".contains(b))
            {
                self.pos += 1;
            }
        }
        fn take(&mut self, byte: u8) {
            self.whitespace();
            assert_eq!(
                self.input.get(self.pos),
                Some(&byte),
                "invalid JSON at {}",
                self.pos
            );
            self.pos += 1;
        }
        fn string(&mut self) -> String {
            self.take(b'"');
            let mut result = Vec::new();
            loop {
                let byte = self.input[self.pos];
                self.pos += 1;
                match byte {
                    b'"' => return String::from_utf8(result).unwrap(),
                    b'\\' => {
                        let escaped = self.input[self.pos];
                        self.pos += 1;
                        match escaped {
                            b'"' | b'\\' | b'/' => result.push(escaped),
                            b'n' => result.push(b'\n'),
                            b'r' => result.push(b'\r'),
                            b't' => result.push(b'\t'),
                            b'b' => result.push(8),
                            b'f' => result.push(12),
                            b'u' => {
                                let hex = std::str::from_utf8(&self.input[self.pos..self.pos + 4])
                                    .unwrap();
                                let value = u32::from_str_radix(hex, 16).unwrap();
                                self.pos += 4;
                                let ch = char::from_u32(value).expect("non-scalar escape");
                                let mut buffer = [0; 4];
                                result.extend_from_slice(ch.encode_utf8(&mut buffer).as_bytes());
                            }
                            _ => panic!("invalid escape"),
                        }
                    }
                    0..=31 => panic!("unescaped control byte"),
                    _ => result.push(byte),
                }
            }
        }
        fn value(&mut self) -> Json {
            self.whitespace();
            match self.input[self.pos] {
                b'"' => Json::String(self.string()),
                b'{' => {
                    self.take(b'{');
                    let mut fields = std::collections::BTreeMap::new();
                    self.whitespace();
                    if self.input[self.pos] != b'}' {
                        loop {
                            let key = self.string();
                            self.take(b':');
                            assert!(fields.insert(key, self.value()).is_none(), "duplicate key");
                            self.whitespace();
                            if self.input[self.pos] == b'}' {
                                break;
                            }
                            self.take(b',');
                        }
                    }
                    self.take(b'}');
                    Json::Object(fields)
                }
                b'[' => {
                    self.take(b'[');
                    let mut values = Vec::new();
                    self.whitespace();
                    if self.input[self.pos] != b']' {
                        loop {
                            values.push(self.value());
                            self.whitespace();
                            if self.input[self.pos] == b']' {
                                break;
                            }
                            self.take(b',');
                        }
                    }
                    self.take(b']');
                    Json::Array(values)
                }
                b'0'..=b'9' => {
                    let start = self.pos;
                    while self.input.get(self.pos).is_some_and(u8::is_ascii_digit) {
                        self.pos += 1;
                    }
                    let number = std::str::from_utf8(&self.input[start..self.pos]).unwrap();
                    assert!(number == "0" || !number.starts_with('0'));
                    Json::Number(number.parse().unwrap())
                }
                _ => {
                    for (literal, value) in [
                        ("true", Json::Bool(true)),
                        ("false", Json::Bool(false)),
                        ("null", Json::Null),
                    ] {
                        if self.input[self.pos..].starts_with(literal.as_bytes()) {
                            self.pos += literal.len();
                            return value;
                        }
                    }
                    panic!("invalid JSON token at {}", self.pos)
                }
            }
        }
    }
    let mut parser = Parser {
        input: bytes,
        pos: 0,
    };
    let result = parser.value();
    parser.whitespace();
    assert_eq!(parser.pos, bytes.len(), "trailing non-JSON output");
    result
}
