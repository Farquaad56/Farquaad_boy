//! Structure principale orchestrant les composants de l'émulateur (CPU, MMU, PPU, APU…).
//!
//! Partie 5 : rendu du Background ; la ROM chargée est exécutée directement à $0100 (état post-boot ROM).
//! Partie 6 : opcodes CPU complets et bug HALT.
//! Étape 2 : les registres I/O matériels repartent aux valeurs laissées par le boot ROM DMG au hand-off
//! PC=$0100 (PanDocs « Power Up Sequence ») — P1 = $CF, SC = $7E, DIV = $AB, TAC = $F8, OBP0/OBP1 = $FF.
//! Étape 3 : le rendu est forcé à chaque frontière de frame même LCD éteint (bit 7 de LCDC à 0) — la PPU
//! gèle son timing mais le compteur global de T-cycles continue d'avancer, donc l'écran devient noir au
//! lieu de rester figé (règle interne de `render_frame`).

use crate::cpu::CPU;
use crate::mmu::MMU;
use crate::ppu::{IRQ_STAT, IRQ_VBLANK, PPU};
use crate::serial::Serial;
use crate::timer::Timer;

/// Nombre de T-cycles par frame vidéo (60 Hz) : 154 lignes × 456 dots (Pan Docs « Rendering »).
pub const FRAME_TCYCLES: u64 = crate::ppu::FRAME_DOTS;

/// État principal de l'émulateur.
pub struct Emulator {
    pub cpu: CPU,
    pub mmu: MMU,
    /// T-cycles exécutés depuis le boot (compteur debug).
    pub t_cycles: u64,
    /// Instructions exécutées depuis le boot (compteur debug).
    pub instructions: u64,
}

impl Emulator {
    /// Crée un émulateur à l'état power-on post-boot ROM (aucune ROM chargée).
    pub fn new() -> Self {
        let mut emu = Self {
            cpu: CPU::new(),
            mmu: MMU::new(),
            t_cycles: 0,
            instructions: 0,
        };
        emu.power_on(); // état post-boot ROM : CPU + registres I/O matériels (PanDocs « Power Up Sequence »)
        emu
    }

    /// Charge une ROM `.gb` et redémarre le système à l'état power-on post-boot ROM ; la ROM est exécutée immédiatement à $0100.
    pub fn load_rom(&mut self, data: Vec<u8>) {
        self.mmu.load_rom(data);
        dump_rom_handlers(&self.mmu); // diagnostic : contenu des 4 vecteurs d'interruption ($0040/$0048/$0050/$0058)
        self.power_on();
    }

    /// Redémarre le système à l'état power-on post-boot ROM ; la ROM chargée est conservée.
    pub fn reset(&mut self) {
        self.power_on();
    }

    /// État power-on post-boot ROM : CPU/PPU/SCC/timer réinitialisés, registres I/O matériels aux valeurs
    /// laissées par le boot ROM DMG au hand-off PC=$0100 (PanDocs « Power Up Sequence »), prêt à exécuter la ROM chargée.
    fn power_on(&mut self) {
        // Les sous-composants repartent chacun à leur état post-boot ROM :
        // - CPU::new()       → PC=0x0100, SP=$FFFE, registres corrects (cpu.rs).
        // - PPU::default()   → LCDC=$91 (LCD allumé, fond activé), BGP=$FC, SCY/SCX/LYC/WX/WY=$00, OBP0/OBP1=$FF (ppu.rs).
        // - Serial::default()→ SB=$00, SC=$00, aucun transfert en cours (serial.rs).
        // - Timer::default() → TIMA/TMA=$00, TAC se lit $F8 → timer désactivé (timer.rs).
        self.cpu = CPU::new();
        self.mmu.ppu = PPU::default();
        self.mmu.serial = Serial::default();
        self.mmu.timer = Timer::default();

        // Registres I/O matériels aux valeurs post-boot DMG (PanDocs « Power Up Sequence »).
        // Certains ne peuvent PAS être posés via mmu.write car le matériel masque des bits :
        self.mmu.write(0xFF00, 0xCF); // P1 ($FF00) : joypad, aucun bouton pressé.
        self.mmu.serial.sc = 0x7E; // SC ($FF02) : seuls les bits 7 et 0 sont écriturables — la valeur post-boot $7E (bits 6..1 à 1) est conservée telle quelle ; mmu.write(0xFF02, 0x7E) donnerait $00.
        self.mmu.timer.counter = (0xABu16) << 8; // DIV ($FF04) se lit $AB : write_div ignore la valeur écrite et remet le compteur à $0000, d'où l'écriture directe du compteur ; mmu.write(0xFF04, 0xAB) donnerait DIV=$00.
        self.mmu.write(0xFF07, 0xF8); // TAC ($FF07) : bits 7-3 toujours lus à 1 → timer désactivé au hand-off (seuls les bits 2-0 sont écrits).
        self.mmu.ie = 0x00; // IE ($FFFF) : toutes les sources d'interruption désactivées au hand-off.

        self.t_cycles = 0;
        self.instructions = 0;

        log::info!("Post-boot ROM initialization complete");
    }

