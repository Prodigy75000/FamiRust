// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Prodigy75000

//! The assembler proper: source lines in, an iNES image out.
//!
//! Two passes. Pass one places labels and decides each instruction's operand width;
//! pass two emits bytes using the widths pass one chose. Deciding the width once
//! and reusing the decision is what keeps a forward reference from silently
//! resizing an instruction between passes and sliding every label after it.

use crate::expr::{self, EvalError, Symbols, QUOTE};
use crate::opcodes::{Mode, Opcodes};

pub struct Options {
    /// Emit a `label = $addr` listing next to the ROM. Useful when reading a
    /// core trace back against the source.
    pub symbol_file: bool,
}

#[derive(Copy, Clone, PartialEq, Eq)]
enum Section {
    Prg,
    Chr,
}

/// One source line, tagged with where it came from so errors can point at it.
struct Line {
    file: String,
    no: usize,
    text: String,
}

pub struct Output {
    pub rom: Vec<u8>,
    pub symbols: String,
    /// PRG bytes actually written, for the "how full is the cart" report.
    pub prg_used: usize,
    pub chr_tiles_used: usize,
}

struct Asm {
    ops: Opcodes,
    syms: Symbols,

    prg: Vec<u8>,
    chr: Vec<u8>,
    prg_base: u16,
    mapper: u8,
    vertical_mirroring: bool,
    configured: bool,

    section: Section,
    pc: u16,
    chr_pos: usize,
    prg_written: Vec<bool>,

    pass: usize,
    /// Operand width chosen per instruction on pass one, replayed on pass two.
    widths: Vec<u16>,
    insn_ord: usize,

    scope: String,
    in_tile: bool,
    tile_rows: Vec<[u8; 8]>,
    max_chr_pos: usize,
}

pub fn assemble(entry: &std::path::Path, opts: &Options) -> Result<Output, String> {
    let lines = load(entry)?;

    let mut a = Asm {
        ops: Opcodes::new(),
        syms: Symbols::new(),
        prg: Vec::new(),
        chr: Vec::new(),
        prg_base: 0x8000,
        mapper: 0,
        vertical_mirroring: false,
        configured: false,
        section: Section::Prg,
        pc: 0x8000,
        chr_pos: 0,
        prg_written: Vec::new(),
        pass: 1,
        widths: Vec::new(),
        insn_ord: 0,
        scope: String::new(),
        in_tile: false,
        tile_rows: Vec::new(),
        max_chr_pos: 0,
    };

    for pass in 1..=2 {
        a.pass = pass;
        a.insn_ord = 0;
        a.section = Section::Prg;
        a.chr_pos = 0;
        a.scope.clear();
        a.in_tile = false;
        a.configured = false;
        a.pc = 0x8000;
        if pass == 2 {
            // Keep labels from pass one; wipe the images so pass two writes clean.
            a.prg.iter_mut().for_each(|b| *b = 0xff);
            a.chr.iter_mut().for_each(|b| *b = 0x00);
            a.prg_written.iter_mut().for_each(|b| *b = false);
        }
        for line in &lines {
            a.line(line)
                .map_err(|e| format!("{}:{}: {}\n  | {}", line.file, line.no, e, line.text.trim()))?;
        }
        if a.in_tile {
            return Err("unterminated .tile block at end of source".into());
        }
    }

    if !a.configured {
        return Err("source never declared a cartridge with .ines".into());
    }

    let mut rom = Vec::with_capacity(16 + a.prg.len() + a.chr.len());
    rom.extend_from_slice(b"NES\x1a");
    rom.push((a.prg.len() / 16384) as u8);
    rom.push((a.chr.len() / 8192) as u8);
    // Flags 6: mirroring in bit 0, mapper low nibble in bits 4-7.
    rom.push((a.vertical_mirroring as u8) | (a.mapper << 4));
    // Flags 7: mapper high nibble. Bits 2-3 stay 0, which keeps this a plain
    // iNES 1.0 header rather than NES 2.0 -- the widest-compatibility choice
    // for a ROM whose whole job is to load anywhere without argument.
    rom.push(a.mapper & 0xf0);
    rom.extend_from_slice(&[0u8; 8]);
    rom.extend_from_slice(&a.prg);
    rom.extend_from_slice(&a.chr);

    let mut symbols = String::new();
    if opts.symbol_file {
        let mut names: Vec<_> = a.syms.iter().collect();
        names.sort_by(|x, y| x.1.cmp(y.1).then(x.0.cmp(y.0)));
        for (name, value) in names {
            symbols.push_str(&format!("{value:04X}  {name}\n"));
        }
    }

    Ok(Output {
        rom,
        symbols,
        prg_used: a.prg_written.iter().filter(|w| **w).count(),
        chr_tiles_used: a.max_chr_pos / 16,
    })
}

