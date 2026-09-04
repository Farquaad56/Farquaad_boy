//! Structure principale orchestrant les composants de l'émulateur (CPU, MMU, PPU, APU…).
//!
//! Partie 5 : rendu du Background ; la ROM chargée est exécutée directement à $0100 (état post-boot ROM).
//! Partie 6 : opcodes CPU complets et bug HALT.
//! Étape 2 : les registres I/O matériels repartent aux valeurs laissées par le boot ROM DMG au hand-off
//! PC=$0100 (PanDocs « Power Up Sequence ») — P1 = $CF, SC = $7E, DIV = $AB, TAC = $F8, OBP0/OBP1 = $FF.
//! Étape 3 : le rendu est forcé à chaque frontière de frame même LCD éteint (bit 7 de LCDC à 0) — la PPU
//! gèle son timing mais le compteur global de T-cycles continue d'avancer, donc l'écran devient noir au
//! lieu de rester figé (règle interne de `render_frame`).
//! Étape 4 : le forçage de STAT=$20 au hand-off est temporairement retiré (voir `power_on`) — le jeu doit configurer lui-même STAT,
//! afin d'éviter de déclencher une interruption VBlank avant que le jeu n'ait fini d'initialiser sa pile ou ses vecteurs.

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
        self.power_on();
    }

    /// Redémarre le système à l'état power-on post-boot ROM ; la ROM chargée est conservée (bouton « ⏹ Stop »).
    pub fn reset(&mut self) {
        self.power_on();
    }

    /// Réinitialise uniquement le CPU (registres + PC/SP aux valeurs post-boot, IME désactivé) ;
    /// la carte mémoire et les registres I/O sont conservés.
    pub fn reset_cpu(&mut self) {
        self.cpu = CPU::new();
    }

    /// État power-on post-boot ROM : CPU/PPU/SCC/timer réinitialisés, registres I/O matériels aux valeurs
    /// laissées par le boot ROM DMG au hand-off PC=$0100 (PanDocs « Power Up Sequence »), prêt à exécuter la ROM chargée.
    fn power_on(&mut self) {
        // Les sous-composants repartent chacun à leur état post-boot ROM :
        // - CPU::new()       → PC=0x0100, SP=$FFFE, registres corrects (cpu.rs).
        // - PPU::default()   → LCDC=$91 (LCD allumé, background and window enabled — bit 0), BGP=$FC, SCY/SCX/LYC/WX/WY=$00, OBP0/OBP1=$FF (ppu.rs).
        // - Serial::default()→ SB=$00, SC=$00, aucun transfert en cours (serial.rs).
        // - Timer::default() → TIMA/TMA=$00, TAC se lit $F8 → timer désactivé (timer.rs).
        self.cpu = CPU::new();
        self.mmu.ppu = PPU::default();
        self.mmu.serial = Serial::default();
        self.mmu.timer = Timer::default();
        // Aucun transfert DMA OAM en cours au power-on (Pan Docs « OAM DMA Transfer »).
        self.mmu.reset_dma();
        // Les régions de mémoire repartent à leurs valeurs au power-on (PanDocs « Power Up Sequence ») :
        // VRAM $FF, WRAM banque 0 = $11 / banque 1 = $FF, OAM $FF, HRAM $FF — SP=$FFFE pointe dans la HRAM.
        self.mmu.reset_memory();
        self.t_cycles = 0;
        self.instructions = 0;

        // Initialisation post-boot ROM (PanDocs « Power Up Sequence ») :
        // ces valeurs simulent l'état laissé par le boot ROM DMG au hand-off PC=$0100.
        // Certains registres ne peuvent PAS être posés via mmu.write car le matériel masque des bits.

        // Registres I/O de base
        self.mmu.write(0xFF00, 0xCF); // P1 ($FF00) : joypad, tous les boutons relâchés.
        self.mmu.timer.counter = (0xABu16) << 8; // DIV ($FF04) se lit $AB : write_div ignore la valeur écrite et remet le compteur à $0000, d'où l'écriture directe du compteur ; mmu.write(0xFF04, 0xAB) donnerait DIV=$00.
        self.mmu.write(0xFF07, 0xF8); // TAC ($FF07) : bits 7-3 toujours lus à 1 → timer désactivé au hand-off (seuls les bits 2-0 sont écrits).

        // Registres PPU
        self.mmu.write(0xFF40, 0x91); // LCDC ($FF40) : LCD ON (bit 7), background and window enabled (bit 0), tuiles non signées $8000-$8FFF (bit 4), carte $9800-$9BFF.
                                      // STAT ($FF41) left at the hardware default ($00): l'entrée en VBlank lève désormais le drapeau VBlank
                                      // (bit 0 de IF) inconditionnellement, so forcing STAT=$20 is no longer needed for a game that never configures
                                      // STAT to wake from HALT — the flag will be raised at each frame anyway.
        self.mmu.write(0xFF47, 0xFC); // BGP ($FF47) : palette background (valeur post-boot standard).
        self.mmu.write(0xFF48, 0xFF); // OBP0 ($FF48) : palette sprite 0.
        self.mmu.write(0xFF49, 0xFF); // OBP1 ($FF49) : palette sprite 1.

        // Registres Serial
        self.mmu.write(0xFF01, 0x00); // SB ($FF01).
        self.mmu.serial.sc = 0x7E; // SC ($FF02) : seuls les bits 7 et 0 sont écriturables — la valeur post-boot $7E (bits 6..1 à 1) est conservée telle quelle ; mmu.write(0xFF02, 0x7E) donnerait $00.

        // Interrupt Enable : Désactivé au hand-off (le jeu le configurera).
        self.mmu.ie = 0x00; // IE ($FFFF) : toutes les sources d'interruption désactivées au hand-off.

        log::info!(
            "[Emulator] Post-boot initialization complete: STAT left at hardware default ($00) — le drapeau VBlank (bit 0 de IF) est levé inconditionnellement à chaque entrée en VBlank"
        );
    }

    /// Exécute une instruction et renvoie les T-cycles consommés.
    pub fn step(&mut self) -> u32 {
        let cycles = self.cpu.step(&mut self.mmu);
        self.instructions += 1;
        self.t_cycles += cycles as u64;
        self.cpu.t_cycles = self.t_cycles; // garde le compteur du CPU synchronisé (throttle des logs par frame)
        self.mmu.cpu_halted = self.cpu.halted; // bit 5 en lecture seule du registre IF ($FF0F)

        // --- Diagnostic de blocage HALT : "battement de cœur" toutes les 10 000 instructions ---
        if self.cpu.halted && self.instructions % 10000 == 0 {
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

        // La PPU avance du même nombre de T-cycles (timing LY/mode, Pan Docs « Rendering ») et dessine
        // chaque scanline à la fin du mode Drawing : le rendu est donc incrémental, plus par frame. Le
        // framebuffer est toujours à jour après ce pas ; app.rs le présente tel quel. La transition LCD
        // on→off noircit l'écran via `on_lcd_off` (la PPU reste gelée tant que bit 7 du LCDC est à 0).
        let frame_done = {
            let ppu = &mut self.mmu.ppu;
            let vram = &self.mmu.vram;
            let oam = &self.mmu.oam;
            ppu.step(cycles, vram, oam) // renvoie true quand la frontière de frame est franchie (LY > 153 → 0)
        };
        if frame_done {
            log::debug!("[PPU] Frame boundary crossed at t_cycles={}", self.t_cycles);
        }
        // Les requêtes d'interruption PPU en attente lèvent les bits correspondants de IF ($FF0F).
        let ppu_irq = self.mmu.ppu.take_interrupts();

        // LOG CRITIQUE
        if ppu_irq != 0 {
            log::debug!(
                "[PPU] IRQ raised: ${:02X}, IF before: ${:02X}, LY={}, mode={}",
                ppu_irq,
                self.mmu.io[0x0F],
                self.mmu.ppu.ly,
                self.mmu.ppu.mode,
            );
        }

        if ppu_irq & IRQ_VBLANK != 0 {
            self.mmu.io[0x0F] |= 0x01; // bit 0 de IF : interruption VBlank demandée (vecteur $40)
            log::debug!("[VBlank] IF bit 0 set, new IF=${:02X}", self.mmu.io[0x0F]);
        }
        if ppu_irq & IRQ_STAT != 0 {
            self.mmu.io[0x0F] |= 0x02; // bit 1 de IF : interruption STAT/LCD demandée (vecteur $48)
        }
        // La SCC avance du même nombre de T-cycles ; un transfert achevé lève le drapeau IF série.
        if self.mmu.serial.tick(cycles) {
            self.mmu.io[0x0F] |= 0x08; // bit 3 de IF ($FF0F) : interruption série demandée (Pan Docs « Interrupt Sources »)
        }
        // Le DMA OAM avance du même nombre de T-cycles : l'écriture de $FF46 démarre un transfert de
        // 160 M-cycles pendant lequel le CPU n'accède plus qu'à la HRAM ($FF80-$FFFE) (Pan Docs « OAM DMA Transfer »).
        self.mmu.advance_dma(cycles);
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
    #[allow(dead_code)] // Réserve : le bouton « CPU Test » retiré du GUI était son seul usage hors tests.
    pub fn load_test_program(&mut self) {
        let mut rom = vec![0xFF; 0x4000];
        rom[0x0134..0x013C].copy_from_slice(b"CPU TEST"); // titre affiché dans la barre de menu
                                                          // Le code est placé AVANT le titre (l'exécution démarre à 0x0100 et passe par 0x0134) :
                                                          // les octets du titre ne doivent pas être exécutés. $FF = RST $38 (opcode valide SM83),
                                                          // d'où un préfixe de NOP explicites pour atteindre le code sans boucle de RST infinie.
        rom[0x0100..0x0108].fill(0x00); // 8 × NOP (32 T-cycles)
        let code: &[u8] = &[
            0x31, 0xFF, 0xDF, // 0x0108: LD SP, $DFFF (immédiat little-endian : FF puis DF)
            0x06, 0x2A, // 0x010B: LD B, $2A (42)
            0x3C, // 0x010D: INC A      ← début de la boucle
            0x05, // 0x010E: DEC B
            0x20,
            0xFC, // 0x010F: JR NZ, -4 → retour à INC A (cible = 0x0111 + (-4) = 0x010D)
            0x3C, // 0x0111: INC A      (après la boucle)
            0x00, // 0x0112: NOP        ← début du spin infini
            0x20, 0xFD, // 0x0113: JR -3 → retour à NOP (cible = 0x0115 + (-3) = 0x0112)
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cpu::flags::Flags;
    use crate::mmu::DMA_OAM_CYCLES;
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
        emu.mmu.write(0xFF40, 0x91); // LCD allumé + background and window enabled (bit 0) ; tuiles non signées $8000-$8FFF (bit 4), carte $9800-$9BFF
        emu.mmu.write(0xFF47, 0xE4); // BGP : teinte v pour une valeur de pixel v

        emu.run_frame(); // une frame vidéo : le rendu est mis à jour à la frontière de frame

        assert_eq!(emu.mmu.ppu.framebuffer[0], PPU::shade(3)); // moitié gauche de la tuile → teinte 3
        assert_eq!(emu.mmu.ppu.framebuffer[4], PPU::shade(0)); // moitié droite → teinte 0
    }

    #[test]
    fn frame_render_updates_sprites_from_oam() {
        let mut emu = Emulator::new();
        emu.load_test_program();
        emu.mmu.vram.fill(0); // VRAM vide (la valeur power-on est $FF) : le fond reste blanc hors de la tuile 1.

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
        emu.mmu.write(0xFE03, 0); // drapeaux : pas de priorité/flip, tuiles $8000-$8FFF (la valeur power-on est $FF).

        emu.run_frame(); // une frame vidéo : le rendu est mis à jour à la frontière de frame

        assert_eq!(
            emu.mmu.ppu.framebuffer[15 * SCREEN_WIDTH + 32],
            PPU::shade(3)
        ); // coin haut-gauche du sprite (ligne Y-1)
        assert_eq!(
            emu.mmu.ppu.framebuffer[22 * SCREEN_WIDTH + 39],
            PPU::shade(3)
        ); // coin bas-droite (ligne Y+6, colonne X+7)
        assert_eq!(
            emu.mmu.ppu.framebuffer[15 * SCREEN_WIDTH + 40],
            PPU::shade(0)
        ); // juste à droite du sprite : fond blanc
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
        // Mini-ROM qui active l'interruption VBlank (bit 5 de STAT), IE = $01, EI puis HALT :
        // le CPU doit se réveiller au vecteur $40 à l'entrée en VBlank de la frame courante.
        let mut rom = vec![0xFF; 0x4000];
        let code: &[u8] = &[
            0x31, 0xFF, 0xDF, // LD SP,$DFFF
            0x3E, 0x20, // LD A,$20
            0xF0,
            0x41, // LDH [$FF41],A → STAT = $20 : bit 5 posé (interruption VBlank activée)
            0x3E, 0x01, // LD A,$01
            0xF0, 0xFF, // LDH [$FFFF],A → IE = $01 : VBlank uniquement
            0xFB, // EI (IME effectif après l'instruction suivante)
            0x76, // HALT
        ];
        rom[0x0100..0x0100 + code.len()].copy_from_slice(code);
        rom[0x40] = 0xF0; // LDH [$FFC0],A : marqueur du handler VBlank
        rom[0x41] = 0xC0;
        rom[0x42] = 0xC9; // RET

        let mut emu = Emulator::new();
        emu.load_rom(rom);
        emu.mmu.write(0xFFC0, 0); // marqueur vidé ($FFC0 est en HRAM, qui contient $FF au power-on)
        assert_eq!(emu.mmu.read(0xFFC0), 0); // marqueur vide avant exécution

        emu.run_frame(); // l'entrée en VBlank (dot 65664) a lieu dans cette frame → bit 0 de IF → le CPU halté se réveille à $40
        assert_eq!(emu.mmu.read(0xFFC0), 0x01); // handler VBlank exécuté (A = $01)
    }

    #[test]
    fn vblank_interrupt_fires_even_without_the_stat_enable_bit() {
        // Mini-ROM qui pose IE = $01, EI puis HALT sans jamais toucher au STAT : l'entrée en VBlank doit lever le drapeau
        // VBlank (bit 0 de IF) inconditionnellement and wake the CPU at vector $40.
        let mut rom = vec![0xFF; 0x4000];
        let code: &[u8] = &[
            0x31, 0xFF, 0xDF, // LD SP,$DFFF
            0x3E, 0x01, // LD A,$01
            0xF0, 0xFF, // LDH [$FFFF],A → IE = $01 : VBlank uniquement
            0xFB, // EI (IME effectif after the next instruction)
            0x76, // HALT
        ];
        rom[0x0100..0x0100 + code.len()].copy_from_slice(code);
        rom[0x40] = 0xF0; // LDH [$FFC0],A : VBlank handler marker
        rom[0x41] = 0xC0;
        rom[0x42] = 0xC9; // RET

        let mut emu = Emulator::new();
        emu.load_rom(rom);
        emu.mmu.write(0xFFC0, 0); // marqueur vidé ($FFC0 est en HRAM, qui contient $FF au power-on)
        assert_eq!(emu.mmu.read(0xFF41) & 0x78, 0); // STAT : aucun bit d'activation posé

        emu.run_frame(); // l'entrée en VBlank (dot 65664) a lieu dans cette frame → bit 0 de IF levé inconditionnellement → le CPU halté se réveille à $40
        assert_eq!(emu.mmu.read(0xFFC0), 0x01); // handler VBlank exécuté (A = $01)
    }

    #[test]
    fn timer_interrupt_wakes_halted_cpu() {
        // Mini-ROM qui active le timer (TAC = $FC : bit 2 posé, sélection 00), TIMA = $FF, IE = $04 puis HALT :
        // le débordement de TIMA doit réveiller le CPU au vecteur $50.
        let mut rom = vec![0xFF; 0x4000];
        let code: &[u8] = &[
            0x31, 0xFF, 0xDF, // LD SP,$DFFF
            0x3E, 0xFC, // LD A,$FC
            0xF0, 0x07, // LDH [$FF07],A → TAC = $FC : timer activé (bit 2), sélection 00
            0x3E, 0xFF, // LD A,$FF
            0xF0, 0x05, // LDH [$FF05],A → TIMA = $FF : débordement au prochain tick
            0x3E, 0x04, // LD A,$04
            0xF0, 0xFF, // LDH [$FFFF],A → IE = $04 : Timer uniquement
            0xFB, // EI (IME effectif après l'instruction suivante)
            0x76, // HALT
        ];
        rom[0x0100..0x0100 + code.len()].copy_from_slice(code);
        rom[0x50] = 0xF0; // LDH [$FFC0],A : marqueur du handler Timer
        rom[0x51] = 0xC0;
        rom[0x52] = 0xC9; // RET

        let mut emu = Emulator::new();
        emu.load_rom(rom);
        emu.mmu.write(0xFFC0, 0); // marqueur vidé ($FFC0 est en HRAM, qui contient $FF au power-on)
        assert_eq!(emu.mmu.read(0xFFC0), 0); // marqueur vide avant exécution

        emu.run_frame();
        assert_eq!(emu.mmu.read(0xFFC0), 0x04); // handler Timer exécuté (A = $04)
    }

    #[test]
    fn timer_if_bit_stays_set_until_the_game_acknowledges_it() {
        // Mini-ROM qui active le timer (TAC = $FC : bit 2 posé, sélection 00), TIMA = $FF, IE = $04 puis HALT.
        // L'ISR à $50 incrémente un compteur HRAM et fait RETI SANS acknowledge the interruption Timer :
        // le bit 2 de IF doit rester posé (pas d'effacement automatique) and l'interruption se re-déclenche
        // immédiatement après each RETI — c'est le comportement réel du matériel pour un ISR qui oublie son ack.
        let mut rom = vec![0xFF; 0x4000];
        let code: &[u8] = &[
            0x31, 0xFF, 0xDF, // LD SP,$DFFF
            0x3E, 0xFC, // LD A,$FC
            0xF0, 0x07, // LDH [$FF07],A → TAC = $FC : timer activé (bit 2), sélection 00
            0x3E, 0xFF, // LD A,$FF
            0xF0, 0x05, // LDH [$FF05],A → TIMA = $FF : débordement au prochain tick
            0x3E, 0x04, // LD A,$04
            0xF0, 0xFF, // LDH [$FFFF],A → IE = $04 : Timer uniquement
            0xFB, // EI (IME effectif after the next instruction)
            0x76, // HALT ($0110)
            0x76, // HALT ($0111) : ré-entrée en HALT après le RETI du handler
        ];
        rom[0x0100..0x0100 + code.len()].copy_from_slice(code);
        rom[0x50] = 0xE0; // LDH A,[$FFC0] : lecture du compteur
        rom[0x51] = 0xC0;
        rom[0x52] = 0x3C; // INC A
        rom[0x53] = 0xF0; // LDH [$FFC0],A : écriture du compteur incrémenté
        rom[0x54] = 0xC0;
        rom[0x55] = 0xD9; // RETI (IME réactivé — aucun acknowledge !)

        let mut emu = Emulator::new();
        emu.load_rom(rom);
        emu.mmu.write(0xFFC0, 0); // compteur vidé ($FFC0 est en HRAM, qui contient $FF au power-on)

        emu.run_frame();
        assert_eq!(emu.mmu.io[0x0F] & 0x04, 0x04); // bit 2 de IF toujours posé : l'émulateur ne l'efface pas
        assert_ne!(emu.mmu.read(0xFFC0), 0); // l'ISR s'est re-déclenché (boucle réelle d'un ISR sans ack)
    }

    #[test]
    fn timer_isr_acknowledge_clears_the_if_bit() {
        // Même mini-ROM, mais the ISR acknowledge the interruption Timer en écrivant $04 dans $FF0F
        // (write-1-to-clear) avant RETI : le bit 2 de IF est effacé par the jeu lui-même and the CPU re-halts.
        let mut rom = vec![0xFF; 0x4000];
        let code: &[u8] = &[
            0x31, 0xFF, 0xDF, // LD SP,$DFFF
            0x3E, 0xFC, // LD A,$FC
            0xF0, 0x07, // LDH [$FF07],A → TAC = $FC : timer activé (bit 2), sélection 00
            0x3E, 0xFF, // LD A,$FF
            0xF0, 0x05, // LDH [$FF05],A → TIMA = $FF : débordement au prochain tick
            0x3E, 0x04, // LD A,$04
            0xF0, 0xFF, // LDH [$FFFF],A → IE = $04 : Timer uniquement
            0xFB, // EI (IME effectif after the next instruction)
            0x76, // HALT ($0110)
            0x76, // HALT ($0111) : ré-entrée en HALT après le RETI du handler
        ];
        rom[0x0100..0x0100 + code.len()].copy_from_slice(code);
        rom[0x50] = 0xF0; // LDH [$FFC0],A : marqueur du handler Timer (A = $04)
        rom[0x51] = 0xC0;
        rom[0x52] = 0xF0; // LDH [$FF0F],A : acknowledge the interruption Timer (write-1-to-clear bit 2)
        rom[0x53] = 0x0F;
        rom[0x54] = 0xD9; // RETI

        let mut emu = Emulator::new();
        emu.load_rom(rom);
        emu.mmu.write(0xFFC0, 0); // marqueur vidé ($FFC0 est en HRAM, qui contient $FF au power-on)

        emu.run_frame();
        assert_eq!(emu.mmu.read(0xFFC0), 0x04); // handler Timer exécuté (A = $04)
        assert_eq!(emu.mmu.io[0x0F] & 0x04, 0); // bit 2 de IF effacé par the write du jeu dans $FF0F
        assert!(emu.cpu.halted); // plus rien de pendan : le CPU re-halts proprement au second HALT ($0111)
    }

    #[test]
    fn vblank_interrupt_wakes_cpu_on_consecutive_frames() {
        // Mini-ROM qui active l'interruption VBlank (bit 5 de STAT), IE = $01, EI puis boucle HALT :
        // le CPU doit se réveiller au vecteur $40 à CHAQUE entrée en VBlank (une par frame).
        let mut rom = vec![0xFF; 0x4000];
        let code: &[u8] = &[
            0x31, 0xFF, 0xDF, // LD SP,$DFFF
            0x3E, 0x20, // LD A,$20
            0xF0,
            0x41, // LDH [$FF41],A → STAT = $20 : bit 5 posé (interruption VBlank activée)
            0x3E, 0x01, // LD A,$01
            0xF0, 0xFF, // LDH [$FFFF],A → IE = $01 : VBlank uniquement
            0xFB, // EI (IME effectif après l'instruction suivante)
            0x76, // HALT ($010C)
            0x76, // HALT ($010D) : ré-entrée en HALT après le RETI du handler
        ];
        rom[0x0100..0x0100 + code.len()].copy_from_slice(code);
        // Handler VBlank à $40 : incrémente un compteur dans HRAM puis RETI (IME réactivé).
        rom[0x40] = 0xE0; // LDH A,[$FFC0] (8) : lecture du compteur
        rom[0x41] = 0xC0;
        rom[0x42] = 0x3C; // INC A (4)
        rom[0x43] = 0xF0; // LDH [$FFC0],A (8) : écriture du compteur incrémenté
        rom[0x44] = 0xC0;
        rom[0x45] = 0x3E; // LD A,$01
        rom[0x46] = 0x01;
        rom[0x47] = 0xF0; // LDH [$FF0F],A (8) : acknowledge VBlank (write-1-to-clear bit 0 de IF)
        rom[0x48] = 0x0F;
        rom[0x49] = 0xD9; // RETI (16) : retour à $010D + IME réactivé

        let mut emu = Emulator::new();
        emu.load_rom(rom);
        emu.mmu.write(0xFFC0, 0); // compteur vidé ($FFC0 est en HRAM, qui contient $FF au power-on)
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
            0x3E, 0x48, // LD A,'H'
            0xF0, 0x01, // LDH [$FF01],A → SB='H'
            0x3E, 0x81, // LD A,$81
            0xF0, 0x02, // LDH [$FF02],A → SC=$81 : transfert démarré ('H')
            0x3E, 0x65, 0xF0, 0x01, 0x3E, 0x81, 0xF0, 0x02, // 'e'
            0x3E, 0x6C, 0xF0, 0x01, 0x3E, 0x81, 0xF0, 0x02, // 'l' (premier)
            0x3E, 0x6C, 0xF0, 0x01, 0x3E, 0x81, 0xF0, 0x02, // 'l' (deuxième)
            0x3E, 0x6F, 0xF0, 0x01, 0x3E, 0x81, 0xF0, 0x02, // 'o'
            0x3E, 0x0A, 0xF0, 0x01, 0x3E, 0x81, 0xF0,
            0x02, // '\n' : la ligne « Hello » est émise dans le log hôte
            0x00, // NOP
            0x20, 0xFD, // JR -3 → boucle infinie (NOP + JR)
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
            0x3E, 0x42, // LD A,'B'
            0xF0, 0x01, // LDH [$FF01],A → SB='B'
            0x3E, 0x81, // LD A,$81
            0xF0, 0x02, // LDH [$FF02],A → SC=$81 : transfert démarré (64 T-cycles)
            0x00, // NOP
            0x20, 0xFD, // JR -3 → boucle infinie (NOP + JR)
        ];
        rom[0x0100..0x0100 + code.len()].copy_from_slice(code);

        let mut emu = Emulator::new();
        emu.load_rom(rom); // exécution immédiate du code ROM (IME reste désactivé : le drapeau doit se lever quand même)

        assert_eq!(emu.mmu.io[0x0F] & 0x08, 0); // pas encore d'interruption série
        emu.run_tcycles(64 + 128); // le transfert de 64 T-cycles s'achève (avec une marge)

        assert_eq!(emu.mmu.io[0x0F] & 0x08, 0x08); // bit 3 de IF levé par l'achèvement du transfert
        assert_eq!(emu.mmu.io[0x0F] & 0x07, 0); // bits 0-2 (V-Blank/LC3C/Timer) non touchés
    }

    #[test]
    fn oam_dma_transfer_is_delayed_by_160_tcycles() {
        // Mini-ROM qui écrit $FF46 = $C0 via une vraie instruction CPU, puis tourne en boucle de NOP.
        let mut rom = vec![0xFF; 0x4000];
        let code: &[u8] = &[
            0x31, 0xFF, 0xDF, // LD SP,$DFFF
            0x3E, 0xC0, // LD A,$C0
            0xF0, 0x46, // LDH [$FF46],A → DMA OAM démarré (source $C000)
            0x00, // NOP
            0x20, 0xFD, // JR -3 → boucle infinie (NOP + JR)
        ];
        rom[0x0100..0x0100 + code.len()].copy_from_slice(code);

        let mut emu = Emulator::new();
        emu.load_rom(rom); // PC=$0100, aucune instruction exécutée encore.
        for i in 0..0xA0 {
            emu.mmu.write(0xC000 + i, (i as u8) ^ 0x5A); // données source en WRAM ($C000-$C09F)
        }

        assert!(!emu.mmu.dma_active()); // aucun transfert avant l'écriture de $FF46.
        emu.step(); // LD SP,$DFFF
        emu.step(); // LD A,$C0
        emu.step(); // LDH [$FF46],A : le transfert démarre après cette instruction (Pan Docs « OAM DMA Transfer »).

        assert!(emu.mmu.dma_active()); // le transfert est en cours...
        assert_eq!(emu.mmu.oam[0], 0xFF); // ...mais aucun octet n'a encore été copié (valeur power-on $FF).
        assert_eq!(emu.mmu.read(0xC000), 0xFF); // les lectures CPU hors HRAM sont bloquées → $FF (OAM comprise).

        emu.run_tcycles(144); // < 160 M-cycles : le transfert n'est pas achevé (marge de sécurité).
        assert!(emu.mmu.dma_active());
        assert_eq!(emu.mmu.read(0xC000), 0xFF); // toujours bloquées...

        emu.run_tcycles(DMA_OAM_CYCLES as u64 + 8); // au-delà de la fenêtre : le transfert est achevé.
        assert!(!emu.mmu.dma_active());

        for i in 0..0xA0 {
            assert_eq!(emu.mmu.read(0xFE00 + i), (i as u8) ^ 0x5A); // OAM = contenu de $C000-$C09F
        }
        assert_eq!(emu.mmu.read(0xFF46), 0xC0); // le registre DMA mémorise l'adresse source
    }
}
