//! Pixel Processing Unit : registres PPU ($FF40-$FF4B), timing et framebuffer (160×144).
//!
//! Étape 1 — fondation : les douze registres 8 bits de la PPU ($FF40-$FF4B, dont le DMA OAM $FF46),
//! leurs sémantiques de lecture/écriture (Pan Docs « LCDC », « STAT ») et l'état post-boot ROM
//! (PanDocs « Power Up Sequence » : LY = 0, LCDC = $91, BGP = $FC ; OBP0/OBP1 sont laissées non
//! initialisées par the boot ROM — la valeur la plus fréquente, $FF, is retenue).
//! Étape 2 — timing cycle-accurate : `step` avance la PPU en T-cycles via a scanline state machine
//! (OAM Scan 80 dots / Drawing 172 dots / HBlank 204 dots / VBlank 456 dots per line, 154 lignes/frame),
//! suit LY et le mode selon Pan Docs « Rendering », and génère les requêtes d'interruption VBlank and STAT
//! (Pan Docs « Interrupt Sources »). L'entrée en VBlank (LY passant de 143 to 144) lève le drapeau VBlank
//! (bit 0 of IF) inconditionnellement, plus the drapeau STAT/LCD (bit 1 of IF) si the interruption mode 1 is activée
//! (bit 5 du STAT) ; la lecture du STAT ($FF41) renvoie le mode courant en bits 0-1. Le LCD éteint
//! (bit 7 de LCDC à 0) gèle la PPU and autorise les écritures directes en VRAM.
//! Étape 3 — rendu : la VRAM ($8000-$9FFF : tuiles + cartes) and l'OAM (160 octets, $FE00-$FE9F) are
//! dédoublonnées with the MMU ; `render_scanline` dessine a scanline (Background défilement SCX/SCY, carte chosen by the bit 3 du LCDC,
//! données de tuiles chosen by the bit 4, palette BGP) puis la fenêtre (WY/WX, carte chosen by the bit 6 du LCDC) selon PanDocs « Background »/« Window ».
//! Étape 4 — objets : `render_scanline` dessine aussi les 40 entrées OAM ($FE00-$FE9F) selon PanDocs « OAM » :
//! position (X, Y-1) en 8×8 and (X, Y-16) in 8×16 with repli à 256, taille 8×8 or 8×16 selon the bit 2 du LCDC, tuile $8000-$8FFF (index non signé),
//! retournements X/Y (bits 6/5 des drapeaux), palette OBP0/OBP1 (bit 4 des drapeaux), transparence of the value 0, priorité face au fond
//! (bit 7 des drapeaux) and limite de 10 objets per line.

pub mod constants;
mod renderer;
mod sprites;
mod timing;

#[cfg(test)]
mod tests;

// Re-exporte les constantes au niveau du module pour préserver les chemins `crate::ppu::*` (ex : SCREEN_WIDTH).
pub use self::constants::*;

/// Pixel Processing Unit.
#[allow(clippy::upper_case_acronyms)]
pub struct PPU {
    /// Contrôle LCD — registre LCDC ($FF40) : activation du LCD/PPU, tuiles and cartes of the fond/fenêtre, sprites.
    pub lcdc: u8,
    /// Partie écrite du STAT ($FF41) : bits d'activation des interruptions (bits 3..6).
    pub stat: u8,
    /// Défilement vertical — registre SCY ($FF42).
    pub scy: u8,
    /// Défilement horizontal — registre SCX ($FF43).
    pub scx: u8,
    /// Ligne de balayage courante (0..=153 : 0..143 visibles, 144..153 VBlank) — registre LY ($FF44), en lecture seule.
    pub ly: u8,
    /// Ligne of comparison — registre LYC ($FF45).
    pub lyc: u8,
    /// DMA OAM — registre $FF46 : source d'adresse du transfert des 160 octets d'OAM.
    pub dma: u8,
    /// Palette Background/Window — registre BGP ($FF47) : chaque paire of bits sélectionne la teinte DMG des pixels of value 0..3.
    pub bgp: u8,
    /// Palette sprite 0 (par défaut) — registre OBP0 ($FF48).
    pub obp0: u8,
    /// Palette sprite 1 — registre OBP1 ($FF49).
    pub obp1: u8,
    /// Position X of the fenêtre — registre WX ($FF4A).
    pub wx: u8,
    /// Position Y of the fenêtre — registre WY ($FF4B).
    pub wy: u8,