/// Read the entry file and splice in every `.include`, depth first.
fn load(path: &std::path::Path) -> Result<Vec<Line>, String> {
    let mut out = Vec::new();
    load_into(path, &mut out, 0)?;
    Ok(out)
}

fn load_into(path: &std::path::Path, out: &mut Vec<Line>, depth: usize) -> Result<(), String> {
    if depth > 8 {
        return Err(format!("include nested too deep at {}", path.display()));
    }
    let text = std::fs::read_to_string(path)
        .map_err(|e| format!("cannot read {}: {e}", path.display()))?;
    let dir = path.parent().unwrap_or(std::path::Path::new("."));
    let name = path.display().to_string();

    for (i, raw) in text.lines().enumerate() {
        let no = i + 1;
        let code = strip_comment(raw);
        let trimmed = code.trim();
        if let Some(rest) = trimmed.strip_prefix(".include") {
            let inc = rest.trim().trim_matches('"');
            if inc.is_empty() {
                return Err(format!("{name}:{no}: .include needs a quoted path"));
            }
            load_into(&dir.join(inc), out, depth + 1)?;
            continue;
        }
        out.push(Line { file: name.clone(), no, text: code });
    }
    Ok(())
}

/// Drop a trailing comment, respecting char and string literals so a `;` inside
/// one survives.
fn strip_comment(line: &str) -> String {
    let b = line.as_bytes();
    let mut i = 0;
    let mut quote: Option<u8> = None;
    while i < b.len() {
        let c = b[i];
        match quote {
            Some(q) => {
                if c == q {
                    quote = None;
                }
            }
            None => {
                if c == b';' {
                    return line[..i].to_string();
                }
                if c == b'"' || c == QUOTE {
                    quote = Some(c);
                }
            }
        }
        i += 1;
    }
    line.to_string()
}

impl Asm {
    fn line(&mut self, line: &Line) -> Result<(), String> {
        let text = line.text.clone();

        if self.in_tile {
            return self.tile_row(text.trim());
        }

        let mut rest = text.trim();
        if rest.is_empty() {
            return Ok(());
        }

        // Leading `label:` (or `@local:`).
        if let Some(colon) = top_level_colon(rest) {
            let label = rest[..colon].trim();
            self.define_label(label)?;
            rest = rest[colon + 1..].trim();
            if rest.is_empty() {
                return Ok(());
            }
        }

        // `NAME = expr` constant.
        if let Some(eq) = constant_split(rest) {
            let name = rest[..eq].trim().to_string();
            let value = self.value(rest[eq + 1..].trim())?.unwrap_or(0);
            return self.define(&name, value);
        }

        if rest.starts_with('.') {
            return self.directive(rest);
        }

        self.instruction(rest)
    }

    // ---- symbols ---------------------------------------------------------

    fn qualify(&self, name: &str) -> String {
        if name.starts_with('@') {
            format!("{}{}", self.scope, name)
        } else {
            name.to_string()
        }
    }

    fn define_label(&mut self, label: &str) -> Result<(), String> {
        if label.is_empty() {
            return Err("empty label".into());
        }
        if !label.starts_with('@') {
            self.scope = label.to_string();
        }
        let here = match self.section {
            Section::Prg => self.pc as i64,
            Section::Chr => (self.chr_pos / 16) as i64,
        };
        let name = self.qualify(label);
        self.define(&name, here)
    }

