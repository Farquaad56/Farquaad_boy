//! Jeu d'instructions complet du CPU Sharp LR35902 (SM83).
//!
//! Partie 6 : les 256 opcodes + le préfixe 0xCB, avec les T-cycles exacts et les
//! effets sur les drapeaux (voir `flags`), le bug HALT, les opcodes non mappés
//! et les instructions d'interruption EI/DI/RETI (PanDocs « CPU Instruction Set »,
//! gbdev « GameBoy CPU »).

use std::sync::atomic::{AtomicU32, Ordering};

use crate::cpu::flags::Flags;
use crate::cpu::CPU;
use crate::mmu::MMU;

// ---------------------------------------------------------------------------
// Accès aux registres
// ---------------------------------------------------------------------------

/// Lit un registre 8 bits (0=B, 1=C, 2=D, 3=E, 4=H, 5=L, 6=(HL), 7=A).
fn get_reg8(cpu: &CPU, mmu: &MMU, idx: u8) -> u8 {
    match idx {
        0 => cpu.b,
        1 => cpu.c,
        2 => cpu.d,
        3 => cpu.e,
        4 => cpu.h,
        5 => cpu.l,
        6 => mmu.read(cpu.hl()),
        _ => cpu.a,
    }
}

/// Écrit un registre 8 bits (0=B, …, 6=(HL), 7=A).
fn set_reg8(cpu: &mut CPU, mmu: &mut MMU, idx: u8, value: u8) {
    match idx {
        0 => cpu.b = value,
        1 => cpu.c = value,
        2 => cpu.d = value,
        3 => cpu.e = value,
        4 => cpu.h = value,
        5 => cpu.l = value,
        6 => mmu.write(cpu.hl(), value),
        _ => cpu.a = value,
    }
}

/// Lit un registre 16 bits (0=BC, 1=DE, 2=HL, 3=SP).
fn reg16(cpu: &CPU, idx: u8) -> u16 {
    match idx {
        0 => u16::from_be_bytes([cpu.b, cpu.c]),
        1 => u16::from_be_bytes([cpu.d, cpu.e]),
        2 => cpu.hl(),
        _ => cpu.sp,
    }
}

/// Écrit un registre 16 bits (0=BC, 1=DE, 2=HL, 3=SP).
fn set_reg16(cpu: &mut CPU, idx: u8, value: u16) {
    let [hi, lo] = value.to_be_bytes();
    match idx {
        0 => {
            cpu.b = hi;
            cpu.c = lo;
        }
        1 => {
            cpu.d = hi;
            cpu.e = lo;
        }
        2 => {
            cpu.h = hi;
            cpu.l = lo;
        }
        _ => cpu.sp = value,
    }
}

/// Lit un immédiat 16 bits little-endian à l'adresse `at` (après l'opcode).
fn read_a16(mmu: &MMU, at: u16) -> u16 {
    u16::from_le_bytes([mmu.read(at), mmu.read(at + 1)])
}

/// PUSH d'une valeur 16 bits (octet haut à SP-1, octet bas à SP-2).
fn push16(cpu: &mut CPU, mmu: &mut MMU, value: u16) {
    let sp = cpu.sp.wrapping_sub(1);
    mmu.write(sp, (value >> 8) as u8);
    let sp = sp.wrapping_sub(1);
    mmu.write(sp, (value & 0xFF) as u8);
    cpu.sp = sp;
}

/// POP d'une valeur 16 bits (octet bas d'abord, SP += 2).
fn pop16(cpu: &mut CPU, mmu: &mut MMU) -> u16 {
    let lo = mmu.read(cpu.sp);
    let hi = mmu.read(cpu.sp.wrapping_add(1));
    let value = u16::from_be_bytes([hi, lo]);
    cpu.sp = cpu.sp.wrapping_add(2);
    value
}

// ---------------------------------------------------------------------------
// ALU 8 bits (ADD / ADC / SUB / SBC / AND / XOR / OR / CP)
// ---------------------------------------------------------------------------

/// Opération ALU sur l'accumulateur.
#[derive(Copy, Clone)]
pub enum Alu8 {
    Add,
    Adc,
    Sub,
    Sbc,
    And,
    Xor,
    Or,
    Cp,
}

impl Alu8 {
    /// Décodage des 3 bits bas d'un opcode de groupe (0x80-0xBF ou n8 group).
    const fn from_bits(bits: u8) -> Self {
        match bits {
            0 => Self::Add,
            1 => Self::Adc,
            2 => Self::Sub,
            3 => Self::Sbc,
            4 => Self::And,
            5 => Self::Xor,
            6 => Self::Or,
            _ => Self::Cp,
        }
    }

    fn is_sub(self) -> bool {
        matches!(self, Self::Sub | Self::Sbc | Self::Cp)
    }

    /// Calcule (résultat, carry, half carry) sans effet de bord.
    /// `carry_in` est le drapeau C avant l'opération (utilisé par ADC/SBC).
    fn compute(self, a: u8, operand: u8, carry_in: bool) -> (u8, bool, bool) {
        match self {
            Self::Add => {
                let (sum, c) = a.overflowing_add(operand);
                (sum, c, (a & 0x0F) + (operand & 0x0F) > 0x0F)
            }
            Self::Adc => {
                let cin = u8::from(carry_in);
                let (sum1, c1) = a.overflowing_add(operand);
                let (sum, c2) = sum1.overflowing_add(cin);
                (sum, c1 || c2, (a & 0x0F) + (operand & 0x0F) + cin > 0x0F)
            }
            Self::Sub => {
                let (diff, c) = a.overflowing_sub(operand);
                (diff, c, (a & 0x0F) < (operand & 0x0F))
            }
            Self::Sbc => {
                let cin = u8::from(carry_in);
                let (diff1, c1) = a.overflowing_sub(operand);
                let (diff, c2) = diff1.overflowing_sub(cin);
                (
                    diff,
                    c1 || c2,
                    ((a & 0x0F) as i16 - (operand & 0x0F) as i16 - cin as i16) < 0,
                )
            }
            Self::And => (a & operand, false, false),
            Self::Xor => (a ^ operand, false, false),
            Self::Or => (a | operand, false, false),
            Self::Cp => {
                let (diff, c) = a.overflowing_sub(operand);
                (diff, c, (a & 0x0F) < (operand & 0x0F))
            }
        }
    }

    /// Exécute `op r8` (l'opérande est déjà chargé) : met à jour A (sauf pour CP)
    /// et les drapeaux Z N H C (N=1 pour SUB/SBC/CP, H/C selon l'arithmétique).
    fn apply(&self, cpu: &mut CPU, operand: u8) {
        // Le drapeau C doit être lu AVANT de le réécrire (ADC/SBC).
        let carry_in = cpu.flags().contains(Flags::C);
        let (res, c, h) = self.compute(cpu.a, operand, carry_in);
        if !matches!(self, Self::Cp) {
            cpu.a = res;
        }
        let mut f = Flags::empty();
        if res == 0 {
            f |= Flags::Z;
        }
        if self.is_sub() {
            f |= Flags::N;
        }
        if h {
            f |= Flags::H;
        }
        if c {
            f |= Flags::C;
        }
        cpu.set_flags(f);
    }
}

// ---------------------------------------------------------------------------
// INC / DEC
// ---------------------------------------------------------------------------

/// INC r8 (4 T-cycles, 12 si (HL)) : Z=(v==0), N=0, H=(v & $0F)==$00, C inchangé.
fn inc8(cpu: &mut CPU, mmu: &mut MMU, idx: u8) -> u32 {
    let value = get_reg8(cpu, mmu, idx).wrapping_add(1);
    set_reg8(cpu, mmu, idx, value);
    let c = cpu.flags().contains(Flags::C);
    let mut f = Flags::empty();
    if value == 0 {
        f |= Flags::Z;
    }
    // Half carry : débordement du bit 3 (la moitié basse passe de $0F à $00).
    if (value & 0x0F) == 0 {
        f |= Flags::H;
    }
    if c {
        f |= Flags::C;
    }
    cpu.set_flags(f);
    if idx == 6 {
        12
    } else {
        4
    }
}

/// DEC r8 (4 T-cycles, 12 si (HL)) : Z=(v==0), N=1, H=(v & $0F)==$0F, C inchangé.
fn dec8(cpu: &mut CPU, mmu: &mut MMU, idx: u8) -> u32 {
    let value = get_reg8(cpu, mmu, idx).wrapping_sub(1);
    set_reg8(cpu, mmu, idx, value);
    let c = cpu.flags().contains(Flags::C);
    let mut f = Flags::N;
    if value == 0 {
        f |= Flags::Z;
    }
    // Half carry : emprunt vers le bit 4 (la moitié basse passe de $00 à $0F).
    if (value & 0x0F) == 0x0F {
        f |= Flags::H;
    }
    if c {
        f |= Flags::C;
    }
    cpu.set_flags(f);
    if idx == 6 {
        12
    } else {
        4
    }
}

/// INC rr / DEC rr (8 T-cycles, pas de drapeaux).
fn inc_dec16(cpu: &mut CPU, idx: u8, delta: i16) -> u32 {
    set_reg16(cpu, idx, (reg16(cpu, idx) as i16).wrapping_add(delta) as u16);
    8
}

/// ADD HL, rr (16 T-cycles) : N=0, H et C selon l'addition 16 bits ; Z n'est PAS modifié.
fn add_hl(cpu: &mut CPU, operand: u16) -> u32 {
    let hl = cpu.hl();
    let sum = (hl as u32).wrapping_add(operand as u32);
    let mut f = Flags::empty(); // N=0
    if (sum & 0x1_0000) != 0 {
        f |= Flags::C;
    }
    if ((hl as u32 & 0x0FFF) + (operand as u32 & 0x0FFF)) > 0x0FFF {
        f |= Flags::H;
    }
    // Z est inchangé par ADD HL, rr.
    if cpu.flags().contains(Flags::Z) {
        f |= Flags::Z;
    }
    cpu.set_flags(f);
    set_reg16(cpu, 2, sum as u16);
    16
}

/// ADD SP, e8 (24 T-cycles) : Z=0, N=0 ; H et C calculés sur l'addition non signée de
/// l'octet bas de SP avec `e8` (interprété comme un octet non signé).
fn add_sp(cpu: &mut CPU, e8: i8) -> u32 {
    let sp_lo = (cpu.sp & 0xFF) as u32;
    let e8_u8 = (e8 as i16) as u32 & 0xFF; // interprétation non signée de l'octet
    let sum8 = sp_lo.wrapping_add(e8_u8);
    let mut f = Flags::empty(); // Z=0, N=0
    if (sum8 & 0x100) != 0 {
        f |= Flags::C;
    }
    if ((sp_lo & 0x0F) + (e8_u8 & 0x0F)) > 0x0F {
        f |= Flags::H;
    }
    cpu.set_flags(f);
    // e8 est ajouté en arithmétique signée (i8) : modulo 2^16, `e8 as i16 as u16`
    // donne exactement SP + e8 (ex. $FF = -1 → SP-1).
    cpu.sp = cpu.sp.wrapping_add(e8 as i16 as u16);
    24
}

