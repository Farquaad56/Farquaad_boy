//! Pixel Processing Unit : registres PPU ($FF40-$FF4B), timing et framebuffer (160×144).
//!
//! Étape 1 — fondation : les douze registres 8 bits de la PPU ($FF40-$FF4B, dont le DMA OAM $FF46),
//! leurs sémantiques de lecture/écriture (Pan Docs « LCDC », « STAT ») et l'état post-boot ROM
//! (PanDocs « Power Up Sequence » : LY = 0, LCDC = $91, BGP = $FC ; OBP0/OBP1 sont laissées non
//! initialisées par le boot ROM — la valeur la plus fréquente, $FF, est retenue).
//! Étape 2 — timing : la PPU avance en T-cycles (456 dots/ligne, 154 lignes/frame), suit LY et le mode
//! (OAM Scan/Drawing/HBlank/VBlank) selon Pan Docs « Rendering », et génère les requêtes d'interruption
//! VBlank et STAT (Pan Docs « Interrupt Sources »). Le LCD éteint (bit 7 de LCDC à 0) gèle la PPU.
//! Étape 3 — rendu : la VRAM ($8000-$9FFF : tuiles + cartes) et l'OAM (160 octets, $FE00-$FE9F) sont
//! dédoublonnées avec le MMU ; `render_frame` dessine la couche Background (défilement SCX/SCY,
//! tuiles $8000/$8800, carte $9800/$9C00, palette BGP) puis la fenêtre (WY/WX, carte $9C00-$9BFF,
//! ensemble de tuiles choisi par le bit 6 du LCDC) selon Pan Docs « Background »/« Window ».
//! Étape 4 — sprites : `render_frame` dessine aussi les 40 entrées OAM ($FE00-$FE9F) selon Pan Docs « Sprite » :
//! position (X, Y-1) avec repli à 256, tuile $8000/$8800 choisie par le bit 3 des drapeaux (index non signé),
//! retournements X/Y, palette OBP0/OBP1, transparence de la valeur 0, priorité face au fond et limite de
//! 10 sprites par ligne.

/// Résolution de l'écran en pixels (Pan Docs « Graphics »).
pub const SCREEN_WIDTH: usize = 160;
pub const SCREEN_HEIGHT: usize = 144;

/// Nombre de dots (T-cycles) par scanline (Pan Docs « Rendering » : 456 dots × 154 lignes/frame).
pub const DOTS_PER_LINE: u32 = 456;

/// Nombre de lignes par frame (Pan Docs « Rendering » : 456 dots × 154 lignes/frame).
pub const FRAME_LINES: u32 = 154;

/// Dots (T-cycles) par frame vidéo : 154 lignes × 456 dots.
pub const FRAME_DOTS: u64 = FRAME_LINES as u64 * DOTS_PER_LINE as u64; // 70224

/// Dot auquel commence le VBlank : début de la ligne 144 (Pan Docs « Rendering »).
pub const VBLANK_START_DOT: u64 = SCREEN_HEIGHT as u64 * DOTS_PER_LINE as u64; // 65664

/// Bit 3 du STAT : activation de l'interruption LYC==LY.
pub const STAT_IRQ_LYC: u8 = 1 << 3;
/// Bit 4 du STAT : activation de l'interruption mode 0 (HBlank).
pub const STAT_IRQ_MODE0: u8 = 1 << 4;
/// Bit 5 du STAT : activation de l'interruption VBlank (mode 1).
pub const STAT_IRQ_VBLANK: u8 = 1 << 5;
/// Bit 6 du STAT : activation de l'interruption mode 2 (OAM Scan).
pub const STAT_IRQ_MODE2: u8 = 1 << 6;

/// Interruption VBlank en attente → bit 0 de IF ($FF0F), vecteur $40 (Pan Docs « Interrupt Sources »).
pub const IRQ_VBLANK: u8 = 1 << 0;
/// Interruption STAT/LCD en attente (mode 0, LYC==LY ou mode 2) → bit 1 de IF ($FF0F), vecteur $48.
pub const IRQ_STAT: u8 = 1 << 1;