    fn define(&mut self, name: &str, value: i64) -> Result<(), String> {
        if self.pass == 1 {
            if self.syms.insert(name.to_string(), value).is_some() {
                return Err(format!("{name} defined twice"));
            }
        } else {
            // A label that moved between passes means an instruction changed
            // size underneath it. Catch it here rather than shipping a ROM that
            // jumps into the middle of an operand.
            match self.syms.get(name) {
                Some(&old) if old == value => {}
                Some(&old) => {
                    return Err(format!(
                        "phase error: {name} was ${old:04X} on pass 1, ${value:04X} on pass 2"
                    ))
                }
                None => return Err(format!("{name} appeared only on pass 2")),
            }
        }
        Ok(())
    }

    /// Rewrite `@local` references into their scoped form before evaluation, so
    /// two subroutines can each have their own `@loop` without colliding.
    fn qualify_expr(&self, src: &str) -> String {
        let b = src.as_bytes();
        let mut out = String::new();
        let mut i = 0;
        while i < b.len() {
            let c = b[i];
            if c == QUOTE || c == b'"' {
                out.push(c as char);
                i += 1;
                while i < b.len() {
                    let ch = b[i];
                    out.push(ch as char);
                    i += 1;
                    if ch == c {
                        break;
                    }
                }
                continue;
            }
            if expr::is_sym_start(c) {
                let start = i;
                while i < b.len() && expr::is_sym_char(b[i]) {
                    i += 1;
                }
                if c == b'@' {
                    out.push_str(&self.scope);
                }
                out.push_str(&src[start..i]);
                continue;
            }
            out.push(c as char);
            i += 1;
        }
        out
    }

    /// `Ok(None)` means "not resolvable yet", which is only tolerable on pass 1.
    fn value(&self, src: &str) -> Result<Option<i64>, String> {
        let src = &self.qualify_expr(src);
        match expr::eval(src, &self.syms) {
            Ok(v) => Ok(Some(v)),
            Err(EvalError::Unknown(n)) => {
                if self.pass == 1 {
                    Ok(None)
                } else {
                    Err(format!("undefined symbol: {n}"))
                }
            }
            Err(EvalError::Syntax(m)) => Err(m),
        }
    }

    fn value_now(&self, src: &str) -> Result<i64, String> {
        self.value(src)?
            .ok_or_else(|| format!("{src:?} must be known here, but is a forward reference"))
    }

    // ---- output ----------------------------------------------------------

    fn emit(&mut self, byte: u8) -> Result<(), String> {
        match self.section {
            Section::Prg => {
                let off = (self.pc as i32) - (self.prg_base as i32);
                if off < 0 || off as usize >= self.prg.len() {
                    return Err(format!(
                        "${:04X} is outside the cartridge window ${:04X}-$FFFF",
                        self.pc, self.prg_base
                    ));
                }
                let off = off as usize;
                if self.prg_written[off] {
                    return Err(format!("two things want to live at ${:04X}", self.pc));
                }
                self.prg[off] = byte;
                self.prg_written[off] = true;
                self.pc = self.pc.wrapping_add(1);
                Ok(())
            }
            Section::Chr => {
                if self.chr_pos >= self.chr.len() {
                    return Err("CHR overflows the character ROM".into());
                }
                self.chr[self.chr_pos] = byte;
                self.chr_pos += 1;
                self.max_chr_pos = self.max_chr_pos.max(self.chr_pos);
                Ok(())
            }
        }
    }

    fn emit_byte_value(&mut self, v: i64, what: &str) -> Result<(), String> {
        if !(-128..=255).contains(&v) {
            return Err(format!("{what} value {v} does not fit in a byte"));
        }
        self.emit((v as i32 as u32 & 0xff) as u8)
    }

    // ---- directives ------------------------------------------------------

