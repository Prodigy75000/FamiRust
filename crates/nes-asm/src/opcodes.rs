// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Prodigy75000

//! The official NMOS 6502 instruction set, as a mnemonic/mode -> opcode table.
//!
//! Only the 151 documented opcodes are here. The core emulates the unofficial
//! ones (and is graded on them by the TomHarte corpus), but a ROM we intend to
//! hand to a third party has no business relying on them, so the assembler
//! simply cannot emit one.

use std::collections::HashMap;

#[derive(Copy, Clone, PartialEq, Eq, Hash, Debug)]
pub enum Mode {
    Imp,
    Acc,
    Imm,
    Zp,
    ZpX,
    ZpY,
    Abs,
    AbsX,
    AbsY,
    Ind,
    IndX,
    IndY,
    Rel,
}

impl Mode {
    /// Operand bytes that follow the opcode.
    pub fn operand_len(self) -> u16 {
        match self {
            Mode::Imp | Mode::Acc => 0,
            Mode::Imm | Mode::Zp | Mode::ZpX | Mode::ZpY | Mode::IndX | Mode::IndY | Mode::Rel => 1,
            Mode::Abs | Mode::AbsX | Mode::AbsY | Mode::Ind => 2,
        }
    }

    /// The zero-page form of an absolute mode, when one exists. Used to shrink
    /// `LDA $0010` to two bytes once the operand is known to fit in a page.
    pub fn zp_form(self) -> Option<Mode> {
        match self {
            Mode::Abs => Some(Mode::Zp),
            Mode::AbsX => Some(Mode::ZpX),
            Mode::AbsY => Some(Mode::ZpY),
            _ => None,
        }
    }
}

fn mode_from_str(s: &str) -> Mode {
    match s {
        "imp" => Mode::Imp,
        "acc" => Mode::Acc,
        "imm" => Mode::Imm,
        "zp" => Mode::Zp,
        "zpx" => Mode::ZpX,
        "zpy" => Mode::ZpY,
        "abs" => Mode::Abs,
        "absx" => Mode::AbsX,
        "absy" => Mode::AbsY,
        "ind" => Mode::Ind,
        "indx" => Mode::IndX,
        "indy" => Mode::IndY,
        "rel" => Mode::Rel,
        other => panic!("bad mode in opcode table: {other}"),
    }
}

/// `mnemonic mode=hex,mode=hex; ...` -- compact enough to eyeball against a
/// hardware reference in one sitting, which is the point.
const TABLE: &str = "\
adc imm=69,zp=65,zpx=75,abs=6d,absx=7d,absy=79,indx=61,indy=71;\
and imm=29,zp=25,zpx=35,abs=2d,absx=3d,absy=39,indx=21,indy=31;\
asl acc=0a,zp=06,zpx=16,abs=0e,absx=1e;\
bcc rel=90; bcs rel=b0; beq rel=f0; bmi rel=30;\
bne rel=d0; bpl rel=10; bvc rel=50; bvs rel=70;\
bit zp=24,abs=2c;\
brk imp=00;\
clc imp=18; cld imp=d8; cli imp=58; clv imp=b8;\
cmp imm=c9,zp=c5,zpx=d5,abs=cd,absx=dd,absy=d9,indx=c1,indy=d1;\
cpx imm=e0,zp=e4,abs=ec;\
cpy imm=c0,zp=c4,abs=cc;\
dec zp=c6,zpx=d6,abs=ce,absx=de;\
dex imp=ca; dey imp=88;\
eor imm=49,zp=45,zpx=55,abs=4d,absx=5d,absy=59,indx=41,indy=51;\
inc zp=e6,zpx=f6,abs=ee,absx=fe;\
inx imp=e8; iny imp=c8;\
jmp abs=4c,ind=6c;\
jsr abs=20;\
lda imm=a9,zp=a5,zpx=b5,abs=ad,absx=bd,absy=b9,indx=a1,indy=b1;\
ldx imm=a2,zp=a6,zpy=b6,abs=ae,absy=be;\
ldy imm=a0,zp=a4,zpx=b4,abs=ac,absx=bc;\
lsr acc=4a,zp=46,zpx=56,abs=4e,absx=5e;\
nop imp=ea;\
ora imm=09,zp=05,zpx=15,abs=0d,absx=1d,absy=19,indx=01,indy=11;\
pha imp=48; php imp=08; pla imp=68; plp imp=28;\
rol acc=2a,zp=26,zpx=36,abs=2e,absx=3e;\
ror acc=6a,zp=66,zpx=76,abs=6e,absx=7e;\
rti imp=40; rts imp=60;\
sbc imm=e9,zp=e5,zpx=f5,abs=ed,absx=fd,absy=f9,indx=e1,indy=f1;\
sec imp=38; sed imp=f8; sei imp=78;\
sta zp=85,zpx=95,abs=8d,absx=9d,absy=99,indx=81,indy=91;\
stx zp=86,zpy=96,abs=8e;\
sty zp=84,zpx=94,abs=8c;\
tax imp=aa; tay imp=a8; tsx imp=ba; txa imp=8a; txs imp=9a; tya imp=98;\
";