    /// Compteur de T-cycles dans le mode courant (machine à états scanline par scanline).
    pub mode_clock: u32,
    /// Mode PPU courant : 0 = HBlank, 1 = VBlank, 2 = OAM Scan, 3 = Drawing.
    pub mode: u8,

    /// Requêtes d'interruption PPU en attente, consommées par `take_interrupts`.
    pending_irq: u8,
    /// Diagnostic : teinte unique of the frame précédente si l'écran était uniforme ($FFFFFFFF = non uniforme).
    last_uniform_shade: u32,

    /// Framebuffer écran (couleurs RGBA sur 32 bits, R dans l'octet le plus bas — compatible zero-copy egui/bytemuck).
    pub framebuffer: [u32; SCREEN_WIDTH * SCREEN_HEIGHT],
}

impl PPU {
    /// Crée une PPU à l'état post-boot ROM (PanDocs « Power Up Sequence ») : LCDC = $91 (LCD allumé,
    /// fond activé, fenêtre éteinte), BGP = $FC ; OBP0/OBP1 sont laissées non initialisées par the boot
    /// ROM — la valeur la plus fréquente, $FF, is retenue. Les autres registres valent $00 ; LY = 0
    /// and le compteur de dots est à 0. Le LCD étant allumé au power-on, la PPU démarre en mode 2
    /// (OAM Scan) sur the line 0 ; le framebuffer is noir opaque (aucune frame rendue).
    pub fn new() -> Self {
        Self {
            lcdc: 0x91, // LCD allumé (bit 7) + tuiles signées $8800-$97FF (bit 4) + fond activé (bit 0)
            stat: 0x00,
            scy: 0x00,
            scx: 0x00,
            ly: 0x00, // LY = 0 au power-on (en lecture seule)
            lyc: 0x00,
            dma: 0x00,
            bgp: 0xFC, // valeurs of pixel 0-1 → teinte claire, 2-3 → teinte foncée (Pan Docs « Power-On Values »)
            obp0: 0xFF, // non initialisées par the boot ROM — valeur la plus fréquente (PanDocs « Power Up Sequence »)
            obp1: 0xFF,
            wx: 0x00,
            wy: 0x00,
            mode_clock: 0, // début of the scanline 0 (OAM Scan)
            mode: 2,       // OAM Scan : LCD allumé, line 0
            pending_irq: 0x00,
            last_uniform_shade: !0u32, // aucune uniformité détectée avant the première frame rendue
            framebuffer: [0xFF00_0000; SCREEN_WIDTH * SCREEN_HEIGHT], // noir opaque (aucune frame rendue)
        }
    }

    /// Lit un registre PPU ($FF40-$FF4B). Le STAT renvoie les bits d'activation écrits (bits 3..6), le mode
    /// PPU courant en bits 0-1 (lecture seule : 0=HBlank, 1=VBlank, 2=OAM Scan, 3=Drawing) et le drapeau LYC==LY
    /// en bit 7 (lecture seule, constamment mis à jour). La ligne courante is lue via $FF44.
    pub fn read_register(&self, addr: u16) -> u8 {
        match addr {
            0xFF40 => self.lcdc,
            0xFF41 => {
                let mut value = self.stat & 0x78; // bits d'activation des interruptions (bits 3..6)
                value |= self.mode & 0x03; // mode PPU courant (lecture seule, bits 0-1) : 0=HBlank, 1=VBlank, 2=OAM Scan, 3=Drawing
                if self.ly == self.lyc {
                    value |= 1 << 7; // drapeau LYC==LY (lecture seule, constamment mis à jour)
                }
                value
            }
            0xFF42 => self.scy,
            0xFF43 => self.scx,
            0xFF44 => self.ly, // en lecture seule
            0xFF45 => self.lyc,
            0xFF46 => self.dma,
            0xFF47 => self.bgp,
            0xFF48 => self.obp0,
            0xFF49 => self.obp1,
            0xFF4A => self.wx,
            0xFF4B => self.wy,
            _ => 0x00, // hors plage $FF40-$FF4B : rien à lire (le MMU ne route que cette plage)
        }
    }