    fn directive(&mut self, rest: &str) -> Result<(), String> {
        let (name, args) = match rest.find(char::is_whitespace) {
            Some(i) => (&rest[..i], rest[i..].trim()),
            None => (rest, ""),
        };

        match name {
            ".ines" => self.d_ines(args),
            ".org" => {
                let v = self.value_now(args)?;
                if !(0..=0xffff).contains(&v) {
                    return Err(format!(".org ${v:X} is not a 16-bit address"));
                }
                self.section = Section::Prg;
                self.pc = v as u16;
                Ok(())
            }
            ".byte" | ".db" => self.d_bytes(args, 0),
            ".str" => self.d_bytes(args, 0x20),
            ".word" | ".dw" => self.d_words(args),
            ".res" => self.d_res(args),
            ".align" => self.d_align(args),
            ".assert" => self.d_assert(args),
            ".chr" => {
                self.section = Section::Chr;
                Ok(())
            }
            ".prg" => {
                self.section = Section::Prg;
                Ok(())
            }
            ".chrskip" => {
                let n = self.value_now(args)?;
                for _ in 0..n * 16 {
                    self.emit(0)?;
                }
                Ok(())
            }
            ".tile" => {
                if self.section != Section::Chr {
                    return Err(".tile only makes sense inside a .chr section".into());
                }
                if !args.is_empty() {
                    self.define_label(args)?;
                }
                self.in_tile = true;
                self.tile_rows.clear();
                Ok(())
            }
            ".endtile" => Err(".endtile without .tile".into()),
            other => Err(format!("unknown directive {other}")),
        }
    }

    fn d_ines(&mut self, args: &str) -> Result<(), String> {
        let mut prg_kb = 32usize;
        let mut chr_kb = 8usize;
        let mut mapper = 0u8;
        let mut vertical = false;

        for field in args.split_whitespace() {
            let (k, v) = field
                .split_once('=')
                .ok_or_else(|| format!("bad .ines field {field:?}, want key=value"))?;
            match k {
                "prg" => prg_kb = v.parse().map_err(|_| "prg must be a number of KB")?,
                "chr" => chr_kb = v.parse().map_err(|_| "chr must be a number of KB")?,
                "mapper" => mapper = v.parse().map_err(|_| "mapper must be a number")?,
                "mirror" => {
                    vertical = match v {
                        "v" | "vertical" => true,
                        "h" | "horizontal" => false,
                        _ => return Err("mirror must be h or v".into()),
                    }
                }
                other => return Err(format!("unknown .ines field {other}")),
            }
        }

        if prg_kb % 16 != 0 || prg_kb == 0 || prg_kb > 64 {
            return Err("prg must be a non-zero multiple of 16 KB".into());
        }
        if chr_kb % 8 != 0 || chr_kb > 8 {
            return Err("chr must be 0 or 8 KB".into());
        }

        if self.pass == 1 {
            self.prg = vec![0xff; prg_kb * 1024];
            self.chr = vec![0x00; chr_kb * 1024];
            self.prg_written = vec![false; prg_kb * 1024];
        }
        self.prg_base = (0x10000 - prg_kb * 1024) as u16;
        self.pc = self.prg_base;
        self.mapper = mapper;
        self.vertical_mirroring = vertical;
        self.configured = true;
        Ok(())
    }

    fn d_bytes(&mut self, args: &str, string_bias: u8) -> Result<(), String> {
        for item in split_top_level(args, ',') {
            let item = item.trim();
            if item.is_empty() {
                continue;
            }
            if item.len() >= 2 && item.starts_with('"') && item.ends_with('"') {
                for ch in item[1..item.len() - 1].bytes() {
                    let v = ch
                        .checked_sub(string_bias)
                        .ok_or_else(|| format!("{:?} is below the character base", ch as char))?;
                    self.emit(v)?;
                }
            } else {
                let v = self.value(item)?.unwrap_or(0);
                self.emit_byte_value(v, ".byte")?;
            }
        }
        Ok(())
    }

    fn d_words(&mut self, args: &str) -> Result<(), String> {
        for item in split_top_level(args, ',') {
            let item = item.trim();
            if item.is_empty() {
                continue;
            }
            let v = self.value(item)?.unwrap_or(0);
            if !(-32768..=65535).contains(&v) {
                return Err(format!(".word value {v} does not fit in 16 bits"));
            }
            let v = v as i32 as u32;
            self.emit((v & 0xff) as u8)?;
            self.emit(((v >> 8) & 0xff) as u8)?;
        }
        Ok(())
    }

    fn d_res(&mut self, args: &str) -> Result<(), String> {
        let mut parts = split_top_level(args, ',');
        let count = self.value_now(parts.next().unwrap_or("").trim())?;
        let fill = match parts.next() {
            Some(f) => self.value_now(f.trim())? as u8,
            None => 0,
        };
        for _ in 0..count {
            self.emit(fill)?;
        }
        Ok(())
    }