/// LD HL, SP+e8 (12 T-cycles) : Z=0, N=0 ; H et C calculés sur l'addition non signée de
/// l'octet bas de SP avec `e8` — comportement vérifié sur le matériel réel (le CPU fait
/// d'abord une addition 8 bits dans l'ALU qui produit les drapeaux, puis étend à 16 bits).
fn ld_hl_sp(cpu: &mut CPU, e8: i8) -> u32 {
    let sp_lo = (cpu.sp & 0xFF) as u32;
    let e8_u8 = (e8 as i16) as u32 & 0xFF; // interprétation non signée de l'octet
    let sum8 = sp_lo.wrapping_add(e8_u8);
    let mut f = Flags::empty(); // Z=0, N=0
    if (sum8 & 0x100) != 0 {
        f |= Flags::C;
    }
    if ((sp_lo & 0x0F) + (e8_u8 & 0x0F)) > 0x0F {
        f |= Flags::H;
    }
    cpu.set_flags(f);
    set_reg16(cpu, 2, (cpu.sp as i16).wrapping_add(e8 as i16) as u16);
    12
}

// ---------------------------------------------------------------------------
// DAA / CPL / SCF / CCF / rotations
// ---------------------------------------------------------------------------

/// DAA (4 T-cycles) : ajuste A en BCD après ADD/ADC/SUB/SBC.
/// N est conservé, H est toujours effacé, C selon l'ajustement des dizaines.
fn daa(cpu: &mut CPU) {
    let a = cpu.a;
    let flags = cpu.flags();
    let is_sub = flags.contains(Flags::N);
    let (sign6, sign60): (i16, i16) = if is_sub {
        (-6, -0x60)
    } else {
        (6, 0x60)
    };
    let mut v = a as i16;
    if (a & 0x0F) > 0x09 || flags.contains(Flags::H) {
        v += sign6;
    }
    let carry = (v as u8 & 0xF0) > 0x90 || flags.contains(Flags::C);
    if carry {
        v += sign60;
    }
    cpu.a = v as u8;
    let mut f = if is_sub { Flags::N } else { Flags::empty() };
    if v as u8 == 0 {
        f |= Flags::Z;
    }
    if carry {
        f |= Flags::C;
    }
    cpu.set_flags(f);
}

/// SCF (4 T-cycles) : C=1, N=0, H=0, Z inchangé.
fn scf(cpu: &mut CPU) {
    let flags = cpu.flags();
    let mut f = Flags::C;
    if flags.contains(Flags::Z) {
        f |= Flags::Z;
    }
    cpu.set_flags(f);
}

/// CCF (4 T-cycles) : C=~C, N=0, H=0, Z inchangé.
fn ccf(cpu: &mut CPU) {
    let flags = cpu.flags();
    let mut f = Flags::empty();
    if flags.contains(Flags::Z) {
        f |= Flags::Z;
    }
    if !flags.contains(Flags::C) {
        f |= Flags::C;
    }
    cpu.set_flags(f);
}

/// RLCA (8 T-cycles) : rotation gauche d'A à travers le carry ; Z toujours effacé.
fn rlca(cpu: &mut CPU) -> u32 {
    let carry = cpu.a & 0x80;
    cpu.a = (cpu.a << 1) | (carry >> 7);
    let f = if carry != 0 { Flags::C } else { Flags::empty() };
    cpu.set_flags(f);
    8
}

/// RRCA (8 T-cycles) : rotation droite d'A à travers le carry ; Z toujours effacé.
fn rrca(cpu: &mut CPU) -> u32 {
    let carry = cpu.a & 0x01;
    cpu.a = (cpu.a >> 1) | (carry << 7);
    let f = if carry != 0 { Flags::C } else { Flags::empty() };
    cpu.set_flags(f);
    8
}

/// RLA (8 T-cycles) : rotation gauche, C entre par le drapeau ; Z toujours effacé.
fn rla(cpu: &mut CPU) -> u32 {
    let c_in = u8::from(cpu.flags().contains(Flags::C));
    let carry = cpu.a & 0x80;
    cpu.a = (cpu.a << 1) | c_in;
    let f = if carry != 0 { Flags::C } else { Flags::empty() };
    cpu.set_flags(f);
    8
}

/// RRA (8 T-cycles) : rotation droite, C entre par le drapeau ; Z toujours effacé.
fn rra(cpu: &mut CPU) -> u32 {
    let c_in = if cpu.flags().contains(Flags::C) { 0x80u8 } else { 0 };
    let carry = cpu.a & 0x01;
    cpu.a = (cpu.a >> 1) | c_in;
    let f = if carry != 0 { Flags::C } else { Flags::empty() };
    cpu.set_flags(f);
    8
}

// ---------------------------------------------------------------------------
// Branchement
// ---------------------------------------------------------------------------

/// JR e8 / JR cond,e8 : `cpu.pc` pointe sur l'octet e8 ; 12 T-cycles si pris, 4 sinon.
fn jr(cpu: &mut CPU, e8: i8, taken: bool) -> u32 {
    if taken {
        // Cible = adresse de l'instruction suivante + décalage signé (l'octet e8 est à `cpu.pc`).
        cpu.pc = (cpu.pc as i16).wrapping_add(e8 as i16 + 1) as u16;
        12
    } else {
        cpu.pc = cpu.pc.wrapping_add(1); // passe l'octet e8
        4
    }
}

/// CALL a16 / CALL cond,a16 : 24 T-cycles si pris, 12 sinon. `cpu.pc` pointe sur le
/// premier octet de l'immédiat ; l'adresse de retour est celle qui suit l'instruction.
fn call(cpu: &mut CPU, mmu: &mut MMU, a16: u16, taken: bool) -> u32 {
    let after = cpu.pc.wrapping_add(2); // adresse de retour : après l'instruction
    if taken {
        push16(cpu, mmu, after);
        cpu.pc = a16;
        24
    } else {
        cpu.pc = after;
        12
    }
}

/// RET cond : 20 T-cycles si pris, 8 sinon ; pas de drapeaux.
fn ret_cond(cpu: &mut CPU, mmu: &mut MMU, taken: bool) -> u32 {
    if taken {
        cpu.pc = pop16(cpu, mmu);
        20
    } else {
        8
    }
}

/// RETI (16 T-cycles) : comme RET mais réactive IME.
fn reti(cpu: &mut CPU, mmu: &mut MMU) -> u32 {
    let return_addr = pop16(cpu, mmu);
    cpu.ime = true; // le drapeau IF correspondant a déjà été effacé à l'acceptation de l'interruption

    log::debug!(
        "[CPU] RETI: return_addr=${:04X}, SP=${:04X}, IME re-enabled, IF={:02X}",
        return_addr, cpu.sp, mmu.io[0x0F],
    );

    cpu.pc = return_addr;
    16
}

/// JP cond,a16 : 16 T-cycles si pris, 10 sinon ; pas de drapeaux. `cpu.pc` pointe sur
/// le premier octet de l'immédiat.
fn jp_cond(cpu: &mut CPU, a16: u16, taken: bool) -> u32 {
    if taken {
        cpu.pc = a16;
        16
    } else {
        cpu.pc = cpu.pc.wrapping_add(2); // passe les deux octets de l'immédiat
        10
    }
}

/// RST n8 (16 T-cycles) : pousse le PC puis saute à n8 << 3.
fn rst(cpu: &mut CPU, mmu: &mut MMU, n7: u8) -> u32 {
    push16(cpu, mmu, cpu.pc);
    cpu.pc = (n7 as u16) << 3;
    16
}

// ---------------------------------------------------------------------------
// Exécution du jeu d'instructions complet (0x00-0xFF)
// ---------------------------------------------------------------------------

/// Évalue une condition (0=NZ, 1=Z, 2=NC, 3=C).
fn cond_met(cpu: &CPU, cond: u8) -> bool {
    let f = cpu.flags();
    match cond {
        0 => !f.contains(Flags::Z), // NZ
        1 => f.contains(Flags::Z),  // Z
        2 => !f.contains(Flags::C), // NC
        _ => f.contains(Flags::C),  // C
    }
}

/// LD r8, r8 (4 T-cycles, 8 si la destination est (HL)) ; pas de drapeaux.
fn ld_r8(cpu: &mut CPU, mmu: &mut MMU, dst: u8, src: u8) -> u32 {
    set_reg8(cpu, mmu, dst, get_reg8(cpu, mmu, src));
    if dst == 6 { 8 } else { 4 }
}

/// LD r16, n16 (12 T-cycles) ; pas de drapeaux. L'immédiat est à `cpu.pc`.
fn ld_r16_imm(cpu: &mut CPU, mmu: &mut MMU, idx: u8) -> u32 {
    set_reg16(cpu, idx, read_a16(mmu, cpu.pc));
    cpu.pc = cpu.pc.wrapping_add(2); // passe l'immédiat n16
    12
}

/// LD r8, n8 (8 T-cycles) ; pas de drapeaux. L'immédiat est à `cpu.pc`.
fn ld_r8_imm(cpu: &mut CPU, mmu: &mut MMU, dst: u8) -> u32 {
    set_reg8(cpu, mmu, dst, mmu.read(cpu.pc));
    cpu.pc = cpu.pc.wrapping_add(1); // passe l'immédiat n8
    8
}

/// LD (r16), A / LD A, (r16) (8 T-cycles) ; pas de drapeaux.
fn ld_mem_r16(cpu: &mut CPU, mmu: &mut MMU, idx: u8, to_mem: bool) -> u32 {
    let addr = reg16(cpu, idx);
    if to_mem {
        mmu.write(addr, cpu.a);
    } else {
        cpu.a = mmu.read(addr);
    }
    8
}

/// LD (HL+), A / LD A, (HL+) (12 T-cycles) ; HL incrémenté après l'accès.
fn ld_hl_inc(cpu: &mut CPU, mmu: &mut MMU, to_mem: bool) -> u32 {
    let addr = cpu.hl();
    if to_mem {
        mmu.write(addr, cpu.a);
    } else {
        cpu.a = mmu.read(addr);
    }
    set_reg16(cpu, 2, addr.wrapping_add(1));
    12
}

/// LD (HL-), A / LD A, (HL-) (12 T-cycles) ; HL décrémenté après l'accès.
fn ld_hl_dec(cpu: &mut CPU, mmu: &mut MMU, to_mem: bool) -> u32 {
    let addr = cpu.hl();
    if to_mem {
        mmu.write(addr, cpu.a);
    } else {
        cpu.a = mmu.read(addr);
    }
    set_reg16(cpu, 2, addr.wrapping_sub(1));
    12
}

