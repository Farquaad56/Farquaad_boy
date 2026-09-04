//! Pixel Processing Unit : registres PPU ($FF40-$FF4B), timing et framebuffer (160×144).
//!
//! Étape 1 — fondation : les douze registres 8 bits de la PPU ($FF40-$FF4B, dont le DMA OAM $FF46),
//! leurs sémantiques de lecture/écriture (Pan Docs « LCDC », « STAT ») et l'état post-boot ROM
//! (PanDocs « Power Up Sequence » : LY = 0, LCDC = $91, BGP = $FC ; OBP0/OBP1 sont laissées non
//! initialisées par the boot ROM — la valeur la plus fréquente, $FF, est retenue).
//! Étape 2 — timing cycle-accurate : `step` avance la PPU en T-cycles via a scanline state machine
//! (OAM Scan 80 dots / Drawing 172 dots / HBlank 204 dots / VBlank 456 dots per line, 154 lignes/frame),
//! suit LY et le mode selon Pan Docs « Rendering », and génère les requêtes d'interruption VBlank and STAT
//! (Pan Docs « Interrupt Sources »). L'entrée en VBlank (LY passant de 143 à 144) lève le drapeau VBlank
//! (bit 0 of IF) inconditionnellement, plus the drapeau STAT/LCD (bit 1 of IF) si the interruption mode 1 is activée
//! (bit 5 du STAT) ; la lecture du STAT ($FF41) renvoie le mode courant en bits 0-1. Le LCD éteint
//! (bit 7 de LCDC à 0) gèle la PPU and autorise les écritures directes en VRAM.
//! Étape 3 — rendu : la VRAM ($8000-$9FFF : tuiles + cartes) and l'OAM (160 octets, $FE00-$FE9F) are
//! dédoublonnées with the MMU ; `render_scanline` dessine a scanline (Background défilement SCX/SCY, carte chosen by the bit 3 du LCDC,
//! données de tuiles chosen by the bit 4, palette BGP) puis la fenêtre (WY/WX, carte chosen by the bit 6 du LCDC) selon PanDocs « Background »/« Window ».
//! Étape 4 — objets : `render_scanline` dessine aussi les 40 entrées OAM ($FE00-$FE9F) selon PanDocs « OAM » :
//! position (X-8, Y-16) with repli à 256, taille 8×8 or 8×16 selon the bit 2 du LCDC, tuile $8000-$8FFF (index non signé),
//! retournements X/Y (bits 6/5 des drapeaux), palette OBP0/OBP1 (bit 4 des drapeaux), transparence of the value 0, priorité face au fond
//! (bit 7 des drapeaux) and limite de 10 objets per line.

/// Résolution de l'écran en pixels (Pan Docs « Graphics »).
pub const SCREEN_WIDTH: usize = 160;
pub const SCREEN_HEIGHT: usize = 144;

/// Nombre of dots (T-cycles) per scanline (Pan Docs « Rendering » : 456 dots × 154 lignes/frame).
pub const DOTS_PER_LINE: u32 = 456;

/// Nombre of lines per frame (Pan Docs « Rendering » : 456 dots × 154 lignes/frame).
pub const FRAME_LINES: u32 = 154;

/// Dots (T-cycles) per frame vidéo : 154 lignes × 456 dots.
pub const FRAME_DOTS: u64 = FRAME_LINES as u64 * DOTS_PER_LINE as u64; // 70224

/// Dot auquel commence le VBlank : début of the line 144 (Pan Docs « Rendering »).
/// Utilisé par les tests de timing (`mod tests`) ; documente aussi le point où démarre le VBlank.
#[allow(dead_code)] // n'est référencé que depuis `mod tests` dans un crate binaire
pub const VBLANK_START_DOT: u64 = SCREEN_HEIGHT as u64 * DOTS_PER_LINE as u64; // 65664

/// Durée du mode OAM Scan en T-cycles (Pan Docs « Rendering » : 80 dots).
const MODE_OAM_CYCLES: u32 = 80;
/// Durée du mode Drawing en T-cycles (172 dots).
const MODE_DRAW_CYCLES: u32 = 172;
/// Durée du mode HBlank en T-cycles (204 dots).
const MODE_HBLANK_CYCLES: u32 = 204;

/// Bit 3 du STAT : activation de l'interruption LYC==LY.
pub const STAT_IRQ_LYC: u8 = 1 << 3;
/// Bit 4 du STAT : activation of the interruption mode 0 (HBlank).
pub const STAT_IRQ_MODE0: u8 = 1 << 4;
/// Bit 5 du STAT : activation of the interruption VBlank (mode 1).
pub const STAT_IRQ_VBLANK: u8 = 1 << 5;
/// Bit 6 du STAT : activation of the interruption mode 2 (OAM Scan).
pub const STAT_IRQ_MODE2: u8 = 1 << 6;

/// Interruption VBlank en attente → bit 0 of IF ($FF0F), vecteur $40 (Pan Docs « Interrupt Sources »).
pub const IRQ_VBLANK: u8 = 1 << 0;
/// Interruption STAT/LCD en attente (mode 0, LYC==LY or mode 2) → bit 1 of IF ($FF0F), vecteur $48.
pub const IRQ_STAT: u8 = 1 << 1;

/// Bit 7 du LCDC : activation du LCD/PPU (à 0, la PPU est gelée and l'écran is noir).
pub const LCDC_LCD_ON: u8 = 1 << 7;
/// Bit 1 du LCDC : activation of the affichage des sprites (OAM).
pub const LCDC_SPRITE_ON: u8 = 1 << 1;
/// Bit 2 du LCDC : ensemble of tuiles du fond en $8800-$97FF (indices signés) au lieu of $8000-$8FFF.
pub const LCDC_TILE_SET_8800: u8 = 1 << 2;
/// Bit 3 du LCDC : carte du fond en $9C00-$9BFF au lieu of $9800-$9BFF.
pub const LCDC_BG_MAP_9C00: u8 = 1 << 3;
/// Bit 4 du LCDC : activation of the couche Background (à 0, aucun pixel de fond n'est généré).
pub const LCDC_BG: u8 = 1 << 4;
/// Bit 5 du LCDC : activation of the fenêtre (carte $9C00-$9BFF).
pub const LCDC_WINDOW: u8 = 1 << 5;
/// Bit 6 du LCDC : ensemble of tuiles of the fenêtre en $8800-$97FF au lieu of $8000-$8FFF.
pub const LCDC_WIN_TILE_8800: u8 = 1 << 6;

/// Bits 4-5 du LCDC : activation of the couche Background and of the fenêtre (ensemble).
#[allow(dead_code)] // Utilisé par les tests PPU ; API publique for the parties futures.
pub const LCDC_BG_WIN_ON: u8 = LCDC_BG | LCDC_WINDOW;
/// Bit 0 du LCDC (inutilisé sur DMG) : objets 8×16 au lieu of 8×8 — TODO(partie PPU) : non encore implémenté.
#[allow(dead_code)] // Utilisé par les tests PPU ; API publique for the future prise en charge des sprites 8×16.
pub const LCDC_OBJ_SIZE_16: u8 = 1 << 0;

/// Bit 3 des drapeaux d'une entrée OAM : ensemble of tuiles of the sprite en $8800-$97FF au lieu of $8000-$8FFF.
pub const SPRITE_TILE_SET_8800: u8 = 1 << 3;
/// Bit 4 des drapeaux d'une entrée OAM : priorité — le sprite is dessiné devant the couche Background.
pub const SPRITE_PRIORITY: u8 = 1 << 4;
/// Bit 5 des drapeaux d'une entrée OAM : retournement vertical of the sprite.
pub const SPRITE_Y_FLIP: u8 = 1 << 5;
/// Bit 6 des drapeaux d'une entrée OAM : retournement horizontal of the sprite.
pub const SPRITE_X_FLIP: u8 = 1 << 6;

/// Base dans la VRAM of the carte du fond par défaut ($9800) — index relatif à $8000.
const BG_MAP_9800: usize = 0x9800 - 0x8000;
/// Base dans la VRAM of the carte en $9C00 (fond si bit 3 du LCDC, and fenêtre) — index relatif à $8000.
const BG_MAP_9C00: usize = 0x9C00 - 0x8000;
/// Base dans la VRAM of the ensemble of tuiles en $8800 (bit 2 du LCDC, indices signés) — index relatif à $8000.
const TILE_SET_8800: usize = 0x8800 - 0x8000;