    /// Écrit un registre PPU ($FF40-$FF4B). LY ($FF44) is en lecture seule (écriture ignorée) ;
    /// seuls les bits 3..6 du STAT sont écrits. La transition LCD on→off (bit 7 du LCDC passant de 1 to 0)
    /// gèle la PPU at line 0 in HBlank (`on_lcd_off`) ; the transition off→on repart d'une frame complète depuis line 0.
    pub fn write_register(&mut self, addr: u16, value: u8) {
        match addr {
            0xFF40 => {
                let old_lcdc = self.lcdc; // Ancienne valeur de LCDC avant l'écriture
                self.lcdc = value;

                if old_lcdc & LCDC_LCD_ON != 0 && self.lcdc & LCDC_LCD_ON == 0 {
                    // LCD vient d'être éteint (bit 7 passe de 1 to 0) : la PPU est gelée at line 0, HBlank.
                    self.on_lcd_off();
                } else if old_lcdc & LCDC_LCD_ON == 0 && self.lcdc & LCDC_LCD_ON != 0 {
                    // LCD vient d'être rallumé (bit 7 passe de 0 to 1) : la PPU repart d'une frame complète depuis line 0 (OAM Scan), comme au power-on.
                    self.ly = 0;
                    self.mode = 2;
                    self.mode_clock = 0;
                    self.update_stat_mode();
                }
            }
            0xFF41 => {
                // Comportement matériel standard (PanDocs « STAT ») : seuls les bits 3..6 sont écrits.
                self.stat = value & 0x78;
            }
            0xFF42 => self.scy = value,
            0xFF43 => self.scx = value,
            0xFF44 => {} // LY : en lecture seule — l'écriture is ignorée
            0xFF45 => self.lyc = value,
            0xFF46 => self.dma = value,
            0xFF47 => self.bgp = value,
            0xFF48 => self.obp0 = value,
            0xFF49 => self.obp1 = value,
            0xFF4A => self.wx = value,
            0xFF4B => self.wy = value,
            _ => {} // hors plage : rien à écrire (le MMU ne route que cette plage)
        }
    }

    /// Transition LCD on→off (bit 7 du LCDC passant de 1 to 0) : LY is remis à 0, la PPU passe en mode 0 (HBlank),
    /// and the écran becomes noir. Les écritures directes en VRAM restent autorisées sans attendre les HBlanks.
    fn on_lcd_off(&mut self) {
        self.ly = 0;
        self.mode = 0; // HBlank : la PPU est gelée au début de la ligne 0 (Gekkio PDF §9)
        self.mode_clock = 0;
        self.update_stat_mode();
        self.framebuffer.fill(0xFF00_0000); // écran noir
        self.last_uniform_shade = 0xFF00_0000;
    }

    /// Renvoie les requêtes d'interruption PPU en attente and les efface (Pan Docs « Interrupt Sources ») :
    /// bit 0 = VBlank, levé à chaque entrée en VBlank (→ bit 0 of IF), bit 1 = STAT/LCD (mode 0, LYC==LY, mode 2,
    /// or entrée en VBlank with the interruption mode 1 activée → bit 1 of IF).
    pub fn take_interrupts(&mut self) -> u8 {
        let irq = self.pending_irq;
        self.pending_irq = 0;
        irq
    }
}

impl Default for PPU {
    fn default() -> Self {
        Self::new()
    }
}