/// LDH [a8], A / LDH A, [a8] (8 T-cycles) : accès à $FF00+a8. L'immédiat est à `cpu.pc`.
fn ldh_a8(cpu: &mut CPU, mmu: &mut MMU, to_mem: bool) -> u32 {
    let a8 = mmu.read(cpu.pc);
    cpu.pc = cpu.pc.wrapping_add(1); // passe l'immédiat n8
    let addr = 0xFF00u16.wrapping_add(a8 as u16);
    if to_mem {
        mmu.write(addr, cpu.a);
    } else {
        cpu.a = mmu.read(addr);
    }
    8
}

/// LDH [C], A / LDH A, [C] (12 T-cycles) : accès à $FF00+C.
fn ldh_c(cpu: &mut CPU, mmu: &mut MMU, to_mem: bool) -> u32 {
    let addr = 0xFF00u16.wrapping_add(cpu.c as u16);
    if to_mem {
        mmu.write(addr, cpu.a);
    } else {
        cpu.a = mmu.read(addr);
    }
    12
}

/// PUSH r16stk (16 T-cycles) ; pas de drapeaux. idx 3 = AF.
fn push_r16(cpu: &mut CPU, mmu: &mut MMU, idx: u8) -> u32 {
    let value = if idx == 3 { cpu.af() } else { reg16(cpu, idx) };
    push16(cpu, mmu, value);
    16
}

/// POP r16stk (12 T-cycles) ; pas de drapeaux. idx 3 = AF (F reçoit les bits 7..4).
fn pop_r16(cpu: &mut CPU, mmu: &mut MMU, idx: u8) -> u32 {
    let value = pop16(cpu, mmu);
    if idx == 3 {
        cpu.a = (value >> 8) as u8;
        cpu.set_flags(Flags::from_bits_truncate(value as u8));
    } else {
        set_reg16(cpu, idx, value);
    }
    12
}

/// HALT (4 T-cycles) : le CPU s'arrête jusqu'à une interruption pendante.
fn halt(cpu: &mut CPU, mmu: &MMU) -> u32 {
    cpu.halted = true;
    log::debug!(
        "[CPU] HALT entered: PC=${:04X}, IF={:02X}, IE={:02X}",
        cpu.pc, mmu.io[0x0F], mmu.ie,
    );
    4
}

/// STOP (n8) (4+4=8 T-cycles sur DMG) : l'octet n8 est lu mais ignoré.
fn stop(cpu: &mut CPU, mmu: &mut MMU) -> u32 {
    let _ = mmu.read(cpu.pc); // n8 : lu mais ignoré sur DMG
    cpu.pc = cpu.pc.wrapping_add(1); // passe l'octet n8
    8
}

/// CPL (4 T-cycles) : A=~A ; N=1, H=1, Z/C inchangés.
fn cpl(cpu: &mut CPU) -> u32 {
    let old = cpu.flags();
    cpu.a = !cpu.a;
    let mut f = Flags::N | Flags::H;
    if old.contains(Flags::Z) {
        f |= Flags::Z;
    }
    if old.contains(Flags::C) {
        f |= Flags::C;
    }
    cpu.set_flags(f);
    4
}

