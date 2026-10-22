//! Standalone LUFP cursor for the challenge's JSON documents.
//!
//! Borrow ordinary strings, decode escapes only when needed, and validate the
//! whole document while retaining only the fields used by the application.
//! Kept in the submission so builds need no external workspace or table data.
use std::borrow::Cow;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ParseError;
type Result<T> = std::result::Result<T, ParseError>;

pub struct Claims<'a> {
    pub sub: Option<Cow<'a, str>>,
    pub username: Option<Cow<'a, str>>,
    pub exp: Option<f64>,
    pub nbf: Option<f64>,
}

struct JSONCursor<'a> {
    bytes: &'a [u8],
    index: usize,
    finite_numbers: bool,
}

impl<'a> JSONCursor<'a> {
    fn new(bytes: &'a [u8], finite_numbers: bool) -> Self {
        Self {
            bytes,
            index: 0,
            finite_numbers,
        }
    }

    #[inline]
    fn skip_ws(&mut self) {
        while matches!(
            self.bytes.get(self.index),
            Some(b' ' | b'\t' | b'\n' | b'\r')
        ) {
            self.index += 1;
        }
    }

    #[inline]
    fn consume(&mut self, byte: u8) -> bool {
        self.skip_ws();
        if self.bytes.get(self.index) == Some(&byte) {
            self.index += 1;
            true
        } else {
            false
        }
    }

    #[inline]
    fn expect(&mut self, byte: u8) -> Result<()> {
        self.consume(byte).then_some(()).ok_or(ParseError)
    }

    fn finish(&mut self) -> Result<()> {
        self.skip_ws();
        (self.index == self.bytes.len())
            .then_some(())
            .ok_or(ParseError)
    }

