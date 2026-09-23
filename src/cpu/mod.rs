//! CPU Sharp LR35902 (dérivé custom du Z80) : registres et boucle Fetch-Decode-Execute.
//!
//! Partie 6 : jeu d'instructions complet (0x00-0xFF + préfixe 0xCB), interruptions
//! (IME, IF, IE), HALT bug, et `step()` renvoyant les T-cycles.

pub mod flags;
pub mod opcodes;

use crate::bus::Bus;
use crate::emulator::FRAME_TCYCLES;
use flags::Flags;

// --- Mécanisme d'exécution pas-à-pas en micro-ops (D2) — étape 1 : infrastructure seulement. ---
// Toutes les instructions réelles restent sur le chemin atomique legacy ; aucune famille n'est encore
// portée en micro-ops (étape 2). Ces types et les latches W/Z du CPU préparent ce portage.

/// Source d'adresse pour un accès mémoire en micro-op (D2) : la paire de registres ou le pointeur visé.
// Étape 1 : infrastructure seulement — aucune famille n'est encore portée, donc les variantes ne sont pas construites.
#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AddrSrc {
    /// Registre HL.
    Hl,
    /// Registre BC.
    Bc,
    /// Registre DE.
    De,
    /// Pointeur de pile SP.
    Sp,
    /// Compteur programme PC (fetch).
    Pc,
}

impl AddrSrc {
    /// Résout l'adresse 16 bits visée depuis l'état courant du CPU.
    fn address(&self, cpu: &CPU) -> u16 {
        match self {
            AddrSrc::Hl => cpu.hl(),
            AddrSrc::Bc => ((cpu.b as u16) << 8) | cpu.c as u16,
            AddrSrc::De => ((cpu.d as u16) << 8) | cpu.e as u16,
            AddrSrc::Sp => cpu.sp,
            AddrSrc::Pc => cpu.pc,
        }
    }
}

/// Source de valeur pour une écriture mémoire en micro-op (D2) : le registre ou le latch mis de côté.
#[allow(dead_code)] // Étape 1 : infrastructure D2 — aucune famille n'est encore portée (étape 2).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ValSrc {
    /// Registre A.
    A,
    /// Registre B.
    B,
    /// Registre C.
    C,
    /// Registre D.
    D,
    /// Registre E.
    E,
    /// Registre H.
    H,
    /// Registre L.
    L,
    /// Latch d'écriture W (valeur mise de côté pour une écriture mémoire différée).
    W,
    /// Latch de lecture Z (octet lu / résultat intermédiaire).
    Z,
}

impl ValSrc {
    /// Résout la valeur 8 bits à écrire depuis l'état courant du CPU.
    fn value(&self, cpu: &CPU) -> u8 {
        match self {
            ValSrc::A => cpu.a,
            ValSrc::B => cpu.b,
            ValSrc::C => cpu.c,
            ValSrc::D => cpu.d,
            ValSrc::E => cpu.e,
            ValSrc::H => cpu.h,
            ValSrc::L => cpu.l,
            ValSrc::W => cpu.w,
            ValSrc::Z => cpu.z,
        }
    }
}

