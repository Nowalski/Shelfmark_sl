use std::collections::BTreeMap;

#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    Undefined,
    Boolean(bool),
    Integer(i32),
    Real(f64),
    Uuid(String),
    String(String),
    Date(f64),
    Uri(String),
    Binary(Vec<u8>),
    Array(Vec<Value>),
    Map(BTreeMap<String, Value>),
}

const MAX_CONTAINER_ITEMS: usize = 1_000_000;
const MAX_STRING_BYTES: usize = 4 * 1024 * 1024;
const MAX_DEPTH: usize = 96;
pub const CANCELLED_MESSAGE: &str = "Import canceled. The existing avatar catalogue is unchanged.";

#[cfg(test)]
pub fn parse(bytes: &[u8]) -> Result<(Value, usize), String> {
    parse_with_progress(bytes, &|| false, &mut |_| {})
}

pub fn parse_with_progress(
    bytes: &[u8],
    is_cancelled: &dyn Fn() -> bool,
    on_progress: &mut dyn FnMut(usize),
) -> Result<(Value, usize), String> {
    if is_cancelled() {
        return Err(CANCELLED_MESSAGE.into());
    }
    let mut parser = Parser {
        bytes,
        offset: 0,
        values_seen: 0,
        is_cancelled,
        on_progress,
    };
    let value = parser.value(0)?;
    if is_cancelled() {
        return Err(CANCELLED_MESSAGE.into());
    }
    Ok((value, parser.offset))
}

struct Parser<'a> {
    bytes: &'a [u8],
    offset: usize,
    values_seen: usize,
    is_cancelled: &'a dyn Fn() -> bool,
    on_progress: &'a mut dyn FnMut(usize),
}

impl Parser<'_> {
    fn value(&mut self, depth: usize) -> Result<Value, String> {
        self.values_seen += 1;
        if self.values_seen.is_multiple_of(4096) {
            if (self.is_cancelled)() {
                return Err(CANCELLED_MESSAGE.into());
            }
            (self.on_progress)(self.offset);
        }
        if depth >= MAX_DEPTH {
            return Err("LLSD nesting limit exceeded".into());
        }
        let token = self.byte()?;
        match token {
            b'!' => Ok(Value::Undefined),
            b'0' => Ok(Value::Boolean(false)),
            b'1' => Ok(Value::Boolean(true)),
            b'i' => Ok(Value::Integer(i32::from_be_bytes(
                self.take(4)?.try_into().unwrap(),
            ))),
            b'r' => Ok(Value::Real(f64::from_bits(u64::from_be_bytes(
                self.take(8)?.try_into().unwrap(),
            )))),
            b'd' => Ok(Value::Date(f64::from_bits(u64::from_be_bytes(
                self.take(8)?.try_into().unwrap(),
            )))),
            b'u' => Ok(Value::Uuid(format_uuid(self.take(16)?))),
            b's' => Ok(Value::String(self.string()?)),
            b'l' => Ok(Value::Uri(self.string()?)),
            b'b' => {
                let size = self.length()?;
                Ok(Value::Binary(self.take(size)?.to_vec()))
            }
            b'[' => {
                let size = self.length()?;
                let mut values = Vec::with_capacity(size.min(100_000));
                for _ in 0..size {
                    values.push(self.value(depth + 1)?);
                }
                self.expect(b']')?;
                Ok(Value::Array(values))
            }
            b'{' => {
                let size = self.length()?;
                let mut values = BTreeMap::new();
                for _ in 0..size {
                    let key_token = self.byte()?;
                    let key = match key_token {
                        b'k' | b's' | b'l' => self.string()?,
                        b'\'' | b'"' => self.quoted_string(key_token)?,
                        _ => return Err("Invalid LLSD map key".into()),
                    };
                    let value = self.value(depth + 1)?;
                    values.insert(key, value);
                }
                self.expect(b'}')?;
                Ok(Value::Map(values))
            }
            _ => Err(format!("Unsupported LLSD binary token 0x{token:02x}")),
        }
    }

    fn byte(&mut self) -> Result<u8, String> {
        let value = *self
            .bytes
            .get(self.offset)
            .ok_or("Unexpected end of LLSD data")?;
        self.offset += 1;
        Ok(value)
    }

    fn take(&mut self, size: usize) -> Result<&[u8], String> {
        let end = self.offset.checked_add(size).ok_or("LLSD size overflow")?;
        let bytes = self
            .bytes
            .get(self.offset..end)
            .ok_or("Truncated LLSD data")?;
        self.offset = end;
        Ok(bytes)
    }

    fn length(&mut self) -> Result<usize, String> {
        let raw = u32::from_be_bytes(self.take(4)?.try_into().unwrap()) as usize;
        if raw > MAX_CONTAINER_ITEMS {
            return Err("LLSD container exceeds the supported item limit".into());
        }
        Ok(raw)
    }

    fn string(&mut self) -> Result<String, String> {
        let size = u32::from_be_bytes(self.take(4)?.try_into().unwrap()) as usize;
        if size > MAX_STRING_BYTES {
            return Err("LLSD string exceeds the supported size limit".into());
        }
        String::from_utf8(self.take(size)?.to_vec())
            .map_err(|_| "Invalid UTF-8 in LLSD string".into())
    }

    fn quoted_string(&mut self, quote: u8) -> Result<String, String> {
        let mut out = Vec::new();
        loop {
            let byte = self.byte()?;
            if byte == quote {
                break;
            }
            if byte == b'\\' {
                let escaped = self.byte()?;
                out.push(match escaped {
                    b'n' => b'\n',
                    b'r' => b'\r',
                    b't' => b'\t',
                    b'\\' => b'\\',
                    b'\'' => b'\'',
                    b'"' => b'"',
                    other => other,
                });
            } else {
                out.push(byte);
            }
            if out.len() > MAX_STRING_BYTES {
                return Err("LLSD map key exceeds the supported size limit".into());
            }
        }
        String::from_utf8(out).map_err(|_| "Invalid UTF-8 in LLSD map key".into())
    }

    fn expect(&mut self, expected: u8) -> Result<(), String> {
        if self.byte()? == expected {
            Ok(())
        } else {
            Err("Malformed LLSD container terminator".into())
        }
    }
}

fn format_uuid(bytes: &[u8]) -> String {
    let s = hex(bytes);
    format!(
        "{}-{}-{}-{}-{}",
        &s[0..8],
        &s[8..12],
        &s[12..16],
        &s[16..20],
        &s[20..32]
    )
}

fn hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        out.push(HEX[(b >> 4) as usize] as char);
        out.push(HEX[(b & 0x0f) as usize] as char);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_llsd_map_array_and_uuid() {
        let data = [
            b'{', 0, 0, 0, 1, b'k', 0, 0, 0, 1, b'x', b'[', 0, 0, 0, 2, b'i', 0, 0, 0, 7, b'u', 0,
            1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, b']', b'}',
        ];
        let (value, consumed) = parse(&data).unwrap();
        let Value::Map(map) = value else {
            panic!("expected map")
        };
        assert_eq!(consumed, data.len());
        assert_eq!(
            map["x"],
            Value::Array(vec![
                Value::Integer(7),
                Value::Uuid("00010203-0405-0607-0809-0a0b0c0d0e0f".into()),
            ])
        );
    }

    #[test]
    fn rejects_truncated_values_and_excessive_container_sizes() {
        assert!(parse(b"i\0\0").is_err());
        assert!(parse(b"[\xff\xff\xff\xff]").is_err());
    }
}