    #[inline]
    fn string(&mut self) -> Result<Cow<'a, str>> {
        self.expect(b'"')?;
        let start = self.index;
        let mut at = start;
        #[cfg(target_arch = "x86_64")]
        while at + 16 <= self.bytes.len() {
            // The bounds check covers the entire unaligned load. High bytes
            // leave this ASCII path and receive full UTF-8 validation below.
            let mask = unsafe {
                use std::arch::x86_64::*;
                let v = _mm_loadu_si128(self.bytes.as_ptr().add(at).cast());
                let special = _mm_or_si128(
                    _mm_or_si128(
                        _mm_cmpeq_epi8(v, _mm_set1_epi8(34)),
                        _mm_cmpeq_epi8(v, _mm_set1_epi8(92)),
                    ),
                    _mm_cmpeq_epi8(_mm_min_epu8(v, _mm_set1_epi8(31)), v),
                );
                (_mm_movemask_epi8(special) | _mm_movemask_epi8(v)) as u32
            };
            if mask != 0 {
                at += mask.trailing_zeros() as usize;
                break;
            }
            at += 16;
        }
        while self
            .bytes
            .get(at)
            .is_some_and(|&b| (32..128).contains(&b) && b != b'"' && b != b'\\')
        {
            at += 1;
        }
        match self.bytes.get(at) {
            Some(b'"') => {
                self.index = at + 1;
                // Both scanners proved that every byte in this span is ASCII.
                return Ok(Cow::Borrowed(unsafe {
                    std::str::from_utf8_unchecked(&self.bytes[start..at])
                }));
            }
            Some(b'\\') => {
                self.index = at;
                return self.escaped_string(start);
            }
            Some(128..=255) => self.utf8_string(start, at),
            _ => Err(ParseError),
        }
    }

    #[cold]
    fn utf8_string(&mut self, start: usize, at: usize) -> Result<Cow<'a, str>> {
        let n = memchr::memchr2(b'"', b'\\', &self.bytes[at..]).ok_or(ParseError)?;
        let end = at + n;
        let raw = &self.bytes[start..end];
        if raw.iter().any(|&b| b < 0x20) {
            return Err(ParseError);
        }
        if self.bytes[end] == b'"' {
            self.index = end + 1;
            return std::str::from_utf8(raw)
                .map(Cow::Borrowed)
                .map_err(|_| ParseError);
        }
        self.index = end;
        self.escaped_string(start)
    }

    #[cold]
    fn escaped_string(&mut self, start: usize) -> Result<Cow<'a, str>> {
        let mut output = self.bytes[start..self.index].to_vec();
        loop {
            let byte = *self.bytes.get(self.index).ok_or(ParseError)?;
            self.index += 1;
            match byte {
                b'"' => {
                    return String::from_utf8(output)
                        .map(Cow::Owned)
                        .map_err(|_| ParseError);
                }
                b'\\' => {
                    let escape = *self.bytes.get(self.index).ok_or(ParseError)?;
                    self.index += 1;
                    match escape {
                        b'"' | b'\\' | b'/' => output.push(escape),
                        b'b' => output.push(8),
                        b'f' => output.push(12),
                        b'n' => output.push(b'\n'),
                        b'r' => output.push(b'\r'),
                        b't' => output.push(b'\t'),
                        b'u' => {
                            let mut code = self.hex4()?;
                            if (0xd800..=0xdbff).contains(&code) {
                                if self.bytes.get(self.index..self.index + 2) != Some(b"\\u") {
                                    return Err(ParseError);
                                }
                                self.index += 2;
                                let low = self.hex4()?;
                                if !(0xdc00..=0xdfff).contains(&low) {
                                    return Err(ParseError);
                                }
                                code = 0x10000 + ((code - 0xd800) << 10) + low - 0xdc00;
                            }
                            let ch = char::from_u32(code).ok_or(ParseError)?;
                            output.extend_from_slice(ch.encode_utf8(&mut [0; 4]).as_bytes());
                        }
                        _ => return Err(ParseError),
                    }
                }
                0..=31 => return Err(ParseError),
                _ => output.push(byte),
            }
        }
    }

    fn hex4(&mut self) -> Result<u32> {
        let raw = self
            .bytes
            .get(self.index..self.index + 4)
            .ok_or(ParseError)?;
        let mut code = 0;
        for &b in raw {
            code = (code << 4)
                | match b {
                    b'0'..=b'9' => u32::from(b - b'0'),
                    b'a'..=b'f' => u32::from(b - b'a' + 10),
                    b'A'..=b'F' => u32::from(b - b'A' + 10),
                    _ => return Err(ParseError),
                };
        }
        self.index += 4;
        Ok(code)
    }

    fn literal(&mut self, text: &[u8]) -> Result<()> {
        self.skip_ws();
        if self.bytes.get(self.index..self.index + text.len()) != Some(text) {
            return Err(ParseError);
        }
        self.index += text.len();
        Ok(())
    }

    fn number_bytes(&mut self) -> Result<&'a [u8]> {
        self.skip_ws();
        let start = self.index;
        if self.bytes.get(self.index) == Some(&b'-') {
            self.index += 1;
        }
        match self.bytes.get(self.index) {
            Some(b'0') => self.index += 1,
            Some(b'1'..=b'9') => {
                self.index += 1;
                while self.bytes.get(self.index).is_some_and(u8::is_ascii_digit) {
                    self.index += 1;
                }
            }
            _ => return Err(ParseError),
        }
        if self.bytes.get(self.index) == Some(&b'.') {
            self.index += 1;
            self.digits()?;
        }
        if matches!(self.bytes.get(self.index), Some(b'e' | b'E')) {
            self.index += 1;
            if matches!(self.bytes.get(self.index), Some(b'+' | b'-')) {
                self.index += 1;
            }
            self.digits()?;
        }
        Ok(&self.bytes[start..self.index])
    }

    fn digits(&mut self) -> Result<()> {
        let start = self.index;
        while self.bytes.get(self.index).is_some_and(u8::is_ascii_digit) {
            self.index += 1;
        }
        (start != self.index).then_some(()).ok_or(ParseError)
    }

    fn number(&mut self) -> Result<f64> {
        let raw = self.number_bytes()?;
        // number_bytes accepts ASCII digits and punctuation only.
        let value: f64 = std::str::from_utf8(raw)
            .map_err(|_| ParseError)?
            .parse()
            .map_err(|_| ParseError)?;
        value.is_finite().then_some(value).ok_or(ParseError)
    }

    fn optional_number(&mut self) -> Result<Option<f64>> {
        self.skip_ws();
        if self.bytes.get(self.index) == Some(&b'n') {
            self.literal(b"null")?;
            Ok(None)
        } else {
            self.number().map(Some)
        }
    }

    fn text_or_skip(&mut self) -> Result<Option<Cow<'a, str>>> {
        self.skip_ws();
        if self.bytes.get(self.index) == Some(&b'"') {
            self.string().map(Some)
        } else {
            self.skip_value(1)?;
            Ok(None)
        }
    }

    #[inline]
    fn object(
        &mut self,
        depth: usize,
        mut field: impl FnMut(&mut Self, Cow<'a, str>) -> Result<()>,
    ) -> Result<()> {
        if depth >= 127 {
            return Err(ParseError);
        }
        self.expect(b'{')?;
        if self.consume(b'}') {
            return Ok(());
        }
        loop {
            let key = self.string()?;
            self.expect(b':')?;
            field(self, key)?;
            if self.consume(b'}') {
                return Ok(());
            }
            self.expect(b',')?;
        }
    }

    fn skip_value(&mut self, depth: usize) -> Result<()> {
        self.skip_ws();
        match self.bytes.get(self.index) {
            Some(b'"') => {
                self.string()?;
            }
            Some(b'{') => self.object(depth, |cursor, _| cursor.skip_value(depth + 1))?,
            Some(b'[') => {
                if depth >= 127 {
                    return Err(ParseError);
                }
                self.index += 1;
                if self.consume(b']') {
                    return Ok(());
                }
                loop {
                    self.skip_value(depth + 1)?;
                    if self.consume(b']') {
                        break;
                    }
                    self.expect(b',')?;
                }
            }
            Some(b't') => self.literal(b"true")?,
            Some(b'f') => self.literal(b"false")?,
            Some(b'n') => self.literal(b"null")?,
            Some(b'-' | b'0'..=b'9') => {
                if self.finite_numbers {
                    self.number()?;
                } else {
                    self.number_bytes()?;
                }
            }
            _ => return Err(ParseError),
        }
        Ok(())
    }
}