/// Les 4 teintes DMG classiques (Pan Docs « Graphics ») : [R, G, B].
pub const DMG_SHADES: [[u8; 3]; 4] = [
    [0x9B, 0xBC, 0x0F], // Teinte 0 : Vert clair (#9BBC0F)
    [0x8B, 0xAC, 0x0F], // Teinte 1 : Vert moyen (#8BAC0F)
    [0x30, 0x62, 0x30], // Teinte 2 : Vert foncé (#306230)
    [0x0F, 0x38, 0x0F], // Teinte 3 : Vert très foncé (#0F380F)
];
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

    /// Le LCD était allumé à l'appel précédent de `step` (pour détecter la transition on→off).
    lcd_was_on: bool,
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
            lcdc: 0x91, // LCD allumé (bit 7) + background and window enabled (bit 0) + tile data unsigned (bit 4)
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
            lcd_was_on: true, // le LCD est allumé au power-on (bit 7 du LCDC = 1)
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
    /// seuls les bits 3..6 du STAT sont écrits.
    pub fn write_register(&mut self, addr: u16, value: u8) {
        match addr {
            0xFF40 => self.lcdc = value,
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
    /// Avance la PPU de `cycles` T-cycles via a cycle-accurate scanline state machine (Pan Docs « Rendering »).
    /// Les T-cycles sont consommés en franchissant les frontières de mode : OAM Scan (80 dots) → Drawing (172 dots,
    /// the scanline courante is dessinée à la fin du Drawing) → HBlank (204 dots) → ligne suivante ; les lignes 144..153
    /// sont en VBlank (456 dots per line). Le LCD éteint (bit 7 de LCDC à 0) gèle le timing and autorise les écritures
    /// directes en VRAM. Les requêtes d'interruption générées are accumulées dans `pending_irq` (`take_interrupts`).
    pub fn step(&mut self, cycles: u32, vram: &[u8; 0x2000], oam: &[u8; 0xA0]) -> bool {
        // LCD éteint (bit 7 du LCDC à 0) : la PPU est gelée — pas d'avancement de mode_clock/ly.
        if self.lcdc & LCDC_LCD_ON == 0 {
            if self.lcd_was_on {
                self.on_lcd_off(); // transition on→off : LY=0, mode=2 (OAM), écran noir (Pan Docs)
            }
            self.lcd_was_on = false;
            return false;
        }
        self.lcd_was_on = true;

        if cycles == 0 {
            return false;
        }

        let mut frame_completed = false;
        self.mode_clock += cycles;

        // On consomme les T-cycles en franchissant les frontières de mode, jusqu'à ce que le compteur soit strictement
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
    /// Met à jour les bits 0-1 du registre STAT ($FF41) with le mode courant ; les bits d'activation (3..6) sont conservés.
    fn update_stat_mode(&mut self) {
        self.stat = (self.stat & !0x03) | (self.mode & 0x03);
    }

    /// Entrée en VBlank (LY passant de 143 à 144) : le drapeau VBlank (bit 0 of IF) is levé inconditionnellement,
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

    /// Transition LCD on→off (bit 7 du LCDC passant de 1 à 0) : LY is remis à 0, la PPU repart en mode 2 (OAM Scan),
    /// and the écran becomes noir. Les écritures directes en VRAM restent autorisées sans attendre les HBlanks.
    fn on_lcd_off(&mut self) {
        self.ly = 0;
        self.mode = 2; // OAM Scan : la PPU reprendra à la line 0 when the LCD is rallumé
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

    /// Rend la frame courante dans le framebuffer : Background (défilement SCX/SCY, carte choisie by the bit 3 du LCDC,
    /// données de tuiles chosen by the bit 4, palette BGP) puis fenêtre (WY/WX), selon PanDocs « Background »/« Window ».
    /// La PPU cycle-accurate (`step`) dessine chaque scanline via `render_scanline` à la fin du mode Drawing ; cette
    /// méthode rend les 144 lignes visibles d'un coup (utile aux tests et au rendu complet).
    #[allow(dead_code)] // le rendu normal est incrémental via `step` ; cette méthode sert surtout aux tests
    pub fn render_frame(&mut self, vram: &[u8; 0x2000], oam: &[u8; 0xA0]) {
        if self.lcdc & LCDC_LCD_ON == 0 {
            self.framebuffer.fill(0xFF00_0000); // LCD éteint : écran noir
            self.last_uniform_shade = 0xFF00_0000;
            return;
        }

        for y in 0..SCREEN_HEIGHT as u32 {
            self.render_scanline(y as u8, vram, oam); // y < SCREEN_HEIGHT (144) : le cast en u8 est sûr
        }

        // Diagnostic : teinte unique si l'écran est uniforme ($FFFFFFFF sinon).
        let first = self.framebuffer[0];
        let uniform = self.framebuffer.iter().all(|&px| px == first);
        self.last_uniform_shade = if uniform { first } else { !0u32 };
    }

    /// Dessine la scanline `ly` (lignes 0..143 visibles) dans le framebuffer : Background (défilement SCX/SCY, carte
    /// choisie by the bit 3 du LCDC, données de tuiles chosen by the bit 4, palette BGP), puis fenêtre (WY/WX), puis les
    /// sprites recouvrant la ligne (au plus 10, dans l'ordre OAM). Appelé par `step` à la fin du mode Drawing.
    pub fn render_scanline(&mut self, ly: u8, vram: &[u8; 0x2000], oam: &[u8; 0xA0]) {
        let y = ly as u32;
        if y >= SCREEN_HEIGHT as u32 {
            return; // VBlank (lignes 144..153) : rien à dessiner
        }

        let scx = self.scx as u32;
        let scy = self.scy as u32;
        let bg_on = self.lcdc & LCDC_BG != 0;
        let map_base = if self.lcdc & LCDC_BG_MAP_9C00 != 0 { BG_MAP_9C00 } else { BG_MAP_9800 };
        let signed_tiles = self.lcdc & LCDC_TILE_SET_8800 != 0;
        let window_on = self.lcdc & LCDC_WINDOW != 0;
        let win_tile_base = if self.lcdc & LCDC_WIN_TILE_8800 != 0 { TILE_SET_8800 } else { 0 };

        // Sélection des sprites recouvrant cette ligne : au plus 10, les premiers dans l'ordre OAM (Pan Docs « Sprite »).
        // Les objets sont rendus quand la couche d'objets est activée (bit 1 du LCDC) OU when the fond is éteint
        // (aucune couche Background à masquer : les sprites apparaissent sur la base noire).
        let mut selected = [false; 40];
        if self.lcdc & LCDC_SPRITE_ON != 0 || self.lcdc & LCDC_BG == 0 {
            let mut count = 0usize;
            for i in 0..40 {
                if Self::sprite_covers_row(y, oam[4 * i]) && Self::sprite_has_visible_column(oam[4 * i + 1]) {
                    selected[i] = true;
                    count += 1;
                    if count == 10 {
                        break; // limite de 10 sprites par ligne : les suivants sont supprimés
                    }
                }
            }
        }

        let row_start = y as usize * SCREEN_WIDTH;
        // Valeur brute (0..3) du pixel Background/Fenêtre sous chaque colonne, pour la priorité des sprites ;
        // None quand aucune couche n'est présente (fond éteint et pas de fenêtre sur cette colonne).
        let mut under: [Option<u8>; SCREEN_WIDTH] = [None; SCREEN_WIDTH];

        if bg_on {
            // Défilement vertical en pixels : la ligne de la carte se répète toutes les 256 lignes.
            let bg_y = (y + scy) & 0xFF;
            let map_row = (bg_y >> 3) as usize;

            for x in 0..SCREEN_WIDTH as u32 {
                // Défilement horizontal en pixels : colonne de tuile + pixel dans la tuile.
                let bg_x = (scx + x) & 0xFF;
                let tile_index = vram[map_base + map_row * 32 + (bg_x >> 3) as usize];
                let tile_addr = if signed_tiles {
                    // Indices signés : la valeur v pointe vers $8800 + v*16 ($8000-$97FF).
                    (TILE_SET_8800 as i32 + (tile_index as i8) as i32 * 16) as usize
                } else {
                    tile_index as usize * 16 // indices non signés ($8000-$8FFF)
                };
                let pixel = Self::tile_pixel(vram, tile_addr, bg_y & 7, bg_x & 7);
                under[x as usize] = Some(pixel);
                self.framebuffer[row_start + x as usize] = Self::shade(self.bgp >> (pixel * 2));
            }
        } else {
            // Couche Background éteinte : base noire.
            self.framebuffer[row_start..row_start + SCREEN_WIDTH].fill(0xFF00_0000);
        }

        // Fenêtre : pas de défilement ; elle apparaît à partir de la ligne WY et de la colonne WX+1 (Pan Docs « Window »).
        if window_on && y >= self.wy as u32 {
            let win_y = y - self.wy as u32;
            for x in (self.wx as u32 + 1)..SCREEN_WIDTH as u32 {
                let win_x = x - self.wx as u32 - 1; // colonne de la fenêtre : le pixel le plus à gauche est en WX+1
                let tile_index = vram[BG_MAP_9C00 + (win_y >> 3) as usize * 32 + (win_x >> 3) as usize];
                let pixel = Self::tile_pixel(
                    vram,
                    win_tile_base + tile_index as usize * 16, // indices non signés ; ensemble $8800-$97FF or $8000-$8FFF (bit 6 du LCDC)
                    win_y & 7,
                    win_x & 7,
                );
                if pixel != 0 {
                    // Les pixels de valeur 0 sont transparents : la couche en dessous passe au travers.
                    under[x as usize] = Some(pixel);
                    self.framebuffer[row_start + x as usize] = Self::shade(self.bgp >> (pixel * 2));
                }
            }
        }

        // Sprites, dans l'ordre OAM : un pixel de valeur non nulle est dessiné devant the couche en dessous si le
        // sprite a la priorité (bit 4), sinon seulement là where cette couche is transparente (valeur 0 or absente).
        for (i, &is_selected) in selected.iter().enumerate() {
            if is_selected {
                self.draw_sprite(vram, oam, i, y, &under);
            }
        }
    }

    /// Dessine l'entrée OAM `i` sur la ligne `y` (Pan Docs « Sprite ») : 8×8 pixels en position (X, Y-1),
    /// repli à 256 ; tuile $8000-$8FFF or $8800-$97FF selon the bit 3 des drapeaux (index non signé) ;
    /// retournements X/Y (bits 6/5) ; la valeur of pixel 0 est transparente ; les valeurs 1..3 are mappées
    /// by OBP0/OBP1 (le bit 1 of the value sélectionne OBP1). Un sprite sans priorité (bit 4 à 0) n'est
    /// dessiné que là où la couche Background/Fenêtre en dessous est transparente (`under` = None or 0) ;
    /// un sprite with priorité is always drawn devant.
    fn draw_sprite(&mut self, vram: &[u8; 0x2000], oam: &[u8; 0xA0], i: usize, y: u32, under: &[Option<u8>; SCREEN_WIDTH]) {
        let flags = oam[4 * i + 3];
        let priority = flags & SPRITE_PRIORITY != 0;
        let tile_base = if flags & SPRITE_TILE_SET_8800 != 0 { TILE_SET_8800 } else { 0 };
        let tile_addr = tile_base + oam[4 * i + 2] as usize * 16; // index non signé (Pan Docs « Sprite »)
        let row_in_sprite = (y.wrapping_sub(oam[4 * i] as u32).wrapping_add(1)) & 0xFF; // 0..7
        let tile_row = if flags & SPRITE_Y_FLIP != 0 { 7 - row_in_sprite } else { row_in_sprite };

        for j in 0..8u32 {
            let screen_col = (oam[4 * i + 1] as u32 + j) & 0xFF; // la colonne se replie à 256
            if screen_col >= SCREEN_WIDTH as u32 {
                continue; // hors écran à droite : pixel non affiché
            }
            let tile_col = if flags & SPRITE_X_FLIP != 0 { 7 - j } else { j };
            let value = Self::tile_pixel(vram, tile_addr, tile_row, tile_col);
            if value == 0 {
                continue; // pixel transparent : la couche en dessous passe au travers
            }
            // Sans priorité (bit 4 à 0), le sprite is masqué by a pixel Background/Fenêtre non transparent (valeur 1..3).
            let under_value = under[screen_col as usize];
            if !priority && matches!(under_value, Some(v) if v != 0) {
                continue;
            }
            // Le bit 1 of the value sélectionne OBP1 (valeurs 2-3), sinon OBP0.
            let palette = if value & 2 != 0 { self.obp1 } else { self.obp0 };
            self.framebuffer[y as usize * SCREEN_WIDTH + screen_col as usize] = Self::shade(palette >> (value * 2));
        }
    }

    /// Un sprite de coordonnée OAM `oam_y` recouvre the line écran `y` si and only if ses 8 lignes
    /// (Y-1 .. Y+6, with repli à 256) incluent y (Pan Docs « Sprite »).
    fn sprite_covers_row(y: u32, oam_y: u8) -> bool {
        ((y.wrapping_sub(oam_y as u32).wrapping_add(1)) & 0xFF) < 8
    }

    /// Un sprite de coordonnée OAM `oam_x` a au moins une colonne visible si and only if ses 8 colonnes
    /// (X .. X+7, with repli à 256) croisent l'écran [0..160).
    fn sprite_has_visible_column(oam_x: u8) -> bool {
        (0..8u32).any(|j| (oam_x as u32 + j) & 0xFF < SCREEN_WIDTH as u32)
    }

    /// Valeur (0..3) du pixel `col` of the line `row` d'une tuile 8×8 : chaque colonne occupe 2 bits, MSB en premier.
    fn tile_pixel(vram: &[u8], tile_addr: usize, row: u32, col: u32) -> u8 {
        let word = ((vram[tile_addr + 2 * row as usize] as u32) << 8) | vram[tile_addr + 2 * row as usize + 1] as u32;
        (word >> (2 * (7 - col)) & 3) as u8
    }

    /// Convertit a teinte DMG (2 bits of BGP/OBP) en pixel du framebuffer (R dans l'octet le plus bas).
    pub fn shade(bits: u8) -> u32 {
        let [r, g, b] = DMG_SHADES[(bits & 3) as usize];
        0xFF00_0000 | ((b as u32) << 16) | ((g as u32) << 8) | r as u32
    }
}

impl Default for PPU {
    fn default() -> Self {
        Self::new()
    }
}
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn power_on_values() {
        let ppu = PPU::new();
        assert_eq!(ppu.lcdc, 0x91); // LCD allumé, fond activé (fenêtre éteinte) — Pan Docs « Power-On Values »
        assert_eq!(ppu.stat, 0x00);
        assert_eq!(ppu.scy, 0x00);
        assert_eq!(ppu.scx, 0x00);
        assert_eq!(ppu.ly, 0x00); // LY = 0 au power-on
        assert_eq!(ppu.lyc, 0x00);
        assert_eq!(ppu.dma, 0x00);
        assert_eq!(ppu.bgp, 0xFC);
        assert_eq!(ppu.obp0, 0xFF); // non initialisées par le boot ROM — valeur la plus fréquente (PanDocs « Power Up Sequence »)
        assert_eq!(ppu.obp1, 0xFF);
        assert_eq!(ppu.wx, 0x00);
        assert_eq!(ppu.wy, 0x00);
        assert_eq!(ppu.mode_clock, 0); // début de la scanline 0
        assert_eq!(ppu.mode, 2); // OAM Scan : LCD allumé, ligne 0
    }

    #[test]
    fn registers_roundtrip_through_read_write() {
        let mut ppu = PPU::new();
        for (addr, value) in [
            (0xFF40u16, 0x95u8), // LCDC : LCD allumé, tuiles $8000, carte $9800, fond activé
            (0xFF42, 0x3C),       // SCY
            (0xFF43, 0x7F),       // SCX
            (0xFF45, 0x80),       // LYC
            (0xFF46, 0xC0),       // DMA OAM
            (0xFF47, 0xE4),       // BGP
            (0xFF48, 0xFC),       // OBP0
            (0xFF49, 0xEF),       // OBP1
            (0xFF4A, 48),         // WX
            (0xFF4B, 112),        // WY
        ] {
            ppu.write_register(addr, value);
            assert_eq!(ppu.read_register(addr), value);
        }
    }

    #[test]
    fn ly_is_read_only() {
        let mut ppu = PPU::new();
        ppu.write_register(0xFF44, 99); // LY ($FF44) : l'écriture est ignorée
        assert_eq!(ppu.read_register(0xFF44), 0);
    }

    #[test]
    fn stat_writes_only_bits_3_to_6() {
        let mut ppu = PPU::new();
        ppu.write_register(0xFF41, 0xFF); // seuls les bits 3..6 sont écrits
        assert_eq!(ppu.stat, 0x78);
    }

    #[test]
    fn stat_read_reports_written_bits_and_lyc_flag() {
        let mut ppu = PPU::new();
        // Au power-on : aucun bit d'activation écrit + mode OAM Scan (bits 0-1) + drapeau LYC==LY (ly == lyc == 0) en bit 7.
        assert_eq!(ppu.read_register(0xFF41), 0x82);

        ppu.write_register(0xFF41, 0x78); // bits d'activation des interruptions
        assert_eq!(ppu.read_register(0xFF41), 0xFA); // bits écrits + mode OAM Scan (bits 0-1) + drapeau LYC==LY (bit 7)

        ppu.lyc = 5; // ly (0) != lyc (5) : le drapeau s'efface
        assert_eq!(ppu.read_register(0xFF41), 0x7A); // bits écrits + mode OAM Scan (bits 0-1), plus de drapeau LYC==LY
    }

    #[test]
    fn framebuffer_starts_opaque_black() {
        let ppu = PPU::new();
        assert_eq!(ppu.framebuffer.len(), SCREEN_WIDTH * SCREEN_HEIGHT); // 160×144 pixels
        assert!(ppu.framebuffer.iter().all(|&px| px == 0xFF00_0000)); // noir opaque, aucune frame rendue
    }

    #[test]
    fn render_frame_with_empty_vram_uses_power_on_bgp() {
        let mut ppu = PPU::new(); // LCDC = $91 (background and window enabled), BGP = $FC : valeur 0 → teinte claire, 1-3 → foncée
        let vram = [0u8; 0x2000]; // tuiles et cartes nulles : toutes les valeurs de pixel valent 0
        let oam = [0u8; 0xA0]; // OAM vide : aucun sprite
        ppu.render_frame(&vram, &oam);
        assert!(ppu.framebuffer.iter().all(|&px| px == PPU::shade(0))); // valeur 0 partout → teinte claire (BGP = $FC)
    }

    #[test]
    fn render_background_scrolls_and_selects_tiles() {
        let mut ppu = PPU::new();
        ppu.write_register(0xFF40, 0x91); // LCD allumé + background and window enabled (bit 0) ; tuiles non signées $8000-$8FFF (bit 4), carte $9800-$9BFF
        ppu.bgp = 0xE4; // teinte v pour une valeur de pixel v (bits 2v..2v+1 valent v)

        let mut vram = [0u8; 0x2000];
        for row in 0..8 {
            vram[0x10 + 2 * row] = 0xFF; // tuile 1 ($8010-$801F) : moitié gauche → valeur 3
            vram[0x11 + 2 * row] = 0x00; // moitié droite → valeur 0
        }
        vram[0x1800..0x1A00].fill(1); // carte $9800-$9BFF : tuile 1 partout
        let oam = [0u8; 0xA0]; // OAM vide : aucun sprite

        ppu.render_frame(&vram, &oam);
        assert_eq!(ppu.framebuffer[0], PPU::shade(3)); // colonne 0 → moitié gauche de la tuile
        assert_eq!(ppu.framebuffer[4], PPU::shade(0)); // colonne 4 → moitié droite
        assert_eq!(ppu.framebuffer[SCREEN_WIDTH + 3], PPU::shade(3)); // ligne 1, colonne 3 : même motif (moitié gauche noire)

        ppu.scx = 1; // défilement horizontal : le motif glisse d'un pixel vers la gauche
        ppu.render_frame(&vram, &oam);
        assert_eq!(ppu.framebuffer[3], PPU::shade(0)); // colonne 3 → colonne 4 de la tuile (blanche)
        assert_eq!(ppu.framebuffer[7], PPU::shade(3)); // colonne 7 → colonne 8 = colonne 0 de la tuile suivante

        ppu.scx = 0;
        for row in 0..8 {
            vram[0x10 + 2 * row] = if row == 1 { 0xFF } else { 0x00 }; // seule la ligne 1 de la tuile est noire
        }
        ppu.scy = 1; // défilement vertical : la ligne écran 0 montre la ligne 1 de la tuile
        ppu.render_frame(&vram, &oam);
        assert_eq!(ppu.framebuffer[0], PPU::shade(3)); // ligne 0 → ligne 1 de la tuile (noire)
        assert_eq!(ppu.framebuffer[SCREEN_WIDTH], PPU::shade(0)); // ligne 1 → ligne 2 de la tuile (blanche)
    }

    #[test]
    fn render_background_selects_tile_set_and_map() {
        let mut ppu = PPU::new();
        ppu.write_register(0xFF40, 0x90); // tuiles $8000-$8FFF (non signées), carte $9800-$9BFF
        ppu.bgp = 0xE4; // teinte v pour une valeur de pixel v

        let mut vram = [0u8; 0x2000];
        for row in 0..8 {
            vram[0x7F0 + 2 * row] = 0xFF; // tuile $87F0 : noire (index non signé $7F)
            vram[0x0FF0 + 2 * row] = 0xFF; // tuile $8FF0 : noire (index signé +$7F)
        }

        vram[BG_MAP_9800] = 0x7F; // cellule (0,0) de la carte $9800
        let oam = [0u8; 0xA0]; // OAM vide : aucun sprite
        ppu.render_frame(&vram, &oam);
        assert_eq!(ppu.framebuffer[0], PPU::shade(3)); // non signé : tuile $87F0 → noire

        ppu.lcdc |= LCDC_TILE_SET_8800; // bit 2 du LCDC : indices signés — la même valeur pointe vers $8FF0
        ppu.render_frame(&vram, &oam);
        assert_eq!(ppu.framebuffer[0], PPU::shade(3)); // signé +$7F : tuile $8FF0 → noire

        vram[BG_MAP_9800] = 0; // carte $9800 : tuile nulle → blanche
        ppu.lcdc |= LCDC_BG_MAP_9C00; // bit 3 du LCDC : carte $9C00-$9BFF
        vram[BG_MAP_9C00] = 0xFF; // cellule (0,0) de la carte $9C00 → tuile -1 → $87F0 (noire)
        ppu.render_frame(&vram, &oam);
        assert_eq!(ppu.framebuffer[0], PPU::shade(3)); // carte $9C00 : noire
    }

    #[test]
    fn render_window_layer() {
        let mut ppu = PPU::new();
        ppu.write_register(0xFF40, 0xF0); // LCD allumé + fond (tuiles $8000-$8FFF) + fenêtre activées ; bit 6 : tuiles de la fenêtre en $8800-$97FF
        ppu.bgp = 0xE4; // teinte v pour une valeur de pixel v
        ppu.wy = 32; // ligne du haut de la fenêtre
        ppu.wx = 7; // le pixel le plus à gauche est en colonne 8 (Pan Docs « Window »)

        let mut vram = [0u8; 0x2000]; // fond blanc : carte $9800 nulle → tuile nulle → valeur 0
        for row in 0..8 {
            vram[TILE_SET_8800 + 5 * 16 + 2 * row] = 0xFF; // tuile 5 de $8800 : noire
        }
        vram[BG_MAP_9C00 + 1] = 5; // cellule (ligne 0, colonne 1) de la carte $9C00 → tuile 5

        let oam = [0u8; 0xA0]; // OAM vide : aucun sprite

        ppu.render_frame(&vram, &oam);
        assert_eq!(ppu.framebuffer[32 * SCREEN_WIDTH + 16], PPU::shade(3)); // ligne WY : fenêtre noire (cellule (0,1))
        assert_eq!(ppu.framebuffer[(32 - 1) * SCREEN_WIDTH + 16], PPU::shade(0)); // ligne WY-1 : fond blanc
        assert_eq!(ppu.framebuffer[32 * SCREEN_WIDTH + 7], PPU::shade(0)); // colonne ≤ WX : pas de fenêtre
        assert_eq!(ppu.framebuffer[32 * SCREEN_WIDTH + 8], PPU::shade(0)); // cellule (0,0) nulle → transparente → fond blanc

        ppu.lcdc &= !LCDC_WIN_TILE_8800; // bit 6 du LCDC à 0 : la fenêtre utilise les tuiles $8000-$8FFF
        for row in 0..8 {
            vram[5 * 16 + 2 * row] = 0xFF; // tuile 5 de $8000 : noire (moitié gauche)
            vram[5 * 16 + 2 * row + 1] = 0xFF;
        }
        ppu.render_frame(&vram, &oam);
        assert_eq!(ppu.framebuffer[32 * SCREEN_WIDTH + 16], PPU::shade(3)); // fenêtre noire via la tuile 5 de $8000
    }

    #[test]
    fn lcd_off_renders_black() {
        let mut ppu = PPU::new();
        ppu.lcdc &= !LCDC_LCD_ON; // LCD éteint (bit 7 à 0)
        let vram = [0u8; 0x2000];
        let oam = [0u8; 0xA0]; // OAM vide : aucun object
        ppu.render_frame(&vram, &oam);
        assert!(ppu.framebuffer.iter().all(|&px| px == 0xFF00_0000)); // écran noir (rendu à chaque frame même LCD éteint)
    }

    #[test]
    fn render_sprite_basic_placement() {
        let mut ppu = PPU::new();
        ppu.write_register(0xFF40, 0x93); // LCD allumé + fond activé + sprites activées (bit 1)
        ppu.bgp = 0xE4; // teinte v pour une valeur de pixel v
        ppu.obp0 = 0xE4;

        let mut vram = [0u8; 0x2000];
        for row in 0..8 {
            vram[0x10 + 2 * row] = 0xFF; // tuile 1 ($8010-$801F) : toutes les valeurs de pixel valent 3
            vram[0x11 + 2 * row] = 0xFF;
        }

        let mut oam = [0u8; 0xA0];
        oam[0] = 16; // Y = 16 : le sprite occupe les lignes écran 15..22 (Y-1 .. Y+6)
        oam[1] = 32; // X = 32 : colonnes 32..39
        oam[2] = 1; // tuile 1, ensemble $8000-$8FFF

        ppu.render_frame(&vram, &oam);
        assert_eq!(ppu.framebuffer[15 * SCREEN_WIDTH + 32], PPU::shade(3)); // coin haut-gauche du sprite (ligne Y-1)
        assert_eq!(ppu.framebuffer[22 * SCREEN_WIDTH + 39], PPU::shade(3)); // coin bas-droite (ligne Y+6, colonne X+7)
        assert_eq!(ppu.framebuffer[14 * SCREEN_WIDTH + 32], PPU::shade(0)); // ligne Y-2 : pas de sprite → fond blanc
        assert_eq!(ppu.framebuffer[23 * SCREEN_WIDTH + 32], PPU::shade(0)); // ligne Y+7 : pas de sprite → fond blanc
        assert_eq!(ppu.framebuffer[15 * SCREEN_WIDTH + 40], PPU::shade(0)); // juste à droite du sprite → fond blanc
    }

    #[test]
    fn render_sprite_wraps_around_the_screen() {
        let mut ppu = PPU::new();
        ppu.write_register(0xFF40, 0x93); // LCD allumé + fond activé + sprites activées (bit 1)
        ppu.bgp = 0xE4; // teinte v pour une valeur de pixel v
        ppu.obp0 = 0xE4;

        let mut vram = [0u8; 0x2000];
        for row in 0..8 {
            vram[0x10 + 2 * row] = 0xFF; // tuile 1 ($8010-$801F) : toutes les valeurs de pixel valent 3
            vram[0x11 + 2 * row] = 0xFF;
        }

        let mut oam = [0u8; 0xA0];
        oam[0] = 48; // Y = 48 : lignes écran 47..54
        oam[1] = 253; // X = 253 : colonnes 253..255 (hors écran) + repli vers 0..4
        oam[2] = 1;

        ppu.render_frame(&vram, &oam);
        assert_eq!(ppu.framebuffer[47 * SCREEN_WIDTH], PPU::shade(3)); // colonne 0 : la colonne 256 du sprite repliée
        assert_eq!(ppu.framebuffer[47 * SCREEN_WIDTH + 4], PPU::shade(3)); // colonne 4 : dernière colonne visible du sprite (j=7)
        assert_eq!(ppu.framebuffer[47 * SCREEN_WIDTH + 5], PPU::shade(0)); // colonne 5 : pas de sprite → fond blanc
    }

    #[test]
    fn render_sprite_y_visibility_window() {
        let mut ppu = PPU::new();
        ppu.write_register(0xFF40, 0x93); // LCD allumé + fond activé + sprites activées (bit 1)
        ppu.bgp = 0xE4; // teinte v pour une valeur de pixel v
        ppu.obp0 = 0xE4;

        let mut vram = [0u8; 0x2000];
        for row in 0..8 {
            vram[0x10 + 2 * row] = 0xFF; // tuile 1 ($8010-$801F) : toutes les valeurs de pixel valent 3
            vram[0x11 + 2 * row] = 0xFF;
        }

        let mut oam = [0u8; 0xA0];
        oam[0] = 143; // Y = 143 : le sprite occupe les lignes écran 142, 143 et 144..149 (hors écran)
        oam[1] = 8;
        oam[2] = 1;

        ppu.render_frame(&vram, &oam);
        assert_eq!(ppu.framebuffer[(SCREEN_HEIGHT - 2) * SCREEN_WIDTH + 8], PPU::shade(3)); // ligne 142 : visible
        assert_eq!(ppu.framebuffer[(SCREEN_HEIGHT - 1) * SCREEN_WIDTH + 8], PPU::shade(3)); // ligne 143 : visible

        oam[0] = 150; // Y = 150 : le sprite occupe les lignes écran 149..156 — toutes hors écran
        ppu.render_frame(&vram, &oam);
        assert!(ppu.framebuffer.iter().all(|&px| px == PPU::shade(0))); // aucun pixel de sprite visible → fond blanc
    }

    #[test]
    fn render_sprite_horizontal_flip() {
        let mut ppu = PPU::new();
        ppu.write_register(0xFF40, 0x93); // LCD allumé + fond activé + sprites activées (bit 1)
        ppu.bgp = 0xE4; // teinte v pour une valeur de pixel v
        ppu.obp0 = 0xE4;

        let mut vram = [0u8; 0x2000];
        for row in 0..8 {
            vram[0x10 + 2 * row] = 0xFF; // tuile 1 ($8010-$801F) : moitié gauche → valeur 3, moitié droite → valeur 0 (transparente)
        }

        let mut oam = [0u8; 0xA0];
        oam[0] = 16; // Y = 16 : lignes écran 15..22
        oam[1] = 32;
        oam[2] = 1;
        oam[3] = SPRITE_X_FLIP; // bit 6 des drapeaux : retournement horizontal

        ppu.render_frame(&vram, &oam);
        assert_eq!(ppu.framebuffer[15 * SCREEN_WIDTH + 32], PPU::shade(0)); // colonne X : moitié droite de la tuile (transparente) → fond blanc
        assert_eq!(ppu.framebuffer[15 * SCREEN_WIDTH + 39], PPU::shade(3)); // colonne X+7 : moitié gauche de la tuile
    }

    #[test]
    fn render_sprite_vertical_flip() {
        let mut ppu = PPU::new();
        ppu.write_register(0xFF40, 0x93); // LCD allumé + fond activé + sprites activées (bit 1)
        ppu.bgp = 0xE4; // teinte v pour une valeur de pixel v
        ppu.obp0 = 0xE4;

        let mut vram = [0u8; 0x2000];
        for row in 0..7 {
            vram[0x10 + 2 * row] = 0xFF; // tuile 1 : lignes 0..6 → valeur 3, ligne 7 → valeur 0 (transparente)
            vram[0x11 + 2 * row] = 0xFF;
        }

        let mut oam = [0u8; 0xA0];
        oam[0] = 16; // Y = 16 : lignes écran 15..22
        oam[1] = 32;
        oam[2] = 1;
        oam[3] = SPRITE_Y_FLIP; // bit 5 des drapeaux : retournement vertical

        ppu.render_frame(&vram, &oam);
        assert_eq!(ppu.framebuffer[15 * SCREEN_WIDTH + 32], PPU::shade(0)); // ligne Y-1 : dernière ligne de la tuile (transparente) → fond blanc
        assert_eq!(ppu.framebuffer[22 * SCREEN_WIDTH + 32], PPU::shade(3)); // ligne Y+6 : première ligne de la tuile
    }

    #[test]
    fn render_sprite_selects_tile_set_8800() {
        let mut ppu = PPU::new();
        ppu.write_register(0xFF40, 0x93); // LCD allumé + fond activé + sprites activées (bit 1)
        ppu.bgp = 0xE4; // teinte v pour une valeur de pixel v
        ppu.obp0 = 0xE4;

        let mut vram = [0u8; 0x2000];
        for row in 0..8 {
            vram[0x10 + 2 * row] = 0xFF; // tuile 1 de $8000 : moitié gauche → valeur 3
            vram[TILE_SET_8800 + 0x10 + 2 * row] = 0xFF; // tuile 1 de $8800 : moitié gauche → valeur 3
        }

        let mut oam = [0u8; 0xA0];
        oam[0] = 16; // Y = 16 : lignes écran 15..22
        oam[1] = 32;
        oam[2] = 1; // tuile 1, ensemble $8000-$8FFF

        ppu.render_frame(&vram, &oam);
        assert_eq!(ppu.framebuffer[15 * SCREEN_WIDTH + 32], PPU::shade(3)); // moitié gauche de la tuile 1 de $8000

        oam[3] = SPRITE_TILE_SET_8800; // bit 3 des drapeaux : ensemble $8800-$97FF (index non signé)
        ppu.render_frame(&vram, &oam);
        assert_eq!(ppu.framebuffer[15 * SCREEN_WIDTH + 32], PPU::shade(3)); // moitié gauche de la tuile 1 de $8800

        vram[TILE_SET_8800 + 0x10] = 0; // efface les pixels 0..3 de la première ligne de la tuile $8810
        ppu.render_frame(&vram, &oam);
        assert_eq!(ppu.framebuffer[15 * SCREEN_WIDTH + 32], PPU::shade(0)); // ligne Y-1 → transparente → fond blanc
    }

    #[test]
    fn render_sprite_transparent_pixels() {
        let mut ppu = PPU::new();
        ppu.write_register(0xFF40, 0x93); // LCD allumé + fond activé + sprites activées (bit 1)
        ppu.bgp = 0xE4; // teinte v pour une valeur de pixel v

        let mut vram = [0u8; 0x2000];
        for row in 0..8 {
            vram[0x10 + 2 * row] = 0xFF; // tuile 1 ($8010-$801F) : moitié gauche → valeur 3, moitié droite → valeur 0 (transparente)
        }

        let mut oam = [0u8; 0xA0];
        oam[0] = 16; // Y = 16 : lignes écran 15..22
        oam[1] = 32;
        oam[2] = 1;

        ppu.render_frame(&vram, &oam);
        assert_eq!(ppu.framebuffer[15 * SCREEN_WIDTH + 32], PPU::shade(3)); // moitié gauche : sprite visible (OBP0)
        assert_eq!(ppu.framebuffer[15 * SCREEN_WIDTH + 40], PPU::shade(0)); // moitié droite : transparente → fond blanc
    }

    #[test]
    fn render_sprite_selects_obp_palette() {
        let mut ppu = PPU::new();
        ppu.write_register(0xFF40, 0x93); // LCD allumé + fond activé + sprites activées (bit 1)
        ppu.bgp = 0xE4; // teinte v pour une valeur de pixel v
        ppu.obp0 = 0xE4; // OBP0 : teinte v pour une valeur de pixel v
        ppu.obp1 = 0x8C; // OBP1 : valeurs 0-1 → teinte claire, 2-3 → foncée

        let mut vram = [0u8; 0x2000];
        for row in 0..8 {
            vram[0x10 + 2 * row] = 0x7F; // tuile 1 : pixel 0 → valeur 1, pixels 1-3 → valeur 3
            vram[0x11 + 2 * row] = 0xFF; // pixels 4-7 → valeur 3
        }

        let mut oam = [0u8; 0xA0];
        oam[0] = 16; // Y = 16 : lignes écran 15..22
        oam[1] = 32;
        oam[2] = 1;

        ppu.render_frame(&vram, &oam);
        assert_eq!(ppu.framebuffer[15 * SCREEN_WIDTH + 32], PPU::shade(1)); // valeur 1 → OBP0
        assert_eq!(ppu.framebuffer[15 * SCREEN_WIDTH + 33], PPU::shade(2)); // valeur 3 → OBP1 (teinte foncée)
    }

    #[test]
    fn render_sprite_priority_over_background() {
        let mut ppu = PPU::new();
        ppu.write_register(0xFF40, 0x93); // LCD allumé + fond activé + sprites activées (bit 1)
        ppu.bgp = 0xE4; // teinte v pour une valeur de pixel v
        ppu.obp0 = 0xE4;

        let mut vram = [0u8; 0x2000];
        for row in 0..8 {
            vram[0x10 + 2 * row] = 0xFF; // tuile 1 : toutes les valeurs de pixel valent 3 (fond noir)
            vram[0x11 + 2 * row] = 0xFF;
            vram[0x20 + 2 * row] = 0x55; // tuile 2 : toutes les valeurs de pixel valent 1 (sprite claire)
            vram[0x21 + 2 * row] = 0x55;
        }
        vram[BG_MAP_9800..BG_MAP_9800 + 32 * 32].fill(1); // carte $9800-$9BFF : tuile 1 partout (fond noir)

        let mut oam = [0u8; 0xA0];
        oam[0] = 16; // Y = 16 : lignes écran 15..22
        oam[1] = 32; // X = 32 : colonnes 32..39
        oam[2] = 2; // tuile 2, mappée par OBP0

        ppu.render_frame(&vram, &oam);
        assert_eq!(ppu.framebuffer[15 * SCREEN_WIDTH + 32], PPU::shade(3)); // sans priorité : le fond noir masque le sprite
        assert_eq!(ppu.framebuffer[15 * SCREEN_WIDTH + 40], PPU::shade(3)); // hors du sprite : fond noir

        oam[3] = SPRITE_PRIORITY; // bit 4 des drapeaux : le sprite est dessiné devant le fond
        ppu.render_frame(&vram, &oam);
        assert_eq!(ppu.framebuffer[15 * SCREEN_WIDTH + 32], PPU::shade(1)); // avec priorité : le sprite (OBP0) passe devant

        ppu.lcdc &= !LCDC_BG; // fond éteint : les sprites sans priorité deviennent visibles sur la base noire
        oam[3] = 0;
        ppu.render_frame(&vram, &oam);
        assert_eq!(ppu.framebuffer[15 * SCREEN_WIDTH + 32], PPU::shade(1)); // sprite visible (fond éteint)
    }

    #[test]
    fn render_sprites_disabled_when_lcdc_bit_1_clear() {
        let mut ppu = PPU::new(); // LCDC = $91 : LCD allumé + fond activé, sprites éteintes (bit 1 à 0)
        ppu.bgp = 0xE4; // teinte v pour une valeur de pixel v
        ppu.obp0 = 0xE4;

        let mut vram = [0u8; 0x2000];
        for row in 0..8 {
            vram[0x10 + 2 * row] = 0xFF; // tuile 1 ($8010-$801F) : toutes les valeurs de pixel valent 3
            vram[0x11 + 2 * row] = 0xFF;
        }

        let mut oam = [0u8; 0xA0];
        oam[0] = 16; // Y = 16 : lignes écran 15..22
        oam[1] = 32;
        oam[2] = 1;

        ppu.render_frame(&vram, &oam);
        assert_eq!(ppu.framebuffer[15 * SCREEN_WIDTH + 32], PPU::shade(0)); // bit 1 du LCDC à 0 : aucun sprite rendu → fond blanc

        ppu.lcdc |= LCDC_SPRITE_ON; // bit 1 du LCDC à 1
        ppu.render_frame(&vram, &oam);
        assert_eq!(ppu.framebuffer[15 * SCREEN_WIDTH + 32], PPU::shade(3)); // sprite rendu (OBP0)
    }

    #[test]
    fn render_sprite_limit_of_ten_per_line() {
        let mut ppu = PPU::new();
        ppu.write_register(0xFF40, 0x93); // LCD allumé + fond activé + sprites activées (bit 1)
        ppu.bgp = 0xE4; // teinte v pour une valeur de pixel v
        ppu.obp0 = 0xE4;

        let mut vram = [0u8; 0x2000];
        for row in 0..8 {
            vram[0x10 + 2 * row] = 0xFF; // tuile 1 ($8010-$801F) : toutes les valeurs de pixel valent 3
            vram[0x11 + 2 * row] = 0xFF;
        }

        let mut oam = [0u8; 0xA0];
        for i in 0..12 {
            oam[4 * i] = 16; // Y = 16 : toutes les entrées recouvrent la ligne écran 15
            oam[4 * i + 1] = (i as u8) * 12; // X espacé de 12 colonnes : pas de chevauchement
            oam[4 * i + 2] = 1; // tuile 1
        }

        ppu.render_frame(&vram, &oam);
        assert_eq!(ppu.framebuffer[15 * SCREEN_WIDTH + 96], PPU::shade(3)); // entrée 8 (X = 96) : rendue
        assert_eq!(ppu.framebuffer[15 * SCREEN_WIDTH + 108], PPU::shade(3)); // entrée 9 (X = 108) : dernière rendue
        assert_eq!(ppu.framebuffer[15 * SCREEN_WIDTH + 120], PPU::shade(0)); // entrée 10 (X = 120) : supprimée par la limite de 10 → fond blanc
    }

    #[test]
    fn render_bg_win_off_shows_black_base_and_objects() {
        let mut ppu = PPU::new();
        ppu.lcdc &= !LCDC_BG_WIN_ON; // bits 4-5 à 0 : background and window éteints (DMG)
        let mut vram = [0u8; 0x2000];
        let oam = [0u8; 0xA0];

        ppu.render_frame(&vram, &oam);
        assert!(ppu.framebuffer.iter().all(|&px| px == 0xFF00_0000)); // base noire (aucune couche)

        // Un objet is still drawn on the black base.
        for row in 0..8 {
            vram[0x10 + 2 * row] = 0xFF; // tuile 1 ($8010-$801F) : toutes the values of pixel valent 3
            vram[0x11 + 2 * row] = 0xFF;
        }
        let mut oam = [0u8; 0xA0];
        oam[0] = 32; // Y = 32 : objet affiché sur les lignes écran 31..38 (Y-1)
        oam[1] = 40; // X = 40 : objet affiché sur the colonnes écran 40..47
        oam[2] = 1; // tuile $8010-$801F
        ppu.render_frame(&vram, &oam);
        assert_eq!(ppu.framebuffer[31 * SCREEN_WIDTH + 40], PPU::shade(3)); // objet visible sur the base noire
        assert_eq!(ppu.framebuffer[0], 0xFF00_0000); // le reste of l'écran : noir
    }

    #[test]
    #[ignore = "sprites 8×16 non encore implémentés dans la PPU (draw_sprite est 8×8 uniquement)"]
    fn render_objects_16px_height_and_tile_selection() {
        let mut ppu = PPU::new();
        ppu.lcdc |= LCDC_OBJ_SIZE_16; // bit 2 à 1 : objets 8×16 (PanDocs « OAM »)
        let mut vram = [0u8; 0x2000];
        let mut oam = [0u8; 0xA0];

        // Tuile 5 en $8050 : moitié gauche value 3, moitié droite value 0.
        for row in 0..8 {
            vram[0x0050 + row] = 0b11_11_11_11;
            vram[0x0058 + row] = 0b00_00_00_00;
        }

        oam[0] = 32; // Y = 32 : objet affiché sur the lignes écran 16..31 (Y-16)
        oam[1] = 40; // X = 40 : objet affiché sur the colonnes écran 32..39 (X-8)
        oam[2] = 5; // tuile du haut « NN & $FE » = 4 → $8040 ; tuile du bas « NN | $01 » = 5 → $8050

        ppu.render_frame(&vram, &oam);
        assert_eq!(ppu.framebuffer[16 * SCREEN_WIDTH + 32], PPU::shade(0)); // moitié gauche of the tuile du haut (4) : vide
        assert_eq!(ppu.framebuffer[24 * SCREEN_WIDTH + 32], PPU::shade(3)); // moitié gauche of the tuile du bas (5)
    }

    #[test]
    fn render_objects_drawn_in_oam_order_later_overwrites_earlier() {
        let mut ppu = PPU::new();
        ppu.lcdc |= LCDC_SPRITE_ON; // sprites activées (bit 1 du LCDC)
        ppu.obp0 = 0xE4; // teinte v pour une valeur de pixel v
        ppu.obp1 = 0xE4;
        let mut vram = [0u8; 0x2000];
        let mut oam = [0u8; 0xA0];

        // Tuile 1 en $8010 : all pixels value 3 ; tuile 2 en $8020 : all pixels value 2.
        for row in 0..8 {
            vram[0x0010 + row] = 0b11_11_11_11;
            vram[0x0018 + row] = 0b11_11_11_11;
            vram[0x0020 + row] = 0b10_10_10_10;
            vram[0x0028 + row] = 0b10_10_10_10;
        }

        // L'objet B (X=48 → colonnes 48..55, tuile 2) is declared first dans l'OAM ; les sprites sont
        // composés dans l'ordre OAM (PanDocs « Sprite »), donc l'objet A déclaré ensuite (X=44 →
        // colonnes 44..51, tuile 3) écrase B sur la zone de chevauchement (colonnes 48..51).
        oam[0] = 32; // Y = 32 : lignes écran 31..38 (Y-1)
        oam[1] = 48; // X = 48 : colonnes écran 48..55
        oam[2] = 2; // tuile $8020-$802F (value 2)

        oam[4] = 32; // Y = 32 : lignes écran 31..38
        oam[5] = 44; // X = 44 : colonnes écran 44..51 → chevauche B sur les colonnes 48..51
        oam[6] = 1; // tuile $8010-$801F (value 3)

        ppu.render_frame(&vram, &oam);
        assert_eq!(ppu.framebuffer[31 * SCREEN_WIDTH + 46], PPU::shade(3)); // colonne de l'objet A seul
        assert_eq!(ppu.framebuffer[31 * SCREEN_WIDTH + 50], PPU::shade(3)); // chevauchement : objet A (plus tardif dans l'OAM) devant
        assert_eq!(ppu.framebuffer[31 * SCREEN_WIDTH + 54], PPU::shade(2)); // colonne de l'objet B seul
    }

    #[test]
    fn step_tracks_ly_and_mode_per_line() {
        let mut ppu = PPU::new(); // ligne 0, mode_clock 0 : OAM Scan (LCD allumé)
        let vram = [0u8; 0x2000];
        let oam = [0u8; 0xA0];

        ppu.step(79, &vram, &oam); // mode_clock 1..79 : toujours en OAM Scan
        assert_eq!((ppu.mode_clock, ppu.mode, ppu.ly), (79, 2, 0));

        ppu.step(1, &vram, &oam); // mode_clock 80 : début du Drawing
        assert_eq!((ppu.mode_clock, ppu.mode), (0, 3));

        ppu.step(176, &vram, &oam); // franchit le Drawing (172) : début du HBlank avec un dépassement de 4 dots
        assert_eq!((ppu.mode_clock, ppu.mode), (4, 0));

        ppu.step(199, &vram, &oam); // mode_clock 203 : dernier dot de la ligne 0 (HBlank)
        assert_eq!((ppu.mode_clock, ppu.ly), (203, 0));

        ppu.step(1, &vram, &oam); // début de la ligne 1 : OAM Scan à nouveau
        assert_eq!((ppu.mode_clock, ppu.mode, ppu.ly), (0, 2, 1));
    }

    #[test]
    fn half_frame_is_at_line_77() {
        let mut ppu = PPU::new();
        let vram = [0u8; 0x2000];
        let oam = [0u8; 0xA0];

        // Demi-frame : exactement 77 lignes × 456 dots.
        ppu.step(35_112, &vram, &oam);
        assert_eq!((ppu.mode_clock, ppu.ly), (0, 77));
    }

    #[test]
    fn full_frame_returns_to_line_zero() {
        let mut ppu = PPU::new();
        let vram = [0u8; 0x2000];
        let oam = [0u8; 0xA0];

        assert!(ppu.step(FRAME_DOTS as u32, &vram, &oam)); // exactement une frame : frontière franchie
        assert_eq!((ppu.mode_clock, ppu.mode, ppu.ly), (0, 2, 0));
        assert!(!ppu.step(1, &vram, &oam)); // pas de nouvelle frame après un seul dot
    }

    #[test]
    fn vblank_spans_lines_144_to_153() {
        let mut ppu = PPU::new();
        let vram = [0u8; 0x2000];
        let oam = [0u8; 0xA0];

        ppu.step(VBLANK_START_DOT as u32, &vram, &oam); // début de la ligne 144 : le VBlank commence
        assert_eq!((ppu.mode_clock, ppu.mode, ppu.ly), (0, 1, 144));

        ppu.step(8 * DOTS_PER_LINE, &vram, &oam); // huit lignes VBlank plus loin : début de la ligne 152
        assert_eq!((ppu.mode_clock, ppu.mode, ppu.ly), (0, 1, 152));

        ppu.step(DOTS_PER_LINE, &vram, &oam); // début de la ligne 153 : dernière ligne de the frame
        assert_eq!((ppu.mode_clock, ppu.mode, ppu.ly), (0, 1, 153));

        assert!(ppu.step(DOTS_PER_LINE, &vram, &oam)); // complète la ligne 153 and revient au début of the frame suivante
        assert_eq!((ppu.mode_clock, ppu.mode, ppu.ly), (0, 2, 0));
    }

    #[test]
    fn lcd_off_freezes_the_ppu() {
        let mut ppu = PPU::new();
        let vram = [0u8; 0x2000];
        let oam = [0u8; 0xA0];

        ppu.lcdc &= !LCDC_LCD_ON; // LCD éteint (bit 7 à 0) : la PPU est gelée
        ppu.step(10_000, &vram, &oam);
        assert_eq!((ppu.mode_clock, ppu.mode, ppu.ly), (0, 2, 0)); // reste à the position power-on
        assert_eq!(ppu.take_interrupts(), 0);

        ppu.lcdc |= LCDC_LCD_ON; // LCD rallumé : le timing reprend là où it was gelé
        ppu.step(DOTS_PER_LINE, &vram, &oam);
        assert_eq!((ppu.mode_clock, ppu.ly), (0, 1));
    }

    #[test]
    fn vblank_interrupt_fires_on_entry() {
        let mut ppu = PPU::new();
        let vram = [0u8; 0x2000];
        let oam = [0u8; 0xA0];

        assert_eq!(ppu.take_interrupts(), 0); // rien en attente au power-on
        ppu.step(VBLANK_START_DOT as u32 - 1, &vram, &oam); // dernier dot of the line 143 (HBlank)
        assert_eq!((ppu.mode, ppu.ly), (0, 143));
        assert_eq!(ppu.take_interrupts(), 0); // pas encore en VBlank

        ppu.step(1, &vram, &oam); // début of the line 144 : le VBlank commence
        assert_eq!((ppu.ly, ppu.mode), (144, 1));
        assert_eq!(ppu.take_interrupts(), IRQ_VBLANK); // drapeau VBlank (bit 0 of IF) levé inconditionnellement

        ppu.step(3 * DOTS_PER_LINE, &vram, &oam); // reste en VBlank : pas de re-déclenchement
        assert_eq!(ppu.take_interrupts(), 0);
    }

    #[test]
    fn vblank_interrupt_respects_its_enable_bit() {
        let vram = [0u8; 0x2000];
        let oam = [0u8; 0xA0];

        // Le drapeau VBlank (bit 0 of IF) is levé inconditionnellement à the entrée en VBlank, mais the drapeau
        // STAT/LCD (bit 1 of IF) seulement si the interruption mode 1 is activée (bit 5 du STAT).
        let mut ppu = PPU::new();
        ppu.step(VBLANK_START_DOT as u32, &vram, &oam); // entre en VBlank sans bit d'activation posé
        assert_eq!(ppu.take_interrupts(), IRQ_VBLANK); // bit 0 levé inconditionnellement, bit 1 pas (STAT bit 5 à 0)

        let mut ppu = PPU::new();
        ppu.write_register(0xFF41, STAT_IRQ_VBLANK); // active the interruption mode 1 (bit 5 du STAT)
        ppu.step(VBLANK_START_DOT as u32, &vram, &oam); // entre en VBlank : les deux drapeaux are levés
        assert_eq!(ppu.take_interrupts(), IRQ_VBLANK | IRQ_STAT);
    }

    #[test]
    fn lyc_match_raises_stat_interrupt() {
        let mut ppu = PPU::new();
        let vram = [0u8; 0x2000];
        let oam = [0u8; 0xA0];

        ppu.write_register(0xFF45, 145); // LYC = 145
        ppu.write_register(0xFF41, STAT_IRQ_LYC); // active the interruption LYC==LY (bit 3 du STAT)

        ppu.step(145 * DOTS_PER_LINE - 1, &vram, &oam); // dernier dot of the line 144 (VBlank)
        assert_eq!(ppu.ly, 144);
        // Le drapeau VBlank (bit 0) is levé à the entrée en VBlank ; le drapeau STAT (bit 1) pas encore (ly != lyc).
        assert_eq!(ppu.take_interrupts(), IRQ_VBLANK);

        ppu.step(1, &vram, &oam); // début of the line 145 : ly == lyc
        assert_eq!(ppu.ly, 145);
        assert_ne!(ppu.read_register(0xFF41) & (1 << 7), 0); // drapeau LYC==LY posé en bit 7 du STAT
        assert_ne!(ppu.take_interrupts() & IRQ_STAT, 0); // drapeau STAT (bit 1 of IF) levé

        ppu.step(DOTS_PER_LINE, &vram, &oam); // ligne 146 : ly != lyc à nouveau, pas de re-déclenchement
        assert_eq!(ppu.take_interrupts(), 0);
    }

    #[test]
    fn mode_interrupts_fire_on_entry() {
        let mut ppu = PPU::new();
        let vram = [0u8; 0x2000];
        let oam = [0u8; 0xA0];

        // Interruptions mode 0 (HBlank) and mode 2 (OAM Scan) activées ; the mode 3 n'a not d'interruption.
        ppu.write_register(0xFF41, STAT_IRQ_MODE0 | STAT_IRQ_MODE2);

        ppu.step(MODE_OAM_CYCLES - 1, &vram, &oam); // toujours en OAM Scan of the line 0 (déjà entré au power-on)
        assert_eq!(ppu.take_interrupts(), 0); // pas de re-entrée : le drapeau n'est not levé à the power-on

        ppu.step(1, &vram, &oam); // dot 80 : début du Drawing — the mode 3 n'a not d'interruption
        assert_eq!(ppu.take_interrupts(), 0);

        ppu.step(MODE_DRAW_CYCLES, &vram, &oam); // dot 256 : début of the HBlank (mode 0)
        assert_eq!((ppu.mode, ppu.ly), (0, 0));
        assert_ne!(ppu.take_interrupts() & IRQ_STAT, 0); // drapeau STAT levé à the entrée en mode 0

        ppu.step(MODE_HBLANK_CYCLES + MODE_OAM_CYCLES - 1, &vram, &oam); // fin of the line 0 and début of the line 1 : OAM Scan (mode 2)
        assert_eq!((ppu.mode_clock, ppu.mode), (MODE_OAM_CYCLES - 1, 2));
        assert_ne!(ppu.take_interrupts() & IRQ_STAT, 0); // drapeau STAT levé à the entrée en mode 2
    }
}