/// Exécute l'opcode déjà lu par `CPU::step` et renvoie les T-cycles consommés.
pub fn execute(cpu: &mut CPU, mmu: &mut MMU, opcode: u8) -> u32 {
    match opcode {
        // --- Block 0 (0x00-0x3F) : LD r16/mem, INC/DEC, rotations A, JR ---
        0x00 => 4,                                        // NOP (4 T-cycles), pas de drapeaux
        0x01 => ld_r16_imm(cpu, mmu, 0),                  // LD BC, n16 (12)
        0x02 => ld_mem_r16(cpu, mmu, 0, true),            // LD (BC), A (8)
        0x03 => inc_dec16(cpu, 0, 1),                     // INC BC (8)
        0x04 => inc8(cpu, mmu, 0),                        // INC B (4) : Z N H C
        0x05 => dec8(cpu, mmu, 0),                        // DEC B (4) : Z N=1 H C
        0x06 => ld_r8_imm(cpu, mmu, 0),                   // LD B, n8 (8)
        0x07 => rlca(cpu),                                // RLCA (8) : Z=0 N=0 H=0 C=résultat
        0x08 => {                                         // LD (a16), SP (20) : octet bas puis haut
            let a16 = read_a16(mmu, cpu.pc);
            mmu.write(a16, (cpu.sp & 0xFF) as u8);
            mmu.write(a16.wrapping_add(1), (cpu.sp >> 8) as u8);
            cpu.pc = cpu.pc.wrapping_add(2);
            20
        }
        0x09 => add_hl(cpu, reg16(cpu, 0)),               // ADD HL, BC (16) : N=0 H C, Z inchangé
        0x0A => ld_mem_r16(cpu, mmu, 0, false),          // LD A, (BC) (8)
        0x0B => inc_dec16(cpu, 0, -1),                    // DEC BC (8)
        0x0C => inc8(cpu, mmu, 1),                        // INC C (4) : Z N H C
        0x0D => dec8(cpu, mmu, 1),                        // DEC C (4) : Z N=1 H C
        0x0E => ld_r8_imm(cpu, mmu, 1),                   // LD C, n8 (8)
        0x0F => rrca(cpu),                                // RRCA (8) : Z=0 N=0 H=0 C=résultat

        0x10 => stop(cpu, mmu),                           // STOP (n8) (4+4=8 sur DMG)
        0x11 => ld_r16_imm(cpu, mmu, 1),                  // LD DE, n16 (12)
        0x12 => ld_mem_r16(cpu, mmu, 1, true),           // LD (DE), A (8)
        0x13 => inc_dec16(cpu, 1, 1),                     // INC DE (8)
        0x14 => inc8(cpu, mmu, 2),                        // INC D (4) : Z N H C
        0x15 => dec8(cpu, mmu, 2),                        // DEC D (4) : Z N=1 H C
        0x16 => ld_r8_imm(cpu, mmu, 2),                   // LD D, n8 (8)
        0x17 => rla(cpu),                                 // RLA (8) : Z=0 N=0 H=0 C=résultat
        0x18 => { let e8 = mmu.read(cpu.pc) as i8; jr(cpu, e8, true) }   // JR e8 (12/4)
        0x19 => add_hl(cpu, reg16(cpu, 1)),               // ADD HL, DE (16) : N=0 H C, Z inchangé
        0x1A => ld_mem_r16(cpu, mmu, 1, false),          // LD A, (DE) (8)
        0x1B => inc_dec16(cpu, 1, -1),                    // DEC DE (8)
        0x1C => inc8(cpu, mmu, 3),                        // INC E (4) : Z N H C
        0x1D => dec8(cpu, mmu, 3),                        // DEC E (4) : Z N=1 H C
        0x1E => ld_r8_imm(cpu, mmu, 3),                   // LD E, n8 (8)
        0x1F => rra(cpu),                                 // RRA (8) : Z=0 N=0 H=0 C=résultat

        0x20 => { let e8 = mmu.read(cpu.pc) as i8; jr(cpu, e8, cond_met(cpu, 0)) } // JR NZ,e8 (12/4)
        0x21 => ld_r16_imm(cpu, mmu, 2),                  // LD HL, n16 (12)
        0x22 => ld_hl_inc(cpu, mmu, true),                // LD (HL+), A (12)
        0x23 => inc_dec16(cpu, 2, 1),                     // INC HL (8)
        0x24 => inc8(cpu, mmu, 4),                        // INC H (4) : Z N H C
        0x25 => dec8(cpu, mmu, 4),                        // DEC H (4) : Z N=1 H C
        0x26 => ld_r8_imm(cpu, mmu, 4),                   // LD H, n8 (8)
        0x27 => { daa(cpu); 4 }                           // DAA (4) : Z N H C recalculés
        0x28 => { let e8 = mmu.read(cpu.pc) as i8; jr(cpu, e8, cond_met(cpu, 1)) } // JR Z,e8 (12/4)
        0x29 => add_hl(cpu, cpu.hl()),                    // ADD HL, HL (16) : N=0 H C, Z inchangé
        0x2A => ld_hl_dec(cpu, mmu, false),               // LD A, (HL-) (12)
        0x2B => inc_dec16(cpu, 2, -1),                    // DEC HL (8)
        0x2C => inc8(cpu, mmu, 5),                        // INC L (4) : Z N H C
        0x2D => dec8(cpu, mmu, 5),                        // DEC L (4) : Z N=1 H C
        0x2E => ld_r8_imm(cpu, mmu, 5),                   // LD L, n8 (8)
        0x2F => cpl(cpu),                                 // CPL (4) : N=1 H=1, Z/C inchangés

        0x30 => { let e8 = mmu.read(cpu.pc) as i8; jr(cpu, e8, cond_met(cpu, 2)) } // JR NC,e8 (12/4)
        0x31 => ld_r16_imm(cpu, mmu, 3),                  // LD SP, n16 (12)
        0x32 => ld_hl_inc(cpu, mmu, false),               // LD A, (HL+) (12)
        0x33 => inc_dec16(cpu, 3, 1),                     // INC SP (8)
        0x34 => inc8(cpu, mmu, 6),                        // INC (HL) (12) : Z N H C
        0x35 => dec8(cpu, mmu, 6),                        // DEC (HL) (12) : Z N=1 H C
        0x36 => ld_r8_imm(cpu, mmu, 6),                   // LD (HL), n8 (8)
        0x37 => { scf(cpu); 4 }                           // SCF (4) : N=1 H=1 C=1, Z inchangé
        0x38 => { let e8 = mmu.read(cpu.pc) as i8; jr(cpu, e8, cond_met(cpu, 3)) } // JR C,e8 (12/4)
        0x39 => add_hl(cpu, cpu.sp),                      // ADD HL, SP (16) : N=0 H C, Z inchangé
        0x3A => ld_hl_dec(cpu, mmu, true),                // LD (HL-), A (12)
        0x3B => inc_dec16(cpu, 3, -1),                    // DEC SP (8)
        0x3C => inc8(cpu, mmu, 7),                        // INC A (4) : Z N H C
        0x3D => dec8(cpu, mmu, 7),                        // DEC A (4) : Z N=1 H C
        0x3E => ld_r8_imm(cpu, mmu, 7),                   // LD A, n8 (8)
        0x3F => { ccf(cpu); 4 }                           // CCF (4) : N=1 H=1 C=~C, Z inchangé

        // --- Block 1 (0x40-0x7F) : LD r8, r8 + HALT ---
        0x40..=0x75 | 0x77..=0x7F => {                   // LD r8, r8 (4 T-cycles, 8 si dst=(HL))
            let dst = (opcode & 0x38) >> 3;
            let src = opcode & 0x07;
            ld_r8(cpu, mmu, dst, src)
        }
        0x76 => halt(cpu, mmu),                                // HALT (4) : le CPU s'arrête

        // --- Block 2 (0x80-0xBF) : ALU A, r8 (4 T-cycles) ---
        0x80..=0xBF => {
            let op = Alu8::from_bits((opcode & 0x38) >> 3);
            op.apply(cpu, get_reg8(cpu, mmu, opcode & 0x07));
            4
        }

        // --- Block 3 (0xC0-0xFF) : ALU A,n8, RET/RETI, JP/CALL/RST, POP/PUSH, CB, LDH/LD a16, DI/EI ---
        0xC0 => ret_cond(cpu, mmu, cond_met(cpu, 0)),    // RET NZ (20/8)
        0xC1 => pop_r16(cpu, mmu, 0),                    // POP BC (12)
        0xC2 => { let a16 = read_a16(mmu, cpu.pc); jp_cond(cpu, a16, cond_met(cpu, 0)) } // JP NZ,a16 (16/10)
        0xC3 => { let a16 = read_a16(mmu, cpu.pc); cpu.pc = a16; 16 } // JP a16 (16)
        0xC4 => { let a16 = read_a16(mmu, cpu.pc); call(cpu, mmu, a16, cond_met(cpu, 0)) } // CALL NZ,a16 (24/12)
        0xC5 => push_r16(cpu, mmu, 0),                   // PUSH BC (16)
        0xC6 => { let op = Alu8::from_bits((opcode & 0x38) >> 3); op.apply(cpu, mmu.read(cpu.pc)); cpu.pc = cpu.pc.wrapping_add(1); 8 } // ADD A,n8 (8)
        0xC7 | 0xCF | 0xD7 | 0xDF | 0xE7 | 0xEF | 0xF7 | 0xFF => rst(cpu, mmu, (opcode & 0x38) >> 3), // RST n8 (16)
        0xC8 => ret_cond(cpu, mmu, cond_met(cpu, 1)),    // RET Z (20/8)
        0xC9 => { cpu.pc = pop16(cpu, mmu); 16 },       // RET (16)
        0xCA => { let a16 = read_a16(mmu, cpu.pc); jp_cond(cpu, a16, cond_met(cpu, 1)) } // JP Z,a16 (16/10)
        0xCB => {                                        // CB prefix : sous-opcode à `cpu.pc`
            let sub = mmu.read(cpu.pc);
            cpu.pc = cpu.pc.wrapping_add(1);
            execute_cb(cpu, mmu, sub)
        }
        0xCC => { let a16 = read_a16(mmu, cpu.pc); call(cpu, mmu, a16, cond_met(cpu, 1)) } // CALL Z,a16 (24/12)
        0xCD => { let a16 = read_a16(mmu, cpu.pc); call(cpu, mmu, a16, true) } // CALL a16 (24)
        0xCE => { let op = Alu8::from_bits((opcode & 0x38) >> 3); op.apply(cpu, mmu.read(cpu.pc)); cpu.pc = cpu.pc.wrapping_add(1); 8 } // ADC A,n8 (8)

        0xD0 => ret_cond(cpu, mmu, cond_met(cpu, 2)),    // RET NC (20/8)
        0xD1 => pop_r16(cpu, mmu, 1),                    // POP DE (12)
        0xD2 => { let a16 = read_a16(mmu, cpu.pc); jp_cond(cpu, a16, cond_met(cpu, 2)) } // JP NC,a16 (16/10)
        0xD3 => 4,                                       // $D3 : opcode invalide (hard-lock sur le matériel)
        0xD4 => { let a16 = read_a16(mmu, cpu.pc); call(cpu, mmu, a16, cond_met(cpu, 2)) } // CALL NC,a16 (24/12)
        0xD5 => push_r16(cpu, mmu, 1),                   // PUSH DE (16)
        0xD6 => { let op = Alu8::from_bits((opcode & 0x38) >> 3); op.apply(cpu, mmu.read(cpu.pc)); cpu.pc = cpu.pc.wrapping_add(1); 8 } // SUB A,n8 (8)
        0xD8 => ret_cond(cpu, mmu, cond_met(cpu, 3)),    // RET C (20/8)
        0xD9 => reti(cpu, mmu),                          // RETI (16) : réactive IME
        0xDA => { let a16 = read_a16(mmu, cpu.pc); jp_cond(cpu, a16, cond_met(cpu, 3)) } // JP C,a16 (16/10)
        0xDB => 4,                                       // $DB : opcode invalide (hard-lock sur le matériel)
        0xDC => { let a16 = read_a16(mmu, cpu.pc); call(cpu, mmu, a16, cond_met(cpu, 3)) } // CALL C,a16 (24/12)
        0xDD => 4,                                       // $DD : opcode invalide (hard-lock sur le matériel)
        0xDE => { let op = Alu8::from_bits((opcode & 0x38) >> 3); op.apply(cpu, mmu.read(cpu.pc)); cpu.pc = cpu.pc.wrapping_add(1); 8 } // SBC A,n8 (8)

        0xE0 => ldh_a8(cpu, mmu, true),                  // LDH [n8], A (8)
        0xE1 => pop_r16(cpu, mmu, 2),                    // POP HL (12)
        0xE2 => ldh_c(cpu, mmu, true),                   // LDH [C], A (12)
        0xE3 | 0xE4 | 0xEB | 0xEC | 0xED => 4,          // opcodes invalides (hard-lock sur le matériel)
        0xE5 => push_r16(cpu, mmu, 2),                   // PUSH HL (16)
        0xE6 => { let op = Alu8::from_bits((opcode & 0x38) >> 3); op.apply(cpu, mmu.read(cpu.pc)); cpu.pc = cpu.pc.wrapping_add(1); 8 } // AND A,n8 (8)
        0xE8 => { let e8 = mmu.read(cpu.pc) as i8; cpu.pc = cpu.pc.wrapping_add(1); add_sp(cpu, e8) } // ADD SP, e8 (24) : Z=0 N=0 H C
        0xE9 => { cpu.pc = cpu.hl(); 16 },               // JP HL (16)
        0xEA => { let a16 = read_a16(mmu, cpu.pc); mmu.write(a16, cpu.a); cpu.pc = cpu.pc.wrapping_add(2); 16 } // LD (a16), A (16)
        0xEE => { let op = Alu8::from_bits((opcode & 0x38) >> 3); op.apply(cpu, mmu.read(cpu.pc)); cpu.pc = cpu.pc.wrapping_add(1); 8 } // XOR A,n8 (8)

        0xF0 => ldh_a8(cpu, mmu, false),                 // LDH A, [n8] (8)
        0xF1 => pop_r16(cpu, mmu, 3),                    // POP AF (12)
        0xF2 => ldh_c(cpu, mmu, false),                  // LDH A, [C] (12)
        0xF3 => { cpu.ime = false; cpu.ei_delay = 0; log::debug!("[CPU] DI: IME disabled"); 4 }, // DI (4) : IME désactivé immédiatement, annule tout retard d'EI en cours
        0xF4 | 0xFC | 0xFD => 4,                         // opcodes invalides (hard-lock sur le matériel)
        0xF5 => push_r16(cpu, mmu, 3),                   // PUSH AF (16)
        0xF6 => { let op = Alu8::from_bits((opcode & 0x38) >> 3); op.apply(cpu, mmu.read(cpu.pc)); cpu.pc = cpu.pc.wrapping_add(1); 8 } // OR A,n8 (8)
        0xF8 => { let e8 = mmu.read(cpu.pc) as i8; cpu.pc = cpu.pc.wrapping_add(1); ld_hl_sp(cpu, e8) } // LD HL, SP+e8 (12) : Z=0 N=0 H C
        0xF9 => { cpu.sp = cpu.hl(); 8 },                // LD SP, HL (8)
        0xFA => { let a16 = read_a16(mmu, cpu.pc); cpu.a = mmu.read(a16); cpu.pc = cpu.pc.wrapping_add(2); 16 } // LD A, (a16) (16)
        0xFB => { cpu.ei_delay = 2; log::debug!("[CPU] EI: IME will be enabled after next instruction"); 4 },                 // EI (4) : IME réactivé après l'instruction suivante (Pan Docs)
        0xFE => { let op = Alu8::from_bits((opcode & 0x38) >> 3); op.apply(cpu, mmu.read(cpu.pc)); cpu.pc = cpu.pc.wrapping_add(1); 8 }, // CP A,n8 (8)
    }
}

// ---------------------------------------------------------------------------
// Préfixe 0xCB : rotations, décalages, BIT/RES/SET
// ---------------------------------------------------------------------------

/// Drapeaux des rotations CB : Z=(v==0), N=0, H=0, C selon le bit décalé.
fn cb_flags(cpu: &mut CPU, result: u8, carry_out: bool) {
    let mut f = Flags::empty();
    if result == 0 {
        f |= Flags::Z;
    }
    if carry_out {
        f |= Flags::C;
    }
    cpu.set_flags(f);
}

/// RLC r8 (8 T-cycles, 16 si (HL)) : rotation gauche à travers le carry ; C=bit7 décalé.
fn rlc_reg(cpu: &mut CPU, mmu: &mut MMU, idx: u8) -> u32 {
    let value = get_reg8(cpu, mmu, idx);
    let result = value.rotate_left(1);
    set_reg8(cpu, mmu, idx, result);
    cb_flags(cpu, result, (value & 0x80) != 0);
    if idx == 6 { 16 } else { 8 }
}

/// RRC r8 (8 T-cycles, 16 si (HL)) : rotation droite à travers le carry ; C=bit0 décalé.
fn rrc_reg(cpu: &mut CPU, mmu: &mut MMU, idx: u8) -> u32 {
    let value = get_reg8(cpu, mmu, idx);
    let result = value.rotate_right(1);
    set_reg8(cpu, mmu, idx, result);
    cb_flags(cpu, result, (value & 0x01) != 0);
    if idx == 6 { 16 } else { 8 }
}

/// RL r8 (8 T-cycles, 16 si (HL)) : décalage gauche, l'ancien C entre par le bit 0 ; C=bit7.
fn rl_reg(cpu: &mut CPU, mmu: &mut MMU, idx: u8) -> u32 {
    let value = get_reg8(cpu, mmu, idx);
    let c_in = u8::from(cpu.flags().contains(Flags::C));
    let result = (value << 1) | c_in;
    set_reg8(cpu, mmu, idx, result);
    cb_flags(cpu, result, (value & 0x80) != 0);
    if idx == 6 { 16 } else { 8 }
}

/// RR r8 (8 T-cycles, 16 si (HL)) : décalage droit, l'ancien C entre par le bit 7 ; C=bit0.
fn rr_reg(cpu: &mut CPU, mmu: &mut MMU, idx: u8) -> u32 {
    let value = get_reg8(cpu, mmu, idx);
    let c_in = if cpu.flags().contains(Flags::C) { 0x80 } else { 0 };
    let result = (value >> 1) | c_in;
    set_reg8(cpu, mmu, idx, result);
    cb_flags(cpu, result, (value & 0x01) != 0);
    if idx == 6 { 16 } else { 8 }
}