pub struct Opcodes {
    map: HashMap<(String, Mode), u8>,
    mnemonics: HashMap<String, Vec<Mode>>,
}

impl Opcodes {
    pub fn new() -> Self {
        let mut map = HashMap::new();
        let mut mnemonics: HashMap<String, Vec<Mode>> = HashMap::new();
        for entry in TABLE.split(';') {
            let entry = entry.trim();
            if entry.is_empty() {
                continue;
            }
            let (mnem, forms) = entry
                .split_once(' ')
                .unwrap_or_else(|| panic!("bad opcode entry: {entry}"));
            for form in forms.split(',') {
                let (mode, hex) = form
                    .split_once('=')
                    .unwrap_or_else(|| panic!("bad opcode form: {form}"));
                let mode = mode_from_str(mode);
                let op = u8::from_str_radix(hex, 16)
                    .unwrap_or_else(|_| panic!("bad opcode byte: {hex}"));
                map.insert((mnem.to_string(), mode), op);
                mnemonics.entry(mnem.to_string()).or_default().push(mode);
            }
        }
        Opcodes { map, mnemonics }
    }

    pub fn is_mnemonic(&self, mnem: &str) -> bool {
        self.mnemonics.contains_key(mnem)
    }

    pub fn lookup(&self, mnem: &str, mode: Mode) -> Option<u8> {
        self.map.get(&(mnem.to_string(), mode)).copied()
    }

    pub fn modes(&self, mnem: &str) -> &[Mode] {
        self.mnemonics.get(mnem).map(|v| &v[..]).unwrap_or(&[])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn table_has_exactly_the_151_official_opcodes() {
        let ops = Opcodes::new();
        assert_eq!(ops.map.len(), 151, "official 6502 opcode count");
        // No two mnemonic/mode pairs may share an opcode byte.
        let mut seen = std::collections::HashSet::new();
        for op in ops.map.values() {
            assert!(seen.insert(*op), "duplicate opcode byte ${op:02x}");
        }
    }

    #[test]
    fn spot_check_against_hardware_reference() {
        let ops = Opcodes::new();
        assert_eq!(ops.lookup("lda", Mode::Imm), Some(0xa9));
        assert_eq!(ops.lookup("sta", Mode::IndY), Some(0x91));
        assert_eq!(ops.lookup("jmp", Mode::Ind), Some(0x6c));
        assert_eq!(ops.lookup("bne", Mode::Rel), Some(0xd0));
        // LDX has no zp,x form -- only zp,y. A classic assembler bug.
        assert_eq!(ops.lookup("ldx", Mode::ZpX), None);
        assert_eq!(ops.lookup("ldx", Mode::ZpY), Some(0xb6));
        // STA has no immediate form.
        assert_eq!(ops.lookup("sta", Mode::Imm), None);
    }
}
