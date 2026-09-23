//! CPU Sharp LR35902 (dérivé custom du Z80) : registres et boucle Fetch-Decode-Execute.
//!
//! Partie 6 : jeu d'instructions complet (0x00-0xFF + préfixe 0xCB), interruptions
//! (IME, IF, IE), HALT bug, et `step()` renvoyant les T-cycles.

pub mod expected_cycles;
pub mod flags;
pub mod opcodes;

use std::collections::VecDeque;

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
/// Les instructions portées en micro-ops sont exécutées pas-à-pas : [`CPU::tick`] consomme un micro-op de
/// [`InProgress::steps`] par appel, jusqu'à épuisement. Étape 2 : seules les familles portées produisent des
/// séquences ; toutes les autres instructions restent sur le chemin atomique legacy.
#[allow(dead_code)] // Étape 1 : `FetchOpcode` / `ReadMem` / `Internal` ne sont pas encore construits (étape 2).
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

/// Instruction portée en cours d'exécution pas-à-pas (D2) : les micro-ops restants à exécuter. Le fetch
/// (phase 0) est déjà consommé au décodage ; chaque appel à [`CPU::tick`] consomme un micro-op de `steps`. Quand
/// `steps` s'épuise, l'instruction est achevée et le champ est remis à `None`.
#[derive(Debug, Clone)]
pub struct InProgress {
    /// Micro-ops restants à exécuter (un par appel à [`CPU::tick`]).
    pub steps: VecDeque<MicroOp>,
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
    /// Instruction portée en cours d'exécution pas-à-pas ; `None` = chemin atomique legacy.
    pub in_progress: Option<InProgress>,
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
            in_progress: None,
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

    /// Exécute un pas et renvoie le nombre de T-cycles consommés ainsi que true si la frontière de frame est
    /// franchie. Le CPU pilote le bus : l'avancement du matériel (PPU / Timer / Série / DMA + bits IF) se fait ici —
    /// par M-cycle pour les instructions portées en micro-ops, et en bloc à la fin d'une instruction legacy.
    ///
    /// Bug HALT DMG : si le CPU est en HALT and any bit of IF ($FF0F) is set — even if IME is faux or the flag
    /// n'est pas activé dans IE — l'état HALT is annulé (le « HALT bug ») and la prochaine instruction fetchée is exécutée
    /// avec un décalage d'un octet : l'octet à PC est sauté et l'opcode est lu depuis PC+1 (GBCTR Chapitre 6.8). Si IME is en plus vrai, the
    /// interruption is additionally servied (voir `opcodes::handle_interrupts`).
    ///
    /// Le retard d'EI est décrémenté après chaque instruction exécutée : IME n'est
    /// réactivé qu'une fois l'instruction EI suivante exécutée (Pan Docs).
    pub fn tick(&mut self, bus: &mut Bus) -> (u32, bool) {
        // Mécanisme D2 : si une instruction portée est en cours d'exécution pas-à-pas, l'avancer d'un M-cycle.
        if let Some(prog) = self.in_progress.take() {
            return self.advance_ported_step(bus, prog);
        }

        // Check for interrupts before fetching the next opcode (also exits HALT — see handle_interrupts).
        // Accès brut au MMU via le champ public du bus (pas de tick par accès) ; l'avancement est fait en bloc.
        let int_cycles = opcodes::handle_interrupts(self, bus.mmu);
        if let Some(cycles) = int_cycles {
            return (cycles, bus.advance(cycles)); // un halt_bug posé par la sortie de HALT persiste jusqu'au fetch suivant
        }

        // If halted and no interrupt is pending, consume 4 T-cycles (HALT loop).
        if self.halted {
            let frame_done = bus.tick_m_cycle();
            return (4, frame_done);
        }

        // Bug HALT DMG : quand le CPU vient de sortir de l'état HALT à cause d'une interruption pendante, la
        // prochaine instruction est fetchée avec un décalage d'un octet — l'octet à PC est sauté et l'opcode est lu depuis PC+1 (GBCTR Chapitre 6.8).
        if self.halt_bug {
            self.pc = self.pc.wrapping_add(1); // saute un octet : l'opcode suivant est lu à PC+1
            self.halt_bug = false;
        }

        let opcode = bus.mmu.read(self.pc); // accès brut (pas de tick) — décodage uniquement

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

        // Instruction portée ? Le fetch a consommé un M-cycle ; les M-cycles suivants sont exécutés pas-à-pas.
        if let Some(steps) = ported_steps(opcode) {
            self.in_progress = Some(InProgress { steps });
            let frame_done = bus.tick_m_cycle(); // le fetch (lu brutalement ci-dessus) a consommé un M-cycle
            return (4, frame_done);
        }

        // Chemin atomique legacy : exécute l'instruction en bloc (accès bruts au MMU) puis avance le matériel d'un seul tenant.
        let cycles = opcodes::execute(self, bus.mmu, opcode);
        // EI : IME devient effectif après l'instruction EI suivante (2 étapes : la fin de
        // l'instruction EI elle-même, puis celle qui suit).
        if self.ei_delay > 0 {
            self.ei_delay -= 1;
            if self.ei_delay == 0 {
                self.ime = true;
            }
        }
        let frame_done = bus.advance(cycles);
        (cycles, frame_done)
    }