/// SLA r8 (8 T-cycles, 16 si (HL)) : décalage gauche logique, bit 0 mis à 0 ; C=bit7 décalé.
fn sla_reg(cpu: &mut CPU, mmu: &mut MMU, idx: u8) -> u32 {
    let value = get_reg8(cpu, mmu, idx);
    set_reg8(cpu, mmu, idx, value << 1);
    cb_flags(cpu, value << 1, (value & 0x80) != 0);
    if idx == 6 { 16 } else { 8 }
}

/// SRA r8 (8 T-cycles, 16 si (HL)) : décalage droit arithmétique, bit 7 conservé ; C=bit0.
fn sra_reg(cpu: &mut CPU, mmu: &mut MMU, idx: u8) -> u32 {
    let value = get_reg8(cpu, mmu, idx);
    set_reg8(cpu, mmu, idx, (value >> 1) | (value & 0x80));
    cb_flags(cpu, (value >> 1) | (value & 0x80), (value & 0x01) != 0);
    if idx == 6 { 16 } else { 8 }
}

/// SWAP r8 (8 T-cycles, 16 si (HL)) : échange des deux nibbles ; Z=(v==0), N=0, H=0, C=0.
fn swap_reg(cpu: &mut CPU, mmu: &mut MMU, idx: u8) -> u32 {
    let value = get_reg8(cpu, mmu, idx);
    let result = value.swap_bytes();
    set_reg8(cpu, mmu, idx, result);
    cb_flags(cpu, result, false);
    if idx == 6 { 16 } else { 8 }
}

/// SRL r8 (8 T-cycles, 16 si (HL)) : décalage droit logique, bit 7 mis à 0 ; C=bit0 décalé.
fn srl_reg(cpu: &mut CPU, mmu: &mut MMU, idx: u8) -> u32 {
    let value = get_reg8(cpu, mmu, idx);
    set_reg8(cpu, mmu, idx, value >> 1);
    cb_flags(cpu, value >> 1, (value & 0x01) != 0);
    if idx == 6 { 16 } else { 8 }
}

/// BIT b,r8 (8 T-cycles, 16 si (HL)) : Z=(bit==0), N=0, H=1 ; C inchangé.
fn bit_reg(cpu: &mut CPU, mmu: &mut MMU, bit: u8, idx: u8) -> u32 {
    let value = get_reg8(cpu, mmu, idx);
    let mut f = Flags::N | Flags::H;
    if ((value >> bit) & 1) == 0 {
        f |= Flags::Z;
    }
    if cpu.flags().contains(Flags::C) {
        f |= Flags::C;
    }
    cpu.set_flags(f);
    if idx == 6 { 16 } else { 8 }
}

/// RES b,r8 (16 T-cycles, 24 si (HL)) : efface le bit ; pas de drapeaux.
fn res_reg(cpu: &mut CPU, mmu: &mut MMU, bit: u8, idx: u8) -> u32 {
    set_reg8(cpu, mmu, idx, get_reg8(cpu, mmu, idx) & !(1 << bit));
    if idx == 6 { 24 } else { 16 }
}

/// SET b,r8 (16 T-cycles, 24 si (HL)) : pose le bit ; pas de drapeaux.
fn set_reg(cpu: &mut CPU, mmu: &mut MMU, bit: u8, idx: u8) -> u32 {
    set_reg8(cpu, mmu, idx, get_reg8(cpu, mmu, idx) | (1 << bit));
    if idx == 6 { 24 } else { 16 }
}

/// Exécute le sous-opcode du préfixe CB déjà lu par `execute`.
///
/// Encodage (Pan Docs « CPU Instruction Set ») : bits 2-0 = registre ; pour les
/// décalages, l'opération est dans les bits 6-3 (RLC=0x00+r, RRC=0x08+r, RL=0x10+r,
/// RR=0x18+r, SLA=0x20+r, SRA=0x28+r, SWAP=0x30+r, SRL=0x38+r) ; pour BIT/RES/SET,
/// le groupe est dans les bits 7-6 (BIT=0x40+, RES=0x80+, SET=0xC0+) et l'indice de
/// bit dans les bits 5-3.
pub fn execute_cb(cpu: &mut CPU, mmu: &mut MMU, sub_opcode: u8) -> u32 {
    let reg_idx = sub_opcode & 0x07;
    match sub_opcode >> 3 {
        0 => rlc_reg(cpu, mmu, reg_idx), // RLC r8
        1 => rrc_reg(cpu, mmu, reg_idx), // RRC r8
        2 => rl_reg(cpu, mmu, reg_idx),  // RL r8
        3 => rr_reg(cpu, mmu, reg_idx),  // RR r8
        4 => sla_reg(cpu, mmu, reg_idx), // SLA r8
        5 => sra_reg(cpu, mmu, reg_idx), // SRA r8
        6 => swap_reg(cpu, mmu, reg_idx), // SWAP r8
        7 => srl_reg(cpu, mmu, reg_idx), // SRL r8
        8..=15 => bit_reg(cpu, mmu, (sub_opcode & 0x38) >> 3, reg_idx), // BIT b,r8
        16..=23 => res_reg(cpu, mmu, (sub_opcode & 0x38) >> 3, reg_idx), // RES b,r8
        _ => set_reg(cpu, mmu, (sub_opcode & 0x38) >> 3, reg_idx),       // SET b,r8
    }
}

// ---------------------------------------------------------------------------
// Interruptions (IF/IE) et HALT bug
// ---------------------------------------------------------------------------

/// Traite les interruptions pendantes avant la prochaine instruction.
///
/// Renvoie `Some(20)` si une interruption est servied : IME est désactivé, le bit IF
/// correspondant à l'interruption acceptée est effacé automatiquement par le matériel,
/// le PC est poussé sur la pile et le CPU saute vers le vecteur de l'interruption
/// pendante+activée de plus bas (V-Blank $40, LC3C $48, Timer $50, Serial $58).
///
/// HALT bug : si le CPU est en HALT et qu'une interruption est pendante (même si IME=false), l'état HALT est annulé ;
/// si IME est également vrai, l'interruption est servied.
pub fn handle_interrupts(cpu: &mut CPU, mmu: &mut MMU) -> Option<u32> {
    let pending = mmu.io[0x0F] & mmu.ie; // IF ($FF0F) & IE

    if cpu.halted && pending != 0 {
        // HALT bug : une interruption pendante sort du HALT même si IME est false.
        cpu.halted = false;
        log::debug!(
            "[CPU] HALT exited: pending={:02X}, IF={:02X}, IE={:02X}",
            pending, mmu.io[0x0F], mmu.ie,
        );
    }

    if !cpu.ime || pending == 0 {
        if !cpu.ime && pending != 0 {
            // Throttle : loggue une occurrence sur 4096 pour éviter le spam instruction par instruction,
            // tout en révélant où le CPU tourne avec IME éteint (PC constant = boucle bloquée).
            static IRQ_OFF_COUNT: AtomicU32 = AtomicU32::new(0);
            let n = IRQ_OFF_COUNT.fetch_add(1, Ordering::Relaxed) + 1;
            if n % 4096 == 1 {
                log::debug!(
                    "[CPU] Interrupts pending but IME=OFF: PC=${:04X}, opcode=${:02X}, pending={:02X}, IF={:02X}, IE={:02X}",
                    cpu.pc, mmu.read(cpu.pc), pending, mmu.io[0x0F], mmu.ie,
                );
            }
        }
        return None;
    }

    // ✅ On récupère à la fois l'adresse du vecteur ET le bit précis à effacer dans IF
    let (vector, bit_to_clear) = match pending {
        p if p & 0x01 != 0 => (0x40, 0x01), // V-Blank (IF bit 0)
        p if p & 0x02 != 0 => (0x48, 0x02), // LCDC / STAT (IF bit 1)
        p if p & 0x04 != 0 => (0x50, 0x04), // Timer (IF bit 2)
        _ => (0x58, 0x08),                  // Serial (IF bit 3)
    };

    log::debug!(
        "[CPU] INTERRUPT ACCEPTED: PC=${:04X} → ${:04X}, IF={:02X}, IE={:02X}, IME=ON→OFF",
        cpu.pc, vector, mmu.io[0x0F], mmu.ie,
    );

    // ✅ On efface le bit d'interruption dans le registre IF pour éviter la boucle infinie !
    mmu.io[0x0F] &= !bit_to_clear;

    cpu.ime = false;
    // ✅ Comportement matériel (Pan Docs « Interrupt Sources ») : l'adresse de retour est poussée sur la pile
    // avant le saut au vecteur — ici `cpu.pc` pointe sur la prochaine instruction à exécuter (fetch pas encore
    // fait), donc RETI repart exactement là où le code a été interrompu. Sans ce push, RETI poppe de la
    // poubelle en RAM ($0000) et le CPU reste bloqué dans une boucle d'interruptions.
    push16(cpu, mmu, cpu.pc);
    cpu.pc = vector;

    Some(20)
}

// ---------------------------------------------------------------------------
// Désassemblage
// ---------------------------------------------------------------------------

const REG8_NAMES: [&str; 8] = ["B", "C", "D", "E", "H", "L", "(HL)", "A"];
const REG16_NAMES: [&str; 4] = ["BC", "DE", "HL", "SP"];
const REG16STK_NAMES: [&str; 4] = ["BC", "DE", "HL", "AF"];

fn reg8_name(idx: u8) -> &'static str {
    REG8_NAMES[idx.min(7) as usize]
}

fn reg16_name(idx: u8) -> &'static str {
    REG16_NAMES[idx.min(3) as usize]
}

fn alu_name(bits: u8) -> &'static str {
    match bits {
        0 => "add",
        1 => "adc",
        2 => "sub",
        3 => "sbc",
        4 => "and",
        5 => "xor",
        6 => "or",
        _ => "cp",
    }
}

fn cb_mnemonic(sub: u8) -> String {
    let r8 = reg8_name(sub & 0x07);
    match sub >> 3 {
        0 => format!("rlc {}", r8),
        1 => format!("rrc {}", r8),
        2 => format!("rl {}", r8),
        3 => format!("rr {}", r8),
        4 => format!("sla {}", r8),
        5 => format!("sra {}", r8),
        6 => format!("swap {}", r8),
        7 => format!("srl {}", r8),
        8..=15 => format!("bit {}, {}", (sub & 0x38) >> 3, r8),
        16..=23 => format!("res {}, {}", (sub & 0x38) >> 3, r8),
        _ => format!("set {}, {}", (sub & 0x38) >> 3, r8),
    }
}

