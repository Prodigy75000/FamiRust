// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Prodigy75000

//! Integer expression evaluation for the assembler.
//!
//! Recursive descent over a character slice. Values are `i64` throughout and
//! only narrowed when a byte or word is finally emitted, so intermediate
//! arithmetic (`SPRITE_BASE + 4*3 - 1`) cannot silently wrap.

use std::collections::HashMap;

#[derive(Debug)]
pub enum EvalError {
    /// A symbol that is not defined *yet*. On the sizing pass this is expected
    /// and means "assume the widest encoding"; on the emit pass it is fatal.
    Unknown(String),
    Syntax(String),
}

pub type Symbols = HashMap<String, i64>;

pub struct Eval<'a> {
    src: &'a [u8],
    pos: usize,
    syms: &'a Symbols,
}

pub fn eval(src: &str, syms: &Symbols) -> Result<i64, EvalError> {
    let mut e = Eval { src: src.as_bytes(), pos: 0, syms };
    let v = e.compare()?;
    e.skip_ws();
    if e.pos != e.src.len() {
        return Err(EvalError::Syntax(format!(
            "trailing text in expression: {:?}",
            &src[e.pos..]
        )));
    }
    Ok(v)
}

impl<'a> Eval<'a> {
    fn skip_ws(&mut self) {
        while self.pos < self.src.len() && (self.src[self.pos] as char).is_whitespace() {
            self.pos += 1;
        }
    }

    fn peek(&mut self) -> Option<u8> {
        self.skip_ws();
        self.src.get(self.pos).copied()
    }

    fn eat(&mut self, s: &str) -> bool {
        self.skip_ws();
        if self.src[self.pos..].starts_with(s.as_bytes()) {
            self.pos += s.len();
            true
        } else {
            false
        }
    }

    /// Lowest precedence, and only really here to give `.assert` something to
    /// test: `.assert TILE_SOLID == $60, "the font grew"`.
    fn compare(&mut self) -> Result<i64, EvalError> {
        let lhs = self.expr()?;
        if self.eat("==") {
            return Ok((lhs == self.expr()?) as i64);
        }
        if self.eat("!=") {
            return Ok((lhs != self.expr()?) as i64);
        }
        // `>=` and `<=` only. A bare `<` or `>` stays the low/high byte prefix
        // it is everywhere else in 6502 source, and `<<`/`>>` were already
        // consumed by the shift layer below.
        if self.eat(">=") {
            return Ok((lhs >= self.expr()?) as i64);
        }
        if self.eat("<=") {
            return Ok((lhs <= self.expr()?) as i64);
        }
        Ok(lhs)
    }

    fn expr(&mut self) -> Result<i64, EvalError> {
        let mut lhs = self.xor()?;
        while self.peek() == Some(b'|') {
            self.pos += 1;
            lhs |= self.xor()?;
        }
        Ok(lhs)
    }

    fn xor(&mut self) -> Result<i64, EvalError> {
        let mut lhs = self.and()?;
        while self.peek() == Some(b'^') {
            self.pos += 1;
            lhs ^= self.and()?;
        }
        Ok(lhs)
    }

    fn and(&mut self) -> Result<i64, EvalError> {
        let mut lhs = self.shift()?;
        while self.peek() == Some(b'&') {
            self.pos += 1;
            lhs &= self.shift()?;
        }
        Ok(lhs)
    }

    fn shift(&mut self) -> Result<i64, EvalError> {
        let mut lhs = self.add()?;
        loop {
            if self.eat("<<") {
                lhs <<= self.add()?;
            } else if self.eat(">>") {
                lhs >>= self.add()?;
            } else {
                return Ok(lhs);
            }
        }
    }

    fn add(&mut self) -> Result<i64, EvalError> {
        let mut lhs = self.mul()?;
        loop {
            match self.peek() {
                Some(b'+') => {
                    self.pos += 1;
                    lhs += self.mul()?;
                }
                Some(b'-') => {
                    self.pos += 1;
                    lhs -= self.mul()?;
                }
                _ => return Ok(lhs),
            }
        }
    }

    fn mul(&mut self) -> Result<i64, EvalError> {
        let mut lhs = self.unary()?;
        loop {
            match self.peek() {
                Some(b'*') => {
                    self.pos += 1;
                    lhs *= self.unary()?;
                }
                Some(b'/') => {
                    self.pos += 1;
                    let d = self.unary()?;
                    if d == 0 {
                        return Err(EvalError::Syntax("division by zero".into()));
                    }
                    lhs /= d;
                }
                Some(b'%') => {
                    self.pos += 1;
                    let d = self.unary()?;
                    if d == 0 {
                        return Err(EvalError::Syntax("modulo by zero".into()));
                    }
                    lhs %= d;
                }
                _ => return Ok(lhs),
            }
        }
    }

    fn unary(&mut self) -> Result<i64, EvalError> {
        match self.peek() {
            // Low byte and high byte, as everywhere else in 6502 land. The high
            // byte operator is how a pointer table gets built.
            Some(b'<') => {
                self.pos += 1;
                Ok(self.unary()? & 0xff)
            }
            Some(b'>') => {
                self.pos += 1;
                Ok((self.unary()? >> 8) & 0xff)
            }
            Some(b'-') => {
                self.pos += 1;
                Ok(-self.unary()?)
            }
            Some(b'~') => {
                self.pos += 1;
                Ok(!self.unary()?)
            }
            _ => self.primary(),
        }
    }