    /// Exécute une instruction et renvoie les T-cycles consommés.
    pub fn step(&mut self) -> u32 {
        let cycles = self.cpu.step(&mut self.mmu);
        self.instructions += 1;
        self.t_cycles += cycles as u64;

        // --- Diagnostic de blocage HALT : "battement de cœur" toutes les 10 000 instructions ---
        if self.cpu.halted && self.instructions.is_multiple_of(10000) {
            log::debug!(
                "[CPU] STUCK IN HALT: PC=${:04X}, IF={:02X}, IE={:02X}, IME={}, PPU_Mode={}, LY={}",
                self.cpu.pc,
                self.mmu.io[0x0F],
                self.mmu.ie,
                if self.cpu.ime { "ON" } else { "OFF" },
                self.mmu.ppu.mode,
                self.mmu.ppu.ly,
            );
        }

        // La PPU avance du même nombre de T-cycles (timing LY/mode, Pan Docs « Rendering »).
        let _frame_done = self.mmu.ppu.advance(cycles as u64);

        // CORRECTION CRITIQUE : Rendre à chaque frame, pas seulement à la frontière exacte — dès que
        // t_cycles franchit un multiple de FRAME_TCYCLES (même si le LCD est éteint et la PPU gelée),
        // le rendu est forcé : écran noir au lieu d'écran figé.
        let frame_tcycle = self.t_cycles % FRAME_TCYCLES;
        if frame_tcycle < cycles as u64 {
            self.mmu.ppu.render_frame(&self.mmu.vram, &self.mmu.oam);
        }
        // Les requêtes d'interruption PPU en attente lèvent les bits correspondants de IF ($FF0F).
        let ppu_irq = self.mmu.ppu.take_interrupts();
        if ppu_irq != 0 {
            log::debug!(
                "[PPU] IRQ raised: {:02X}, IF before: {:02X}, LY={}, mode={}",
                ppu_irq,
                self.mmu.io[0x0F],
                self.mmu.ppu.ly,
                self.mmu.ppu.mode,
            );
        }
        if ppu_irq & IRQ_VBLANK != 0 {
            self.mmu.io[0x0F] |= 0x01; // bit 0 de IF : interruption VBlank demandée (vecteur $40)
            log::debug!("[VBlank] IF set bit 0, new IF={:02X}", self.mmu.io[0x0F]);
        }
        if ppu_irq & IRQ_STAT != 0 {
            self.mmu.io[0x0F] |= 0x02; // bit 1 de IF : interruption STAT/LCD demandée (vecteur $48)
        }
        // La SCC avance du même nombre de T-cycles ; un transfert achevé lève le drapeau IF série.
        if self.mmu.serial.tick(cycles) {
            self.mmu.io[0x0F] |= 0x08; // bit 3 de IF ($FF0F) : interruption série demandée (Pan Docs « Interrupt Sources »)
        }
        // Le Timer avance du même nombre de T-cycles ; un débordement de TIMA lève le drapeau IF Timer.
        if self.mmu.timer.tick(cycles) {
            self.mmu.io[0x0F] |= 0x04; // bit 2 de IF ($FF0F) : interruption Timer demandée (Pan Docs « Interrupt Sources »)
        }
        cycles
    }

    /// Exécute exactement `n` T-cycles.
    pub fn run_tcycles(&mut self, mut n: u64) {
        while n > 0 {
            let c = self.step() as u64;
            n = n.saturating_sub(c);
        }
    }