/// Bit 7 du LCDC : activation du LCD/PPU (à 0, la PPU est gelée et l'écran est noir).
pub const LCDC_LCD_ON: u8 = 1 << 7;
/// Bit 1 du LCDC : activation de l'affichage des sprites (OAM).
pub const LCDC_SPRITE_ON: u8 = 1 << 1;
/// Bit 2 du LCDC : ensemble de tuiles du fond en $8800-$97FF (indices signés) au lieu de $8000-$8FFF.
pub const LCDC_TILE_SET_8800: u8 = 1 << 2;
/// Bit 3 du LCDC : carte du fond en $9C00-$9BFF au lieu de $9800-$9BFF.
pub const LCDC_BG_MAP_9C00: u8 = 1 << 3;
/// Bit 4 du LCDC : activation de la couche Background (à 0, aucun pixel de fond n'est généré).
pub const LCDC_BG: u8 = 1 << 4;
/// Bit 5 du LCDC : activation de la fenêtre (carte $9C00-$9BFF).
pub const LCDC_WINDOW: u8 = 1 << 5;
/// Bit 6 du LCDC : ensemble de tuiles de la fenêtre en $8800-$97FF au lieu de $8000-$8FFF.
pub const LCDC_WIN_TILE_8800: u8 = 1 << 6;

/// Bit 3 des drapeaux d'une entrée OAM : ensemble de tuiles du sprite en $8800-$97FF au lieu de $8000-$8FFF.
pub const SPRITE_TILE_SET_8800: u8 = 1 << 3;
/// Bit 4 des drapeaux d'une entrée OAM : priorité — le sprite est dessiné devant la couche Background.
pub const SPRITE_PRIORITY: u8 = 1 << 4;
/// Bit 5 des drapeaux d'une entrée OAM : retournement vertical du sprite.
pub const SPRITE_Y_FLIP: u8 = 1 << 5;
/// Bit 6 des drapeaux d'une entrée OAM : retournement horizontal du sprite.
pub const SPRITE_X_FLIP: u8 = 1 << 6;

/// Base dans la VRAM de la carte du fond par défaut ($9800) — index relatif à $8000.
const BG_MAP_9800: usize = 0x9800 - 0x8000;
/// Base dans la VRAM de la carte en $9C00 (fond si bit 3 du LCDC, et fenêtre) — index relatif à $8000.
const BG_MAP_9C00: usize = 0x9C00 - 0x8000;
/// Base dans la VRAM de l'ensemble de tuiles en $8800 (bit 2 du LCDC, indices signés) — index relatif à $8000.
const TILE_SET_8800: usize = 0x8800 - 0x8000;

/// Les 4 teintes DMG (Pan Docs « Graphics »), de la plus claire à la plus foncée : [R, G, B].
pub const DMG_SHADES: [[u8; 3]; 4] = [
    [252, 252, 252], // teinte 0 : blanc
    [191, 191, 191], // teinte 1 : gris clair
    [95, 95, 95],    // teinte 2 : gris foncé
    [0, 0, 0],       // teinte 3 : noir
];

/// Pixel Processing Unit.
#[allow(clippy::upper_case_acronyms)]
pub struct PPU {
    /// Contrôle LCD — registre LCDC ($FF40) : activation du LCD/PPU, tuiles et cartes du fond/fenêtre, sprites.
    pub lcdc: u8,
    /// Partie écrite du STAT ($FF41) : bits d'activation des interruptions (bits 3..6).
    pub stat: u8,
    /// Défilement vertical — registre SCY ($FF42).
    pub scy: u8,
    /// Défilement horizontal — registre SCX ($FF43).
    pub scx: u8,
    /// Ligne de balayage courante (0..=152) — registre LY ($FF44), en lecture seule.
    pub ly: u8,
    /// Ligne de comparaison — registre LYC ($FF45).
    pub lyc: u8,
    /// DMA OAM — registre $FF46 : source d'adresse du transfert des 160 octets d'OAM.
    pub dma: u8,
    /// Palette Background/Window — registre BGP ($FF47) : chaque paire de bits sélectionne la teinte DMG des pixels de valeur 0..3.
    pub bgp: u8,
    /// Palette sprite 0 (par défaut) — registre OBP0 ($FF48).
    pub obp0: u8,
    /// Palette sprite 1 — registre OBP1 ($FF49).
    pub obp1: u8,
    /// Position X de la fenêtre — registre WX ($FF4A).
    pub wx: u8,
    /// Position Y de la fenêtre — registre WY ($FF4B).
    pub wy: u8,

