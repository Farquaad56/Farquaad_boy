//! Rendu des couches Background et Fenêtre (Pan Docs « Background »/« Window ») : la scanline est dessinée
//! à la fin du mode Drawing, puis les sprites recouvrant la ligne sont composés par `sprites`.

use crate::ppu::constants::{
    BG_MAP_9800, BG_MAP_9C00, LCDC_BG, LCDC_BG_MAP_9C00, LCDC_LCD_ON, LCDC_OBJ_SIZE_16,
    LCDC_SPRITE_ON, LCDC_TILE_SET_8800, LCDC_WINDOW, LCDC_WIN_MAP_9800, SCREEN_HEIGHT,
    SCREEN_WIDTH, TILE_SET_9000,
};

use super::PPU;

impl PPU {
    /// Rend la frame courante dans le framebuffer : Background (défilement SCX/SCY, carte choisie by the bit 3 du LCDC,
    /// données de tuiles chosen by the bit 4, palette BGP) puis fenêtre (WY/WX), selon PanDocs « Background »/« Window ».
    /// La PPU cycle-accurate (`step`) dessine chaque scanline via `render_scanline` à la fin du mode Drawing ; cette
    /// méthode rend les 144 lignes visibles d'un coup (utile aux tests et au rendu complet).
    #[allow(dead_code)] // le rendu normal est incrémental via `step` ; cette méthode sert surtout aux tests
    pub fn render_frame(&mut self, vram: &[u8; 0x2000], oam: &[u8; 0xA0]) {
        if self.lcdc & LCDC_LCD_ON == 0 {
            self.framebuffer.fill(0xFF00_0000); // LCD éteint : écran noir
            return;
        }

        for y in 0..SCREEN_HEIGHT as u32 {
            self.render_scanline(y as u8, vram, oam); // y < SCREEN_HEIGHT (144) : le cast en u8 est sûr
        }
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
        // Pan Docs « LCDC » : bit 4 set → tuiles du fond $8000-$8FFF (indices non signés) ; à 0 → $8800-$97FF, l'octet v
        // pointant vers $9000 + v*16 avec v interprété comme signé (-128..127).
        let unsigned_tiles = self.lcdc & LCDC_TILE_SET_8800 != 0;
        let window_on = self.lcdc & LCDC_WINDOW != 0;
        // Bit 6 du LCDC selects the character map of the fenêtre ($9800-$9BFF if set, $9C00-$9FFF if clear) ; les données de tuiles sont
        // partagées with le fond (bit 4 : unsigned $8000 or signed, origine $9000).
        let win_map_base = if self.lcdc & LCDC_WIN_MAP_9800 != 0 { BG_MAP_9800 } else { BG_MAP_9C00 };

        // Sélection des sprites recouvrant cette ligne : au plus 10, les premiers dans l'ordre OAM (Pan Docs « Sprite »).
        // Les objets sont rendus only when the couche d'objets est activée (bit 1 du LCDC) ; sans fond ni fenêtre,
        // ils apparaissent sur la base noire.
        let mut selected = [false; 40];
        if self.lcdc & LCDC_SPRITE_ON != 0 {
            let objects_16px = self.lcdc & LCDC_OBJ_SIZE_16 != 0; // bit 2 du LCDC : objets 8×16 (Pan Docs « OAM »)
            let mut count = 0usize;
            for i in 0..40 {
                if Self::sprite_covers_row(y, oam[4 * i], objects_16px) && Self::sprite_has_visible_column(oam[4 * i + 1]) {
                    selected[i] = true;
                    count += 1;
                    if count == 10 {
                        break; // limite de 10 sprites par ligne : les suivants are supprimés
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
                let tile_addr = if unsigned_tiles {
                    tile_index as usize * 16 // $8000 + idx*16 : index non signé ($8000-$8FFF)
                } else {
                    (TILE_SET_9000 as i32 + (tile_index as i8) as i32 * 16) as usize // $9000 + idx signé*16 ($8800-$97FF)
                };
                let pixel = Self::tile_pixel(vram, tile_addr, bg_y & 7, bg_x & 7);
                under[x as usize] = Some(pixel);
                self.framebuffer[row_start + x as usize] = Self::shade(self.bgp >> (pixel * 2));
            }
        } else {
            // Couche Background éteinte : base noire.
            self.framebuffer[row_start..row_start + SCREEN_WIDTH].fill(0xFF00_0000);
        }

        // Fenêtre : pas de défilement ; elle apparaît à partir of the ligne WY et of the colonne WX+1 (Pan Docs « Window »).
        if window_on && y >= self.wy as u32 {
            let win_y = y - self.wy as u32;
            for x in (self.wx as u32 + 1)..SCREEN_WIDTH as u32 {
                let win_x = x - self.wx as u32 - 1; // colonne de la fenêtre : le pixel le plus à gauche est en WX+1
                let tile_index = vram[win_map_base + (win_y >> 3) as usize * 32 + (win_x >> 3) as usize];
                let tile_addr = if unsigned_tiles {
                    tile_index as usize * 16 // $8000 + idx*16 : index non signé ($8000-$8FFF) — partagé with le fond (bit 4 du LCDC)
                } else {
                    (TILE_SET_9000 as i32 + (tile_index as i8) as i32 * 16) as usize // $9000 + idx signé*16 ($8800-$97FF)
                };
                let pixel = Self::tile_pixel(vram, tile_addr, win_y & 7, win_x & 7);
                if pixel != 0 {
                    // Les pixels de valeur 0 are transparents : la couche en dessous passes au travers.
                    under[x as usize] = Some(pixel);
                    self.framebuffer[row_start + x as usize] = Self::shade(self.bgp >> (pixel * 2));
                }
            }
        }

        // Sprites, dans l'ordre OAM : un pixel de value non nulle est dessiné devant the couche en dessous si the
        // sprite a la priorité (bit 4), sinon seulement là where cette couche is transparente (valeur 0 or absente).
        for (i, &is_selected) in selected.iter().enumerate() {
            if is_selected {
                self.draw_sprite(vram, oam, i, y, &under);
            }
        }
    }
}