    fn d_align(&mut self, args: &str) -> Result<(), String> {
        let n = self.value_now(args)?;
        if n <= 0 {
            return Err(".align needs a positive boundary".into());
        }
        while (self.pc as i64) % n != 0 {
            self.emit(0xff)?;
        }
        Ok(())
    }

    fn d_assert(&mut self, args: &str) -> Result<(), String> {
        // `.assert <expr>, "message"`. Evaluated on pass two, when every label
        // is final. This is how the source pins its own layout invariants --
        // "this table does not straddle a page", "the vectors did not move".
        let mut parts = split_top_level(args, ',');
        let cond = parts.next().unwrap_or("").trim().to_string();
        let msg = parts
            .next()
            .map(|m| m.trim().trim_matches('"').to_string())
            .unwrap_or_else(|| cond.clone());
        if self.pass != 2 {
            return Ok(());
        }
        if self.value_now(&cond)? == 0 {
            return Err(format!("assertion failed: {msg}"));
        }
        Ok(())
    }

    // ---- CHR text-art tiles ---------------------------------------------

    fn tile_row(&mut self, row: &str) -> Result<(), String> {
        if row == ".endtile" {
            if self.tile_rows.len() != 8 {
                return Err(format!(
                    "a tile is 8 rows, this one has {}",
                    self.tile_rows.len()
                ));
            }
            self.in_tile = false;
            let rows = std::mem::take(&mut self.tile_rows);
            // Two bitplanes, 8 bytes apart: plane 0 holds bit 0 of each pixel,
            // plane 1 holds bit 1. That is the 2C02 pattern-table layout.
            for plane in 0..2 {
                for r in &rows {
                    let mut byte = 0u8;
                    for (x, px) in r.iter().enumerate() {
                        if (px >> plane) & 1 != 0 {
                            byte |= 0x80 >> x;
                        }
                    }
                    self.emit(byte)?;
                }
            }
            return Ok(());
        }
        if row.is_empty() {
            return Ok(());
        }
        if row.len() != 8 {
            return Err(format!(
                "a tile row is 8 pixels, this one is {} ({row:?})",
                row.len()
            ));
        }
        let mut out = [0u8; 8];
        for (i, c) in row.bytes().enumerate() {
            out[i] = match c {
                b'.' | b'0' => 0,
                b'1' => 1,
                b'2' => 2,
                b'3' => 3,
                other => {
                    return Err(format!(
                        "tile pixels are . 1 2 3, not {:?}",
                        other as char
                    ))
                }
            };
        }
        self.tile_rows.push(out);
        Ok(())
    }

    // ---- instructions ----------------------------------------------------