    /// Exécute un M-cycle d'une instruction portée en cours d'exécution pas-à-pas (mécanisme D2). Consomme
    /// exactement un M-cycle (4 T-cycles) : chaque accès mémoire passe par le bus (`read`/`write` = accès + tick),
    /// et renvoie true si ce M-cycle a franchi la frontière de frame. Quand les phases s'épuisent, l'instruction
    /// est achevée et `in_progress` est remis à `None`.
    fn advance_ported_step(&mut self, bus: &mut Bus, mut prog: InProgress) -> (u32, bool) {
        let op = prog.steps.pop_front().expect("une instruction portée en cours a au moins un micro-op");
        let frame_done = match op {
            MicroOp::FetchOpcode => {
                let (opcode, fd) = bus.read(self.pc); // accès + tick (D1)
                self.z = opcode;
                self.pc = self.pc.wrapping_add(1);
                fd
            }
            MicroOp::ReadPcByte => {
                let (byte, fd) = bus.read(self.pc); // accès + tick (D1)
                self.z = byte;
                self.pc = self.pc.wrapping_add(1);
                fd
            }
            MicroOp::ReadMem(addr_src) => {
                let addr = addr_src.address(self);
                let (value, fd) = bus.read(addr); // accès + tick (D1)
                self.z = value;
                fd
            }
            MicroOp::WriteMem(addr_src, val_src) => {
                let addr = addr_src.address(self);
                let value = val_src.value(self);
                bus.write(addr, value) // accès + tick (D1)
            }
            MicroOp::Internal => {
                bus.idle_m_cycle() // pas interne : aucun accès bus, mais le matériel avance d'un M-cycle
            }
        };

        if prog.steps.is_empty() {
            self.in_progress = None; // instruction achevée
        } else {
            self.in_progress = Some(prog); // les phases restantes sont exécutées sur les ticks suivants
        }
        (4, frame_done)
    }
}

/// Les M-cycles restants (après le fetch) d'une instruction non préfixée portée en micro-ops (D2). Étape 1 :
/// infrastructure only — AUCUNE famille n'est encore portée, so this returns `None` for every opcode and all real
/// instructions stay on the chemin atomique legacy. Stage 2 will populate this match with the first instruction family.
#[allow(dead_code)] // dormant in stage 1 (no family ported yet) ; populated by stage 2
fn ported_steps(_opcode: u8) -> Option<VecDeque<MicroOp>> {
    None
}

impl Default for CPU {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mmu::MMU;
    use expected_cycles::{expected_cb_m_cycles, expected_unprefixed_m_cycles};