/// Désassemble l'instruction située à `CPU.pc` en mnémonique.
pub fn disasm(cpu: &CPU, mmu: &MMU) -> String {
    let opcode = mmu.read(cpu.pc);
    match opcode {
        0x00 => "nop".to_string(),
        0x76 => "halt".to_string(),
        0x10 => format!("stop ${:02X}", mmu.read(cpu.pc.wrapping_add(1))),

        // LD r16, n16 (l'immédiat est à pc+1)
        0x01 | 0x11 | 0x21 | 0x31 => format!(
            "ld {}, ${:04X}",
            reg16_name(opcode >> 4),
            read_a16(mmu, cpu.pc.wrapping_add(1))
        ),

        // LD (r16), A / LD A, (r16) et variantes HL±
        0x02 => "ld (bc), a".to_string(),
        0x0A => "ld a, (bc)".to_string(),
        0x12 => "ld (de), a".to_string(),
        0x1A => "ld a, (de)".to_string(),
        0x22 => "ld (hl+), a".to_string(),
        0x32 => "ld a, (hl+)".to_string(),
        0x2A => "ld a, (hl-)".to_string(),
        0x3A => "ld (hl-), a".to_string(),

        // INC/DEC r16
        0x03 | 0x13 | 0x23 | 0x33 => format!("inc {}", reg16_name(opcode >> 4)),
        0x0B | 0x1B | 0x2B | 0x3B => format!("dec {}", reg16_name(opcode >> 4)),

        // INC/DEC/LD r8, n8 (lignes 0-3)
        0x04 | 0x05 | 0x06 | 0x0C | 0x0D | 0x0E | 0x14 | 0x15 | 0x16 | 0x1C | 0x1D | 0x1E
        | 0x24 | 0x25 | 0x26 | 0x2C | 0x2D | 0x2E | 0x34 | 0x35 | 0x36 | 0x3C | 0x3D | 0x3E => {
            let r8 = reg8_name((opcode & 0x38) >> 3);
            match opcode & 0x07 {
                4 => format!("inc {}", r8),
                5 => format!("dec {}", r8),
                _ => format!("ld {}, ${:02X}", r8, mmu.read(cpu.pc.wrapping_add(1))),
            }
        }

        // Rotations A et instructions diverses des lignes 0-3
        0x07 => "rlca".to_string(),
        0x0F => "rrca".to_string(),
        0x17 => "rla".to_string(),
        0x1F => "rra".to_string(),
        0x27 => "daa".to_string(),
        0x2F => "cpl".to_string(),
        0x37 => "scf".to_string(),
        0x3F => "ccf".to_string(),

        // LD (a16), SP / ADD HL, r16
        0x08 => format!("ld (${ :04X}), sp", read_a16(mmu, cpu.pc.wrapping_add(1))),
        0x09 | 0x19 | 0x29 | 0x39 => format!(
            "add hl, {}",
            reg16_name(if opcode == 0x29 { 2 } else { opcode >> 4 })
        ),

        // JR e8 / JR cond,e8 (l'octet est à pc+1)
        0x18 | 0x20 | 0x28 | 0x30 | 0x38 => {
            let conds = ["nz, ", "z, ", "nc, ", "c, "];
            let cond = if opcode == 0x18 { "" } else { conds[(opcode - 0x20) as usize / 8] };
            format!(
                "jr {}${:04X}",
                cond,
                (cpu.pc.wrapping_add(1) as i16).wrapping_add((mmu.read(cpu.pc.wrapping_add(1)) as i8) as i16) as u16
            )
        }

        // LD r8, r8 (lignes 4-7, hors HALT)
        0x40..=0x75 | 0x77..=0x7F => format!(
            "ld {}, {}",
            reg8_name((opcode & 0x38) >> 3),
            reg8_name(opcode & 0x07)
        ),

        // ALU A, r8 / ALU A, n8
        0x80..=0xAF => format!(
            "{} a, {}",
            alu_name((opcode & 0x38) >> 3),
            reg8_name(opcode & 0x07)
        ),
        0xC6 | 0xCE | 0xD6 | 0xDE | 0xE6 | 0xEE | 0xF6 | 0xFE => format!(
            "{} a, ${:02X}",
            alu_name((opcode & 0x38) >> 3),
            mmu.read(cpu.pc.wrapping_add(1))
        ),

        // RET / RET cond / RETI
        0xC9 => "ret".to_string(),
        0xD9 => "reti".to_string(),
        0xC0 | 0xC8 | 0xD0 | 0xD8 => {
            format!("ret {}", ["nz", "z", "nc", "c"][(opcode - 0xC0) as usize / 8])
        }

        // JP a16 / JP cond,a16 / JP HL
        0xC3 => format!("jp ${:04X}", read_a16(mmu, cpu.pc.wrapping_add(1))),
        0xE9 => "jp hl".to_string(),
        0xC2 | 0xCA | 0xD2 | 0xDA => format!(
            "jp {}, ${:04X}",
            ["nz", "z", "nc", "c"][(opcode - 0xC2) as usize / 8],
            read_a16(mmu, cpu.pc.wrapping_add(1))
        ),

        // CALL a16 / CALL cond,a16
        0xCD => format!("call ${:04X}", read_a16(mmu, cpu.pc.wrapping_add(1))),
        0xC4 | 0xCC | 0xD4 | 0xDC => format!(
            "call {}, ${:04X}",
            ["nz", "z", "nc", "c"][(opcode - 0xC4) as usize / 8],
            read_a16(mmu, cpu.pc.wrapping_add(1))
        ),

        // RST n8
        0xC7 | 0xCF | 0xD7 | 0xDF | 0xE7 | 0xEF | 0xF7 | 0xFF => {
            format!("rst ${:02X}", ((opcode & 0x38) >> 3) * 8)
        }

        // PUSH/POP r16stk
        0xC5 | 0xD5 | 0xE5 | 0xF5 => {
            format!("push {}", REG16STK_NAMES[((opcode - 0xC5) as usize / 0x10).min(3)])
        }
        0xC1 | 0xD1 | 0xE1 | 0xF1 => {
            format!("pop {}", REG16STK_NAMES[((opcode - 0xC1) as usize / 0x10).min(3)])
        }

        // Préfixe CB (sous-opcode à pc+1)
        0xCB => format!("cb {}", cb_mnemonic(mmu.read(cpu.pc.wrapping_add(1)))),

        // LDH / LD a16 / ADD SP / DI / EI
        0xE0 => format!("ldh [${:02X}], a", mmu.read(cpu.pc.wrapping_add(1))),
        0xF0 => format!("ldh a, [${:02X}]", mmu.read(cpu.pc.wrapping_add(1))),
        0xE2 => "ldh [c], a".to_string(),
        0xF2 => "ldh a, [c]".to_string(),
        0xEA => format!("ld (${ :04X}), a", read_a16(mmu, cpu.pc.wrapping_add(1))),
        0xFA => format!("ld a, (${ :04X})", read_a16(mmu, cpu.pc.wrapping_add(1))),
        0xE8 => format!("add sp, ${:02X}", mmu.read(cpu.pc.wrapping_add(1))),
        0xF8 => format!("ld hl, sp+${:02X}", mmu.read(cpu.pc.wrapping_add(1))),
        0xF9 => "ld sp, hl".to_string(),
        0xF3 => "di".to_string(),
        0xFB => "ei".to_string(),

        // Opcodes invalides (hard-lock sur le matériel) et inconnus
        _ => format!("db ${:02X}", opcode),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mmu::MMU;

    /// MMU avec une ROM de $FF contenant `code` à $0100.
    fn rom_with(code: &[u8]) -> MMU {
        let mut rom = vec![0xFF; 0x4000];
        rom[0x0100..0x0100 + code.len()].copy_from_slice(code);
        let mut mmu = MMU::new();
        mmu.load_rom(rom);
        mmu
    }

    #[test]
    fn reset_state_matches_dmg_boot_rom_final_values() {
        let cpu = CPU::new();
        assert_eq!(cpu.a, 0x01);
        assert_eq!(cpu.f, 0xB0); // Z|H|C (le carry est actif au power-on)
        assert_eq!(cpu.b, 0x00);
        assert_eq!(cpu.c, 0x13);
        assert_eq!(cpu.d, 0x00);
        assert_eq!(cpu.e, 0xD8);
        assert_eq!(cpu.h, 0x01);
        assert_eq!(cpu.l, 0x4D);
        assert_eq!(cpu.sp, 0xFFFE);
        assert_eq!(cpu.pc, 0x0100); // cible du vecteur de reset
        assert!(!cpu.ime);
        assert!(!cpu.halted);
    }

    #[test]
    fn inc_dec8_half_carry_and_zero_flags() {
        let mut mmu = MMU::new();
        let mut cpu = CPU::new();

        // INC A : half carry quand la moitié basse passe de $0F à $00.
        cpu.a = 0x0F;
        assert_eq!(execute(&mut cpu, &mut mmu, 0x3C), 4);
        assert_eq!(cpu.a, 0x10);
        let f = cpu.flags();
        assert!(!f.contains(Flags::Z));
        assert!(f.contains(Flags::H));

        // INC A : Z posé au passage à $00 (et H aussi).
        cpu.a = 0xFF;
        execute(&mut cpu, &mut mmu, 0x3C);
        assert_eq!(cpu.a, 0x00);
        let f = cpu.flags();
        assert!(f.contains(Flags::Z));
        assert!(f.contains(Flags::H));

        // DEC A : half carry quand la moitié basse devient $0F.
        cpu.a = 0x10;
        assert_eq!(execute(&mut cpu, &mut mmu, 0x3D), 4);
        assert_eq!(cpu.a, 0x0F);
        let f = cpu.flags();
        assert!(!f.contains(Flags::Z));
        assert!(f.contains(Flags::N));
        assert!(f.contains(Flags::H));

        // DEC A : Z posé au passage à $00 ; H non posé ($00 ≠ $0F).
        cpu.a = 0x01;
        execute(&mut cpu, &mut mmu, 0x3D);
        assert_eq!(cpu.a, 0x00);
        let f = cpu.flags();
        assert!(f.contains(Flags::Z));
        assert!(!f.contains(Flags::H));

        // C est conservé par INC/DEC.
        cpu.set_flags(Flags::C);
        execute(&mut cpu, &mut mmu, 0x3C);
        assert!(cpu.flags().contains(Flags::C));

        // --- EXTENSION : Cas avec nibble haut non nul (vérification robuste du Half-Carry) ---

        // INC A : 0x1F + 1 = 0x20 (débordement du bit 3 vers le bit 4 → H=1, Z=0)
        cpu.a = 0x1F;
        cpu.set_flags(Flags::empty()); // Z=0, N=0, H=0, C=0
        assert_eq!(execute(&mut cpu, &mut mmu, 0x3C), 4); // INC A
        assert_eq!(cpu.a, 0x20);
        let f = cpu.flags();
        assert!(!f.contains(Flags::Z), "Z doit être 0 pour 0x20");
        assert!(!f.contains(Flags::N), "N doit être 0 pour INC");
        assert!(f.contains(Flags::H), "H doit être 1 (débordement 0x1F -> 0x20)");
        assert!(!f.contains(Flags::C), "C doit rester 0");

        // DEC A : 0x20 - 1 = 0x1F (emprunt sur le bit 3 depuis le bit 4 → H=1, N=1, Z=0)
        cpu.a = 0x20;
        cpu.set_flags(Flags::empty()); // Z=0, N=0, H=0, C=0
        assert_eq!(execute(&mut cpu, &mut mmu, 0x3D), 4); // DEC A
        assert_eq!(cpu.a, 0x1F);
        let f = cpu.flags();
        assert!(!f.contains(Flags::Z), "Z doit être 0 pour 0x1F");
        assert!(f.contains(Flags::N), "N doit être 1 pour DEC");
        assert!(f.contains(Flags::H), "H doit être 1 (emprunt 0x20 -> 0x1F)");
        assert!(!f.contains(Flags::C), "C doit rester 0");
    }

    #[test]
    fn adc_sbc_use_the_carry_flag() {
        let mut mmu = MMU::new();
        let mut cpu = CPU::new();

        // ADC A,B avec C posé : 0x7F + 0x80 + 1 = $100.
        cpu.a = 0x7F;
        cpu.b = 0x80;
        cpu.set_flags(Flags::C);
        assert_eq!(execute(&mut cpu, &mut mmu, 0x88), 4); // ADC A,B
        assert_eq!(cpu.a, 0x00);
        let f = cpu.flags();
        assert!(f.contains(Flags::Z));
        assert!(!f.contains(Flags::N));
        assert!(f.contains(Flags::H));
        assert!(f.contains(Flags::C));

        // ADC A,B avec C effacé : ni retenue ni half carry.
        cpu.a = 0x40;
        cpu.b = 0x3F;
        cpu.set_flags(Flags::empty());
        execute(&mut cpu, &mut mmu, 0x88);
        assert_eq!(cpu.a, 0x7F);
        let f = cpu.flags();
        assert!(!f.contains(Flags::H));
        assert!(!f.contains(Flags::C));

        // SBC A,B avec C posé : 0x50 - 0x60 - 1.
        cpu.a = 0x50;
        cpu.b = 0x60;
        cpu.set_flags(Flags::C);
        assert_eq!(execute(&mut cpu, &mut mmu, 0x98), 4); // SBC A,B
        assert_eq!(cpu.a, 0xEF);
        let f = cpu.flags();
        assert!(!f.contains(Flags::Z));
        assert!(f.contains(Flags::N));
        assert!(f.contains(Flags::H));
        assert!(f.contains(Flags::C));

        // SBC A,B avec C effacé : emprunt vers la moitié basse → H posé, pas de retenue.
        cpu.a = 0x74;
        cpu.b = 0x68;
        cpu.set_flags(Flags::empty());
        execute(&mut cpu, &mut mmu, 0x98);
        assert_eq!(cpu.a, 0x0C);
        let f = cpu.flags();
        assert!(f.contains(Flags::N));
        assert!(f.contains(Flags::H)); // $4 - $8 : emprunt du bit 3
        assert!(!f.contains(Flags::C));
    }

    #[test]
    fn add_hl_flags_and_z_unchanged() {
        let mut mmu = MMU::new();
        let mut cpu = CPU::new();
        cpu.h = 0xFF;
        cpu.l = 0xF0; // HL = $FFF0
        cpu.b = 0x00;
        cpu.c = 0x20; // BC = $0020
        cpu.set_flags(Flags::Z); // Z n'est PAS modifié par ADD HL, rr.

        assert_eq!(execute(&mut cpu, &mut mmu, 0x09), 16); // ADD HL,BC
        assert_eq!(cpu.hl(), 0x0010);
        let f = cpu.flags();
        assert!(f.contains(Flags::Z)); // inchangé
        assert!(!f.contains(Flags::N));
        assert!(f.contains(Flags::H)); // débordement du bit 12
        assert!(f.contains(Flags::C)); // débordement du bit 15

        // Sans aucun débordement : tous les drapeaux arithmétiques effacés.
        cpu.h = 0x00;
        cpu.l = 0x10;
        cpu.set_flags(Flags::empty());
        execute(&mut cpu, &mut mmu, 0x09);
        assert_eq!(cpu.hl(), 0x0030);
        assert_eq!(cpu.flags(), Flags::empty());
    }

    #[test]
    fn add_sp_unsigned_e8_flags() {
        let mut mmu = rom_with(&[0xE8, 0x10, 0xE8, 0xFF]); // ADD SP,$10 / ADD SP,-1 à $0100
        let mut cpu = CPU::new();
        cpu.sp = 0xFF00;
        cpu.pc = 0x0101; // pointe sur l'immédiat (comme après la lecture de l'opcode)

        assert_eq!(execute(&mut cpu, &mut mmu, 0xE8), 24);
        assert_eq!(cpu.sp, 0xFF10);
        assert_eq!(cpu.flags(), Flags::empty()); // Z=0, N=0, ni H ni C

        // Retenue sur l'octet bas : $F8 + $10 = $108.
        cpu.sp = 0xFFF8;
        cpu.pc = 0x0101; // re-pointe sur le premier immédiat (execute() l'avait passé)
        execute(&mut cpu, &mut mmu, 0xE8);
        assert_eq!(cpu.sp, 0x0008);
        let f = cpu.flags();
        assert!(f.contains(Flags::C));
        assert!(!f.contains(Flags::H)); // $08 + $10 : pas de débordement du bit 3

        // e8 est interprété en non signé pour les drapeaux, signé pour SP.
        cpu.sp = 0xFF00;
        cpu.pc = 0x0103; // pointe sur l'immédiat $FF (-1)
        execute(&mut cpu, &mut mmu, 0xE8);
        assert_eq!(cpu.sp, 0xFEFF); // -1 signé : $FF00 - 1
        let f = cpu.flags();
        assert!(!f.contains(Flags::C)); // non signé : $00 + $FF = $FF (pas de retenue)
        assert!(!f.contains(Flags::H));
    }

    #[test]
    fn ld_hl_sp_flags_like_add_sp() {
        let mut mmu = rom_with(&[0xF8, 0x11, 0xF8, 0xFF]); // LD HL,SP+$11 / LD HL,SP-1 à $0100
        let mut cpu = CPU::new();
        cpu.sp = 0xFF00;
        cpu.pc = 0x0101;

        assert_eq!(execute(&mut cpu, &mut mmu, 0xF8), 12); // LD HL,SP+$11
        assert_eq!(cpu.hl(), 0xFF11);
        assert_eq!(cpu.flags(), Flags::empty()); // Z=0 N=0, ni H ni C

        // Retenue sur l'octet bas : $F8 + $11 = $109.
        cpu.sp = 0xFFF8;
        cpu.pc = 0x0101;
        execute(&mut cpu, &mut mmu, 0xF8);
        assert_eq!(cpu.hl(), 0x0009);
        let f = cpu.flags();
        assert!(f.contains(Flags::C));
        assert!(!f.contains(Flags::H));

        // Half carry : $0F + $11 → la moitié basse déborde le bit 3.
        cpu.sp = 0x000F;
        cpu.pc = 0x0101;
        execute(&mut cpu, &mut mmu, 0xF8);
        assert_eq!(cpu.hl(), 0x0020);
        let f = cpu.flags();
        assert!(!f.contains(Flags::C)); // $0F + $11 = $20 : pas de retenue sur l'octet
        assert!(f.contains(Flags::H));

        // e8 négatif : drapeaux calculés en non signé, HL en signé.
        cpu.sp = 0xFF00;
        cpu.pc = 0x0103; // pointe sur l'immédiat $FF (-1)
        execute(&mut cpu, &mut mmu, 0xF8);
        assert_eq!(cpu.hl(), 0xFEFF);
        let f = cpu.flags();
        assert!(!f.contains(Flags::C));
        assert!(!f.contains(Flags::H));
    }

    #[test]
    fn jr_taken_and_not_taken_pc_and_cycles() {
        let mut mmu = rom_with(&[0x18, 0x05]); // JR +5 à $0100
        let mut cpu = CPU::new();
        cpu.pc = 0x0101; // pointe sur l'immédiat (comme après la lecture de l'opcode)

        assert_eq!(execute(&mut cpu, &mut mmu, 0x18), 12); // pris
        assert_eq!(cpu.pc, 0x0107); // $0101 + 1 + 5

        // JR NZ non pris (Z posé) : le PC passe l'immédiat.
        let mut mmu = rom_with(&[0x20, 0x05]);
        let mut cpu = CPU::new();
        cpu.pc = 0x0101;
        cpu.set_flags(Flags::Z);
        assert_eq!(execute(&mut cpu, &mut mmu, 0x20), 4); // non pris
        assert_eq!(cpu.pc, 0x0102);

        // JR NZ pris (Z effacé).
        let mut cpu = CPU::new();
        cpu.pc = 0x0101;
        cpu.set_flags(Flags::empty()); // un CPU neuf a Z posé (F=$B0)
        assert_eq!(execute(&mut cpu, &mut mmu, 0x20), 12);
        assert_eq!(cpu.pc, 0x0107);

        // Décalage négatif.
        let mut mmu = rom_with(&[0x18, 0xFB]); // JR -5
        let mut cpu = CPU::new();
        cpu.pc = 0x0101;
        assert_eq!(execute(&mut cpu, &mut mmu, 0x18), 12);
        assert_eq!(cpu.pc, 0x00FD); // $0102 + (-5) : l'immédiat est à $0101
    }

    #[test]
    fn jp_call_ret_conditional_pc_and_cycles() {
        let mut mmu = rom_with(&[0xCA, 0x34, 0x12]); // JP Z,$1234 à $0100
        let mut cpu = CPU::new();
        cpu.pc = 0x0101;
        cpu.set_flags(Flags::empty()); // un CPU neuf a Z posé (F=$B0)

        // Non pris (Z effacé) : le PC passe les deux octets de l'adresse.
        assert_eq!(execute(&mut cpu, &mut mmu, 0xCA), 10);
        assert_eq!(cpu.pc, 0x0103);

        // Pris (Z posé).
        let mut cpu = CPU::new();
        cpu.pc = 0x0101;
        cpu.set_flags(Flags::Z);
        assert_eq!(execute(&mut cpu, &mut mmu, 0xCA), 16);
        assert_eq!(cpu.pc, 0x1234);

        // CALL a16 : pousse l'adresse de retour SUIVANT l'instruction.
        let mut mmu = rom_with(&[0xCD, 0x78, 0xAB]); // CALL $AB78 à $0100
        let mut cpu = CPU::new();
        cpu.pc = 0x0101;
        assert_eq!(execute(&mut cpu, &mut mmu, 0xCD), 24);
        assert_eq!(cpu.pc, 0xAB78);
        assert_eq!(cpu.sp, 0xFFFC);
        assert_eq!(mmu.read(0xFFFD), 0x01); // octet haut à SP+1
        assert_eq!(mmu.read(0xFFFC), 0x03); // octet bas à SP

        // RET : pop de l'adresse de retour.
        assert_eq!(execute(&mut cpu, &mut mmu, 0xC9), 16);
        assert_eq!(cpu.pc, 0x0103);
        assert_eq!(cpu.sp, 0xFFFE);

        // RETI : pop + réactivation de IME.
        let mut mmu = rom_with(&[0xCD, 0x78, 0xAB]);
        let mut cpu = CPU::new();
        cpu.pc = 0x0101;
        execute(&mut cpu, &mut mmu, 0xCD); // CALL $AB78 → SP=FFFC
        assert_eq!(execute(&mut cpu, &mut mmu, 0xD9), 16); // RETI
        assert_eq!(cpu.pc, 0x0103);
        assert!(cpu.ime);
    }

    #[test]
    fn cb_prefix_flags_and_cycles() {
        let mut mmu = MMU::new();
        let mut cpu = CPU::new();

        // RLC B : $81 → $03, C posé (bit 7 décalé).
        cpu.b = 0x81;
        assert_eq!(execute_cb(&mut cpu, &mut mmu, 0x00), 8);
        assert_eq!(cpu.b, 0x03);
        let f = cpu.flags();
        assert!(!f.contains(Flags::Z));
        assert!(f.contains(Flags::C));

        // RLC B : $00 → $00, Z posé, C effacé.
        cpu.b = 0x00;
        execute_cb(&mut cpu, &mut mmu, 0x00);
        let f = cpu.flags();
        assert!(f.contains(Flags::Z));
        assert!(!f.contains(Flags::C));

        // SRL A : $81 → $40, C posé (bit 0 décalé).
        cpu.a = 0x81;
        assert_eq!(execute_cb(&mut cpu, &mut mmu, 0x3F), 8); // SRL A
        assert_eq!(cpu.a, 0x40);
        let f = cpu.flags();
        assert!(!f.contains(Flags::Z));
        assert!(f.contains(Flags::C));

        // BIT b,r8 : Z=(bit==0), N=1, H=1 ; C inchangé.
        cpu.b = 0x80;
        cpu.set_flags(Flags::C);
        assert_eq!(execute_cb(&mut cpu, &mut mmu, 0x78), 8); // BIT 7,B
        let f = cpu.flags();
        assert!(!f.contains(Flags::Z)); // bit 7 posé
        assert!(f.contains(Flags::N));
        assert!(f.contains(Flags::H));
        assert!(f.contains(Flags::C));

        cpu.b = 0x40;
        execute_cb(&mut cpu, &mut mmu, 0x78); // BIT 7,B avec bit 7 effacé
        let f = cpu.flags();
        assert!(f.contains(Flags::Z));
        assert!(f.contains(Flags::N));
        assert!(f.contains(Flags::H));

        // SET b,r8 : pose le bit, drapeaux inchangés.
        cpu.b = 0x00;
        cpu.set_flags(Flags::Z | Flags::C);
        assert_eq!(execute_cb(&mut cpu, &mut mmu, 0xE0), 16); // SET 4,B (forme registre)
        assert_eq!(cpu.b, 0x10);
        let f = cpu.flags();
        assert!(f.contains(Flags::Z)); // inchangés
        assert!(f.contains(Flags::C));

        // RES b,r8 : efface le bit, drapeaux inchangés.
        cpu.b = 0xFF;
        execute_cb(&mut cpu, &mut mmu, 0xA0); // RES 4,B (forme registre)
        assert_eq!(cpu.b, 0xEF);
        assert!(f.contains(Flags::Z));

        // Forme (HL) : 16 T-cycles pour les rotations/BIT.
        let mut mmu = rom_with(&[0xCB, 0x06]); // RLC (HL) à $0100
        let mut cpu = CPU::new();
        cpu.h = 0xC0;
        cpu.l = 0x00;
        mmu.write(0xC000, 0x81);
        assert_eq!(execute_cb(&mut cpu, &mut mmu, 0x06), 16); // RLC (HL)
        assert_eq!(mmu.read(0xC000), 0x03);

        // RES/SET sur (HL) : 24 T-cycles.
        assert_eq!(execute_cb(&mut cpu, &mut mmu, 0xFE), 24); // SET 7,(HL)
        assert_eq!(mmu.read(0xC000), 0x83);
    }

    #[test]
    fn interrupt_service_vectors_and_halt_bug() {
        let mut mmu = MMU::new();
        let mut cpu = CPU::new();
        cpu.pc = 0x0150;
        cpu.ime = true;
        mmu.io[0x0F] = 0x01; // IF : V-Blank en attente
        mmu.ie = 0x01;       // IE : V-Blank activé

        // Service : IME désactivé, PC poussé, saut à $40, 20 T-cycles.
        assert_eq!(handle_interrupts(&mut cpu, &mut mmu), Some(20));
        assert!(!cpu.ime);
        assert_eq!(cpu.pc, 0x40);
        assert_eq!(cpu.sp, 0xFFFC); // FFFE - 2 (un u16 poussé)
        assert_eq!(mmu.read(0xFFFD), 0x01); // octet haut de l'adresse de retour
        assert_eq!(mmu.read(0xFFFC), 0x50);
        // Le matériel efface automatiquement le bit IF de l'interruption acceptée.
        assert_eq!(mmu.io[0x0F], 0x00);

        // Pas de service quand IME est faux (même interruption en attente).
        let mut cpu = CPU::new();
        mmu.io[0x0F] = 0x02; // LC3C/STAT en attente
        mmu.ie = 0x07;
        assert_eq!(handle_interrupts(&mut cpu, &mut mmu), None);

        // Le bit le plus bas (pendant+activé) gagne.
        let mut cpu = CPU::new();
        cpu.ime = true;
        mmu.io[0x0F] = 0x03; // V-Blank + LC3C
        assert_eq!(handle_interrupts(&mut cpu, &mut mmu), Some(20));
        assert_eq!(cpu.pc, 0x40);
        // Seul le bit accepté est effacé : LC3C (bit 1) reste en attente.
        assert_eq!(mmu.io[0x0F], 0x02);

        let mut cpu = CPU::new();
        cpu.ime = true;
        mmu.io[0x0F] = 0x06; // LC3C + Timer
        assert_eq!(handle_interrupts(&mut cpu, &mut mmu), Some(20));
        assert_eq!(cpu.pc, 0x48);
        // Timer (bit 2) reste en attente.
        assert_eq!(mmu.io[0x0F], 0x04);

        let mut cpu = CPU::new();
        cpu.ime = true;
        mmu.io[0x0F] = 0x08; // Serial seul
        mmu.ie = 0x0F;       // toutes les interruptions activées
        assert_eq!(handle_interrupts(&mut cpu, &mut mmu), Some(20));
        assert_eq!(cpu.pc, 0x58);
        assert_eq!(mmu.io[0x0F], 0x00); // le bit Serial est effacé

        // HALT bug : une interruption en attente sort du HALT même si IME=false (sans service).
        let mut cpu = CPU::new();
        cpu.halted = true;
        mmu.io[0x0F] = 0x01;
        mmu.ie = 0x01;
        assert_eq!(handle_interrupts(&mut cpu, &mut mmu), None); // IME=false → pas de service
        assert!(!cpu.halted); // mais l'état HALT est annulé

        // …et avec IME=true l'interruption est servie en plus.
        let mut cpu = CPU::new();
        cpu.halted = true;
        cpu.ime = true;
        mmu.io[0x0F] = 0x01;
        mmu.ie = 0x01;
        assert_eq!(handle_interrupts(&mut cpu, &mut mmu), Some(20));
        assert!(!cpu.halted);
        assert_eq!(cpu.pc, 0x40);
    }

    #[test]
    fn ei_delayed_ime_enable_and_di_cancels() {
        let mut mmu = rom_with(&[0xFB, 0x00]); // EI puis NOP à $0100
        let mut cpu = CPU::new();
        cpu.pc = 0x0100;

        // Pas 1 : EI — IME pas encore effectif (retard en cours).
        assert_eq!(cpu.step(&mut mmu), 4);
        assert!(!cpu.ime);
        assert_eq!(cpu.ei_delay, 1);

        // Pas 2 : l'instruction EI suivante s'exécute avec IME toujours off…
        assert_eq!(cpu.step(&mut mmu), 4); // NOP
        assert!(cpu.ime); // …IME devient effectif après son exécution (Pan Docs).
        assert_eq!(cpu.ei_delay, 0);

        // DI annule un retard d'EI en cours.
        let mut mmu = rom_with(&[0xFB, 0xF3, 0x00]); // EI puis DI puis NOP
        let mut cpu = CPU::new();
        cpu.pc = 0x0100;
        assert_eq!(cpu.step(&mut mmu), 4); // EI → ei_delay=2
        assert!(!cpu.ime);
        assert_eq!(cpu.step(&mut mmu), 4); // DI → ime=false, retard annulé
        assert!(!cpu.ime);
        assert_eq!(cpu.ei_delay, 0);
        assert_eq!(cpu.step(&mut mmu), 4); // instruction suivante : IME toujours off
        assert!(!cpu.ime);
    }

    #[test]
    fn all_256_opcodes_are_covered_and_execute() {
        // Le match d'execute() est exhaustif sur $00-$FF (vérifié à la compilation) ; ce test le confirme à l'exécution.
        for opcode in 0u8..=0xFF {
            let mut mmu = rom_with(&[]); // ROM remplie de $FF (valeurs des immédiats)
            let mut cpu = CPU::new();    // pc=$0100, F=$B0 (Z=0, C=1), SP=$FFFE
            let cycles = execute(&mut cpu, &mut mmu, opcode);
            assert!((4..=24).contains(&cycles), "opcode ${:02X} : {} T-cycles", opcode, cycles);
        }

        // La lecture d'un immédiat n16 doit avancer le PC de exactement 2 octets.
        for &opcode in &[0x01u8, 0x11, 0x21, 0x31] { // LD r16,n16
            let mut mmu = rom_with(&[]);
            let mut cpu = CPU::new();
            execute(&mut cpu, &mut mmu, opcode);
            assert_eq!(cpu.pc, 0x0102, "opcode ${:02X} doit consommer 2 octets", opcode);
        }

        // La lecture d'un immédiat n8 (ou du sous-opcode CB) doit avancer le PC de exactement 1 octet.
        for &opcode in &[
            0x06u8, 0x0E, 0x16, 0x1E, 0x26, 0x2E, 0x36, 0x3E, // LD r8,n8
            0xC6, 0xCE, 0xD6, 0xDE, 0xE6, 0xEE, 0xF6, 0xFE,    // ALU A,n8
            0xE8, 0xF8,                                          // ADD SP,e8 / LD HL,SP+e8
            0xCB,                                                // préfixe CB (sous-opcode lu)
        ] {
            let mut mmu = rom_with(&[]);
            let mut cpu = CPU::new();
            execute(&mut cpu, &mut mmu, opcode);
            assert_eq!(cpu.pc, 0x0101, "opcode ${:02X} doit consommer 1 octet", opcode);
        }
    }

}

