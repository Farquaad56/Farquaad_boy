//! CPU Sharp LR35902 (dérivé custom du Z80) : registres et boucle Fetch-Decode-Execute.
//!
//! Partie 6 : jeu d'instructions complet (0x00-0xFF + préfixe 0xCB), interruptions
//! (IME, IF, IE), HALT bug, et `step()` renvoyant les T-cycles.

pub mod flags;
pub mod opcodes;

use crate::mmu::MMU;
use flags::Flags;

/// Registres du CPU Sharp LR35902.
#[allow(clippy::upper_case_acronyms)]
#[derive(Debug, Clone)]
pub struct CPU {
    /// Accumulateur.
    pub a: u8,
    /// Registre de drapeaux (seuls les bits 7..4 sont utilisés — voir `flags::Flags`).
    pub f: u8,
    pub b: u8,
    pub c: u8,
    pub d: u8,
    pub e: u8,
    pub h: u8,
    pub l: u8,
    /// Pointeur de pile.
    pub sp: u16,
    /// Compteur programme.
    pub pc: u16,
    /// Interrupt Master Enable.
    pub ime: bool,
    /// Le CPU est-il en état HALT ? (sorti par une interruption pendante — le « HALT bug »).
    pub halted: bool,
    /// Retard d'activation de IME après un EI : IME n'est réactivé qu'une fois l'instruction
    /// suivant l'EI exécutée (Pan Docs « CPU Instruction Set »). 0 = pas de retard en cours.
    pub ei_delay: u8,
}

impl CPU {
    /// État de reset power-on (valeurs finales du boot ROM DMG).
    pub fn new() -> Self {
        let cpu = Self {
            a: 0x01,
            f: 0xB0,
            b: 0x00,
            c: 0x13,
            d: 0x00,
            e: 0xD8,
            h: 0x01,
            l: 0x4D,
            sp: 0xFFFE, // valeur post-boot ROM DMG (Pan Docs « Power Up Sequence »)
            pc: 0x0100, // cible du vecteur de reset
            ime: false,
            halted: false,
            ei_delay: 0,
        };
        log::info!("[CPU] Initial state: SP=${:04X}, PC=${:04X}", cpu.sp, cpu.pc);
        cpu
    }

    /// Registre AF combiné (A haut, F bas).
    pub fn af(&self) -> u16 {
        ((self.a as u16) << 8) | self.f as u16
    }

    /// Registre HL combiné.
    pub fn hl(&self) -> u16 {
        ((self.h as u16) << 8) | self.l as u16
    }

    /// Drapeaux courants (bits 7..4 du registre F).
    pub fn flags(&self) -> Flags {
        Flags::from_bits_truncate(self.f)
    }

    /// Définit les drapeaux (le nibble bas est conservé, toujours à zéro sur le matériel).
    pub fn set_flags(&mut self, f: Flags) {
        self.f = (self.f & 0x0F) | f.bits();
    }

    /// Exécute une instruction et renvoie le nombre de T-cycles consommés.
    ///
    /// Si le CPU est en HALT et qu'une interruption est pendante, l'état HALT est
    /// annulé (le « HALT bug ») et l'interruption est servie même si IME est faux.
    ///
    /// Le retard d'EI est décrémenté après chaque instruction exécutée : IME n'est
    /// réactivé qu'une fois l'instruction suivant l'EI exécutée (Pan Docs).
    pub fn step(&mut self, mmu: &mut MMU) -> u32 {
        // Check for interrupts before fetching the next opcode.
        if let Some(cycles) = opcodes::handle_interrupts(self, mmu) {
            return cycles;
        }

        // If halted and no interrupt is pending, consume 4 T-cycles (HALT loop).
        if self.halted {
            return 4;
        }

        let opcode = mmu.read(self.pc);

        // LOG : tracer les instructions dans la zone des handlers d'interruption ($0040-$006F)
        if (0x0040..=0x006F).contains(&self.pc) {
            log::debug!("[CPU] ISR executing: PC=${:04X}, opcode=${:02X}", self.pc, opcode);
        }

        self.pc = self.pc.wrapping_add(1);
        let cycles = opcodes::execute(self, mmu, opcode);

        // EI : IME devient effectif après l'instruction suivant l'EI (2 étapes : la fin de
        // l'instruction EI elle-même, puis celle qui suit).
        if self.ei_delay > 0 {
            self.ei_delay -= 1;
            if self.ei_delay == 0 {
                self.ime = true;
            }
        }
        cycles
    }
}

impl Default for CPU {
    fn default() -> Self {
        Self::new()
    }
}