    fn instruction(&mut self, text: &str) -> Result<(), String> {
        let (mnem, operand) = match text.find(char::is_whitespace) {
            Some(i) => (text[..i].to_ascii_lowercase(), text[i..].trim()),
            None => (text.to_ascii_lowercase(), ""),
        };
        if !self.ops.is_mnemonic(&mnem) {
            return Err(format!("{mnem} is not a 6502 instruction"));
        }
        if self.section != Section::Prg {
            return Err("code cannot go in a .chr section".into());
        }

        let (mut mode, arg) = parse_operand(operand)?;

        // A branch mnemonic only has a relative form; the parser cannot know
        // that from the syntax, since `bne loop` looks exactly like `lda addr`.
        if self.ops.lookup(&mnem, Mode::Rel).is_some() && mode == Mode::Abs {
            mode = Mode::Rel;
        }

        let ord = self.insn_ord;
        self.insn_ord += 1;

        let value = match arg {
            Some(ref a) => self.value(a)?,
            None => None,
        };

        // Width is chosen once, on pass one, and replayed on pass two.
        if self.pass == 1 {
            if let Some(zp) = mode.zp_form() {
                let fits = matches!(value, Some(v) if (0..=0xff).contains(&v));
                if fits && self.ops.lookup(&mnem, zp).is_some() {
                    mode = zp;
                }
            }
            self.widths.push(mode.operand_len());
        } else {
            let want = self.widths[ord];
            if want == 1 {
                if let Some(zp) = mode.zp_form() {
                    if self.ops.lookup(&mnem, zp).is_some() {
                        mode = zp;
                    }
                }
            }
        }

        let op = self.ops.lookup(&mnem, mode).ok_or_else(|| {
            format!(
                "{mnem} has no {mode:?} form (it has {:?})",
                self.ops.modes(&mnem)
            )
        })?;

        let at = self.pc;
        self.emit(op)?;

        match mode {
            Mode::Imp | Mode::Acc => {}
            Mode::Rel => {
                let target = value.unwrap_or(at as i64 + 2);
                let delta = target - (at as i64 + 2);
                if !(-128..=127).contains(&delta) {
                    return Err(format!(
                        "branch to ${target:04X} is {delta} bytes away; only -128..127 reaches"
                    ));
                }
                self.emit((delta as i8) as u8)?;
            }
            Mode::Imm => {
                let v = value.unwrap_or(0);
                self.emit_byte_value(v, "immediate")?;
            }
            Mode::Zp | Mode::ZpX | Mode::ZpY | Mode::IndX | Mode::IndY => {
                let v = value.unwrap_or(0);
                if !(0..=0xff).contains(&v) {
                    return Err(format!("${v:X} is not a zero-page address"));
                }
                self.emit(v as u8)?;
            }
            Mode::Abs | Mode::AbsX | Mode::AbsY | Mode::Ind => {
                let v = value.unwrap_or(0);
                if !(0..=0xffff).contains(&v) {
                    return Err(format!("${v:X} is not a 16-bit address"));
                }
                self.emit((v & 0xff) as u8)?;
                self.emit(((v >> 8) & 0xff) as u8)?;
            }
        }
        Ok(())
    }
}

// ---- operand syntax ------------------------------------------------------

fn parse_operand(s: &str) -> Result<(Mode, Option<String>), String> {
    let s = s.trim();
    if s.is_empty() {
        return Ok((Mode::Imp, None));
    }
    if s.eq_ignore_ascii_case("a") {
        return Ok((Mode::Acc, None));
    }
    if let Some(rest) = s.strip_prefix('#') {
        return Ok((Mode::Imm, Some(rest.trim().to_string())));
    }

    if s.starts_with('(') {
        if let Some(close) = matching_paren(s) {
            let inner = s[1..close].trim();
            let after = s[close + 1..].trim();
            if after.is_empty() {
                // `(addr)` or `(addr,X)`
                if let Some(base) = strip_index(inner, 'x') {
                    return Ok((Mode::IndX, Some(base)));
                }
                return Ok((Mode::Ind, Some(inner.to_string())));
            }
            if after.eq_ignore_ascii_case(",y") {
                return Ok((Mode::IndY, Some(inner.to_string())));
            }
            if after.eq_ignore_ascii_case(",x") {
                return Err("(addr),X is not a 6502 addressing mode; did you mean (addr,X)?".into());
            }
            // Anything else, e.g. `(a+b)*2`, is just an expression.
        }
    }

    if let Some(base) = strip_index(s, 'x') {
        return Ok((Mode::AbsX, Some(base)));
    }
    if let Some(base) = strip_index(s, 'y') {
        return Ok((Mode::AbsY, Some(base)));
    }
    Ok((Mode::Abs, Some(s.to_string())))
}

/// Peel a trailing `,X` / `,Y` written at the top level of the operand, so that
/// a comma inside parentheses is not mistaken for the index separator.
fn strip_index(s: &str, reg: char) -> Option<String> {
    let idx = last_top_level_comma(s)?;
    let tail = s[idx + 1..].trim();
    if tail.len() == 1 && tail.as_bytes()[0].to_ascii_lowercase() == reg as u8 {
        Some(s[..idx].trim().to_string())
    } else {
        None
    }
}

fn matching_paren(s: &str) -> Option<usize> {
    let b = s.as_bytes();
    let mut depth = 0i32;
    for (i, &c) in b.iter().enumerate() {
        match c {
            b'(' => depth += 1,
            b')' => {
                depth -= 1;
                if depth == 0 {
                    return Some(i);
                }
            }
            _ => {}
        }
    }
    None
}

