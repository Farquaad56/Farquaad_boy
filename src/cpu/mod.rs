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
    /// Port haute banque $FF00+n8 : l'octet n8 est dans le latch Z (ex LDH A,(n8)).
    HBankZ,
    /// Port haute banque $FF00+C : l'adresse est formée du registre C (ex LD A,[C]).
    HBankC,
    /// Adresse 16 bits formée des latches W (msb) et Z (lsb) — ex LD A,(a16).
    Wz,
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
            AddrSrc::HBankZ => 0xFF00 | cpu.z as u16,
            AddrSrc::HBankC => 0xFF00 | cpu.c as u16,
            AddrSrc::Wz => ((cpu.w as u16) << 8) | cpu.z as u16,
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

    /// Charge `value` dans le registre cible (côté écriture de [`MicroOp::LoadReg`] : l'octet lu est mis dans ce
    /// registre). Les latches W/Z ne sont pas des registres cibles valides — ils sont ignorés.
    fn store(&self, cpu: &mut CPU, value: u8) {
        match self {
            ValSrc::A => cpu.a = value,
            ValSrc::B => cpu.b = value,
            ValSrc::C => cpu.c = value,
            ValSrc::D => cpu.d = value,
            ValSrc::E => cpu.e = value,
            ValSrc::H => cpu.h = value,
            ValSrc::L => cpu.l = value,
            ValSrc::W | ValSrc::Z => {} // pas un registre cible valide pour LoadReg
        }
    }
}

/// Micro-opération : un pas d'exécution d'une instruction (D2). Chaque micro-op consomme exactement un M-cycle.
/// Les instructions portées en micro-ops sont exécutées pas-à-pas : [`CPU::tick`] consomme un micro-op de
/// [`InProgress::steps`] par appel, jusqu'à épuisement. Étape 2 : seules les familles portées produisent des
/// séquences ; toutes les autres instructions restent sur le chemin atomique legacy.
#[allow(dead_code)] // Étape 2 : `FetchOpcode` / `ReadMem` / `StorePcByte` / `Internal` ne sont pas encore construits par une famille portée ; portés plus tard.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MicroOp {
    /// Lit l'opcode à PC → latch Z ; le PC avance d'un octet.
    FetchOpcode,
    /// Lit l'octet d'opérande à PC → latch Z ; le PC avance d'un octet.
    ReadPcByte,
    /// Lit l'octet d'opérande à PC → latch W (msb) ; le PC avance d'un octet (ex LD A,(a16)).
    ReadPcByteW,
    /// Lit la mémoire pointée par `addr` → latch Z.
    ReadMem(AddrSrc),
    /// Lit la mémoire pointée par `addr` → latch Z puis charge le résultat dans le registre `reg`, le tout dans un seul M-cycle (ex LD r,(HL)).
    LoadReg(ValSrc, AddrSrc),
    /// Écrit la valeur `val` dans la mémoire pointée par `addr`.
    WriteMem(AddrSrc, ValSrc),
    /// Lit l'octet à PC (→ latch Z, PC+=1) puis écrit Z dans la mémoire pointée par `addr`, le tout dans un seul M-cycle.
    StorePcByte(AddrSrc),
    /// Écrit la valeur `val` en [HL] puis ajuste HL de `delta`, le tout dans un seul M-cycle (ex LD (HL±),A).
    StoreHlDelta(ValSrc, i8),
    /// Lit la mémoire pointée par [HL] → latch Z puis pose les drapeaux BIT depuis le bit `bit` (Z=(bit==0), N=0, H=1, C conservé), le tout dans un seul M-cycle (ex CB BIT b,(HL)).
    BitTest(u8),
    /// INC (HL) : result = Z+1 ; écrit [HL]=result et pose les drapeaux (Z=(r==0), N=0, H=((r&$0F)==0), C inchangé). Un M-cycle.
    IncHl,
    /// DEC (HL) : result = Z-1 ; écrit [HL]=result et pose les drapeaux (Z=(r==0), N=1, H=((r&$0F)==$0F), C inchangé). Un M-cycle.
    DecHl,
    /// Rotation/décalage CB b,(HL) : transforme Z selon `kind`, écrit [HL]=result et pose les drapeaux (Z=(r==0), N=0, H=0, C=décalé). Un M-cycle.
    RotateShiftHl(RotateKind),
    /// RES/SET b,(HL) : efface (`is_set`=false) / pose (`is_set`=true) the bit `bit` de Z, écrit [HL]=result ; pas de drapeaux. Un M-cycle.
    ResSetHl(bool, u8),
    /// Pas interne (aucun accès bus) — ex. calcul de drapeaux ; le matériel avance quand même d'un M-cycle.
    Internal,
}

/// Opération de rotation/décalage CB sur (HL) (D2) : l'octet lu dans Z est transformé puis réécrit en [HL].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RotateKind {
    /// RLC b,(HL) : rotation gauche circulaire ; C=bit7 décalé.
    Rlc,
    /// RRC b,(HL) : rotation droite circulaire ; C=bit0 décalé.
    Rrc,
    /// RL b,(HL) : décalage gauche, l'ancien C entre par le bit 0 ; C=bit7.
    Rl,
    /// RR b,(HL) : décalage droit, l'ancien C entre par le bit 7 ; C=bit0.
    Rr,
    /// SLA b,(HL) : décalage gauche logique, bit 0 mis à 0 ; C=bit7 décalé.
    Sla,
    /// SRA b,(HL) : décalage droit arithmétique, bit 7 conservé ; C=bit0.
    Sra,
    /// SWAP b,(HL) : échange des deux nibbles ; C=0.
    Swap,
    /// SRL b,(HL) : décalage droit logique, bit 7 mis à 0 ; C=bit0 décalé.
    Srl,
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
        // Pour le préfixe CB, l'octet sous-opcode est lu brutalement (décodage) afin de construire le programme ; il est
        // re-lu par ReadPcByte au M-cycle suivant (M2), qui consomme ce M-cycle et avance le PC.
        let cb_sub = if opcode == 0xCB { Some(bus.mmu.read(self.pc)) } else { None };
        if let Some(steps) = ported_steps(opcode, cb_sub) {
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
            MicroOp::ReadPcByteW => {
                let (byte, fd) = bus.read(self.pc); // accès + tick (D1) : msb d'un a16 → latch W
                self.w = byte;
                self.pc = self.pc.wrapping_add(1);
                fd
            }
            MicroOp::ReadMem(addr_src) => {
                let addr = addr_src.address(self);
                let (value, fd) = bus.read(addr); // accès + tick (D1)
                self.z = value;
                fd
            }
            MicroOp::LoadReg(reg, addr_src) => {
                // M-cycle combiné : lit [addr] → Z puis charge le résultat dans the register `reg` — un seul accès + tick.
                let addr = addr_src.address(self);
                let (value, fd) = bus.read(addr); // accès + tick (D1)
                self.z = value;
                reg.store(self, value);           // registre ← Z
                fd
            }
            MicroOp::BitTest(bit) => {
                // M-cycle combiné : lit [HL] → Z puis pose les drapeaux BIT — un seul accès + tick.
                let addr = self.hl();
                let (value, fd) = bus.read(addr); // accès + tick (D1)
                self.z = value;
                let mut f = Flags::H;             // N effacé, H posé
                if ((value >> bit) & 1) == 0 {
                    f |= Flags::Z;               // Z posé si le bit testé vaut 0
                }
                if self.flags().contains(Flags::C) {
                    f |= Flags::C;              // C conservé
                }
                self.set_flags(f);
                fd
            }
            MicroOp::IncHl => {
                // M-cycle combiné : result = Z+1 ; écrit [HL]=result et pose les drapeaux — un seul accès + tick.
                let addr = self.hl();
                let value = self.z.wrapping_add(1);
                let fd = bus.write(addr, value); // accès + tick (D1)
                let c = self.flags().contains(Flags::C);
                let mut f = Flags::empty();      // N effacé
                if value == 0 {
                    f |= Flags::Z;               // Z posé si le résultat vaut 0
                }
                if (value & 0x0F) == 0 {         // H : débordement du demi-mot bas ($0F → $xx)
                    f |= Flags::H;
                }
                if c {
                    f |= Flags::C;              // C conservé
                }
                self.set_flags(f);
                fd
            }
            MicroOp::DecHl => {
                // M-cycle combiné : result = Z-1 ; écrit [HL]=result et pose les drapeaux — un seul accès + tick.
                let addr = self.hl();
                let value = self.z.wrapping_sub(1);
                let fd = bus.write(addr, value); // accès + tick (D1)
                let c = self.flags().contains(Flags::C);
                let mut f = Flags::N;            // N posé
                if value == 0 {
                    f |= Flags::Z;               // Z posé si le résultat vaut 0
                }
                if (value & 0x0F) == 0x0F {      // H : emprunt vers le bit 4 ($00 → $xx)
                    f |= Flags::H;
                }
                if c {
                    f |= Flags::C;              // C conservé
                }
                self.set_flags(f);
                fd
            }
            MicroOp::RotateShiftHl(kind) => {
                // M-cycle combiné : transforme Z selon `kind`, écrit [HL]=result et pose les drapeaux — un seul accès + tick.
                let addr = self.hl();
                let value = self.z;
                let c_in = u8::from(self.flags().contains(Flags::C)); // C d'origine (inchangé par M1..M3)
                let (result, carry_out) = match kind {
                    RotateKind::Rlc => (value.rotate_left(1), (value & 0x80) != 0),
                    RotateKind::Rrc => (value.rotate_right(1), (value & 0x01) != 0),
                    RotateKind::Rl => ((value << 1) | c_in, (value & 0x80) != 0),
                    RotateKind::Rr => ((value >> 1) | if c_in != 0 { 0x80 } else { 0 }, (value & 0x01) != 0),
                    RotateKind::Sla => (value << 1, (value & 0x80) != 0),
                    RotateKind::Sra => ((value >> 1) | (value & 0x80), (value & 0x01) != 0),
                    RotateKind::Swap => ((value >> 4) | (value << 4), false),
                    RotateKind::Srl => (value >> 1, (value & 0x01) != 0),
                };
                let fd = bus.write(addr, result); // accès + tick (D1)
                let mut f = Flags::empty();       // N=0, H=0
                if result == 0 {
                    f |= Flags::Z;               // Z posé si le résultat vaut 0
                }
                if carry_out {
                    f |= Flags::C;              // C = bit décalé
                }
                self.set_flags(f);
                fd
            }
            MicroOp::ResSetHl(is_set, bit) => {
                // M-cycle combiné : efface/pose the bit `bit` de Z, écrit [HL]=result — un seul accès + tick. Pas de drapeaux.
                let addr = self.hl();
                let value = if is_set {
                    self.z | (1 << bit)
                } else {
                    self.z & !(1 << bit)
                };
                bus.write(addr, value) // accès + tick (D1) — les drapeaux ne sont pas modifiés
            }
            MicroOp::WriteMem(addr_src, val_src) => {
                let addr = addr_src.address(self);
                let value = val_src.value(self);
                bus.write(addr, value) // accès + tick (D1)
            }
            MicroOp::StorePcByte(addr_src) => {
                // M-cycle combiné : lit l'octet à PC puis écrit Z en [addr] — deux accès bruts, un seul tick.
                let byte = bus.mmu.read(self.pc); // accès brut (pas de tick) : lit l'opérande à PC
                self.z = byte;
                self.pc = self.pc.wrapping_add(1);
                let addr = addr_src.address(self);
                bus.mmu.write(addr, self.z); // accès brut (pas de tick) : écrit [addr]=Z
                bus.tick_m_cycle()            // un seul M-cycle pour la lecture + l'écriture combinées
            }
            MicroOp::StoreHlDelta(val_src, delta) => {
                // M-cycle combiné : écrit val en [HL] puis HL += delta — accès brut + ajustement, un seul tick.
                let addr = self.hl();                        // adresse courante de (HL), avant ajustement
                bus.mmu.write(addr, val_src.value(self));   // accès brut (pas de tick) : écrit [addr]=val
                let new_hl = (self.hl() as i16).wrapping_add(delta as i16) as u16;
                self.h = (new_hl >> 8) as u8;               // HL += delta
                self.l = new_hl as u8;
                bus.tick_m_cycle()                          // un seul M-cycle pour l'écriture + l'ajustement
            }
            MicroOp::Internal => {
                bus.idle_m_cycle() // pas interne : aucun accès bus, mais le matériel avance d'un M-cycle
            }
        };

        if prog.steps.is_empty() {
            self.in_progress = None; // instruction achevée
            // EI : même logique que le chemin atomique legacy — IME n'est réactivé qu'une fois l'instruction qui suit
            // celle ayant posé `ei_delay` exécutée (Pan Docs). Appliquée une seule fois, à la fin de l'instruction.
            if self.ei_delay > 0 {
                self.ei_delay -= 1;
                if self.ei_delay == 0 {
                    self.ime = true;
                }
            }
        } else {
            self.in_progress = Some(prog); // les phases restantes sont exécutées sur les ticks suivants
        }
        (4, frame_done)
    }
}