    /// Exécute une instruction isolée dans un ROM minimal (rempli de NOP) et renvoie le total de T-cycles
    /// consommés. `bytes` : les octets de l'instruction (opcode + opérandes) placés à $0100 ; les opérandes
    /// absents sont lus comme 0x00 depuis le ROM. `flags` : la valeur du registre F avant exécution (Z=bit7, C=bit4).
    fn run_isolated_t_cycles(bytes: &[u8], flags: u8) -> u32 {
        let mut rom = vec![0x00u8; 0x4000]; // ROM minimal : NOP partout
        for (i, b) in bytes.iter().enumerate() {
            rom[0x0100 + i] = *b;
        }
        let mut mmu = MMU::new();
        mmu.load_rom(rom);
        let mut cpu = CPU::new();
        cpu.pc = 0x0100;
        cpu.sp = 0xFFFE; // pile valide (WRAM) pour RET / CALL conditionnels
        cpu.f = flags;
        let mut bus = Bus::new(&mut mmu);

        // Une instruction non portée s'achève en un tick ; une portée s'achève quand ses micro-ops sont épuisés.
        let mut total = 0u32;
        loop {
            let (cycles, _) = cpu.tick(&mut bus);
            total += cycles;
            if cpu.in_progress.is_none() {
                break;
            }
        }
        total
    }

    /// Valeur du registre F qui force la branche `taken` pour l'opcode conditionnel donné (None si non conditionnel).
    fn flags_for_branch(opcode: u8, taken: bool) -> Option<u8> {
        // (contrôlé par C ? , pris quand le drapeau est posé ?)
        let (is_c, set_means_taken) = match opcode {
            0x20 | 0xC0 | 0xC2 | 0xC4 => (false, false), // NZ : pris si Z=0
            0x28 | 0xC8 | 0xCA | 0xCC => (false, true),  // Z  : pris si Z=1
            0x30 | 0xD0 | 0xD2 | 0xD4 => (true, false),  // NC : pris si C=0
            0x38 | 0xD8 | 0xDA | 0xDC => (true, true),   // C  : pris si C=1
            _ => return None,
        };
        let want_set = taken == set_means_taken;
        let mut f = 0u8;
        if is_c {
            if want_set {
                f |= Flags::C.bits(); // bit 4
            }
        } else if want_set {
            f |= Flags::Z.bits(); // bit 7
        }
        Some(f)
    }

    /// Le mécanisme pas-à-pas (D2) avance exactement un M-cycle par tick et pose les latches W/Z — sans qu'aucune
    /// famille réelle ne soit portée (étape 1) : le programme d'instruction en cours est construit à la main.
    #[test]
    fn advance_ported_step_consumes_one_m_cycle_and_latches() {
        let mut mmu = MMU::new();
        let mut rom = vec![0x00u8; 0x4000]; // ROM minimal : NOP partout, opérande à $0100
        rom[0x0100] = 0x42; // opérande n8 lu par ReadPcByte à PC=$0100
        mmu.load_rom(rom);

        let mut cpu = CPU::new();
        cpu.pc = 0x0100;
        cpu.h = 0xC0; // HL = $C050 (WRAM, toujours écritable)
        cpu.l = 0x50;
        // Programme d'instruction porté construit à la main : ReadPcByte → WriteMem(HL, Z).
        let mut steps: VecDeque<MicroOp> = VecDeque::new();
        steps.push_back(MicroOp::ReadPcByte);
        steps.push_back(MicroOp::WriteMem(AddrSrc::Hl, ValSrc::Z));
        cpu.in_progress = Some(InProgress { steps });

        let mut bus = Bus::new(&mut mmu);

        // Tick 1 : ReadPcByte → Z = ROM[PC] = $42, PC → $0101. Un M-cycle (4 T-cycles).
        let (t, _) = cpu.tick(&mut bus);
        assert_eq!(t, 4);
        assert_eq!(cpu.z, 0x42);
        assert_eq!(cpu.pc, 0x0101);

        // Tick 2 : WriteMem(HL, Z) → [HL] = $42. Un M-cycle ; l'instruction est achevée.
        let (t, _) = cpu.tick(&mut bus);
        assert_eq!(t, 4);
        assert!(cpu.in_progress.is_none());

        drop(bus);
        assert_eq!(mmu.read(0xC050), 0x42);
    }

