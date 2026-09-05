//! Rendu des objets OAM (sprites) : visibilité par ligne, tuiles 8×8/8×16, retournements X/Y,
//! transparence de la valeur 0, palette OBP0/OBP1 et priorité face au fond (Pan Docs « Sprite »/« OAM »).

use crate::ppu::constants::{
    DMG_SHADES, LCDC_OBJ_SIZE_16, SCREEN_WIDTH, SPRITE_PRIORITY, SPRITE_TILE_SET_8800,
    SPRITE_X_FLIP, SPRITE_Y_FLIP, TILE_SET_8800,
};

use super::PPU;

impl PPU {
    /// Dessine l'entrée OAM `i` sur la ligne `y` (Pan Docs « Sprite »/« OAM ») : 8×8 pixels en position (X, Y-1), or 8×16 in
    /// position (X, Y-16) when bit 2 du LCDC is set — repli à 256 ; tuile $8000-$8FFF or $8800-$97FF selon the bit 3 des drapeaux
    /// (index non signé), and in 8×16 mode the tile number NN selects two tiles: upper half `NN & $FE`, lower half `NN | $01` ;
    /// retournements X/Y (bits 6/5 — a vertical flip swaps the two halves of an object 8×16) ; la valeur of pixel 0 est transparente ;
    /// les valeurs 1..3 are mappées by OBP0/OBP1 (le bit 1 of the value sélectionne OBP1). Un sprite sans priorité (bit 4 à 0) n'est
    /// dessiné que là where the couche Background/Fenêtre en dessous is transparente (`under` = None or 0) ;
    /// un sprite with priorité is always drawn devant.
    pub(crate) fn draw_sprite(&mut self, vram: &[u8; 0x2000], oam: &[u8; 0xA0], i: usize, y: u32, under: &[Option<u8>; SCREEN_WIDTH]) {
        let flags = oam[4 * i + 3];
        let priority = flags & SPRITE_PRIORITY != 0;
        let tile_base = if flags & SPRITE_TILE_SET_8800 != 0 { TILE_SET_8800 } else { 0 };
        let tile_index = oam[4 * i + 2];

        // Ligne de l'objet recouverte par la scanline, comptée depuis son sommet (Pan Docs « OAM ») : un objet 8×8 démarre à Y-1 ;
        // un objet 8×16 (bit 2 du LCDC) démarre à Y-16.
        let row_from_top = if self.lcdc & LCDC_OBJ_SIZE_16 != 0 {
            y.wrapping_sub(oam[4 * i] as u32).wrapping_add(16) & 0xFF // 0..15
        } else {
            y.wrapping_sub(oam[4 * i] as u32).wrapping_add(1) & 0xFF // 0..7
        };

        for j in 0..8u32 {
            let screen_col = (oam[4 * i + 1] as u32 + j) & 0xFF; // la colonne se replie à 256
            if screen_col >= SCREEN_WIDTH as u32 {
                continue; // hors écran à droite : pixel non affiché
            }

            // Tuile et ligne de tuile for this scanline. En 8×16, the tile number NN selects two tiles: upper half `NN & $FE`,
            // lower half `NN | $01` ; a vertical flip (bit 5) swaps the two halves and inverts each line within its half.
            let (tile_index, tile_row) = if self.lcdc & LCDC_OBJ_SIZE_16 != 0 {
                let mut index = if row_from_top < 8 { tile_index & 0xFE } else { tile_index | 0x01 };
                let mut row = row_from_top & 7;
                if flags & SPRITE_Y_FLIP != 0 {
                    index ^= 0x01; // swap the two halves of the object
                    row = 7 - row;
                }
                (index, row)
            } else {
                let tile_row = if flags & SPRITE_Y_FLIP != 0 { 7 - row_from_top } else { row_from_top };
                (tile_index, tile_row)
            };

            let tile_addr = tile_base + tile_index as usize * 16; // index non signé (Pan Docs « Sprite »)
            let tile_col = if flags & SPRITE_X_FLIP != 0 { 7 - j } else { j };
            let value = Self::tile_pixel(vram, tile_addr, tile_row, tile_col);
            if value == 0 {
                continue; // pixel transparent : la couche en dessous passes au travers
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

    /// Un objet de coordonnée OAM `oam_y` recouvre the line écran `y` if and only if ses lignes incluent y (Pan Docs « OAM »),
    /// with repli à 256 : Y-1 .. Y+6 for an object 8×8, Y-16 .. Y-1 for an object 8×16 (bit 2 du LCDC).
    pub(crate) fn sprite_covers_row(y: u32, oam_y: u8, sixteen_px: bool) -> bool {
        if sixteen_px {
            ((y.wrapping_sub(oam_y as u32).wrapping_add(16)) & 0xFF) < 16
        } else {
            ((y.wrapping_sub(oam_y as u32).wrapping_add(1)) & 0xFF) < 8
        }
    }

    /// Un sprite de coordonnée OAM `oam_x` a au moins une colonne visible if and only if ses 8 colonnes
    /// (X .. X+7, with repli à 256) croisent l'écran [0..160).
    pub(crate) fn sprite_has_visible_column(oam_x: u8) -> bool {
        (0..8u32).any(|j| (oam_x as u32 + j) & 0xFF < SCREEN_WIDTH as u32)
    }

    /// Valeur (0..3) du pixel `col` de la ligne `row` d'une tuile 8×8 (Pan Docs « Tile »/GBCTR) : le premier octet de la
    /// ligne contient les bits de poids fort (MSB) des pixels, le second les bits de poids faible (LSB), bit 7 = pixel le plus à gauche.
    pub(crate) fn tile_pixel(vram: &[u8], tile_addr: usize, row: u32, col: u32) -> u8 {
        let byte1 = vram[tile_addr + 2 * row as usize]; // MSB des pixels de la ligne
        let byte2 = vram[tile_addr + 2 * row as usize + 1]; // LSB des pixels de la ligne
        let bit_index = 7 - col; // le pixel 0 est à gauche (bit 7)

        let msb = (byte1 >> bit_index) & 1;
        let lsb = (byte2 >> bit_index) & 1;

        (msb << 1) | lsb // valeur du pixel : 0, 1, 2 ou 3
    }

    /// Convertit a teinte DMG (2 bits of BGP/OBP) en pixel du framebuffer (R dans l'octet le plus bas).
    pub fn shade(bits: u8) -> u32 {
        let bits = bits & 3;
        let [r, g, b] = DMG_SHADES[bits as usize];
        0xFF00_0000 | ((b as u32) << 16) | ((g as u32) << 8) | r as u32
    }
}