    /// Exécute une frame vidéo complète (70224 T-cycles).
    pub fn run_frame(&mut self) {
        self.run_tcycles(FRAME_TCYCLES);
    }

    /// Charge un programme test intégré qui exerce le jeu d'instructions de la partie 3.
    pub fn load_test_program(&mut self) {
        let mut rom = vec![0xFF; 0x4000];
        rom[0x0134..0x013C].copy_from_slice(b"CPU TEST"); // titre affiché dans la barre de menu
        // Le code est placé AVANT le titre (l'exécution démarre à 0x0100 et passe par 0x0134) :
        // les octets du titre ne doivent pas être exécutés. $FF = RST $38 (opcode valide SM83),
        // d'où un préfixe de NOP explicites pour atteindre le code sans boucle de RST infinie.
        rom[0x0100..0x0108].fill(0x00); // 8 × NOP (32 T-cycles)
        let code: &[u8] = &[
            0x31, 0xFF, 0xDF, // 0x0108: LD SP, $DFFF (immédiat little-endian : FF puis DF)
            0x06, 0x2A,       // 0x010B: LD B, $2A (42)
            0x3C,             // 0x010D: INC A      ← début de la boucle
            0x05,             // 0x010E: DEC B
            0x20, 0xFC,       // 0x010F: JR NZ, -4 → retour à INC A (cible = 0x0111 + (-4) = 0x010D)
            0x3C,             // 0x0111: INC A      (après la boucle)
            0x00,             // 0x0112: NOP        ← début du spin infini
            0x20, 0xFD,       // 0x0113: JR -3 → retour à NOP (cible = 0x0115 + (-3) = 0x0112)
        ];
        rom[0x0108..0x0115].copy_from_slice(code);
        self.load_rom(rom);
    }
}

impl Default for Emulator {
    fn default() -> Self {
        Self::new()
    }
}