    fn primary(&mut self) -> Result<i64, EvalError> {
        let c = self
            .peek()
            .ok_or_else(|| EvalError::Syntax("expression ended early".into()))?;
        match c {
            b'(' => {
                self.pos += 1;
                let v = self.expr()?;
                if !self.eat(")") {
                    return Err(EvalError::Syntax("missing close paren".into()));
                }
                Ok(v)
            }
            b'$' => {
                self.pos += 1;
                self.radix(16, |c| c.is_ascii_hexdigit())
            }
            b'%' => {
                self.pos += 1;
                self.radix(2, |c| c == b'0' || c == b'1')
            }
            QUOTE => {
                // A character literal, so text tables can be written as text.
                self.pos += 1;
                let ch = *self
                    .src
                    .get(self.pos)
                    .ok_or_else(|| EvalError::Syntax("unterminated char literal".into()))?;
                self.pos += 1;
                if self.src.get(self.pos) != Some(&QUOTE) {
                    return Err(EvalError::Syntax("unterminated char literal".into()));
                }
                self.pos += 1;
                Ok(ch as i64)
            }
            b'0'..=b'9' => self.radix(10, |c| c.is_ascii_digit()),
            c if is_sym_start(c) => {
                let start = self.pos;
                while self.pos < self.src.len() && is_sym_char(self.src[self.pos]) {
                    self.pos += 1;
                }
                let name = std::str::from_utf8(&self.src[start..self.pos]).unwrap();
                self.syms
                    .get(name)
                    .copied()
                    .ok_or_else(|| EvalError::Unknown(name.to_string()))
            }
            other => Err(EvalError::Syntax(format!(
                "unexpected character {:?}",
                other as char
            ))),
        }
    }

    fn radix(&mut self, radix: u32, ok: fn(u8) -> bool) -> Result<i64, EvalError> {
        let start = self.pos;
        while self.pos < self.src.len() && ok(self.src[self.pos]) {
            self.pos += 1;
        }
        if start == self.pos {
            return Err(EvalError::Syntax(format!("empty base-{radix} number")));
        }
        let text = std::str::from_utf8(&self.src[start..self.pos]).unwrap();
        i64::from_str_radix(text, radix)
            .map_err(|e| EvalError::Syntax(format!("bad number {text:?}: {e}")))
    }
}

/// ASCII apostrophe, the char-literal delimiter.
pub const QUOTE: u8 = 0x27;

pub fn is_sym_start(c: u8) -> bool {
    c.is_ascii_alphabetic() || c == b'_' || c == b'@' || c == b'.'
}

pub fn is_sym_char(c: u8) -> bool {
    c.is_ascii_alphanumeric() || c == b'_' || c == b'@' || c == b'.'
}

#[cfg(test)]
mod tests {
    use super::*;

    fn syms() -> Symbols {
        let mut s = Symbols::new();
        s.insert("base".into(), 0x8123);
        s.insert("two".into(), 2);
        s
    }

    #[test]
    fn arithmetic_and_bases() {
        let s = syms();
        assert_eq!(eval("$10", &s).unwrap(), 16);
        assert_eq!(eval("%1011", &s).unwrap(), 11);
        assert_eq!(eval("10", &s).unwrap(), 10);
        assert_eq!(eval("1 + two * 3", &s).unwrap(), 7);
        assert_eq!(eval("(1 + two) * 3", &s).unwrap(), 9);
        assert_eq!(eval("$f0 | $0f", &s).unwrap(), 0xff);
        assert_eq!(eval("1 << 4", &s).unwrap(), 16);
    }

    #[test]
    fn char_literal_is_its_ascii_value() {
        let s = syms();
        let src = format!("{q}A{q}", q = QUOTE as char);
        assert_eq!(eval(&src, &s).unwrap(), 65);
    }

    #[test]
    fn lo_and_hi_byte_operators() {
        let s = syms();
        assert_eq!(eval("<base", &s).unwrap(), 0x23);
        assert_eq!(eval(">base", &s).unwrap(), 0x81);
        assert_eq!(eval(">base + 1", &s).unwrap(), 0x82);
    }

    #[test]
    fn intermediate_values_do_not_wrap_at_eight_bits() {
        // 200 + 100 must stay 300 until the emitter narrows it, not become 44.
        let s = syms();
        assert_eq!(eval("200 + 100", &s).unwrap(), 300);
    }

    #[test]
    fn comparisons_yield_one_or_zero() {
        let s = syms();
        assert_eq!(eval("two == 2", &s).unwrap(), 1);
        assert_eq!(eval("two == 3", &s).unwrap(), 0);
        assert_eq!(eval("two != 3", &s).unwrap(), 1);
        assert_eq!(eval("base >= $8000", &s).unwrap(), 1);
        assert_eq!(eval("base <= $8000", &s).unwrap(), 0);
        // The high-byte prefix still works on the right of a comparison.
        assert_eq!(eval(">base >= $81", &s).unwrap(), 1);
        // The shift operator must not be eaten by the low-byte prefix.
        assert_eq!(eval("1 << two", &s).unwrap(), 4);
    }

    #[test]
    fn unknown_symbol_is_distinguishable_from_a_syntax_error() {
        let s = syms();
        assert!(matches!(eval("nope", &s), Err(EvalError::Unknown(n)) if n == "nope"));
        assert!(matches!(eval("1 +", &s), Err(EvalError::Syntax(_))));
        assert!(matches!(eval("$10 $20", &s), Err(EvalError::Syntax(_))));
    }
}