/// Les M-cycles restants (après le fetch) d'une instruction non préfixée portée en micro-ops (D2). Étape 2 : les
/// familles portées renvoient leur programme de micro-ops ; toutes les autres opcodes retournent `None` et restent sur
/// le chemin atomique legacy. Le fetch a déjà consommé un M-cycle dans [`CPU::tick`] ; chaque micro-op ci-dessous en
/// consomme un supplémentaire, donc le total = 1 + steps.len() doit égaler le nombre de M-cycles du chemin legacy
/// (vérifié contre `expected_cycles`).
fn ported_steps(opcode: u8, cb_sub: Option<u8>) -> Option<VecDeque<MicroOp>> {
    let mut steps = VecDeque::new();
    match opcode {
        // --- Famille (HL) write ---
        // LD (HL),B/C/D/E/H/L : écrit le registre dans [HL] ; 2 M-cycles (fetch + WriteMem). Pas de drapeaux.
        0x70 => steps.push_back(MicroOp::WriteMem(AddrSrc::Hl, ValSrc::B)),
        0x71 => steps.push_back(MicroOp::WriteMem(AddrSrc::Hl, ValSrc::C)),
        0x72 => steps.push_back(MicroOp::WriteMem(AddrSrc::Hl, ValSrc::D)),
        0x73 => steps.push_back(MicroOp::WriteMem(AddrSrc::Hl, ValSrc::E)),
        0x74 => steps.push_back(MicroOp::WriteMem(AddrSrc::Hl, ValSrc::H)),
        0x75 => steps.push_back(MicroOp::WriteMem(AddrSrc::Hl, ValSrc::L)),
        // LD (HL),A : écrit A dans [HL] ; 2 M-cycles (fetch + WriteMem). Pas de drapeaux.
        0x77 => steps.push_back(MicroOp::WriteMem(AddrSrc::Hl, ValSrc::A)),
        // LD (HL),n8 : lit l'octet à PC → Z, puis écrit Z dans [HL] ; 3 M-cycles (fetch + ReadPcByte + WriteMem).
        // Pas de drapeaux. La lecture et l'écriture sont deux M-cycles distincts (pas de StorePcByte combiné) : la
        // référence (Opcodes.json / Pan Docs) documente LD (HL),n8 à 12 T-cycles, unlike the legacy atomic path (8).
        0x36 => {
            steps.push_back(MicroOp::ReadPcByte);
            steps.push_back(MicroOp::WriteMem(AddrSrc::Hl, ValSrc::Z));
        }
        // LD (HL+),A : écrit A dans [HL] puis HL+=1 ; 2 M-cycles (fetch + StoreHlDelta). Pas de drapeaux.
        0x22 => steps.push_back(MicroOp::StoreHlDelta(ValSrc::A, 1)),
        // LD (HL-),A : écrit A dans [HL] puis HL-=1 ; 2 M-cycles (fetch + StoreHlDelta). Pas de drapeaux.
        0x32 => steps.push_back(MicroOp::StoreHlDelta(ValSrc::A, -1)),

        // --- Famille (HL) read-modify-write : INC (HL) / DEC (HL) ---
        // GBCTR chapitre 6 : la lecture mémoire est le M-cycle avant-dernier et l'écriture + les drapeaux sont le
        // dernier M-cycle. 3 M-cycles au total (fetch + ReadMem(Hl) + IncHl/DecHl).
        0x34 => {
            steps.push_back(MicroOp::ReadMem(AddrSrc::Hl)); // M2 : [HL] → Z
            steps.push_back(MicroOp::IncHl);                // M3 : [HL]=Z+1 + drapeaux (Z, N=0, H, C inchangé)
        }
        0x35 => {
            steps.push_back(MicroOp::ReadMem(AddrSrc::Hl)); // M2 : [HL] → Z
            steps.push_back(MicroOp::DecHl);                // M3 : [HL]=Z-1 + drapeaux (Z, N=1, H, C inchangé)
        }

        // --- Famille (HL) read : LD r,(HL) ---
        // Lit [HL] → Z puis charge le résultat dans le registre `r` ; 2 M-cycles (fetch + LoadReg). Pas de drapeaux.
        // GBCTR chapitre 6 : la lecture mémoire et la mise en registre sont le même M-cycle (le dernier).
        0x46 => steps.push_back(MicroOp::LoadReg(ValSrc::B, AddrSrc::Hl)),
        0x4E => steps.push_back(MicroOp::LoadReg(ValSrc::C, AddrSrc::Hl)),
        0x56 => steps.push_back(MicroOp::LoadReg(ValSrc::D, AddrSrc::Hl)),
        0x5E => steps.push_back(MicroOp::LoadReg(ValSrc::E, AddrSrc::Hl)),
        0x66 => steps.push_back(MicroOp::LoadReg(ValSrc::H, AddrSrc::Hl)),
        0x6E => steps.push_back(MicroOp::LoadReg(ValSrc::L, AddrSrc::Hl)),
        0x7E => steps.push_back(MicroOp::LoadReg(ValSrc::A, AddrSrc::Hl)),

        // --- LD A,(BC) / LD A,(DE) : lit [BC]/[DE] → A ; 2 M-cycles (fetch + LoadReg). Pas de drapeaux. ---
        0x0A => steps.push_back(MicroOp::LoadReg(ValSrc::A, AddrSrc::Bc)),
        0x1A => steps.push_back(MicroOp::LoadReg(ValSrc::A, AddrSrc::De)),

        // --- LDH A,(n8) : lit n8 → Z puis [FF00+Z] → A ; 3 M-cycles (fetch + ReadPcByte + LoadReg). Pas de drapeaux. ---
        0xF0 => {
            steps.push_back(MicroOp::ReadPcByte); // n8 → Z
            steps.push_back(MicroOp::LoadReg(ValSrc::A, AddrSrc::HBankZ)); // [FF00+Z] → A
        }

        // --- LD A,[C] : lit [FF00+C] → A ; 2 M-cycles (fetch + LoadReg). Pas de drapeaux. ---
        0xF2 => steps.push_back(MicroOp::LoadReg(ValSrc::A, AddrSrc::HBankC)),

        // --- LD A,(a16) : lit lsb → Z, msb → W, puis [(W<<8)|Z] → A ; 4 M-cycles (fetch + ReadPcByte + ReadPcByteW + LoadReg).
        // Pas de drapeaux. GBCTR chapitre 6 : la lecture mémoire est le dernier M-cycle. ---
        0xFA => {
            steps.push_back(MicroOp::ReadPcByte);    // lsb → Z
            steps.push_back(MicroOp::ReadPcByteW);   // msb → W
            steps.push_back(MicroOp::LoadReg(ValSrc::A, AddrSrc::Wz)); // [(W<<8)|Z] → A
        }

        // --- CB b,(HL) : lit le sous-opcode (M2), puis [HL] → Z (M3), then the last M-cycle acts on [HL]. ---
        // Seules the formes sur (HL) are portées ; les autres restent sur le chemin legacy. GBCTR chapitre 6 : la lecture
        // mémoire est le M-cycle avant-dernier et l'écriture (+ drapeaux, s'il y en a) is the last M-cycle.
        0xCB => {
            let sub = match cb_sub {
                Some(s) => s,
                None => return None, // pas de sous-opcode : non porté
            };
            if (sub & 0xC7) == 0x46 {
                // BIT b,(HL) : 3 M-cycles (fetch $CB + ReadPcByte + BitTest). Le bit testé est dans les bits 5-3.
                steps.push_back(MicroOp::ReadPcByte);          // M2 : sous-opcode → Z, PC+=1
                steps.push_back(MicroOp::BitTest((sub >> 3) & 7)); // M3 : [HL] → Z + drapeaux BIT
            } else if sub & 0x07 == 6 && (sub & 0xC0) == 0x00 {
                // RLC/RRC/RL/RR/SLA/SRA/SWAP/SRL b,(HL) : 4 M-cycles (fetch $CB + ReadPcByte + ReadMem(Hl) + RotateShiftHl).
                steps.push_back(MicroOp::ReadPcByte);           // M2 : sous-opcode → Z, PC+=1
                steps.push_back(MicroOp::ReadMem(AddrSrc::Hl)); // M3 : [HL] → Z
                let kind = match sub {
                    0x06 => RotateKind::Rlc,
                    0x0E => RotateKind::Rrc,
                    0x16 => RotateKind::Rl,
                    0x1E => RotateKind::Rr,
                    0x26 => RotateKind::Sla,
                    0x2E => RotateKind::Sra,
                    0x36 => RotateKind::Swap,
                    _ => RotateKind::Srl, // 0x3E
                };
                steps.push_back(MicroOp::RotateShiftHl(kind)); // M4 : [HL]=transform(Z) + drapeaux (Z, N=0, H=0, C)
            } else if sub & 0x07 == 6 && (sub & 0xC0) == 0x80 {
                // RES b,(HL) : efface the bit ; 4 M-cycles (fetch $CB + ReadPcByte + ReadMem(Hl) + ResSetHl). Pas de drapeaux.
                steps.push_back(MicroOp::ReadPcByte);           // M2 : sous-opcode → Z, PC+=1
                steps.push_back(MicroOp::ReadMem(AddrSrc::Hl)); // M3 : [HL] → Z
                steps.push_back(MicroOp::ResSetHl(false, (sub >> 3) & 7)); // M4 : [HL]=Z & !(1<<bit)
            } else if sub & 0x07 == 6 && (sub & 0xC0) == 0xC0 {
                // SET b,(HL) : pose the bit ; 4 M-cycles (fetch $CB + ReadPcByte + ReadMem(Hl) + ResSetHl). Pas de drapeaux.
                steps.push_back(MicroOp::ReadPcByte);           // M2 : sous-opcode → Z, PC+=1
                steps.push_back(MicroOp::ReadMem(AddrSrc::Hl)); // M3 : [HL] → Z
                steps.push_back(MicroOp::ResSetHl(true, (sub >> 3) & 7)); // M4 : [HL]=Z | (1<<bit)
            } else {
                return None; // autre forme CB — reste sur le chemin legacy
            }
        }

        _ => return None,
    }
    Some(steps)
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

    /// Étape 2 : la famille (HL) write portée en micro-ops produit les mêmes effets d'observation que le chemin legacy —
    /// registres, mémoire et nombre de T-cycles — pour LD (HL),A / LD (HL),n8 / LD (HL+),A / LD (HL-),A.
    #[test]
    fn ported_hl_write_family_matches_legacy_side_effects() {
        // Exécute l'instruction placée à $0100 jusqu'à son achèvement et renvoie le total de T-cycles consommés.
        fn tick_until_done(cpu: &mut CPU, bus: &mut Bus) -> u32 {
            let mut total = 0u32;
            loop {
                let (cycles, _) = cpu.tick(bus);
                total += cycles;
                if cpu.in_progress.is_none() {
                    break;
                }
            }
            total
        }

        // LD (HL),A : [HL]=A ; pas de drapeaux ; 8 T-cycles.
        let mut mmu = MMU::new();
        let mut rom = vec![0x00u8; 0x4000];
        rom[0x0100] = 0x77;
        mmu.load_rom(rom);
        let mut cpu = CPU::new();
        cpu.pc = 0x0100;
        cpu.a = 0xAB;
        cpu.h = 0xC0; // HL=$C050 (WRAM, toujours écritable)
        cpu.l = 0x50;
        let f_before = cpu.f;
        {
            let mut bus = Bus::new(&mut mmu);
            assert_eq!(tick_until_done(&mut cpu, &mut bus), 8);
        }
        assert_eq!(mmu.read(0xC050), 0xAB);
        assert_eq!(cpu.a, 0xAB);
        assert_eq!(cpu.hl(), 0xC050);
        assert_eq!(cpu.f, f_before);

        // LD (HL),n8 : [HL]=n8 lu à PC+1 ; pas de drapeaux ; 12 T-cycles (3 M-cycles) — conforme à la référence.
        let mut mmu = MMU::new();
        let mut rom = vec![0x00u8; 0x4000];
        rom[0x0100] = 0x36;
        rom[0x0101] = 0xCD; // opérande n8
        mmu.load_rom(rom);
        let mut cpu = CPU::new();
        cpu.pc = 0x0100;
        cpu.a = 0xAB;
        cpu.h = 0xC0;
        cpu.l = 0x50;
        let f_before = cpu.f;
        {
            let mut bus = Bus::new(&mut mmu);
            assert_eq!(tick_until_done(&mut cpu, &mut bus), 12);
        }
        assert_eq!(mmu.read(0xC050), 0xCD);
        assert_eq!(cpu.pc, 0x0102); // PC passe l'opérande n8
        assert_eq!(cpu.hl(), 0xC050);
        assert_eq!(cpu.f, f_before);

        // LD (HL+),A : [HL]=A puis HL+=1 ; pas de drapeaux ; 8 T-cycles.
        let mut mmu = MMU::new();
        let mut rom = vec![0x00u8; 0x4000];
        rom[0x0100] = 0x22;
        mmu.load_rom(rom);
        let mut cpu = CPU::new();
        cpu.pc = 0x0100;
        cpu.a = 0xAB;
        cpu.h = 0xC0;
        cpu.l = 0x50;
        let f_before = cpu.f;
        {
            let mut bus = Bus::new(&mut mmu);
            assert_eq!(tick_until_done(&mut cpu, &mut bus), 8);
        }
        assert_eq!(mmu.read(0xC050), 0xAB);
        assert_eq!(cpu.hl(), 0xC051); // HL+=1
        assert_eq!(cpu.f, f_before);

        // LD (HL-),A : [HL]=A puis HL-=1 ; pas de drapeaux ; 8 T-cycles.
        let mut mmu = MMU::new();
        let mut rom = vec![0x00u8; 0x4000];
        rom[0x0100] = 0x32;
        mmu.load_rom(rom);
        let mut cpu = CPU::new();
        cpu.pc = 0x0100;
        cpu.a = 0xAB;
        cpu.h = 0xC0;
        cpu.l = 0x50;
        let f_before = cpu.f;
        {
            let mut bus = Bus::new(&mut mmu);
            assert_eq!(tick_until_done(&mut cpu, &mut bus), 8);
        }
        assert_eq!(mmu.read(0xC050), 0xAB);
        assert_eq!(cpu.hl(), 0xC04F); // HL-=1
        assert_eq!(cpu.f, f_before);

        // LD (HL),r : [HL]=r ; pas de drapeaux ; 8 T-cycles — pour r in B,C,D,E,H,L ($70-$75).
        let hl_write_cases = [
            (0x70u8, 0x11), // B
            (0x71, 0x22),   // C
            (0x72, 0x33),   // D
            (0x73, 0x44),   // E
            (0x74, 0xC0),   // H
            (0x75, 0x50),   // L
        ];
        for (opcode, expected) in hl_write_cases {
            let mut mmu = MMU::new();
            let mut rom = vec![0x00u8; 0x4000];
            rom[0x0100] = opcode;
            mmu.load_rom(rom);
            let mut cpu = CPU::new();
            cpu.pc = 0x0100;
            // Registres distincts pour identifier la source ; HL=$C050 (WRAM, toujours écritable).
            cpu.b = 0x11;
            cpu.c = 0x22;
            cpu.d = 0x33;
            cpu.e = 0x44;
            cpu.h = 0xC0;
            cpu.l = 0x50;
            let f_before = cpu.f;
            {
                let mut bus = Bus::new(&mut mmu);
                assert_eq!(tick_until_done(&mut cpu, &mut bus), 8);
            }
            assert_eq!(mmu.read(0xC050), expected); // [HL]=r
            assert_eq!(cpu.hl(), 0xC050);          // HL inchangé
            assert_eq!(cpu.f, f_before);           // pas de drapeaux
        }
    }

    /// Étape 2 : prouve que la famille (HL) write portée s'exécute VRAIMENT pas-à-pas — et non « 8 T consommés au
    /// total, peu importe comment ». Chaque M-cycle est un appel distinct de [`CPU::tick`] qui avance le matériel
    /// d'exactement un M-cycle (4 T), et l'écriture bus a lieu au dernier M-cycle, pas avant. On observe l'état
    /// intermédiaire entre les deux M-cycles ([HL] encore non-écrit après le fetch) ainsi qu'une horloge matérielle
    /// indépendante (le Timer) qui ne peut avancer que si chaque `tick_m_cycle` a bien fait progresser le matériel.
    #[test]
    fn ported_hl_write_family_interleaves_m_cycles() {
        let cases = [0x70u8, 0x71, 0x72, 0x73, 0x74, 0x75, 0x77, 0x22, 0x32]; // $36 (LD (HL),n8) traité séparément : 3 M-cycles
        for opcode in cases {
            let mut mmu = MMU::new();
            let mut rom = vec![0x00u8; 0x4000];
            rom[0x0100] = opcode;
            mmu.load_rom(rom);
            let mut cpu = CPU::new();
            cpu.pc = 0x0100;
            cpu.a = 0xAB;
            cpu.b = 0x11;
            cpu.c = 0x22;
            cpu.d = 0x33;
            cpu.e = 0x44;
            cpu.h = 0xC0; // HL=$C050 (WRAM, toujours écritable)
            cpu.l = 0x50;
            let sentinel = 0xEE;
            mmu.write(0xC050, sentinel); // valeur initiale de [HL] : doit survivre au fetch

            let mut bus = Bus::new(&mut mmu);
            // M-cycle #1 (le fetch) : un seul appel tick avance le matériel d'un M-cycle ; [HL] n'est PAS encore écrit.
            let (c1, _) = cpu.tick(&mut bus);
            assert_eq!(c1, 4, "${:02X} : le fetch doit consommer exactement un M-cycle (4 T)", opcode);
            assert!(cpu.in_progress.is_some(), "l'instruction doit être en cours après le fetch");
            assert_eq!(
                bus.mmu.read(0xC050), // lecture brute (pas de tick) : [HL] pendant le fetch
                sentinel,
                "[HL] doit rester non-écrit pendant le fetch (pas d'écriture anticipée)"
            );

            // M-cycle #2 (l'écriture bus) : un second appel tick avance le matériel d'un autre M-cycle ; [HL] est écrit ici.
            let (c2, _) = cpu.tick(&mut bus);
            assert_eq!(c2, 4, "${:02X} : l'écriture doit consommer exactement un M-cycle (4 T)", opcode);
            assert!(cpu.in_progress.is_none(), "l'instruction doit être achevée après le write");

            let expected = match opcode {
                0x70 => 0x11, // B
                0x71 => 0x22, // C
                0x72 => 0x33, // D
                0x73 => 0x44, // E
                0x74 => 0xC0, // H
                0x75 => 0x50, // L
                _ => 0xAB,     // $77 (A) ; $22/$32 (A)
            };
            assert_eq!(bus.mmu.read(0xC050), expected, "${:02X} : [HL] doit être écrit au 2ᵉ M-cycle", opcode);
        }

        // LD (HL),n8 ($36) : 3 M-cycles distincts — fetch → lecture de n8 dans Z ([HL] toujours non-écrit) → écriture de Z.
        {
            let mut mmu = MMU::new();
            let mut rom = vec![0x00u8; 0x4000];
            rom[0x0100] = 0x36;
            rom[0x0101] = 0xCD; // opérande n8 de LD (HL),n8
            mmu.load_rom(rom);
            let mut cpu = CPU::new();
            cpu.pc = 0x0100;
            cpu.a = 0xAB;
            cpu.h = 0xC0; // HL=$C050 (WRAM, toujours écritable)
            cpu.l = 0x50;
            let sentinel = 0xEE;
            mmu.write(0xC050, sentinel);

            let mut bus = Bus::new(&mut mmu);
            // M-cycle #1 (le fetch) : [HL] non-écrit.
            let (c1, _) = cpu.tick(&mut bus);
            assert_eq!(c1, 4, "$36 : le fetch doit consommer exactement un M-cycle (4 T)");
            assert!(cpu.in_progress.is_some(), "l'instruction doit être en cours après le fetch");
            assert_eq!(bus.mmu.read(0xC050), sentinel, "[HL] non-écrit pendant le fetch");

            // M-cycle #2 (lecture de n8 → Z) : [HL] toujours non-écrit.
            let (c2, _) = cpu.tick(&mut bus);
            assert_eq!(c2, 4, "$36 : la lecture de n8 doit consommer exactement un M-cycle (4 T)");
            assert!(cpu.in_progress.is_some(), "l'instruction doit être en cours après la lecture de n8");
            assert_eq!(bus.mmu.read(0xC050), sentinel, "[HL] non-écrit pendant la lecture de n8 (pas d'écriture anticipée)");

            // M-cycle #3 (écriture Z → [HL]) : [HL] finally correct.
            let (c3, _) = cpu.tick(&mut bus);
            assert_eq!(c3, 4, "$36 : l'écriture doit consommer exactement un M-cycle (4 T)");
            assert!(cpu.in_progress.is_none(), "l'instruction doit être achevée après l'écriture");
            assert_eq!(bus.mmu.read(0xC050), 0xCD, "[HL] écrit au 3ᵉ M-cycle = n8 lu à PC+1");
        }

        // Horloge matérielle indépendante (le Timer) : chaque appel tick avance le matériel d'EXACTEMENT un M-cycle.
        // TAC=$05 active le timer avec la période la plus fine (16 T). Huit instructions portées = 16 M-cycles = 64 T.
        // Le décalage structurel d'1 M-cycle (RELOAD_WINDOW_LEN) retarde l'action de chaque front relativement au wrap physique :
        // sur ces 64 T, seuls les fronts aux points effectifs e=20/36/52 tombent dans la fenêtre (le 4ᵉ, à e=68, passe au-delà),
        // donc TIMA doit valoir 3 — ce qui ne tient que si chaque tick a bien fait progresser le matériel de 4 T.
        let mut mmu = MMU::new();
        let rom = vec![0x77u8; 0x4000]; // LD (HL),A répété : huit instructions consécutives à $0100-$0107
        let mut cpu = CPU::new();
        cpu.pc = 0x0100;
        cpu.a = 0xAB;
        cpu.h = 0xC0;
        cpu.l = 0x50;
        mmu.load_rom(rom);
        mmu.write(0xFF07, 0x05); // TAC : timer activé (bit 2) + période la plus fine (bits 1-0 = $01 → 16 T)
        assert_eq!(mmu.read(0xFF05), 0x00); // TIMA à zéro au départ
        let mut bus = Bus::new(&mut mmu);
        for _ in 0..8 {
            cpu.tick(&mut bus); // fetch d'une instruction (4 T)
            cpu.tick(&mut bus); // écriture de l'instruction (4 T)
        }
        assert_eq!(cpu.pc, 0x0108); // les huit instructions ont bien été exécutées
        assert_eq!(
            bus.mmu.read(0xFF05), // lecture brute (pas de tick) : TIMA après 64 T
            3,
            "TIMA doit valoir 3 sur ces 64 T (décalage structurel d'1 M-cycle, RELOAD_WINDOW_LEN) : chaque tick a avancé le matériel d'un M-cycle"
        );
    }

    /// Étape 2 : la famille (HL) read portée en micro-ops produit les mêmes effets d'observation que le chemin legacy —
    /// registres, mémoire et nombre de T-cycles — for LD r,(HL) / LD A,(BC/DE) / LDH A,(n8) / LD A,[C] / LD A,(a16).
    #[test]
    fn ported_hl_read_family_matches_legacy_side_effects() {
        // Exécute l'instruction placée à $0100 until its completion and returns the total T-cycles consumed.
        fn tick_until_done(cpu: &mut CPU, bus: &mut Bus) -> u32 {
            let mut total = 0u32;
            loop {
                let (cycles, _) = cpu.tick(bus);
                total += cycles;
                if cpu.in_progress.is_none() {
                    break;
                }
            }
            total
        }

        // LD r,(HL) : [HL]→r ; pas de drapeaux ; 8 T-cycles — for r in B,C,D,E,H,L,A ($46/$4E/$56/$5E/$66/$6E/$7E).
        let hl_read_cases = [0x46u8, 0x4E, 0x56, 0x5E, 0x66, 0x6E, 0x7E];
        for opcode in hl_read_cases {
            let mut mmu = MMU::new();
            let mut rom = vec![0x00u8; 0x4000];
            rom[0x0100] = opcode;
            mmu.load_rom(rom);
            let mut cpu = CPU::new();
            cpu.pc = 0x0100;
            // Registres distincts pour identifier la cible ; [HL]=0xAB (WRAM, toujours lisible).
            cpu.a = 0xAA;
            cpu.b = 0xBB;
            cpu.c = 0xCC;
            cpu.d = 0xDD;
            cpu.e = 0xEE;
            cpu.h = 0xC0; // HL=$C050 (WRAM)
            cpu.l = 0x50;
            mmu.write(0xC050, 0xAB);
            let f_before = cpu.f;
            {
                let mut bus = Bus::new(&mut mmu);
                assert_eq!(tick_until_done(&mut cpu, &mut bus), 8);
            }
            match opcode {
                0x46 => assert_eq!(cpu.b, 0xAB), // B←[HL]
                0x4E => assert_eq!(cpu.c, 0xAB), // C←[HL]
                0x56 => assert_eq!(cpu.d, 0xAB), // D←[HL]
                0x5E => assert_eq!(cpu.e, 0xAB), // E←[HL]
                0x66 => assert_eq!(cpu.h, 0xAB), // H←[HL]
                0x6E => assert_eq!(cpu.l, 0xAB), // L←[HL]
                0x7E => assert_eq!(cpu.a, 0xAB), // A←[HL]
                _ => unreachable!(),
            }
            let expected_hl = match opcode {
                0x66 => 0xAB50, // H←[HL] modifie HL
                0x6E => 0xC0AB, // L←[HL] modifie HL
                _ => 0xC050,     // HL inchangé
            };
            assert_eq!(cpu.hl(), expected_hl);
            assert_eq!(cpu.f, f_before); // pas de drapeaux
        }

        // LD A,(BC) : [BC]→A ; pas de drapeaux ; 8 T-cycles.
        {
            let mut mmu = MMU::new();
            let mut rom = vec![0x00u8; 0x4000];
            rom[0x0100] = 0x0A;
            mmu.load_rom(rom);
            let mut cpu = CPU::new();
            cpu.pc = 0x0100;
            cpu.a = 0xAA;
            cpu.b = 0xC0; // BC=$C050 (WRAM)
            cpu.c = 0x50;
            mmu.write(0xC050, 0xAB);
            let f_before = cpu.f;
            {
                let mut bus = Bus::new(&mut mmu);
                assert_eq!(tick_until_done(&mut cpu, &mut bus), 8);
            }
            assert_eq!(cpu.a, 0xAB); // A←[BC]
            assert_eq!(cpu.f, f_before); // pas de drapeaux
        }

        // LD A,(DE) : [DE]→A ; pas de drapeaux ; 8 T-cycles.
        {
            let mut mmu = MMU::new();
            let mut rom = vec![0x00u8; 0x4000];
            rom[0x0100] = 0x1A;
            mmu.load_rom(rom);
            let mut cpu = CPU::new();
            cpu.pc = 0x0100;
            cpu.a = 0xAA;
            cpu.d = 0xC0; // DE=$C050 (WRAM)
            cpu.e = 0x50;
            mmu.write(0xC050, 0xAB);
            let f_before = cpu.f;
            {
                let mut bus = Bus::new(&mut mmu);
                assert_eq!(tick_until_done(&mut cpu, &mut bus), 8);
            }
            assert_eq!(cpu.a, 0xAB); // A←[DE]
            assert_eq!(cpu.f, f_before); // pas de drapeaux
        }

        // LDH A,(n8) : [FF00+n8]→A ; pas de drapeaux ; 12 T-cycles (3 M-cycles). PC passe l'opérande n8.
        {
            let mut mmu = MMU::new();
            let mut rom = vec![0x00u8; 0x4000];
            rom[0x0100] = 0xF0;
            rom[0x0101] = 0xC0; // n8 → port $FFC0 (HRAM)
            mmu.load_rom(rom);
            let mut cpu = CPU::new();
            cpu.pc = 0x0100;
            cpu.a = 0xAA;
            mmu.write(0xFFC0, 0xCD);
            let f_before = cpu.f;
            {
                let mut bus = Bus::new(&mut mmu);
                assert_eq!(tick_until_done(&mut cpu, &mut bus), 12);
            }
            assert_eq!(cpu.a, 0xCD); // A←[FFC0]
            assert_eq!(cpu.pc, 0x0102); // PC passe l'opérande n8
            assert_eq!(cpu.f, f_before); // pas de drapeaux
        }

        // LD A,[C] : [FF00+C]→A ; pas de drapeaux ; 8 T-cycles.
        {
            let mut mmu = MMU::new();
            let mut rom = vec![0x00u8; 0x4000];
            rom[0x0100] = 0xF2;
            mmu.load_rom(rom);
            let mut cpu = CPU::new();
            cpu.pc = 0x0100;
            cpu.a = 0xAA;
            cpu.c = 0xC0; // port $FFC0 (HRAM)
            mmu.write(0xFFC0, 0xAB);
            let f_before = cpu.f;
            {
                let mut bus = Bus::new(&mut mmu);
                assert_eq!(tick_until_done(&mut cpu, &mut bus), 8);
            }
            assert_eq!(cpu.a, 0xAB); // A←[FFC0]
            assert_eq!(cpu.f, f_before); // pas de drapeaux
        }

        // LD A,(a16) : [a16]→A ; pas de drapeaux ; 16 T-cycles (4 M-cycles). PC passe l'opérande a16.
        {
            let mut mmu = MMU::new();
            let mut rom = vec![0x00u8; 0x4000];
            rom[0x0100] = 0xFA;
            rom[0x0101] = 0x50; // a16 lsb → $C050 (WRAM)
            rom[0x0102] = 0xC0; // a16 msb
            mmu.load_rom(rom);
            let mut cpu = CPU::new();
            cpu.pc = 0x0100;
            cpu.a = 0xAA;
            mmu.write(0xC050, 0xAB);
            let f_before = cpu.f;
            {
                let mut bus = Bus::new(&mut mmu);
                assert_eq!(tick_until_done(&mut cpu, &mut bus), 16);
            }
            assert_eq!(cpu.a, 0xAB); // A←[$C050]
            assert_eq!(cpu.pc, 0x0103); // PC passe l'opérande a16
            assert_eq!(cpu.f, f_before); // pas de drapeaux
        }

        // CB BIT b,(HL) : [HL]→Z + drapeaux (Z=(bit==0), N=0, H=1, C conservé) ; 12 T-cycles (3 M-cycles).
        {
            let mut mmu = MMU::new();
            let mut rom = vec![0x00u8; 0x4000];
            rom[0x0100] = 0xCB;
            rom[0x0101] = 0x7E; // BIT 7,(HL)
            mmu.load_rom(rom);
            let mut cpu = CPU::new();
            cpu.pc = 0x0100;
            cpu.h = 0xC0; // HL=$C050 (WRAM)
            cpu.l = 0x50;
            mmu.write(0xC050, 0x80); // bit 7 posé → Z=0
            let c_before = cpu.flags().contains(Flags::C);
            {
                let mut bus = Bus::new(&mut mmu);
                assert_eq!(tick_until_done(&mut cpu, &mut bus), 12);
            }
            let f = cpu.flags(); // BIT 7,(HL) avec [HL]=0x80 : bit 7 posé → Z=0 ; N=0, H=1.
            assert!(!f.contains(Flags::Z));
            assert!(!f.contains(Flags::N));
            assert!(f.contains(Flags::H));
            assert_eq!(f.contains(Flags::C), c_before); // C conservé
        }
    }

    /// Étape 2 : prouve que la famille (HL) read portée s'exécute VRAIMENT pas-à-pas — and non « N T consommés au
    /// total, peu importe how ». Chaque M-cycle is an appel distinct de [`CPU::tick`] qui avance le matériel d'exactement
    /// un M-cycle (4 T), et the lecture bus a lieu au dernier M-cycle, pas avant. On observe l'état intermédiaire entre
    /// les M-cycles (le registre cible encore non chargé after the fetch) ainsi qu'une horloge matérielle indépendante
    /// (the Timer) qui ne peut avancer que si each `tick_m_cycle` a bien fait progresser le matériel.
    #[test]
    fn ported_hl_read_family_interleaves_m_cycles() {
        // LD r,(HL) : 2 M-cycles distincts — fetch → [HL]→r (the registre cible is loaded au 2ᵉ M-cycle, pas avant).
        let cases = [0x46u8, 0x4E, 0x56, 0x5E, 0x66, 0x6E, 0x7E];
        for opcode in cases {
            let mut mmu = MMU::new();
            let mut rom = vec![0x00u8; 0x4000];
            rom[0x0100] = opcode;
            mmu.load_rom(rom);
            let mut cpu = CPU::new();
            cpu.pc = 0x0100;
            // Valeurs initiales distinctes : the registre cible doit rester intact pendant the fetch.
            cpu.a = 0xAA;
            cpu.b = 0xBB;
            cpu.c = 0xCC;
            cpu.d = 0xDD;
            cpu.e = 0xEE;
            cpu.h = 0xC0; // HL=$C050 (WRAM)
            cpu.l = 0x50;
            mmu.write(0xC050, 0xAB);

            let mut bus = Bus::new(&mut mmu);
            // M-cycle #1 (the fetch) : un seul appel tick avance the matériel d'un M-cycle ; the registre cible is NOT yet loaded.
            let (c1, _) = cpu.tick(&mut bus);
            assert_eq!(c1, 4, "${:02X} : the fetch doit consommer exactement un M-cycle (4 T)", opcode);
            assert!(cpu.in_progress.is_some(), "l'instruction doit être en cours after the fetch");
            match opcode {
                0x46 => assert_eq!(cpu.b, 0xBB), // B encore non chargé pendant the fetch
                0x4E => assert_eq!(cpu.c, 0xCC),
                0x56 => assert_eq!(cpu.d, 0xDD),
                0x5E => assert_eq!(cpu.e, 0xEE),
                0x66 => assert_eq!(cpu.h, 0xC0),
                0x6E => assert_eq!(cpu.l, 0x50),
                0x7E => assert_eq!(cpu.a, 0xAA),
                _ => unreachable!(),
            }

            // M-cycle #2 (the lecture [HL]→r) : un second appel tick ; the registre cible is loaded here.
            let (c2, _) = cpu.tick(&mut bus);
            assert_eq!(c2, 4, "${:02X} : the lecture doit consommer exactement un M-cycle (4 T)", opcode);
            assert!(cpu.in_progress.is_none(), "l'instruction doit être achevée after the lecture");
            match opcode {
                0x46 => assert_eq!(cpu.b, 0xAB), // B←[HL] au 2ᵉ M-cycle
                0x4E => assert_eq!(cpu.c, 0xAB),
                0x56 => assert_eq!(cpu.d, 0xAB),
                0x5E => assert_eq!(cpu.e, 0xAB),
                0x66 => assert_eq!(cpu.h, 0xAB),
                0x6E => assert_eq!(cpu.l, 0xAB),
                0x7E => assert_eq!(cpu.a, 0xAB),
                _ => unreachable!(),
            }
        }

        // LDH A,(n8) ($F0) : 3 M-cycles distincts — fetch → lecture de n8 (A encore non chargé) → [FF00+n8]→A.
        {
            let mut mmu = MMU::new();
            let mut rom = vec![0x00u8; 0x4000];
            rom[0x0100] = 0xF0;
            rom[0x0101] = 0xC0; // n8 → port $FFC0 (HRAM)
            mmu.load_rom(rom);
            let mut cpu = CPU::new();
            cpu.pc = 0x0100;
            cpu.a = 0xAA;
            mmu.write(0xFFC0, 0xCD);

            let mut bus = Bus::new(&mut mmu);
            // M-cycle #1 (the fetch) : A non chargé.
            let (c1, _) = cpu.tick(&mut bus);
            assert_eq!(c1, 4, "$F0 : the fetch doit consommer exactement un M-cycle (4 T)");
            assert!(cpu.in_progress.is_some(), "l'instruction doit être en cours after the fetch");
            assert_eq!(cpu.a, 0xAA, "A non chargé pendant the fetch");

            // M-cycle #2 (lecture de n8 → Z) : A encore non chargé.
            let (c2, _) = cpu.tick(&mut bus);
            assert_eq!(c2, 4, "$F0 : the lecture de n8 doit consommer exactement un M-cycle (4 T)");
            assert!(cpu.in_progress.is_some(), "l'instruction doit être en cours after the lecture de n8");
            assert_eq!(cpu.a, 0xAA, "A non chargé pendant the lecture de n8 (pas de chargement anticipé)");

            // M-cycle #3 ([FFC0]→A) : A finally correct.
            let (c3, _) = cpu.tick(&mut bus);
            assert_eq!(c3, 4, "$F0 : the lecture doit consommer exactement un M-cycle (4 T)");
            assert!(cpu.in_progress.is_none(), "l'instruction doit être achevée after the lecture");
            assert_eq!(cpu.a, 0xCD, "A←[FFC0] au 3ᵉ M-cycle = n8 lu à PC+1");
        }

        // CB BIT b,(HL) : 3 M-cycles distincts — fetch $CB → sous-opcode (drapeaux not yet posés) → [HL]+drapeaux.
        {
            let mut mmu = MMU::new();
            let mut rom = vec![0x00u8; 0x4000];
            rom[0x0100] = 0xCB;
            rom[0x0101] = 0x7E; // BIT 7,(HL)
            mmu.load_rom(rom);
            let mut cpu = CPU::new();
            cpu.pc = 0x0100;
            cpu.h = 0xC0; // HL=$C050 (WRAM)
            cpu.l = 0x50;
            mmu.write(0xC050, 0x80); // bit 7 posé → Z=0 au M-cycle #3
            let f_before = cpu.f;

            let mut bus = Bus::new(&mut mmu);
            // M-cycle #1 (the fetch $CB) : drapeaux inchangés.
            let (c1, _) = cpu.tick(&mut bus);
            assert_eq!(c1, 4, "CB BIT b,(HL) : the fetch doit consommer exactement un M-cycle (4 T)");
            assert!(cpu.in_progress.is_some(), "l'instruction doit être en cours after the fetch $CB");
            assert_eq!(cpu.f, f_before, "drapeaux inchangés pendant the fetch $CB");

            // M-cycle #2 (sous-opcode → Z) : drapeaux still not posés.
            let (c2, _) = cpu.tick(&mut bus);
            assert_eq!(c2, 4, "CB BIT b,(HL) : the lecture du sous-opcode doit consommer exactement un M-cycle (4 T)");
            assert!(cpu.in_progress.is_some(), "l'instruction doit être en cours after the sous-opcode");
            assert_eq!(cpu.f, f_before, "drapeaux not yet posés pendant the lecture du sous-opcode");

            // M-cycle #3 ([HL]→Z + drapeaux) : Z=0 (bit 7 posé), N=0, H=1.
            let (c3, _) = cpu.tick(&mut bus);
            assert_eq!(c3, 4, "CB BIT b,(HL) : the lecture [HL]+drapeaux doit consommer exactement un M-cycle (4 T)");
            assert!(cpu.in_progress.is_none(), "l'instruction doit être achevée after the drapeaux");
            let f = cpu.flags();
            assert!(!f.contains(Flags::Z), "Z=0 car le bit 7 de [HL]=0x80 est posé");
            assert!(f.contains(Flags::H), "H posé par BIT");
        }

        // Horloge matérielle indépendante (le Timer) : chaque appel tick avance le matériel d'EXACTEMENT un M-cycle.
        // TAC=$05 active le timer avec la période la plus fine (16 T). Huit instructions portées = 16 M-cycles = 64 T.
        // Le décalage structurel d'1 M-cycle (RELOAD_WINDOW_LEN) retarde l'action de chaque front relativement au wrap physique :
        // sur ces 64 T, seuls les fronts aux points effectifs e=20/36/52 tombent dans la fenêtre (le 4ᵉ, à e=68, passe au-delà),
        // donc TIMA doit valoir 3 — ce qui ne tient que si chaque tick a bien fait progresser le matériel de 4 T.
        let mut mmu = MMU::new();
        let rom = vec![0x7Eu8; 0x4000]; // LD A,(HL) répété : huit instructions consécutives à $0100-$0107
        let mut cpu = CPU::new();
        cpu.pc = 0x0100;
        cpu.a = 0xAA;
        cpu.h = 0xC0; // HL=$C050 (WRAM)
        cpu.l = 0x50;
        mmu.load_rom(rom);
        mmu.write(0xFF07, 0x05); // TAC : timer activé (bit 2) + period la plus fine (bits 1-0 = $01 → 16 T)
        assert_eq!(mmu.read(0xFF05), 0x00); // TIMA à zéro au départ
        let mut bus = Bus::new(&mut mmu);
        for _ in 0..8 {
            cpu.tick(&mut bus); // fetch d'une instruction (4 T)
            cpu.tick(&mut bus); // lecture [HL]→A de l'instruction (4 T)
        }
        assert_eq!(cpu.pc, 0x0108); // les huit instructions ont bien été exécutées
        assert_eq!(
            bus.mmu.read(0xFF05), // lecture brute (pas de tick) : TIMA après 64 T
            3,
            "TIMA doit valoir 3 sur ces 64 T (décalage structurel d'1 M-cycle, RELOAD_WINDOW_LEN) : chaque tick a avancé le matériel d'un M-cycle"
        );
    }

    /// Étape 2 : la famille (HL) read-modify-write portée en micro-ops interleave ses M-cycles exactly as GBCTR chapitre 6 —
    /// INC/DEC (HL) at 3 M-cycles and CB rotate/shift / RES / SET b,(HL) at 4 M-cycles. The lecture de [HL] is the M-cycle
    /// avant-dernier, l'écriture (+ les drapeaux, s'il y en a) is the last M-cycle ; [HL] et les drapeaux sont inchangés until then.
    #[test]
    fn ported_hl_rmw_family_interleaves_m_cycles() {
        // --- INC (HL) ($34) : 3 M-cycles — fetch → [HL]→Z → [HL]=Z+1+drapeaux. ---
        {
            let mut mmu = MMU::new();
            let mut rom = vec![0x00u8; 0x4000];
            rom[0x0100] = 0x34; // INC (HL)
            mmu.load_rom(rom);
            let mut cpu = CPU::new();
            cpu.pc = 0x0100;
            cpu.h = 0xC0; // HL=$C050 (WRAM, toujours écritable)
            cpu.l = 0x50;
            mmu.write(0xC050, 0x0F); // [HL]=$0F → résultat $10 : H posé (demi-mot bas $0F→$xx), Z=0
            let f_before = cpu.f;

            let mut bus = Bus::new(&mut mmu);
            // M-cycle #1 (the fetch) : [HL] et drapeaux inchangés.
            let (c1, _) = cpu.tick(&mut bus);
            assert_eq!(c1, 4, "INC (HL) : the fetch doit consommer exactement un M-cycle (4 T)");
            assert!(cpu.in_progress.is_some(), "l'instruction doit être en cours after the fetch");
            assert_eq!(bus.mmu.read(0xC050), 0x0F, "[HL] inchangé pendant the fetch");
            assert_eq!(cpu.f, f_before, "drapeaux inchangés pendant the fetch");

            // M-cycle #2 ([HL]→Z) : [HL] still not written, drapeaux still not posés.
            let (c2, _) = cpu.tick(&mut bus);
            assert_eq!(c2, 4, "INC (HL) : the lecture [HL] doit consommer exactement un M-cycle (4 T)");
            assert!(cpu.in_progress.is_some(), "l'instruction doit être en cours after the lecture [HL]");
            assert_eq!(bus.mmu.read(0xC050), 0x0F, "[HL] not yet written pendant the lecture (pas d'écriture anticipée)");
            assert_eq!(cpu.f, f_before, "drapeaux not yet posés pendant the lecture [HL]");

            // M-cycle #3 ([HL]=Z+1 + drapeaux) : [HL]=$10, Z=0, N=0, H=1.
            let (c3, _) = cpu.tick(&mut bus);
            assert_eq!(c3, 4, "INC (HL) : the écriture+drapeaux doit consommer exactement un M-cycle (4 T)");
            assert!(cpu.in_progress.is_none(), "l'instruction doit être achevée after the drapeaux");
            assert_eq!(bus.mmu.read(0xC050), 0x10, "[HL]=$0F+1=$10 au 3ᵉ M-cycle");
            let f = cpu.flags();
            assert!(!f.contains(Flags::Z), "Z=0 car le résultat $10 n'est pas nul");
            assert!(!f.contains(Flags::N), "N=0 par INC");
            assert!(f.contains(Flags::H), "H posé : demi-mot bas passe de $0F à $xx");
        }

        // --- DEC (HL) ($35) : 3 M-cycles — fetch → [HL]→Z → [HL]=Z-1+drapeaux. ---
        {
            let mut mmu = MMU::new();
            let mut rom = vec![0x00u8; 0x4000];
            rom[0x0100] = 0x35; // DEC (HL)
            mmu.load_rom(rom);
            let mut cpu = CPU::new();
            cpu.pc = 0x0100;
            cpu.h = 0xC0;
            cpu.l = 0x50;
            mmu.write(0xC050, 0x00); // [HL]=$00 → résultat $FF : H posé (demi-mot bas $00→$xx), Z=0, N=1
            let f_before = cpu.f;

            let mut bus = Bus::new(&mut mmu);
            let (c1, _) = cpu.tick(&mut bus);
            assert_eq!(c1, 4, "DEC (HL) : the fetch doit consommer exactement un M-cycle (4 T)");
            assert!(cpu.in_progress.is_some(), "l'instruction doit être en cours after the fetch");
            assert_eq!(bus.mmu.read(0xC050), 0x00, "[HL] inchangé pendant the fetch");
            assert_eq!(cpu.f, f_before, "drapeaux inchangés pendant the fetch");

            let (c2, _) = cpu.tick(&mut bus);
            assert_eq!(c2, 4, "DEC (HL) : the lecture [HL] doit consommer exactement un M-cycle (4 T)");
            assert!(cpu.in_progress.is_some(), "l'instruction doit être en cours after the lecture [HL]");
            assert_eq!(bus.mmu.read(0xC050), 0x00, "[HL] not yet written pendant the lecture");
            assert_eq!(cpu.f, f_before, "drapeaux not yet posés pendant the lecture [HL]");

            let (c3, _) = cpu.tick(&mut bus);
            assert_eq!(c3, 4, "DEC (HL) : the écriture+drapeaux doit consommer exactement un M-cycle (4 T)");
            assert!(cpu.in_progress.is_none(), "l'instruction doit être achevée after the drapeaux");
            assert_eq!(bus.mmu.read(0xC050), 0xFF, "[HL]=$00-1=$FF au 3ᵉ M-cycle");
            let f = cpu.flags();
            assert!(!f.contains(Flags::Z), "Z=0 car le résultat $FF n'est pas nul");
            assert!(f.contains(Flags::N), "N posé par DEC");
            assert!(f.contains(Flags::H), "H posé : demi-mot bas passe de $00 à $xx (emprunt)");
        }

        // --- CB RLC b,(HL) ($CB 06) : 4 M-cycles — fetch $CB → sous-opcode → [HL]→Z → [HL]=RLC(Z)+drapeaux. ---
        {
            let mut mmu = MMU::new();
            let mut rom = vec![0x00u8; 0x4000];
            rom[0x0100] = 0xCB;
            rom[0x0101] = 0x06; // RLC (HL)
            mmu.load_rom(rom);
            let mut cpu = CPU::new();
            cpu.pc = 0x0100;
            cpu.h = 0xC0;
            cpu.l = 0x50;
            mmu.write(0xC050, 0x80); // [HL]=$80 → RLC → $01 : C posé (bit7 décalé), Z=0
            let f_before = cpu.f;

            let mut bus = Bus::new(&mut mmu);
            for m in 1..=3 {
                let (c, _) = cpu.tick(&mut bus);
                assert_eq!(c, 4, "CB RLC b,(HL) : M-cycle #{m} doit consommer exactement un M-cycle (4 T)");
                assert!(cpu.in_progress.is_some(), "l'instruction doit être en cours before the last M-cycle");
                assert_eq!(bus.mmu.read(0xC050), 0x80, "[HL] not yet written before the last M-cycle");
                assert_eq!(cpu.f, f_before, "drapeaux not yet posés before the last M-cycle");
            }
            let (c4, _) = cpu.tick(&mut bus);
            assert_eq!(c4, 4, "CB RLC b,(HL) : the écriture+drapeaux doit consommer exactement un M-cycle (4 T)");
            assert!(cpu.in_progress.is_none(), "l'instruction doit être achevée after the drapeaux");
            assert_eq!(bus.mmu.read(0xC050), 0x01, "[HL]=$80 RLC → $01 au 4ᵉ M-cycle");
            let f = cpu.flags();
            assert!(!f.contains(Flags::Z), "Z=0 car le résultat $01 n'est pas nul");
            assert!(!f.contains(Flags::N), "N=0 par RLC");
            assert!(!f.contains(Flags::H), "H=0 par RLC");
            assert!(f.contains(Flags::C), "C posé : bit7 de $80 décalé");
        }

        // --- CB SWAP b,(HL) ($CB 36) : 4 M-cycles — [HL]=SWAP(Z)+drapeaux (Z, N=0, H=0, C effacé). ---
        {
            let mut mmu = MMU::new();
            let mut rom = vec![0x00u8; 0x4000];
            rom[0x0100] = 0xCB;
            rom[0x0101] = 0x36; // SWAP (HL)
            mmu.load_rom(rom);
            let mut cpu = CPU::new();
            cpu.pc = 0x0100;
            cpu.h = 0xC0;
            cpu.l = 0x50;
            mmu.write(0xC050, 0xAB); // [HL]=$AB → SWAP → $BA : Z=0, H=0, C effacé (même si C était posé)
            let f_before = cpu.f | Flags::C.bits(); // C posé à l'avance pour vérifier qu'il est effacé
            cpu.f = f_before;

            let mut bus = Bus::new(&mut mmu);
            for m in 1..=3 {
                let (c, _) = cpu.tick(&mut bus);
                assert_eq!(c, 4, "CB SWAP b,(HL) : M-cycle #{m} doit consommer exactement un M-cycle (4 T)");
                assert!(cpu.in_progress.is_some(), "l'instruction doit être en cours before the last M-cycle");
                assert_eq!(bus.mmu.read(0xC050), 0xAB, "[HL] not yet written before the last M-cycle");
            }
            let (c4, _) = cpu.tick(&mut bus);
            assert_eq!(c4, 4, "CB SWAP b,(HL) : the écriture+drapeaux doit consommer exactement un M-cycle (4 T)");
            assert!(cpu.in_progress.is_none(), "l'instruction doit être achevée after the drapeaux");
            assert_eq!(bus.mmu.read(0xC050), 0xBA, "[HL]=$AB SWAP → $BA au 4ᵉ M-cycle");
            let f = cpu.flags();
            assert!(!f.contains(Flags::Z), "Z=0 car le résultat $BA n'est pas nul");
            assert!(!f.contains(Flags::H), "H=0 par SWAP");
            assert!(!f.contains(Flags::C), "C effacé par SWAP (même si C était posé)");
        }

        // --- CB RES b,(HL) ($CB A6 = RES 4,(HL)) : 4 M-cycles — [HL]=Z & !(1<<bit) ; pas de drapeaux. ---
        {
            let mut mmu = MMU::new();
            let mut rom = vec![0x00u8; 0x4000];
            rom[0x0100] = 0xCB;
            rom[0x0101] = 0xA6; // RES 4,(HL)
            mmu.load_rom(rom);
            let mut cpu = CPU::new();
            cpu.pc = 0x0100;
            cpu.h = 0xC0;
            cpu.l = 0x50;
            mmu.write(0xC050, 0xFF); // [HL]=$FF → RES 4 → $EF (bit 4 effacé) ; drapeaux inchangés
            let f_before = Flags::Z.bits() | Flags::C.bits(); // F=$B1 : Z et C posés à préserver
            cpu.f = f_before;

            let mut bus = Bus::new(&mut mmu);
            for m in 1..=3 {
                let (c, _) = cpu.tick(&mut bus);
                assert_eq!(c, 4, "CB RES b,(HL) : M-cycle #{m} doit consommer exactement un M-cycle (4 T)");
                assert!(cpu.in_progress.is_some(), "l'instruction doit être en cours before the last M-cycle");
                assert_eq!(bus.mmu.read(0xC050), 0xFF, "[HL] not yet written before the last M-cycle");
                assert_eq!(cpu.f, f_before, "drapeaux inchangés before the last M-cycle");
            }
            let (c4, _) = cpu.tick(&mut bus);
            assert_eq!(c4, 4, "CB RES b,(HL) : the écriture doit consommer exactement un M-cycle (4 T)");
            assert!(cpu.in_progress.is_none(), "l'instruction doit être achevée after the écriture");
            assert_eq!(bus.mmu.read(0xC050), 0xEF, "[HL]=$FF RES 4 → $EF au 4ᵉ M-cycle");
            assert_eq!(cpu.f, f_before, "drapeaux inchangés par RES (Z et C préservés)");
        }

        // --- CB SET b,(HL) ($CB CE = SET 1,(HL)) : 4 M-cycles — [HL]=Z | (1<<bit) ; pas de drapeaux. ---
        {
            let mut mmu = MMU::new();
            let mut rom = vec![0x00u8; 0x4000];
            rom[0x0100] = 0xCB;
            rom[0x0101] = 0xCE; // SET 1,(HL)
            mmu.load_rom(rom);
            let mut cpu = CPU::new();
            cpu.pc = 0x0100;
            cpu.h = 0xC0;
            cpu.l = 0x50;
            mmu.write(0xC050, 0x00); // [HL]=$00 → SET 1 → $02 (bit 1 posé) ; drapeaux inchangés
            let f_before = Flags::Z.bits() | Flags::N.bits(); // F=$C0 : Z et N posés à préserver
            cpu.f = f_before;

            let mut bus = Bus::new(&mut mmu);
            for m in 1..=3 {
                let (c, _) = cpu.tick(&mut bus);
                assert_eq!(c, 4, "CB SET b,(HL) : M-cycle #{m} doit consommer exactement un M-cycle (4 T)");
                assert!(cpu.in_progress.is_some(), "l'instruction doit être en cours before the last M-cycle");
                assert_eq!(bus.mmu.read(0xC050), 0x00, "[HL] not yet written before the last M-cycle");
                assert_eq!(cpu.f, f_before, "drapeaux inchangés before the last M-cycle");
            }
            let (c4, _) = cpu.tick(&mut bus);
            assert_eq!(c4, 4, "CB SET b,(HL) : the écriture doit consommer exactement un M-cycle (4 T)");
            assert!(cpu.in_progress.is_none(), "l'instruction doit être achevée after the écriture");
            assert_eq!(bus.mmu.read(0xC050), 0x02, "[HL]=$00 SET 1 → $02 au 4ᵉ M-cycle");
            assert_eq!(cpu.f, f_before, "drapeaux inchangés par SET (Z et N préservés)");
        }

        // --- Couverture exhaustive : un cas par opcode individuel de la famille — [HL] et les drapeaux restent inchangés until the last M-cycle. ---
        fn tick_sequence(bytes: &[u8], initial: u8) -> Vec<(u8, u8)> {
            let mut mmu = MMU::new();
            let mut rom = vec![0x00u8; 0x4000]; // ROM minimal : NOP partout
            for (i, b) in bytes.iter().enumerate() {
                rom[0x0100 + i] = *b;
            }
            mmu.load_rom(rom);
            let mut cpu = CPU::new();
            cpu.pc = 0x0100;
            cpu.h = 0xC0; // HL=$C050 (WRAM, toujours écritable)
            cpu.l = 0x50;
            cpu.f = Flags::empty().bits(); // F=0 : tout drapeau posé avant le dernier M-cycle est détecté
            mmu.write(0xC050, initial);
            let mut bus = Bus::new(&mut mmu);
            let mut seq = Vec::new();
            loop {
                let (_, _) = cpu.tick(&mut bus);
                seq.push((bus.mmu.read(0xC050), cpu.f));
                if cpu.in_progress.is_none() {
                    break;
                }
            }
            seq
        }

        // (octets de l'opcode, [HL] initial, [HL] attendu au dernier M-cycle) — un cas par opcode individuel.
        let cases: &[(&[u8], u8, u8)] = &[
            (&[0x34], 0x0F, 0x10), // INC (HL) : $0F → $10
            (&[0x35], 0x00, 0xFF), // DEC (HL) : $00 → $FF
            (&[0xCB, 0x06], 0x80, 0x01), // RLC b,(HL)
            (&[0xCB, 0x0E], 0x01, 0x80), // RRC b,(HL)
            (&[0xCB, 0x16], 0x7F, 0xFE), // RL b,(HL) (C=0)
            (&[0xCB, 0x1E], 0x80, 0x40), // RR b,(HL) (C=0)
            (&[0xCB, 0x26], 0x7F, 0xFE), // SLA b,(HL)
            (&[0xCB, 0x2E], 0x80, 0xC0), // SRA b,(HL)
            (&[0xCB, 0x36], 0xAB, 0xBA), // SWAP b,(HL)
            (&[0xCB, 0x3E], 0x81, 0x40), // SRL b,(HL)
            (&[0xCB, 0x86], 0xFF, 0xFE), // RES 0,(HL)
            (&[0xCB, 0x8E], 0xFF, 0xFD), // RES 1,(HL)
            (&[0xCB, 0x96], 0xFF, 0xFB), // RES 2,(HL)
            (&[0xCB, 0x9E], 0xFF, 0xF7), // RES 3,(HL)
            (&[0xCB, 0xA6], 0xFF, 0xEF), // RES 4,(HL)
            (&[0xCB, 0xAE], 0xFF, 0xDF), // RES 5,(HL)
            (&[0xCB, 0xB6], 0xFF, 0xBF), // RES 6,(HL)
            (&[0xCB, 0xBE], 0xFF, 0x7F), // RES 7,(HL)
            (&[0xCB, 0xC6], 0x00, 0x01), // SET 0,(HL)
            (&[0xCB, 0xCE], 0x00, 0x02), // SET 1,(HL)
            (&[0xCB, 0xD6], 0x00, 0x04), // SET 2,(HL)
            (&[0xCB, 0xDE], 0x00, 0x08), // SET 3,(HL)
            (&[0xCB, 0xE6], 0x00, 0x10), // SET 4,(HL)
            (&[0xCB, 0xEE], 0x00, 0x20), // SET 5,(HL)
            (&[0xCB, 0xF6], 0x00, 0x40), // SET 6,(HL)
            (&[0xCB, 0xFE], 0x00, 0x80), // SET 7,(HL)
        ];

        for &(bytes, initial, expected_final) in cases {
            let seq = tick_sequence(bytes, initial);
            let n = seq.len();
            assert_eq!(n, if bytes[0] == 0xCB { 4 } else { 3 }, "M-cycle count pour {:02X?}", bytes);
            for (i, (hl, f)) in seq.iter().enumerate() {
                if i < n - 1 {
                    assert_eq!(*hl, initial, "[HL] doit rester ${:02X} avant le dernier M-cycle ({:02X?}, M#{})", initial, bytes, i + 1);
                    assert_eq!(*f, 0, "drapeaux doivent rester F=0 before the last M-cycle ({:02X?}, M#{})", bytes, i + 1);
                } else {
                    assert_eq!(*hl, expected_final, "[HL] doit valoir ${:02X} au dernier M-cycle ({:02X?})", expected_final, bytes);
                }
            }
        }
    }

    /// Étape 2 : la famille (HL) read-modify-write portée en micro-ops produit les mêmes effets d'observation que le chemin
    /// legacy — [HL], drapeaux, PC — pour INC/DEC (HL), tous the CB rotate/shift b,(HL), et tous the CB RES/SET b,(HL).
    #[test]
    fn ported_hl_rmw_family_matches_legacy_side_effects() {
        // Exécute l'instruction placée à $0100 (opcode + opérandes) with [HL]=mem_val and F=flags, then renvoie ([HL], F).
        fn run(bytes: &[u8], mem_val: u8, flags: u8) -> (u8, u8) {
            let mut mmu = MMU::new();
            let mut rom = vec![0x00u8; 0x4000]; // ROM minimal : NOP partout
            for (i, b) in bytes.iter().enumerate() {
                rom[0x0100 + i] = *b;
            }
            mmu.load_rom(rom);
            let mut cpu = CPU::new();
            cpu.pc = 0x0100;
            cpu.h = 0xC0; // HL=$C050 (WRAM, toujours écritable)
            cpu.l = 0x50;
            cpu.f = flags;
            mmu.write(0xC050, mem_val);
            let mut bus = Bus::new(&mut mmu);
            loop {
                let (_, _) = cpu.tick(&mut bus);
                if cpu.in_progress.is_none() {
                    break;
                }
            }
            (bus.mmu.read(0xC050), cpu.f)
        }

        // --- INC (HL) : [HL]=old+1 ; Z=(r==0), N=0, H=((r&$0F)==0), C inchangé. ---
        for &mem_val in &[0x00u8, 0x0F, 0x7F, 0xFF] {
            let (res, f) = run(&[0x34], mem_val, Flags::C.bits()); // C posé à l'avance : doit être préservé
            assert_eq!(res, mem_val.wrapping_add(1), "INC (HL) [HL]=${:02X}", mem_val);
            let fl = Flags::from_bits_truncate(f);
            assert!(!fl.contains(Flags::N), "INC (HL) : N=0");
            assert_eq!(fl.contains(Flags::Z), res == 0, "INC (HL) : Z=(r==0)");
            assert_eq!(fl.contains(Flags::H), (res & 0x0F) == 0, "INC (HL) : H=((r&$0F)==0)");
            assert!(fl.contains(Flags::C), "INC (HL) : C préservé");
        }

        // --- DEC (HL) : [HL]=old-1 ; Z=(r==0), N=1, H=((r&$0F)==$0F), C inchangé. ---
        for &mem_val in &[0x00u8, 0x10, 0x80, 0xFF] {
            let (res, f) = run(&[0x35], mem_val, Flags::C.bits()); // C posé à l'avance : doit être préservé
            assert_eq!(res, mem_val.wrapping_sub(1), "DEC (HL) [HL]=${:02X}", mem_val);
            let fl = Flags::from_bits_truncate(f);
            assert!(fl.contains(Flags::N), "DEC (HL) : N=1");
            assert_eq!(fl.contains(Flags::Z), res == 0, "DEC (HL) : Z=(r==0)");
            assert_eq!(fl.contains(Flags::H), (res & 0x0F) == 0x0F, "DEC (HL) : H=((r&$0F)==$0F)");
            assert!(fl.contains(Flags::C), "DEC (HL) : C préservé");
        }

        // --- CB rotate/shift b,(HL) : [HL]=transform(old) ; Z=(r==0), N=0, H=0, C=décalé. ---
        let cases: &[(u8, u8, u8)] = &[
            (0x06, 0x80, 0x01), // RLC : $80 → $01
            (0x0E, 0x01, 0x80), // RRC : $01 → $80
            (0x16, 0x7F, 0xFE), // RL (C=0) : $7F → $FE
            (0x1E, 0x80, 0x40), // RR (C=0) : $80 → $40
            (0x26, 0x7F, 0xFE), // SLA : $7F → $FE
            (0x2E, 0x80, 0xC0), // SRA : $80 → $C0
            (0x36, 0xAB, 0xBA), // SWAP : $AB → $BA
            (0x3E, 0x81, 0x40), // SRL : $81 → $40
        ];
        for &(sub, mem_val, expected) in cases {
            let (res, f) = run(&[0xCB, sub], mem_val, Flags::empty().bits());
            assert_eq!(res, expected, "CB ${:02X} b,(HL) [HL]=${:02X}", sub, mem_val);
            let fl = Flags::from_bits_truncate(f);
            assert!(!fl.contains(Flags::N), "CB rotate/shift : N=0");
            assert!(!fl.contains(Flags::H), "CB rotate/shift : H=0");
            assert_eq!(fl.contains(Flags::Z), res == 0, "CB rotate/shift : Z=(r==0)");
        }

        // --- CB RES b,(HL) : [HL]=old & !(1<<bit) ; pas de drapeaux. ---
        for bit in 0..=7u8 {
            let sub = 0x86 | (bit << 3); // RES b,(HL)
            let f_before = Flags::Z.bits() | Flags::N.bits() | Flags::C.bits();
            let (res, f) = run(&[0xCB, sub], 0xFF, f_before);
            assert_eq!(res, 0xFF & !(1 << bit), "CB RES {} b,(HL)", bit);
            assert_eq!(f, f_before, "CB RES {} : drapeaux inchangés", bit);
        }

        // --- CB SET b,(HL) : [HL]=old | (1<<bit) ; pas de drapeaux. ---
        for bit in 0..=7u8 {
            let sub = 0xC6 | (bit << 3); // SET b,(HL)
            let f_before = Flags::Z.bits() | Flags::N.bits() | Flags::C.bits();
            let (res, f) = run(&[0xCB, sub], 0x00, f_before);
            assert_eq!(res, 1 << bit, "CB SET {} b,(HL)", bit);
            assert_eq!(f, f_before, "CB SET {} : drapeaux inchangés", bit);
        }
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
            // ADD HL,HL / ADD HL,SP : le code consomme 4 M-cycles (« DMG »), la référence dit 2.
            "$$29 (non-cond): got 4 M-cycle(s), expected 2".into(), // ADD HL,HL
            "$$39 (non-cond): got 4 M-cycle(s), expected 2".into(), // ADD HL,SP
            // ALU A,(HL) : le code consomme 1 M-cycle, la référence dit 2. (hors mandat — non porté en micro-ops)
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