/// Diagnostic : affiche les 8 octets de chaque vecteur d'interruption ($0040/$0048/$0050/$0058) pour vérifier
/// que les handlers de la ROM sont bien là où on s'attend (Pan Docs « Interrupt Sources »).
fn dump_rom_handlers(mmu: &MMU) {
    log::info!("=== ROM Handler Dump ===");
    for addr in [0x0040u16, 0x0048, 0x0050, 0x0058] {
        let bytes: Vec<u8> = (0..8).map(|i| mmu.read(addr + i)).collect();
        log::info!(
            "Handler ${:04X}: {:02X} {:02X} {:02X} {:02X} {:02X} {:02X} {:02X} {:02X}",
            addr, bytes[0], bytes[1], bytes[2], bytes[3], bytes[4], bytes[5], bytes[6], bytes[7]
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cpu::flags::Flags;
    use crate::ppu::{DOTS_PER_LINE, SCREEN_WIDTH, STAT_IRQ_LYC, STAT_IRQ_VBLANK};

    #[test]
    fn boot_state_after_load_rom() {
        let mut emu = Emulator::new();
        assert_eq!(emu.cpu.pc, 0x0100); // cible du vecteur de reset
        assert_eq!(emu.cpu.sp, 0xFFFE);
        assert_eq!(emu.cpu.af(), 0x01B0);

        let mut rom = vec![0xFF; 0x4000];
        rom[0x0134..0x013B].copy_from_slice(b"POKEMON");
        emu.load_rom(rom);

        // L'état de boot est restauré après le chargement.
        assert_eq!(emu.cpu.pc, 0x0100);
        assert_eq!(emu.cpu.sp, 0xFFFE);
        // Le titre du jeu est lisible dans l'en-tête cartouche à 0x0134.
        let title: Vec<u8> = (0..7).map(|i| emu.mmu.read(0x0134 + i)).collect();
        assert_eq!(title, b"POKEMON");
    }

    #[test]
    fn post_boot_io_registers_match_pandocs() {
        let emu = Emulator::new();
        // Valeurs I/O laissées par le boot ROM DMG au hand-off PC=$0100 (PanDocs « Power Up Sequence », colonne DMG/MGB).
        assert_eq!(emu.mmu.read(0xFF00), 0xCF); // P1 : joypad, aucun bouton pressé
        assert_eq!(emu.mmu.read(0xFF02), 0x7E); // SC : valeur post-boot (bits 6..1 non écriturables par le logiciel)
        assert_eq!(emu.mmu.read(0xFF04), 0xAB); // DIV : valeur enregistrée au hand-off
        assert_eq!(emu.mmu.read(0xFF05), 0x00); // TIMA
        assert_eq!(emu.mmu.read(0xFF06), 0x00); // TMA
        assert_eq!(emu.mmu.read(0xFF07), 0xF8); // TAC : bits 7-3 lus à 1, timer désactivé
        assert_eq!(emu.mmu.read(0xFF48), 0xFF); // OBP0 : non initialisée par le boot ROM → valeur la plus fréquente
        assert_eq!(emu.mmu.read(0xFF49), 0xFF); // OBP1
        assert_eq!(emu.mmu.read(0xFFFF), 0x00); // IE : toutes les sources d'interruption désactivées au hand-off

        // L'état CPU post-boot ROM est inchangé.
        assert_eq!(emu.cpu.pc, 0x0100);
        assert_eq!(emu.cpu.sp, 0xFFFE);
        assert_eq!(emu.cpu.af(), 0x01B0);
    }

    #[test]
    fn reset_restores_post_boot_io_registers() {
        let mut emu = Emulator::new();
        emu.load_rom(vec![0xFF; 0x4000]);

        // La ROM « salit » les registres I/O…
        emu.mmu.write(0xFF00, 0x00);
        emu.mmu.write(0xFF02, 0x81); // transfert série démarré
        emu.mmu.timer.write_div(0x42); // le compteur système est remis à zéro → DIV = $00
        emu.mmu.ppu.obp0 = 0xE4;
        emu.mmu.ie = 0xFF; // toutes les sources d'interruption activées

        // …et un reset les ramène aux valeurs post-boot ROM.
        emu.reset();
        assert_eq!(emu.mmu.read(0xFF00), 0xCF);
        assert_eq!(emu.mmu.read(0xFF02), 0x7E);
        assert_eq!(emu.mmu.read(0xFF04), 0xAB);
        assert_eq!(emu.mmu.read(0xFF07), 0xF8);
        assert_eq!(emu.mmu.read(0xFF48), 0xFF);
        assert_eq!(emu.mmu.read(0xFF49), 0xFF);
        assert_eq!(emu.mmu.read(0xFFFF), 0x00); // IE : réinitialisé à $00 au hand-off
    }

    #[test]
    fn cpu_executes_test_program() {
        let mut emu = Emulator::new();
        emu.load_test_program();
        assert_eq!(emu.cpu.pc, 0x0100); // état de boot avant exécution

        emu.run_tcycles(FRAME_TCYCLES * 2); // ~2 frames : largement suffisant

        assert_eq!(emu.cpu.a, 0x2C); // 0x01 + 42 (boucle) + 1
        assert_eq!(emu.cpu.b, 0x00); // compte à rebours terminé
        assert_eq!(emu.cpu.c, 0x13); // non modifié (valeur de reset)
        assert_eq!(emu.cpu.sp, 0xDFFF);
        // C est conservé depuis le power-on (F = $B0 → Z|H|C) ; Z/N/H sont effacés par le dernier INC A.
        assert_eq!(emu.cpu.flags(), Flags::C);
        assert!(emu.instructions > 42);
    }

    #[test]
    fn run_frame_completes_one_ppu_frame() {
        let mut emu = Emulator::new();
        emu.load_test_program();
        emu.run_frame(); // 70224 T-cycles : exactement une frame vidéo.

        // La dernière instruction peut dépasser légèrement la cible (saturating_sub).
        assert!(emu.t_cycles >= FRAME_TCYCLES && emu.t_cycles < FRAME_TCYCLES + 20);
        // La PPU a achevé exactement une frame : retour à la ligne 0, OAM Scan.
        assert_eq!(emu.mmu.ppu.ly, 0);
        assert_eq!(emu.mmu.ppu.mode, 2);
    }

    #[test]
    fn lcd_off_renders_black_at_each_frame_boundary() {
        let mut emu = Emulator::new();
        // Programme qui tourne en boucle de NOP/JR (LCD allumé au power-on) : les T-cycles par instruction
        // ne s'alignent pas sur FRAME_TCYCLES — le rendu forcé doit donc se déclencher à chaque franchissement
        // de frontière, et non seulement quand t_cycles tombe pile sur un multiple.
        emu.load_test_program();

        emu.run_tcycles(FRAME_TCYCLES); // une frame LCD allumé : écran non noir (fond blanc, VRAM vide)
        assert!(emu.mmu.ppu.framebuffer.iter().any(|&p| p != 0xFF00_0000));

        emu.mmu.write(0xFF40, 0x11); // LCDC = $11 : LCD éteint (fond activé) — la PPU est gelée
        emu.run_tcycles(FRAME_TCYCLES * 2); // ~2 frames : advance() renvoie false à chaque pas…
        assert!(emu.mmu.ppu.framebuffer.iter().all(|&p| p == 0xFF00_0000)); // …mais le rendu forcé à la frontière de frame noircit l'écran

        emu.mmu.write(0xFF40, 0x91); // LCD rallumé
        emu.run_tcycles(FRAME_TCYCLES * 2);
        assert!(emu.mmu.ppu.framebuffer.iter().any(|&p| p != 0xFF00_0000)); // l'écran se rend à nouveau
    }

    #[test]
    fn frame_render_updates_the_framebuffer() {
        let mut emu = Emulator::new();
        emu.load_test_program();

        // Tuile 1 noire/blanche en $8010-$801F et carte du fond $9800-$9BFF pointant vers elle,
        // le tout écrit via le MMU (routing VRAM $8000-$9FFF).
        for row in 0..8 {
            emu.mmu.write(0x8010 + 2 * row as u16, 0xFF); // moitié gauche → valeur 3
            emu.mmu.write(0x8011 + 2 * row as u16, 0x00); // moitié droite → valeur 0
        }
        for addr in 0x9800..=0x9BFF {
            emu.mmu.write(addr, 1);
        }
        emu.mmu.write(0xFF40, 0x90); // LCD allumé, fond activé ; tuiles $8000-$8FFF, carte $9800-$9BFF
        emu.mmu.write(0xFF47, 0xE4); // BGP : teinte v pour une valeur de pixel v

        emu.run_frame(); // une frame vidéo : le rendu est mis à jour à la frontière de frame

        assert_eq!(emu.mmu.ppu.framebuffer[0], PPU::shade(3)); // moitié gauche de la tuile → teinte 3
        assert_eq!(emu.mmu.ppu.framebuffer[4], PPU::shade(0)); // moitié droite → teinte 0
    }

    #[test]
    fn frame_render_updates_sprites_from_oam() {
        let mut emu = Emulator::new();
        emu.load_test_program();

        // Tuile 1 noire en $8010-$801F, écrite via le MMU (routing VRAM $8000-$9FFF).
        for row in 0..8 {
            emu.mmu.write(0x8010 + 2 * row as u16, 0xFF); // toutes les valeurs de pixel valent 3
            emu.mmu.write(0x8011 + 2 * row as u16, 0xFF);
        }
        emu.mmu.write(0xFF40, 0x93); // LCD allumé, fond activé + sprites activées (bit 1)
        emu.mmu.write(0xFF48, 0xE4); // OBP0 : teinte v pour une valeur de pixel v

        // Entrée OAM 0 ($FE00-$FE03), écrite via le MMU (routing $FE00-$FE9F) : Y = 16, X = 32, tuile 1.
        emu.mmu.write(0xFE00, 16);
        emu.mmu.write(0xFE01, 32);
        emu.mmu.write(0xFE02, 1);

        emu.run_frame(); // une frame vidéo : le rendu est mis à jour à la frontière de frame

        assert_eq!(emu.mmu.ppu.framebuffer[15 * SCREEN_WIDTH + 32], PPU::shade(3)); // coin haut-gauche du sprite (ligne Y-1)
        assert_eq!(emu.mmu.ppu.framebuffer[22 * SCREEN_WIDTH + 39], PPU::shade(3)); // coin bas-droite (ligne Y+6, colonne X+7)
        assert_eq!(emu.mmu.ppu.framebuffer[15 * SCREEN_WIDTH + 40], PPU::shade(0)); // juste à droite du sprite : fond blanc
    }

    #[test]
    fn ppu_interrupts_raise_if_bits() {
        let mut emu = Emulator::new();
        emu.load_test_program(); // programme qui tourne en boucle de NOP/JR

        emu.mmu.ppu.stat = STAT_IRQ_VBLANK | STAT_IRQ_LYC; // active les interruptions VBlank + LYC==LY
        emu.mmu.ppu.lyc = 145;

        emu.run_tcycles(145 * DOTS_PER_LINE as u64 + 1); // franchit le début de la ligne 144 (VBlank) puis de la ligne 145 (ly == lyc)

        assert_eq!(emu.mmu.io[0x0F] & 0x03, 0x03); // bits 0 (VBlank) + 1 (STAT) de IF levés
    }

    #[test]
    fn timer_overflow_raises_the_if_flag() {
        let mut emu = Emulator::new();
        emu.load_test_program(); // programme qui tourne en boucle de NOP/JR

        emu.mmu.write(0xFF06, 0x33); // TMA : rechargement au débordement
        emu.mmu.write(0xFF05, 0xFF); // TIMA : déborde à la prochaine incrémentation
        emu.mmu.write(0xFF07, 0x07); // TAC : timer activé, tick toutes les 256 T-cycles

        assert_eq!(emu.mmu.io[0x0F] & 0x04, 0); // pas encore d'interruption Timer
        emu.run_tcycles(300); // > 256 : le débordement a eu lieu (+ un cycle pour lever le drapeau)

        assert_eq!(emu.mmu.io[0x0F] & 0x04, 0x04); // bit 2 de IF levé par le débordement de TIMA
        assert_eq!(emu.mmu.read(0xFF05), 0x33); // TIMA rechargé depuis TMA
        assert_eq!(emu.mmu.read(0xFF04), 0xAC); // DIV a avancé d'un pas : compteur $AB00 (post-boot) + ~300 T-cycles → $ACxx
    }

    #[test]
    fn timer_keeps_running_while_executing_a_rom() {
        let mut emu = Emulator::new();
        // Boucle de JR sur elle-même à $0100 (aucune écriture mémoire, aucun push de pile) :
        // le timer avance uniquement avec les T-cycles exécutés.
        let mut rom = vec![0x00; 0x4000];
        rom[0x0100..0x0102].copy_from_slice(&[0x18, 0xFE]); // JR -2 → $0100 (boucle sur elle-même, 12 T-cycles par itération)
        emu.load_rom(rom);

        assert_eq!(emu.mmu.read(0xFF04), 0xAB); // DIV à l'état post-boot ROM avant exécution (compteur $AB00)
        emu.run_tcycles(FRAME_TCYCLES * 2); // ~2 frames : le timer avance pendant l'exécution
        assert_eq!(emu.mmu.read(0xFF04), 0xCF); // = (0xAB00 + FRAME_TCYCLES*2) mod $10000 → $CFAx (compteur système sur 16 bits, départ $AB00)
    }

    #[test]
    fn vblank_interrupt_wakes_halted_cpu() {
        // Mini-ROM qui active l'interruption VBlank (bit 3 de STAT), IE = $01, EI puis HALT :
        // le CPU doit se réveiller au vecteur $40 à l'entrée en VBlank de la frame courante.
        let mut rom = vec![0xFF; 0x4000];
        let code: &[u8] = &[
            0x31, 0xFF, 0xDF, // LD SP,$DFFF
            0x3E, 0x08,       // LD A,$08
            0xE0, 0x41,       // LDH [$FF41],A → STAT = $08 : bit 3 posé (interruption VBlank activée)
            0x3E, 0x01,       // LD A,$01
            0xE0, 0xFF,       // LDH [$FFFF],A → IE = $01 : VBlank uniquement
            0xFB,             // EI (IME effectif après l'instruction suivante)
            0x76,             // HALT
        ];
        rom[0x0100..0x0100 + code.len()].copy_from_slice(code);
        rom[0x40] = 0xE0; // LDH [$FFC0],A : marqueur du handler VBlank
        rom[0x41] = 0xC0;
        rom[0x42] = 0xC9; // RET

        let mut emu = Emulator::new();
        emu.load_rom(rom);
        assert_eq!(emu.mmu.read(0xFFC0), 0); // marqueur vide avant exécution

        emu.run_frame(); // l'entrée en VBlank (dot 65664) a lieu dans cette frame → bit 0 de IF → le CPU halté se réveille à $40
        assert_eq!(emu.mmu.read(0xFFC0), 0x01); // handler VBlank exécuté (A = $01)
    }

    #[test]
    fn timer_interrupt_wakes_halted_cpu() {
        // Mini-ROM qui active le timer (TAC = $FC : bit 2 posé, sélection 00), TIMA = $FF, IE = $04 puis HALT :
        // le débordement de TIMA doit réveiller le CPU au vecteur $50.
        let mut rom = vec![0xFF; 0x4000];
        let code: &[u8] = &[
            0x31, 0xFF, 0xDF, // LD SP,$DFFF
            0x3E, 0xFC,       // LD A,$FC
            0xE0, 0x07,       // LDH [$FF07],A → TAC = $FC : timer activé (bit 2), sélection 00
            0x3E, 0xFF,       // LD A,$FF
            0xE0, 0x05,       // LDH [$FF05],A → TIMA = $FF : débordement au prochain tick
            0x3E, 0x04,       // LD A,$04
            0xE0, 0xFF,       // LDH [$FFFF],A → IE = $04 : Timer uniquement
            0xFB,             // EI (IME effectif après l'instruction suivante)
            0x76,             // HALT
        ];
        rom[0x0100..0x0100 + code.len()].copy_from_slice(code);
        rom[0x50] = 0xE0; // LDH [$FFC0],A : marqueur du handler Timer
        rom[0x51] = 0xC0;
        rom[0x52] = 0xC9; // RET

        let mut emu = Emulator::new();
        emu.load_rom(rom);
        assert_eq!(emu.mmu.read(0xFFC0), 0); // marqueur vide avant exécution

        emu.run_frame();
        assert_eq!(emu.mmu.read(0xFFC0), 0x04); // handler Timer exécuté (A = $04)
    }

    #[test]
    fn vblank_interrupt_wakes_cpu_on_consecutive_frames() {
        // Mini-ROM qui active l'interruption VBlank (bit 3 de STAT), IE = $01, EI puis boucle HALT :
        // le CPU doit se réveiller au vecteur $40 à CHAQUE entrée en VBlank (une par frame).
        let mut rom = vec![0xFF; 0x4000];
        let code: &[u8] = &[
            0x31, 0xFF, 0xDF, // LD SP,$DFFF
            0x3E, 0x08,       // LD A,$08
            0xE0, 0x41,       // LDH [$FF41],A → STAT = $08 : bit 3 posé (interruption VBlank activée)
            0x3E, 0x01,       // LD A,$01
            0xE0, 0xFF,       // LDH [$FFFF],A → IE = $01 : VBlank uniquement
            0xFB,             // EI (IME effectif après l'instruction suivante)
            0x76,             // HALT ($010C)
            0x76,             // HALT ($010D) : ré-entrée en HALT après le RETI du handler
        ];
        rom[0x0100..0x0100 + code.len()].copy_from_slice(code);
        // Handler VBlank à $40 : incrémente un compteur dans HRAM puis RETI (IME réactivé).
        rom[0x40] = 0xF0; // LDH A,[$FFC0] (8)
        rom[0x41] = 0xC0;
        rom[0x42] = 0x3C; // INC A (4)
        rom[0x43] = 0xE0; // LDH [$FFC0],A (8)
        rom[0x44] = 0xC0;
        rom[0x45] = 0xD9; // RETI (16) : retour à $010D + IME réactivé

        let mut emu = Emulator::new();
        emu.load_rom(rom);
        assert_eq!(emu.mmu.read(0xFFC0), 0); // compteur vide avant exécution

        for frame in 0..3 {
            emu.run_frame();
            assert_eq!(emu.mmu.read(0xFFC0), (frame + 1) as u8); // un réveil VBlank par frame
        }
    }

    #[test]
    fn flags_roundtrip() {
        let mut cpu = CPU::new(); // f = 0xB0 (Z|H|C — le carry est actif au power-on)
        assert_eq!(cpu.flags(), Flags::Z | Flags::H | Flags::C);
        cpu.set_flags(Flags::empty());
        assert_eq!(cpu.f, 0x00);
        cpu.set_flags(Flags::C);
        assert_eq!(cpu.f, 0x10);
    }

    #[test]
    fn step_counts_cycles_and_instructions() {
        let mut emu = Emulator::new();
        emu.load_test_program();
        for _ in 0..13 {
            emu.step();
        }
        // 8 × NOP (4) + LD SP (12) + LD B (8) + INC A (4) + DEC B (4) + JR pris (12) = 72.
        assert_eq!(emu.instructions, 13);
        assert_eq!(emu.t_cycles, 72);
    }

    #[test]
    fn serial_output_of_rom_appears_in_transcript() {
        // Mini-ROM qui imprime « Hello\n » sur le port link : pour chaque caractère,
        // LD A,c puis LDH [$FF01],A (SB) et LDH [$FF02],A avec A=$81 (SC = $81,
        // horloge interne / master) démarre un transfert de 64 T-cycles.
        let mut rom = vec![0xFF; 0x4000];
        rom[0x0134..0x013F].copy_from_slice(b"SERIAL TEST"); // titre affiché dans la barre de menu
        let code: &[u8] = &[
            0x31, 0xFF, 0xDF, // LD SP, $DFFF
            0x3E, 0x48,       // LD A,'H'
            0xE0, 0x01,       // LDH [$FF01],A → SB='H'
            0x3E, 0x81,       // LD A,$81
            0xE0, 0x02,       // LDH [$FF02],A → SC=$81 : transfert démarré ('H')
            0x3E, 0x65, 0xE0, 0x01, 0x3E, 0x81, 0xE0, 0x02, // 'e'
            0x3E, 0x6C, 0xE0, 0x01, 0x3E, 0x81, 0xE0, 0x02, // 'l' (premier)
            0x3E, 0x6C, 0xE0, 0x01, 0x3E, 0x81, 0xE0, 0x02, // 'l' (deuxième)
            0x3E, 0x6F, 0xE0, 0x01, 0x3E, 0x81, 0xE0, 0x02, // 'o'
            0x3E, 0x0A, 0xE0, 0x01, 0x3E, 0x81, 0xE0, 0x02, // '\n' : la ligne « Hello » est émise dans le log hôte
            0x00,             // NOP
            0x20, 0xFD,       // JR -3 → boucle infinie (NOP + JR)
        ];
        rom[0x0100..0x0100 + code.len()].copy_from_slice(code);

        let mut emu = Emulator::new();
        emu.load_rom(rom);
        emu.run_tcycles(FRAME_TCYCLES * 2); // largement suffisant (transferts de 64 T-cycles)

        assert_eq!(emu.mmu.serial.take_transcript(), b"Hello\n");
        assert_eq!(emu.mmu.serial.last_line(), "Hello");
    }

    #[test]
    fn serial_completion_raises_if_bit_3() {
        // Mini-ROM qui démarre un transfert série en mode master (SC = $81) puis tourne en boucle de NOP.
        let mut rom = vec![0xFF; 0x4000];
        let code: &[u8] = &[
            0x31, 0xFF, 0xDF, // LD SP,$DFFF
            0x3E, 0x42,       // LD A,'B'
            0xE0, 0x01,       // LDH [$FF01],A → SB='B'
            0x3E, 0x81,       // LD A,$81
            0xE0, 0x02,       // LDH [$FF02],A → SC=$81 : transfert démarré (64 T-cycles)
            0x00,             // NOP
            0x20, 0xFD,       // JR -3 → boucle infinie (NOP + JR)
        ];
        rom[0x0100..0x0100 + code.len()].copy_from_slice(code);

        let mut emu = Emulator::new();
        emu.load_rom(rom); // exécution immédiate du code ROM (IME reste désactivé : le drapeau doit se lever quand même)

        assert_eq!(emu.mmu.io[0x0F] & 0x08, 0); // pas encore d'interruption série
        emu.run_tcycles(64 + 128); // le transfert de 64 T-cycles s'achève (avec une marge)

        assert_eq!(emu.mmu.io[0x0F] & 0x08, 0x08); // bit 3 de IF levé par l'achèvement du transfert
        assert_eq!(emu.mmu.io[0x0F] & 0x07, 0);    // bits 0-2 (V-Blank/LC3C/Timer) non touchés
    }
}