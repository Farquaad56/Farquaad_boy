//! Timing cycle-accurate de la PPU : machine à états scanline par scanline (Pan Docs « Rendering »),
//! transitions de modes et génération des requêtes d'interruption.

use crate::ppu::constants::{
    DOTS_PER_LINE, IRQ_STAT, IRQ_VBLANK, LCDC_LCD_ON, MODE_DRAW_CYCLES, MODE_HBLANK_CYCLES,
    MODE_OAM_CYCLES, SCREEN_HEIGHT, STAT_IRQ_LYC, STAT_IRQ_MODE0, STAT_IRQ_MODE2,
    STAT_IRQ_VBLANK,
};

use super::PPU;

impl PPU {
    /// Avance la PPU de `cycles` T-cycles via a cycle-accurate scanline state machine (Pan Docs « Rendering »).
    /// Les T-cycles sont consommés en franchissant les frontières de mode : OAM Scan (80 dots) → Drawing (172 dots,
    /// the scanline courante is dessinée à la fin du Drawing) → HBlank (204 dots) → ligne suivante ; les lignes 144..153
    /// sont en VBlank (456 dots per line). Le LCD éteint (bit 7 de LCDC à 0) gèle la PPU complètement : aucun T-cycle n'est
    /// consommé and aucune avance de mode_clock/ly — les transitions on→off / off→on are gérées at the écriture of $FF40
    /// (`write_register`). Les requêtes d'interruption générées are accumulées dans `pending_irq` (`take_interrupts`).
    pub fn step(&mut self, cycles: u32, vram: &[u8; 0x2000], oam: &[u8; 0xA0]) -> bool {
        // LCD éteint (bit 7 du LCDC à 0) : la PPU est complètement gelée — elle ne consomme aucun T-cycle and n'avance pas.
        if self.lcdc & LCDC_LCD_ON == 0 {
            return false;
        }

        if cycles == 0 {
            return false;
        }

        let mut frame_completed = false;
        self.mode_clock += cycles;

        // On consomme les T-cycles en franchissant les frontières de mode, jusqu'à ce que the compteur soit strictement
        // inférieur à la durée du mode courant (une instruction peut en franchir plusieurs).
        loop {
            match self.mode {
                2 => {
                    // OAM Scan (80 dots) → Drawing.
                    if self.mode_clock < MODE_OAM_CYCLES {
                        break;
                    }
                    self.mode_clock -= MODE_OAM_CYCLES;
                    self.mode = 3;
                    self.update_stat_mode();
                }
                3 => {
                    // Drawing (172 dots) → HBlank : la scanline courante is terminée, on la dessine.
                    if self.mode_clock < MODE_DRAW_CYCLES {
                        break;
                    }
                    self.mode_clock -= MODE_DRAW_CYCLES;
                    self.mode = 0;
                    self.update_stat_mode();
                    self.render_scanline(self.ly, vram, oam); // C'est ICI que la scanline (self.ly) is dessinée
                    if self.stat & STAT_IRQ_MODE0 != 0 {
                        self.pending_irq |= IRQ_STAT; // entrée en mode 0 (HBlank) → bit 1 of IF
                    }
                }
                0 => {
                    // HBlank (204 dots) → ligne suivante.
                    if self.mode_clock < MODE_HBLANK_CYCLES {
                        break;
                    }
                    self.mode_clock -= MODE_HBLANK_CYCLES;
                    self.ly += 1;
                    if self.ly == SCREEN_HEIGHT as u8 {
                        // LY = 144 : entrée en VBlank.
                        self.mode = 1;
                        self.update_stat_mode();
                        self.request_vblank_interrupt();
                    } else {
                        // OAM Scan of the line suivante.
                        self.mode = 2;
                        self.update_stat_mode();
                        if self.stat & STAT_IRQ_MODE2 != 0 {
                            self.pending_irq |= IRQ_STAT; // entrée en mode 2 → bit 1 of IF
                        }
                    }
                    self.check_lyc_interrupt();
                }
                _ => {
                    // VBlank (456 dots per line, LY=144..153) → ligne suivante.
                    if self.mode_clock < DOTS_PER_LINE {
                        break;
                    }
                    self.mode_clock -= DOTS_PER_LINE;
                    self.ly += 1;
                    if self.ly > 153 {
                        // Retour à la line 0 : OAM Scan — une frame complète vient de se terminer.
                        frame_completed = true;
                        self.ly = 0;
                        self.mode = 2;
                        self.update_stat_mode();
                        if self.stat & STAT_IRQ_MODE2 != 0 {
                            self.pending_irq |= IRQ_STAT; // entrée en mode 2 → bit 1 of IF
                        }
                    }
                    self.check_lyc_interrupt();
                }
            }
        }

        frame_completed
    }

    /// Met à jour les bits 0-1 du registre STAT ($FF41) with le mode courant ; les bits d'activation (3..6) are conservés.
    pub(crate) fn update_stat_mode(&mut self) {
        self.stat = (self.stat & !0x03) | (self.mode & 0x03);
    }

    /// Entrée en VBlank (LY passant de 143 to 144) : le drapeau VBlank (bit 0 of IF) is levé inconditionnellement,
    /// plus the drapeau STAT/LCD (bit 1 of IF) si the interruption mode 1 is activée (bit 5 du STAT).
    fn request_vblank_interrupt(&mut self) {
        self.pending_irq |= IRQ_VBLANK; // bit 0 of IF
        if self.stat & STAT_IRQ_VBLANK != 0 {
            self.pending_irq |= IRQ_STAT; // interruption mode 1 activée (bit 5 du STAT) → bit 1 of IF aussi
        }
    }

    /// LYC==LY (début of the line lyc) : si l'interruption is activée (bit 3 du STAT), le drapeau STAT (bit 1 of IF) is levé.
    fn check_lyc_interrupt(&mut self) {
        if self.stat & STAT_IRQ_LYC != 0 && self.ly == self.lyc {
            self.pending_irq |= IRQ_STAT; // bit 1 of IF
        }
    }
}