fn last_top_level_comma(s: &str) -> Option<usize> {
    let b = s.as_bytes();
    let mut depth = 0i32;
    let mut found = None;
    for (i, &c) in b.iter().enumerate() {
        match c {
            b'(' => depth += 1,
            b')' => depth -= 1,
            b',' if depth == 0 => found = Some(i),
            _ => {}
        }
    }
    found
}

fn split_top_level(s: &str, sep: char) -> impl Iterator<Item = &str> {
    let mut parts = Vec::new();
    let mut depth = 0i32;
    let mut quote: Option<u8> = None;
    let mut start = 0usize;
    let bytes = s.as_bytes();
    for i in 0..bytes.len() {
        let c = bytes[i];
        match quote {
            Some(q) => {
                if c == q {
                    quote = None;
                }
            }
            None => {
                if c == b'"' || c == QUOTE {
                    quote = Some(c);
                } else if c == b'(' {
                    depth += 1;
                } else if c == b')' {
                    depth -= 1;
                } else if c as char == sep && depth == 0 {
                    parts.push(&s[start..i]);
                    start = i + 1;
                }
            }
        }
    }
    parts.push(&s[start..]);
    parts.into_iter()
}

/// Index of the `:` that ends a leading label, if there is one.
fn top_level_colon(s: &str) -> Option<usize> {
    let b = s.as_bytes();
    if b.is_empty() || !expr::is_sym_start(b[0]) {
        return None;
    }
    let mut i = 0;
    while i < b.len() && expr::is_sym_char(b[i]) {
        i += 1;
    }
    if i < b.len() && b[i] == b':' {
        Some(i)
    } else {
        None
    }
}

/// Index of the `=` in a `NAME = expr` line, if that is what this line is.
fn constant_split(s: &str) -> Option<usize> {
    let b = s.as_bytes();
    if b.is_empty() || !expr::is_sym_start(b[0]) {
        return None;
    }
    let mut i = 0;
    while i < b.len() && expr::is_sym_char(b[i]) {
        i += 1;
    }
    let mut j = i;
    while j < b.len() && b[j] == b' ' {
        j += 1;
    }
    // `=` but not `==`, and not `>=` / `<=` (which cannot start a line anyway).
    if j < b.len() && b[j] == b'=' && b.get(j + 1) != Some(&b'=') {
        Some(j)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mode_of(s: &str) -> Mode {
        parse_operand(s).unwrap().0
    }

    #[test]
    fn addressing_modes_parse() {
        assert_eq!(mode_of(""), Mode::Imp);
        assert_eq!(mode_of("A"), Mode::Acc);
        assert_eq!(mode_of("#$10"), Mode::Imm);
        assert_eq!(mode_of("$2000"), Mode::Abs);
        assert_eq!(mode_of("$2000,X"), Mode::AbsX);
        assert_eq!(mode_of("$2000,y"), Mode::AbsY);
        assert_eq!(mode_of("($2000)"), Mode::Ind);
        assert_eq!(mode_of("($10,X)"), Mode::IndX);
        assert_eq!(mode_of("($10),Y"), Mode::IndY);
    }

    #[test]
    fn a_comma_inside_parens_is_not_an_index() {
        // `(a+b)*2` must stay absolute, and the paren contents must not be
        // mistaken for an indexed operand.
        assert_eq!(mode_of("(base+2)*2"), Mode::Abs);
        assert_eq!(parse_operand("(base+2)*2").unwrap().1.unwrap(), "(base+2)*2");
    }

    #[test]
    fn indirect_x_is_not_confused_with_indirect_y() {
        assert!(parse_operand("($10),X").is_err());
    }

    #[test]
    fn label_and_constant_lines_are_told_apart() {
        assert_eq!(top_level_colon("loop: lda #0"), Some(4));
        assert_eq!(top_level_colon("lda #0"), None);
        assert_eq!(constant_split("SCREEN = $2000"), Some(7));
        assert_eq!(constant_split("lda #0"), None);
    }

    #[test]
    fn comments_stop_at_a_semicolon_but_not_inside_a_literal() {
        assert_eq!(strip_comment("lda #1 ; go").trim(), "lda #1");
        let src = format!(".byte {q};{q}, 2 ; real comment", q = QUOTE as char);
        assert_eq!(strip_comment(&src).trim(), format!(".byte {q};{q}, 2", q = QUOTE as char).trim());
    }
}