/// Micro-opération : un pas d'exécution d'une instruction (D2). Chaque micro-op consomme exactement un M-cycle.
#[allow(dead_code)] // Étape 1 : infrastructure D2 — aucune famille n'est encore portée (étape 2).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MicroOp {
    /// Lit l'opcode à PC → latch Z ; le PC avance d'un octet.
    FetchOpcode,
    /// Lit l'octet d'opérande à PC → latch Z ; le PC avance d'un octet.
    ReadPcByte,
    /// Lit la mémoire pointée par `addr` → latch Z.
    ReadMem(AddrSrc),
    /// Écrit la valeur `val` dans la mémoire pointée par `addr`.
    WriteMem(AddrSrc, ValSrc),
    /// Pas interne (aucun accès bus) — ex. calcul de drapeaux ; le matériel avance quand même d'un M-cycle.
    Internal,
}

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
    /// Bug HALT DMG : posé quand le CPU sort de l'état HALT à cause d'une interruption pendante (bit de IF) ; la
    /// prochaine instruction fetchée est exécutée avec un décalage d'un octet — l'octet à PC est sauté et l'opcode
    /// est lu depuis PC+1 (GBCTR Chapitre 6.8). Ce n'est PAS « exécuter deux fois » : c'est un saut d'un octet qui
    /// réinterprète les bytes suivants comme un nouvel opcode.
    pub halt_bug: bool,
    /// Retard d'activation de IME après un EI : IME n'est réactivé qu'une fois l'instruction
    /// EI suivante exécutée (Pan Docs « CPU Instruction Set »). 0 = pas de retard en cours.
    pub ei_delay: u8,
    /// T-cycles écoulés au total — maintenu par `Emulator::step` (sert à throttler les logs par frame).
    pub t_cycles: u64,
    /// Index de la dernière frame où un log ISR a été émis (throttle 1/frame ; MAX = jamais émis).
    pub isr_log_frame: u32,
    /// Latch d'écriture W : valeur mise de côté pour une écriture mémoire différée en micro-op (D2).
    pub w: u8,
    /// Latch de lecture Z : octet lu / résultat intermédiaire d'un micro-op (D2).
    pub z: u8,
    /// Micro-op en cours d'exécution pas-à-pas ; `None` = chemin atomique legacy (étape 1 — aucune famille portée).
    pub micro_op: Option<MicroOp>,
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
            halt_bug: false,
            ei_delay: 0,
            t_cycles: 0,
            isr_log_frame: u32::MAX,
            w: 0,
            z: 0,
            micro_op: None,
        };
        log::info!(
            "[CPU] Initial state: SP=${:04X}, PC=${:04X}",
            cpu.sp,
            cpu.pc
        );
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
    /// Bug HALT DMG : si le CPU est en HALT and any bit of IF ($FF0F) is set — even if IME is faux or the flag
    /// n'est pas activé dans IE — l'état HALT is annulé (le « HALT bug ») and la prochaine instruction fetchée is exécutée
    /// avec un décalage d'un octet : l'octet à PC est sauté et l'opcode est lu depuis PC+1 (GBCTR Chapitre 6.8). Si IME is en plus vrai, the
    /// interruption is additionally servied (voir `opcodes::handle_interrupts`).
    ///
    /// Le retard d'EI est décrémenté après chaque instruction exécutée : IME n'est
    /// réactivé qu'une fois l'instruction EI suivante exécutée (Pan Docs).
    pub fn tick(&mut self, bus: &mut Bus) -> u32 {
        // Mécanisme D2 : si un micro-op est en cours d'exécution pas-à-pas, l'exécuter (un M-cycle).
        if let Some(op) = self.micro_op.take() {
            return self.step_micro_op(bus, op);
        }

        // Chemin atomique legacy : toutes les instructions réelles restent ici (étape 1 — aucune famille portée).
        // L'accès au MMU est brut (pas de tick par accès) ; l'avancement du matériel est fait en bloc par `Emulator`.
        let mmu = &mut *bus;
        // Check for interrupts before fetching the next opcode (also exits HALT — see handle_interrupts).
        if let Some(cycles) = opcodes::handle_interrupts(self, mmu) {
            return cycles; // un halt_bug posé par la sortie de HALT persiste jusqu'au fetch suivant
        }

        // If halted and no interrupt is pending, consume 4 T-cycles (HALT loop).
        if self.halted {
            return 4;
        }

        // Bug HALT DMG : quand le CPU vient de sortir de l'état HALT à cause d'une interruption pendante, la
        // prochaine instruction est fetchée avec un décalage d'un octet — l'octet à PC est sauté et l'opcode est lu depuis PC+1 (GBCTR Chapitre 6.8).
        if self.halt_bug {
            self.pc = self.pc.wrapping_add(1); // saute un octet : l'opcode suivant est lu à PC+1
            self.halt_bug = false;
        }

        let mut cycles = 0u32;
        let opcode = mmu.read(self.pc);

        // 🚨 DÉTECTEUR DE CRASH : Si le PC entre in HRAM, on le loggue immediately — cela nous dira how the CPU got there.
        if self.pc >= 0xFF00 && self.pc <= 0xFFFE {
            log::error!(
                "🚨 CRASH CPU : Le PC est entré in HRAM (${:04X}) ! Opcode: ${:02X}, SP: ${:04X}",
                self.pc,
                opcode,
                self.sp
            );
        }

        // LOG : tracer les instructions dans la zone des handlers d'interruption ($0040-$006F),
        // throttlé à 1 log par frame (70224 T-cycles) : seul le premier opcode ISR de chaque frame est tracé.
        // Un simple `t_cycles % FRAME_TCYCLES == 0` ne se déclencherait jamais — les instructions de longueur
        // variable ne tombent jamais pile sur un multiple de 70224 ; d'où la détection par index de frame.
        if (0x0040..=0x006F).contains(&self.pc) {
            let frame = self.t_cycles / FRAME_TCYCLES;
            if frame != self.isr_log_frame as u64 {
                self.isr_log_frame = frame as u32;
                log::debug!(
                    "[CPU] ISR executing (1/frame): PC=${:04X}, opcode=${:02X}",
                    self.pc,
                    opcode
                );
            }
        }

        self.pc = self.pc.wrapping_add(1);
        cycles += opcodes::execute(self, mmu, opcode);
        // EI : IME devient effectif après l'instruction EI suivante (2 étapes : la fin de
        // l'instruction EI elle-même, puis celle qui suit).
        if self.ei_delay > 0 {
            self.ei_delay -= 1;
            if self.ei_delay == 0 {
                self.ime = true;
            }
        }
        cycles
    }

    /// Exécute un micro-op pas-à-pas (mécanisme D2) : consomme exactement un M-cycle (4 T-cycles).
    /// Chaque accès mémoire passe par le bus (`read`/`write` = accès + tick) ; un `Internal` pas n'accède
    /// à rien mais avance quand même le matériel d'un M-cycle. Étape 1 : aucune famille n'est encore portée,
    /// donc ce chemin n'est jamais pris (le champ `micro_op` reste `None`).
    fn step_micro_op(&mut self, bus: &mut Bus, op: MicroOp) -> u32 {
        match op {
            MicroOp::FetchOpcode => {
                let opcode = bus.read(self.pc); // accès + tick (D1)
                self.z = opcode;
                self.pc = self.pc.wrapping_add(1);
            }
            MicroOp::ReadPcByte => {
                let byte = bus.read(self.pc); // accès + tick (D1)
                self.z = byte;
                self.pc = self.pc.wrapping_add(1);
            }
            MicroOp::ReadMem(addr_src) => {
                let addr = addr_src.address(self);
                self.z = bus.read(addr); // accès + tick (D1)
            }
            MicroOp::WriteMem(addr_src, val_src) => {
                let addr = addr_src.address(self);
                let value = val_src.value(self);
                bus.write(addr, value); // accès + tick (D1)
            }
            MicroOp::Internal => {
                bus.idle_m_cycle(); // pas interne : aucun accès bus, mais le matériel avance d'un M-cycle
            }
        }
        4 // un M-cycle consommé
    }
}

impl Default for CPU {
    fn default() -> Self {
        Self::new()
    }
}