    /// Compteur interne de dots (T-cycles PPU) dans la scanline courante.
    pub dots: u32,
    /// Mode PPU courant : 0 = HBlank, 1 = VBlank, 2 = OAM Scan, 3 = Drawing.
    pub mode: u8,

    /// Index de la ligne courante dans la frame (0..=153) : état interne du timing.
    line: u8,
    /// Requêtes d'interruption PPU en attente, consommées par `take_interrupts`.
    pending_irq: u8,

    /// Framebuffer écran (couleurs RGBA sur 32 bits, R dans l'octet le plus bas — compatible zero-copy egui/bytemuck).
    pub framebuffer: [u32; SCREEN_WIDTH * SCREEN_HEIGHT],
}

impl PPU {
    /// Crée une PPU à l'état post-boot ROM (PanDocs « Power Up Sequence ») : LCDC = $91 (LCD allumé,
    /// fond activé, fenêtre éteinte), BGP = $FC ; OBP0/OBP1 sont laissées non initialisées par le boot
    /// ROM — la valeur la plus fréquente, $FF, est retenue. Les autres registres valent $00 ; LY = 0
    /// et le compteur de dots est à 0. Le LCD étant allumé au power-on, la PPU démarre en mode 2
    /// (OAM Scan) sur la ligne 0 ; le framebuffer est noir opaque (aucune frame rendue).
    pub fn new() -> Self {
        Self {
            lcdc: 0x91, // LCD allumé, fond activé (fenêtre éteinte), tuiles $8000-$8FFF
            stat: 0x00,
            scy: 0x00,
            scx: 0x00,
            ly: 0x00,   // LY = 0 au power-on (en lecture seule)
            lyc: 0x00,
            dma: 0x00,
            bgp: 0xFC,  // valeurs de pixel 0-1 → teinte claire, 2-3 → teinte foncée (Pan Docs « Power-On Values »)
            obp0: 0xFF, // non initialisées par le boot ROM — valeur la plus fréquente (PanDocs « Power Up Sequence »)
            obp1: 0xFF,
            wx: 0x00,
            wy: 0x00,
            dots: 0,    // début de la scanline 0
            mode: 2,    // OAM Scan : LCD allumé, ligne 0
            line: 0,    // première ligne de la frame
            pending_irq: 0x00,
            framebuffer: [0xFF00_0000; SCREEN_WIDTH * SCREEN_HEIGHT], // noir opaque (aucune frame rendue)
        }
    }

