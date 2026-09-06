//! Constantes de la PPU : résolution, timing des modes, masques de bits (LCDC, STAT), teintes DMG.

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
pub(crate) const MODE_OAM_CYCLES: u32 = 80;
/// Durée du mode Drawing en T-cycles (172 dots).
pub(crate) const MODE_DRAW_CYCLES: u32 = 172;
/// Durée du mode HBlank en T-cycles (204 dots).
pub(crate) const MODE_HBLANK_CYCLES: u32 = 204;

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
/// Bit 4 du LCDC : when set, tuiles du fond en $8000-$8FFF (indices non signés) ; à 0, en $8800-$97FF (indices signés, origine $9000).
pub const LCDC_TILE_SET_8800: u8 = 1 << 4;
/// Bit 3 du LCDC : carte du fond en $9C00-$9BFF au lieu of $9800-$9BFF.
pub const LCDC_BG_MAP_9C00: u8 = 1 << 3;
/// Bit 0 du LCDC : activation of the couche Background (à 0, aucun pixel de fond n'est généré).
pub const LCDC_BG: u8 = 1 << 0;
/// Bit 5 du LCDC : activation of the fenêtre (la carte est choisie par le bit 6 — Pan Docs « Window »).
pub const LCDC_WINDOW: u8 = 1 << 5;
/// Bit 6 du LCDC : when set, the character map of the fenêtre is in $9800-$9BFF ; à 0, en $9C00-$9FFF. Les données de tuiles sont
/// partagées with le fond (bit 4 du LCDC) — Pan Docs « Window ».
pub const LCDC_WIN_MAP_9800: u8 = 1 << 6;

/// Bits 0 et 5 du LCDC : activation of the couche Background and of the fenêtre (ensemble).
#[allow(dead_code)] // Utilisé par les tests PPU ; API publique for the parties futures.
pub const LCDC_BG_WIN_ON: u8 = LCDC_BG | LCDC_WINDOW;
/// Bit 2 du LCDC : objets 8×16 au lieu of 8×8 (Pan Docs « LCDC ») — when set, an object occupies the lines Y-16..Y-1 and its tile number NN selects two tiles: upper half `NN & $FE`, lower half `NN | $01` (Pan Docs « OAM »).
pub const LCDC_OBJ_SIZE_16: u8 = 1 << 2;

/// Bit 3 des drapeaux d'une entrée OAM : ensemble of tuiles of the sprite en $8800-$97FF au lieu of $8000-$8FFF.
pub const SPRITE_TILE_SET_8800: u8 = 1 << 3;
/// Bit 4 des drapeaux d'une entrée OAM : priorité — le sprite is dessiné devant the couche Background.
pub const SPRITE_PRIORITY: u8 = 1 << 4;
/// Bit 5 des drapeaux d'une entrée OAM : retournement vertical of the sprite.
pub const SPRITE_Y_FLIP: u8 = 1 << 5;
/// Bit 6 des drapeaux d'une entrée OAM : retournement horizontal of the sprite.
pub const SPRITE_X_FLIP: u8 = 1 << 6;

/// Base dans la VRAM of the carte du fond par défaut ($9800) — index relatif à $8000.
pub(crate) const BG_MAP_9800: usize = 0x9800 - 0x8000;
/// Base dans la VRAM of the carte en $9C00 (fond si bit 3 du LCDC, and fenêtre) — index relatif à $8000.
pub(crate) const BG_MAP_9C00: usize = 0x9C00 - 0x8000;
/// Base dans la VRAM of the ensemble of tuiles en $8800-$97FF (bit 3 des drapeaux OAM) — index relatif à $8000.
pub(crate) const TILE_SET_8800: usize = 0x8800 - 0x8000;
/// Origine dans la VRAM of the ensemble de tuiles signé (bits 4/6 du LCDC à 0, Pan Docs « VRAM Tile Data ») : l'octet 0 pointe vers $9000,
/// les octets 1-127 vers $9010-$97F0, and the octets 128-255 (interprétés comme -128..-1) vers $8800-$8FF0 — index relatif à $8000.
pub(crate) const TILE_SET_9000: usize = 0x9000 - 0x8000; // 0x1000

/// Les 4 teintes DMG classiques (Pan Docs « Graphics ») : [R, G, B].
pub const DMG_SHADES: [[u8; 3]; 4] = [
    [0x9B, 0xBC, 0x0F], // Teinte 0 : Vert clair (#9BBC0F)
    [0x8B, 0xAC, 0x0F], // Teinte 1 : Vert moyen (#8BAC0F)
    [0x30, 0x62, 0x30], // Teinte 2 : Vert foncé (#306230)
    [0x0F, 0x38, 0x0F], // Teinte 3 : Vert très foncé (#0F380F)
];

