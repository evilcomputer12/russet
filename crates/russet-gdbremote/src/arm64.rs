// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Russet contributors

//! The few AArch64 facts the JIT servicer needs (RD §7.5): debugserver's register numbers and a decoder that
//! recognises only `ret`, `retaa`/`retab`, `bti`, `paciasp`/`pacibsp` and `nop`, so no disassembler is needed.

/// debugserver's arm64 register numbers: `x0`…`x28` are 0…28.
pub const REG_X0: u32 = 0;
pub const REG_X1: u32 = 1;
pub const REG_FP: u32 = 29;
pub const REG_LR: u32 = 30;
pub const REG_SP: u32 = 31;
pub const REG_PC: u32 = 32;
pub const REG_CPSR: u32 = 33;

/// The instruction classes the decoder knows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Insn {
    /// `ret` (through `x30`).
    Ret,
    /// `ret xN` with another register.
    RetReg(u8),
    /// `retaa` / `retab` (pointer-authenticated return through `x30`).
    RetAuth,
    /// `bti`, `bti c|j|jc`.
    Bti,
    /// `paciasp` / `pacibsp` (sign `lr` against `sp`).
    PacSp,
    Nop,
    Other,
}

/// Classifies one instruction word (little-endian as read from memory, already converted to `u32`).
pub fn classify(insn: u32) -> Insn {
    match insn {
        0xD65F_03C0 => Insn::Ret,
        0xD65F_0BFF | 0xD65F_0FFF => Insn::RetAuth,
        0xD503_201F => Insn::Nop,
        0xD503_241F | 0xD503_245F | 0xD503_249F | 0xD503_24DF => Insn::Bti,
        0xD503_233F | 0xD503_237F => Insn::PacSp,
        w if w & 0xFFFF_FC1F == 0xD65F_0000 => Insn::RetReg(((w >> 5) & 0x1F) as u8),
        _ => Insn::Other,
    }
}

/// Whether `insn` is a plain `ret`, so a stop at it can be "returned from" by setting `pc = lr`.
pub fn is_plain_return(insn: u32) -> bool {
    classify(insn) == Insn::Ret
}

/// Whether a function whose first instructions are `insns` returns at once without side effects: any `bti`,
/// `nop` or `paciasp`/`pacibsp` prologue followed by `ret` or `retaa`/`retab`. Then `pc = lr` emulates the call.
pub fn returns_immediately(insns: &[u32]) -> bool {
    for &w in insns {
        match classify(w) {
            Insn::Bti | Insn::Nop | Insn::PacSp => continue,
            Insn::Ret | Insn::RetAuth => return true,
            Insn::RetReg(_) | Insn::Other => return false,
        }
    }
    false
}

/// Decodes little-endian instruction words from memory.
pub fn words(bytes: &[u8]) -> Vec<u32> {
    bytes.as_chunks::<4>().0.iter().map(|c| u32::from_le_bytes(*c)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_the_known_instructions() {
        assert!(is_plain_return(0xD65F_03C0));
        assert_eq!(classify(0xD65F_0BFF), Insn::RetAuth);
        assert_eq!(classify(0xD65F_0FFF), Insn::RetAuth);
        assert_eq!(classify(0xD65F_0000), Insn::RetReg(0));
        assert_eq!(classify(0xD65F_0220), Insn::RetReg(17));
        assert_eq!(classify(0xD503_201F), Insn::Nop);
        assert_eq!(classify(0xD503_245F), Insn::Bti);
        assert_eq!(classify(0xD503_233F), Insn::PacSp);
        // stp x29, x30, [sp, #-16]!
        assert_eq!(classify(0xA9BF_7BFD), Insn::Other);
        assert!(!is_plain_return(0xD65F_0BFF));
    }

    #[test]
    fn prologues_that_return_at_once() {
        assert!(returns_immediately(&[0xD65F_03C0]));
        assert!(returns_immediately(&[0xD503_245F, 0xD65F_03C0]));
        assert!(returns_immediately(&[0xD503_237F, 0xD65F_0FFF]));
        assert!(!returns_immediately(&[0xA9BF_7BFD, 0xD65F_03C0]));
        assert!(!returns_immediately(&[0xD65F_0220]));
        assert!(!returns_immediately(&[0xD503_201F]));
        assert_eq!(words(&[0xC0, 0x03, 0x5F, 0xD6]), vec![0xD65F_03C0]);
    }
}