    /// Test exhaustif générique (étape 1) : pour chacun des opcodes non préfixés — les deux branches des
    /// conditionnelles incluses — l'instruction est exécutée isolée dans un ROM minimal et le nombre de M-cycles
    /// consommés est comparé à la table générée depuis `data/Opcodes.json`.
    ///
    /// Étape 1 : AUCUNE instruction n'est corrigée (corriger changerait le comportement observable, interdit en
    /// étape 1). Les écarts code/JSON connus sont donc listés ci-dessous comme référence de base (« baseline ») ; ce
    /// test passe tant que l'ensemble des écarts correspond EXACTEMENT à cette liste. Toute divergence nouvelle — ou
    /// un écart corrigé plus tard — fait échouer le test et impose de mettre à jour la liste avant correction.
    #[test]
    fn all_unprefixed_opcodes_match_expected_m_cycles() {
        let mut discrepancies = Vec::new();
        for opcode in 0u8..=0xFF {
            let (_, not_taken_c) = expected_cycles::EXPECTED_UNPREFIXED[opcode as usize];
            if not_taken_c.is_none() {
                // Non conditionnelle : un seul tirage.
                let m = run_isolated_t_cycles(&[opcode], 0) / 4;
                let expected = expected_unprefixed_m_cycles(opcode, false);
                if m != expected as u32 {
                    discrepancies.push(format!(
                        "$${:02X} (non-cond): got {} M-cycle(s), expected {}",
                        opcode, m, expected
                    ));
                }
            } else {
                // Conditionnelle : les deux branches.
                for &taken in &[true, false] {
                    let flags = flags_for_branch(opcode, taken).expect("opcode conditionnel");
                    let m = run_isolated_t_cycles(&[opcode], flags) / 4;
                    let expected = expected_unprefixed_m_cycles(opcode, taken);
                    if m != expected as u32 {
                        discrepancies.push(format!(
                            "$${:02X} ({}): got {} M-cycle(s), expected {}",
                            opcode,
                            if taken { "taken" } else { "not-taken" },
                            m,
                            expected
                        ));
                    }
                }
            }
        }

        // Écarts code/JSON connus (étape 1) — listés et soumis avant correction. Chaque entrée : cause.
        let mut known_gaps: Vec<String> = vec![
            // Rotations / STOP : le code consomme 2 M-cycles, la référence dit 1 (4 T-cycles).
            "$$07 (non-cond): got 2 M-cycle(s), expected 1".into(), // RLCA
            "$$0F (non-cond): got 2 M-cycle(s), expected 1".into(), // RRCA
            "$$10 (non-cond): got 2 M-cycle(s), expected 1".into(), // STOP n8
            "$$17 (non-cond): got 2 M-cycle(s), expected 1".into(), // RLA
            "$$1F (non-cond): got 2 M-cycle(s), expected 1".into(), // RRA
            // LD (HL),n8 : le code consomme 2 M-cycles, la référence dit 3 (12 T-cycles).
            "$$36 (non-cond): got 2 M-cycle(s), expected 3".into(), // LD (HL),n8
            // ADD HL,HL / ADD HL,SP : le code consomme 4 M-cycles (« DMG »), la référence dit 2.
            "$$29 (non-cond): got 4 M-cycle(s), expected 2".into(), // ADD HL,HL
            "$$39 (non-cond): got 4 M-cycle(s), expected 2".into(), // ADD HL,SP
            // LD r8,(HL) : le code consomme 1 M-cycle, la référence dit 2.
            "$$46 (non-cond): got 1 M-cycle(s), expected 2".into(), // LD B,(HL)
            "$$4E (non-cond): got 1 M-cycle(s), expected 2".into(), // LD C,(HL)
            "$$56 (non-cond): got 1 M-cycle(s), expected 2".into(), // LD D,(HL)
            "$$5E (non-cond): got 1 M-cycle(s), expected 2".into(), // LD E,(HL)
            "$$66 (non-cond): got 1 M-cycle(s), expected 2".into(), // LD H,(HL)
            "$$6E (non-cond): got 1 M-cycle(s), expected 2".into(), // LD L,(HL)
            "$$7E (non-cond): got 1 M-cycle(s), expected 2".into(), // LD A,(HL)
            // ALU A,(HL) : le code consomme 1 M-cycle, la référence dit 2.
            "$$86 (non-cond): got 1 M-cycle(s), expected 2".into(), // ADD A,(HL)
            "$$8E (non-cond): got 1 M-cycle(s), expected 2".into(), // ADC A,(HL)
            "$$96 (non-cond): got 1 M-cycle(s), expected 2".into(), // SUB (HL)
            "$$9E (non-cond): got 1 M-cycle(s), expected 2".into(), // SBC A,(HL)
            "$$A6 (non-cond): got 1 M-cycle(s), expected 2".into(), // AND (HL)
            "$$AE (non-cond): got 1 M-cycle(s), expected 2".into(), // XOR (HL)
            "$$B6 (non-cond): got 1 M-cycle(s), expected 2".into(), // OR (HL)
            "$$BE (non-cond): got 1 M-cycle(s), expected 2".into(), // CP (HL)
            // $CB : octet de préfixe seul — exécuté isolément, il consomme le sous-opcode suivant ($00 = BIT 0,B).
            "$$CB (non-cond): got 2 M-cycle(s), expected 1".into(), // CB prefix byte
            // JP HL : le code consomme 4 M-cycles, la référence dit 1.
            "$$E9 (non-cond): got 4 M-cycle(s), expected 1".into(), // JP HL
            // LDH A,(a8) : le code consomme 2 M-cycles, la référence dit 3.
            "$$F0 (non-cond): got 2 M-cycle(s), expected 3".into(), // LDH A,(a8)
            // JR cc non-taken : le code consomme 1 M-cycle (4 T-cycles, conforme au HW) ; la référence dit 2.
            "$$20 (not-taken): got 1 M-cycle(s), expected 2".into(), // JR NZ
            "$$28 (not-taken): got 1 M-cycle(s), expected 2".into(), // JR Z
            "$$30 (not-taken): got 1 M-cycle(s), expected 2".into(), // JR NC
            "$$38 (not-taken): got 1 M-cycle(s), expected 2".into(), // JR C
        ];

        discrepancies.sort();
        known_gaps.sort();
        assert_eq!(
            discrepancies,
            known_gaps,
            "l'ensemble des écarts code/JSON a changé — mettre à jour la liste d'écarts connus avant correction"
        );
    }

    /// Test exhaustif générique (étape 1) : pour chacun des opcodes préfixés CB ($CB xx), l'instruction est exécutée
    /// isolée dans un ROM minimal et le nombre de M-cycles consommés doit correspondre exactement à la table.
    #[test]
    fn all_cb_opcodes_match_expected_m_cycles() {
        let mut discrepancies = Vec::new();
        for cb in 0u8..=0xFF {
            let m = run_isolated_t_cycles(&[0xCB, cb], 0) / 4;
            let expected = expected_cb_m_cycles(cb);
            if m != expected as u32 {
                discrepancies.push(format!("$CB ${:02X}: got {} M-cycle(s), expected {}", cb, m, expected));
            }
        }
        assert!(
            discrepancies.is_empty(),
            "écart code/JSON ({}):\n{}",
            discrepancies.len(),
            discrepancies.join("\n")
        );
    }
}