pub fn header(bytes: &[u8]) -> Result<()> {
    let mut cursor = JSONCursor::new(bytes, false);
    let mut alg = false;
    cursor.object(0, |cursor, key| {
        if key == "alg" {
            if alg || cursor.string()? != "HS256" {
                return Err(ParseError);
            }
            alg = true;
        } else {
            cursor.skip_value(1)?;
        }
        Ok(())
    })?;
    cursor.finish()?;
    alg.then_some(()).ok_or(ParseError)
}

pub fn claims(bytes: &[u8]) -> Result<Claims<'_>> {
    let mut cursor = JSONCursor::new(bytes, false);
    let mut claims = Claims {
        sub: None,
        username: None,
        exp: None,
        nbf: None,
    };
    let mut seen = 0u8;
    cursor.object(0, |cursor, key| {
        let bit = match key.as_ref() {
            "sub" => 1,
            "username" => 2,
            "exp" => 4,
            "nbf" => 8,
            _ => 0,
        };
        if seen & bit != 0 {
            return Err(ParseError);
        }
        seen |= bit;
        match bit {
            1 => claims.sub = cursor.text_or_skip()?,
            2 => claims.username = cursor.text_or_skip()?,
            4 => claims.exp = cursor.optional_number()?,
            8 => claims.nbf = cursor.optional_number()?,
            _ => cursor.skip_value(1)?,
        }
        Ok(())
    })?;
    cursor.finish()?;
    Ok(claims)
}

pub fn post_body(bytes: &[u8]) -> Result<Option<Cow<'_, str>>> {
    let mut cursor = JSONCursor::new(bytes, true);
    cursor.skip_ws();
    let mut body = None;
    if cursor.bytes.get(cursor.index) == Some(&b'{') {
        cursor.object(0, |cursor, key| {
            if key == "body" {
                body = cursor.text_or_skip()?;
            } else {
                cursor.skip_value(1)?;
            }
            Ok(())
        })?;
    } else {
        cursor.skip_value(0)?;
    }
    cursor.finish()?;
    Ok(body)
}

#[cfg(test)]
#[path = "../tests/support/lufp.rs"]
mod tests;