    /// Lit un registre PPU ($FF40-$FF4B). Le STAT renvoie les bits d'activation écrits (bits 3..6)
    /// plus le drapeau LYC==LY en bit 7 (lecture seule, constamment mis à jour) ; les bits 0-2 sont
    /// inutilisés sur le DMG et se lisent à 0. La ligne courante est lue via $FF44.
    pub fn read_register(&self, addr: u16) -> u8 {
        match addr {
            0xFF40 => self.lcdc,
            0xFF41 => {
                let mut value = self.stat & 0x78; // bits d'activation des interruptions (bits 3..6)
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

    /// Écrit un registre PPU ($FF40-$FF4B). LY ($FF44) est en lecture seule (écriture ignorée) ;
    /// seuls les bits 3..6 du STAT sont écrits.
    pub fn write_register(&mut self, addr: u16, value: u8) {
        match addr {
            0xFF40 => self.lcdc = value,
            0xFF41 => {
                log::debug!(
                    "[PPU] STAT write: ${:02X} (VBlank IRQ: {}, LYC IRQ: {}, Mode0: {}, Mode2: {})",
                    value,
                    (value & 0x20) != 0, // bit 5 = VBlank
                    (value & 0x08) != 0, // bit 3 = LYC
                    (value & 0x10) != 0, // bit 4 = Mode 0 (HBlank)
                    (value & 0x40) != 0, // bit 6 = Mode 2 (OAM Scan)
                );
                self.stat = value & 0x78; // bits 3..6 seulement (les bits 0-2 sont en lecture seule)
            }
            0xFF42 => self.scy = value,
            0xFF43 => self.scx = value,
            0xFF44 => {} // LY : en lecture seule — l'écriture est ignorée
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

    /// Avance la PPU de `cycles` T-cycles (Pan Docs « Rendering » : 456 dots/ligne, 154 lignes/frame).
    /// Renvoie true si une frontière de frame a été franchie pendant cet avancement.
    /// Le LCD éteint (bit 7 de LCDC à 0) gèle la PPU : aucun avancement du timing, aucune interruption.
    pub fn advance(&mut self, cycles: u64) -> bool {
        if cycles == 0 || self.lcdc & LCDC_LCD_ON == 0 {
            return false; // LCD éteint : PPU gelée (pas d'avancement du timing, pas d'interruptions)
        }

        let prev_total = self.line as u64 * DOTS_PER_LINE as u64 + self.dots as u64;
        let new_total = prev_total + cycles;

        // LOG CRITIQUE : Détecter le passage en VBlank (utilise crosses() pour gérer les wrap-around)
        if Self::crosses(prev_total, new_total, VBLANK_START_DOT) {
            log::debug!(
                "[PPU] VBlank boundary crossed! STAT=${:02X}, VBlank bit: {}",
                self.stat,
                (self.stat & STAT_IRQ_VBLANK) != 0,
            );
        }

        // Requêtes d'interruption : détection des transitions franchies entre l'ancienne et la nouvelle position.
        // Vérifier que l'interruption VBlank est bien levée
        if self.stat & STAT_IRQ_VBLANK != 0 && Self::crosses(prev_total, new_total, VBLANK_START_DOT) {
            self.pending_irq |= IRQ_VBLANK; // entrée en VBlank (début de la ligne 144) → bit 0 de IF
            log::debug!("[PPU] VBlank IRQ pending set! pending_irq=${:02X}", self.pending_irq);
        }
        if self.stat & STAT_IRQ_LYC != 0
            && self.lyc <= 152
            && Self::crosses(prev_total, new_total, self.lyc as u64 * DOTS_PER_LINE as u64)
        {
            self.pending_irq |= IRQ_STAT; // ly == lyc (début de la ligne lyc) → bit 1 de IF
        }
        if self.stat & STAT_IRQ_MODE0 != 0 {
            for line in 0..SCREEN_HEIGHT as u64 {
                if Self::crosses(prev_total, new_total, line * DOTS_PER_LINE as u64 + 256) {
                    self.pending_irq |= IRQ_STAT; // entrée en mode 0 (HBlank) → bit 1 de IF
                    break;
                }
            }
        }
        if self.stat & STAT_IRQ_MODE2 != 0 {
            for line in 0..SCREEN_HEIGHT as u64 {
                if Self::crosses(prev_total, new_total, line * DOTS_PER_LINE as u64) {
                    self.pending_irq |= IRQ_STAT; // entrée en mode 2 (OAM Scan) → bit 1 de IF
                    break;
                }
            }
        }

        // Nouvelle position dans la frame.
        let wrapped = new_total % FRAME_DOTS;
        self.line = (wrapped / DOTS_PER_LINE as u64) as u8;
        self.dots = (wrapped % DOTS_PER_LINE as u64) as u32;
        self.update_ly_and_mode();

        new_total >= FRAME_DOTS // une frontière de frame a été franchie pendant cet avancement
    }

    /// True si l'intervalle ouvert à gauche de dots (prev, new] contient au moins une occurrence de
    /// `boundary` (qui se répète chaque frame). `prev` est toujours dans [0, FRAME_DOTS).
    fn crosses(prev: u64, new: u64, boundary: u64) -> bool {
        debug_assert!(prev < FRAME_DOTS && boundary < FRAME_DOTS);
        if boundary > prev {
            new >= boundary
        } else {
            new >= boundary + FRAME_DOTS
        }
    }

    /// Met à jour LY et le mode courant depuis la position (ligne, dot).
    fn update_ly_and_mode(&mut self) {
        if (self.line as u32) < SCREEN_HEIGHT as u32 {
            // Ligne visible : OAM Scan (dots 0..79), Drawing (80..255), HBlank (256..455).
            self.mode = match self.dots {
                0..80 => 2,
                80..256 => 3,
                _ => 0,
            };
            self.ly = self.line;
        } else {
            // VBlank (lignes 144..153) : mode 1 ; LY borné à 152 comme sur le hardware.
            self.mode = 1;
            self.ly = self.line.min(152);
        }
    }

    /// Renvoie les requêtes d'interruption PPU en attente et les efface (Pan Docs « Interrupt Sources ») :
    /// bit 0 = VBlank (→ bit 0 de IF), bit 1 = STAT/LCD (mode 0, LYC==LY ou mode 2 → bit 1 de IF).
    pub fn take_interrupts(&mut self) -> u8 {
        let irq = self.pending_irq;
        self.pending_irq = 0;
        irq
    }

    /// Rend la frame complète dans le framebuffer depuis les registres courants, la VRAM et l'OAM
    /// (Pan Docs « Background »/« Window »/« Sprite ») : d'abord la couche Background défilée par SCX/SCY
    /// si activée (sinon une base noire), puis la fenêtre si activée ; les sprites sont ensuite composées
    /// pixel par pixel, un sprite sans priorité passant seulement là où le fond est transparent. Le LCD
    /// éteint (bit 7 du LCDC à 0) rend l'écran noir.
    pub fn render_frame(&mut self, vram: &[u8; 0x2000], oam: &[u8; 0xA0]) {
        if self.lcdc & LCDC_LCD_ON == 0 {
            self.framebuffer.fill(0xFF00_0000); // LCD éteint : écran noir
            return;
        }

        let scx = self.scx as u32;
        let scy = self.scy as u32;
        let bg_on = self.lcdc & LCDC_BG != 0;
        let map_base = if self.lcdc & LCDC_BG_MAP_9C00 != 0 { BG_MAP_9C00 } else { BG_MAP_9800 };
        let signed_tiles = self.lcdc & LCDC_TILE_SET_8800 != 0;
        let window_on = self.lcdc & LCDC_WINDOW != 0;
        let win_tile_base = if self.lcdc & LCDC_WIN_TILE_8800 != 0 { TILE_SET_8800 } else { 0 };

        for y in 0..SCREEN_HEIGHT as u32 {
            // Sélection des sprites recouvrant cette ligne : au plus 10, les premiers dans l'ordre OAM (Pan Docs « Sprite »).
            let mut selected = [false; 40];
            if self.lcdc & LCDC_SPRITE_ON != 0 {
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
                        win_tile_base + tile_index as usize * 16, // indices non signés ; ensemble $8800-$97FF ou $8000-$8FFF (bit 6 du LCDC)
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

            // Sprites, dans l'ordre OAM : un pixel de valeur non nulle est dessiné devant la couche en dessous si le
            // sprite a la priorité (bit 4), sinon seulement là où cette couche est transparente (valeur 0 ou absente).
            for (i, &is_selected) in selected.iter().enumerate() {
                if is_selected {
                    self.draw_sprite(vram, oam, i, y, &under);
                }
            }
        }
    }

    /// Dessine l'entrée OAM `i` sur la ligne `y` (Pan Docs « Sprite ») : 8×8 pixels en position (X, Y-1),
    /// repli à 256 ; tuile $8000-$8FFF ou $8800-$97FF selon le bit 3 des drapeaux (index non signé) ;
    /// retournements X/Y (bits 6/5) ; la valeur de pixel 0 est transparente ; les valeurs 1..3 sont mappées
    /// par OBP0/OBP1 (le bit 1 de la valeur sélectionne OBP1). Un sprite sans priorité (bit 4 à 0) n'est
    /// dessiné que là où la couche Background/Fenêtre en dessous est transparente (`under` = None ou 0) ;
    /// un sprite avec priorité est toujours dessiné devant.
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
            // Sans priorité (bit 4 à 0), le sprite est masqué par un pixel Background/Fenêtre non transparent (valeur 1..3).
            let under_value = under[screen_col as usize];
            if !priority && matches!(under_value, Some(v) if v != 0) {
                continue;
            }
            // Le bit 1 de la valeur sélectionne OBP1 (valeurs 2-3), sinon OBP0.
            let palette = if value & 2 != 0 { self.obp1 } else { self.obp0 };
            self.framebuffer[y as usize * SCREEN_WIDTH + screen_col as usize] = Self::shade(palette >> (value * 2));
        }
    }

    /// Un sprite de coordonnée OAM `oam_y` recouvre la ligne écran `y` si et seulement si ses 8 lignes
    /// (Y-1 .. Y+6, avec repli à 256) incluent y (Pan Docs « Sprite »).
    fn sprite_covers_row(y: u32, oam_y: u8) -> bool {
        ((y.wrapping_sub(oam_y as u32).wrapping_add(1)) & 0xFF) < 8
    }

    /// Un sprite de coordonnée OAM `oam_x` a au moins une colonne visible si et seulement si ses 8 colonnes
    /// (X .. X+7, avec repli à 256) croisent l'écran [0..160).
    fn sprite_has_visible_column(oam_x: u8) -> bool {
        (0..8u32).any(|j| (oam_x as u32 + j) & 0xFF < SCREEN_WIDTH as u32)
    }

    /// Valeur (0..3) du pixel `col` de la ligne `row` d'une tuile 8×8 : chaque colonne occupe 2 bits, MSB en premier.
    fn tile_pixel(vram: &[u8], tile_addr: usize, row: u32, col: u32) -> u8 {
        let word = ((vram[tile_addr + 2 * row as usize] as u32) << 8) | vram[tile_addr + 2 * row as usize + 1] as u32;
        (word >> (2 * (7 - col)) & 3) as u8
    }

    /// Convertit une teinte DMG (2 bits de BGP/OBP) en pixel du framebuffer (R dans l'octet le plus bas).
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
        assert_eq!(ppu.dots, 0); // début de la scanline 0
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
        // Au power-on : aucun bit d'activation écrit + drapeau LYC==LY (ly == lyc == 0) en bit 7.
        assert_eq!(ppu.read_register(0xFF41), 0x80);

        ppu.write_register(0xFF41, 0x78); // bits d'activation des interruptions
        assert_eq!(ppu.read_register(0xFF41), 0xF8); // bits écrits + drapeau LYC==LY (bit 7)

        ppu.lyc = 5; // ly (0) != lyc (5) : le drapeau s'efface
        assert_eq!(ppu.read_register(0xFF41), 0x78);
    }

    #[test]
    fn framebuffer_starts_opaque_black() {
        let ppu = PPU::new();
        assert_eq!(ppu.framebuffer.len(), SCREEN_WIDTH * SCREEN_HEIGHT); // 160×144 pixels
        assert!(ppu.framebuffer.iter().all(|&px| px == 0xFF00_0000)); // noir opaque, aucune frame rendue
    }

    #[test]
    fn render_frame_with_empty_vram_uses_power_on_bgp() {
        let mut ppu = PPU::new(); // LCDC = $91 (fond activé), BGP = $FC : valeurs 0-1 → teinte claire, 2-3 → foncée
        let vram = [0u8; 0x2000]; // tuiles et cartes nulles : toutes les valeurs de pixel valent 0
        let oam = [0u8; 0xA0]; // OAM vide : aucun sprite
        ppu.render_frame(&vram, &oam);
        assert!(ppu.framebuffer.iter().all(|&px| px == PPU::shade(0))); // valeur 0 partout → teinte claire (BGP = $FC)
    }

    #[test]
    fn render_background_scrolls_and_selects_tiles() {
        let mut ppu = PPU::new();
        ppu.write_register(0xFF40, 0x90); // LCD allumé, fond activé ; tuiles $8000-$8FFF (non signées), carte $9800-$9BFF
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
        ppu.lcdc &= !LCDC_LCD_ON; // LCD éteint
        let vram = [0u8; 0x2000];
        let oam = [0u8; 0xA0]; // OAM vide : aucun sprite
        ppu.render_frame(&vram, &oam);
        assert!(ppu.framebuffer.iter().all(|&px| px == 0xFF00_0000)); // écran noir
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
    fn advance_tracks_ly_and_mode_per_line() {
        let mut ppu = PPU::new(); // ligne 0, dot 0 : OAM Scan (LCD allumé)
        ppu.advance(79); // dots 1..79 : toujours en OAM Scan
        assert_eq!((ppu.dots, ppu.mode, ppu.ly), (79, 2, 0));

        ppu.advance(1); // dot 80 : début du Drawing
        assert_eq!((ppu.dots, ppu.mode), (80, 3));

        ppu.advance(176); // dot 256 : début du HBlank
        assert_eq!((ppu.dots, ppu.mode), (256, 0));

        ppu.advance(199); // dot 455 : dernier dot de la ligne 0
        assert_eq!((ppu.dots, ppu.ly), (455, 0));

        ppu.advance(1); // début de la ligne 1 : OAM Scan à nouveau
        assert_eq!((ppu.dots, ppu.mode, ppu.ly), (0, 2, 1));
    }

    #[test]
    fn half_frame_is_at_line_77() {
        let mut ppu = PPU::new();
        ppu.advance(35_112); // demi-frame : exactement 77 lignes × 456 dots
        assert_eq!((ppu.dots, ppu.ly), (0, 77));
        assert_eq!(ppu.mode, 2); // début de la ligne 77 → OAM Scan
    }

    #[test]
    fn full_frame_returns_to_line_zero() {
        let mut ppu = PPU::new();
        assert!(ppu.advance(FRAME_DOTS)); // exactement une frame : frontière franchie
        assert_eq!((ppu.dots, ppu.mode, ppu.ly), (0, 2, 0));
        assert!(!ppu.advance(1)); // pas de nouvelle frame après un seul dot
    }

    #[test]
    fn vblank_spans_lines_144_to_153() {
        let mut ppu = PPU::new();
        ppu.advance(VBLANK_START_DOT); // début de la ligne 144 : le VBlank commence
        assert_eq!((ppu.dots, ppu.mode, ppu.ly), (0, 1, 144));

        ppu.advance(8 * DOTS_PER_LINE as u64); // lignes 145..152
        assert_eq!((ppu.dots, ppu.mode, ppu.ly), (0, 1, 152));

        ppu.advance(DOTS_PER_LINE as u64); // début de la ligne 153 : dernière ligne de la frame (LY maintenu à 152)
        assert_eq!((ppu.dots, ppu.mode, ppu.ly), (0, 1, 152));

        assert!(ppu.advance(DOTS_PER_LINE as u64)); // complète la ligne 153 et revient au début de la frame suivante
        assert_eq!((ppu.dots, ppu.mode, ppu.ly), (0, 2, 0));
    }

    #[test]
    fn lcd_off_freezes_the_ppu() {
        let mut ppu = PPU::new();
        ppu.lcdc &= !0x80; // LCD éteint : la PPU est gelée
        ppu.advance(10_000);
        assert_eq!((ppu.dots, ppu.mode, ppu.ly), (0, 2, 0)); // reste à la position power-on
        assert_eq!(ppu.take_interrupts(), 0);

        ppu.lcdc |= 0x80; // LCD rallumé : le timing reprend là où il était gelé
        ppu.advance(DOTS_PER_LINE as u64);
        assert_eq!((ppu.dots, ppu.ly), (0, 1));
    }

    #[test]
    fn vblank_interrupt_fires_on_entry() {
        let mut ppu = PPU::new();
        ppu.stat = STAT_IRQ_VBLANK; // active uniquement l'interruption VBlank

        ppu.advance(VBLANK_START_DOT - 1); // dernier dot de la ligne 143 (HBlank)
        assert_eq!(ppu.mode, 0);
        assert_eq!(ppu.take_interrupts(), 0); // pas encore en VBlank

        ppu.advance(1); // début de la ligne 144 : le VBlank commence
        assert_eq!((ppu.ly, ppu.mode), (144, 1));
        assert_eq!(ppu.take_interrupts(), IRQ_VBLANK); // → bit 0 de IF

        ppu.advance(3 * DOTS_PER_LINE as u64); // reste en VBlank : pas de re-déclenchement
        assert_eq!(ppu.take_interrupts(), 0);
    }

    #[test]
    fn vblank_interrupt_respects_its_enable_bit() {
        let mut ppu = PPU::new();
        ppu.stat = 0x00; // aucune interruption activée
        ppu.advance(VBLANK_START_DOT + 1); // entre en VBlank sans bit d'activation posé
        assert_eq!(ppu.take_interrupts(), 0);
    }

    #[test]
    fn lyc_match_raises_stat_interrupt() {
        let mut ppu = PPU::new();
        ppu.stat = STAT_IRQ_LYC; // active uniquement l'interruption LYC==LY
        ppu.lyc = 145;

        ppu.advance(145 * DOTS_PER_LINE as u64 - 1); // dernier dot de la ligne 144 (VBlank)
        assert_eq!(ppu.take_interrupts(), 0); // ly (144) != lyc (145)

        ppu.advance(1); // début de la ligne 145 : ly == lyc
        assert_eq!(ppu.ly, 145);
        assert_eq!(ppu.read_register(0xFF41) & (1 << 7), 1 << 7); // drapeau LYC==LY posé en bit 7 du STAT
        assert_eq!(ppu.take_interrupts(), IRQ_STAT); // → bit 1 de IF

        ppu.advance(DOTS_PER_LINE as u64); // ligne 146 : ly != lyc à nouveau, pas de re-déclenchement
        assert_eq!(ppu.take_interrupts(), 0);
    }

    #[test]
    fn mode_interrupts_fire_on_entry() {
        let mut ppu = PPU::new();
        ppu.stat = STAT_IRQ_MODE0 | STAT_IRQ_MODE2; // active les interruptions mode 0 et mode 2

        ppu.advance(79); // toujours en OAM Scan de la ligne 0 (déjà entré au power-on)
        assert_eq!(ppu.take_interrupts(), 0); // pas d'entrée nouvelle

        ppu.advance(1); // dot 80 : début du Drawing — le mode 3 n'a pas d'interruption
        assert_eq!(ppu.take_interrupts(), 0);

        ppu.advance(176); // dot 256 : début du HBlank (mode 0)
        assert_eq!(ppu.mode, 0);
        assert_eq!(ppu.take_interrupts(), IRQ_STAT); // entrée en mode 0 → bit 1 de IF

        ppu.advance(200); // fin de la ligne 0 et début de la ligne 1 : OAM Scan (mode 2) à nouveau
        assert_eq!((ppu.dots, ppu.mode), (0, 2));
        assert_eq!(ppu.take_interrupts(), IRQ_STAT); // entrée en mode 2 → bit 1 de IF
    }
}