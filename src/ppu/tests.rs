//! Tests unitaires de la PPU : registres, rendu Background/Fenêtre/sprites, timing et interruptions.

use super::*;
use crate::ppu::constants::*;

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
    ppu.write_register(0xFF41, 0xFF); // seuls les bits 3..6 are écrits
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
    let mut ppu = PPU::new(); // LCDC = $91 : LCD allumé (bit 7) + fond activé (bit 0), tuiles du fond non signées $8000-$8FFF (bit 4 set) ; BGP = $FC : valeur 0 → teinte claire, 1-3 → foncée
    let vram = [0u8; 0x2000]; // tuiles et cartes nulles : toutes les valeurs de pixel valent 0
    let oam = [0u8; 0xA0]; // OAM vide : aucun sprite
    ppu.render_frame(&vram, &oam);
    assert!(ppu.framebuffer.iter().all(|&px| px == PPU::shade(0))); // valeur 0 partout → teinte claire (BGP = $FC)
}

#[test]
fn tile_pixel_combines_the_two_row_bytes_per_gb_spec() {
    let mut vram = [0u8; 0x2000];
    // Tuile $8030 : la ligne 3 a tous les MSB à 1 et les LSB à 0 (valeur 2) ; la ligne 4, l'inverse (valeur 1).
    vram[0x36] = 0xFF; // premier octet de la ligne 3 : MSB des pixels
    vram[0x37] = 0x00; // second octet : LSB des pixels
    vram[0x38] = 0x00; // ligne 4 : MSB à 0
    vram[0x39] = 0xFF; // LSB à 1

    assert_eq!(PPU::tile_pixel(&vram, 0x30, 3, 0), 2); // pixel le plus à gauche (bit 7) : MSB=1, LSB=0
    assert_eq!(PPU::tile_pixel(&vram, 0x30, 3, 7), 2); // pixel le plus à droite (bit 0) : même valeur
    assert_eq!(PPU::tile_pixel(&vram, 0x30, 4, 0), 1); // MSB=0, LSB=1 — l'ordre des deux octets est respecté
    assert_eq!(PPU::tile_pixel(&vram, 0x30, 4, 7), 1);
}

#[test]
fn render_background_scrolls_and_selects_tiles() {
    let mut ppu = PPU::new();
    ppu.write_register(0xFF40, 0x91); // LCD allumé (bit 7) + fond activé (bit 0) ; tuiles non signées $8000-$8FFF (bit 4 set), carte $9800-$9BFF
    ppu.bgp = 0xE4; // teinte v for a pixel value v (bits 2v..2v+1 valent v)

    let mut vram = [0u8; 0x2000];
    for row in 0..8 {
        vram[0x10 + 2 * row] = 0b11_11_00_00; // tuile 1 ($8010-$801F) : moitié gauche → valeur 3 (MSB des pixels)
        vram[0x11 + 2 * row] = 0b11_11_00_00; // moitié droite → valeur 0 (LSB des pixels)
    }
    vram[0x1800..0x1A00].fill(1); // carte $9800-$9BFF : tuile 1 partout
    let oam = [0u8; 0xA0]; // OAM vide : aucun sprite

    ppu.render_frame(&vram, &oam);
    assert_eq!(ppu.framebuffer[0], PPU::shade(3)); // colonne 0 → moitié gauche of the tuile
    assert_eq!(ppu.framebuffer[4], PPU::shade(0)); // colonne 4 → moitié droite
    assert_eq!(ppu.framebuffer[SCREEN_WIDTH + 3], PPU::shade(3)); // ligne 1, colonne 3 : même motif (moitié gauche noire)

    ppu.scx = 1; // défilement horizontal : le motif glisse d'un pixel vers the left
    ppu.render_frame(&vram, &oam);
    assert_eq!(ppu.framebuffer[3], PPU::shade(0)); // colonne 3 → colonne 4 of the tuile (blanche)
    assert_eq!(ppu.framebuffer[7], PPU::shade(3)); // colonne 7 → colonne 8 = colonne 0 of the tuile suivante

    ppu.scx = 0;
    for row in 0..8 {
        let v = if row == 1 { 0xFF } else { 0x00 }; // seule la ligne 1 de la tuile est noire (valeur 3)
        vram[0x10 + 2 * row] = v; // MSB des pixels
        vram[0x11 + 2 * row] = v; // LSB des pixels
    }
    ppu.scy = 1; // défilement vertical : la ligne écran 0 montre the line 1 of the tuile
    ppu.render_frame(&vram, &oam);
    assert_eq!(ppu.framebuffer[0], PPU::shade(3)); // ligne 0 → ligne 1 of the tuile (noire)
    assert_eq!(ppu.framebuffer[SCREEN_WIDTH], PPU::shade(0)); // ligne 1 → ligne 2 of the tuile (blanche)
}

#[test]
fn render_background_selects_tile_set_and_map() {
    let mut ppu = PPU::new();
    ppu.write_register(0xFF40, 0x91); // LCD allumé + fond activé ; bit 4 set : tuiles $8000-$8FFF (non signées), carte $9800-$9BFF
    ppu.bgp = 0xE4; // teinte v for a pixel value v

    let mut vram = [0u8; 0x2000];
    for row in 0..8 {
        vram[0x7F0 + 2 * row] = 0xFF; // tuile $87F0 : noire (index non signé $7F) — MSB des pixels
        vram[0x7F1 + 2 * row] = 0xFF; // LSB des pixels
    }

    vram[BG_MAP_9800] = 0x7F; // cellule (0,0) of the carte $9800
    let oam = [0u8; 0xA0]; // OAM vide : aucun sprite
    ppu.render_frame(&vram, &oam);
    assert_eq!(ppu.framebuffer[0], PPU::shade(3)); // non signé : tuile $87F0 → noire

    ppu.lcdc &= !LCDC_TILE_SET_8800; // bit 4 du LCDC à 0 : indices signés — la même value pointe vers $97F0 (origine $9000)
    for row in 0..8 {
        vram[0x17F0 + 2 * row] = 0xFF; // tuile $97F0 : noire (index signé +$7F) — MSB des pixels
        vram[0x17F1 + 2 * row] = 0xFF; // LSB des pixels
    }
    ppu.render_frame(&vram, &oam);
    assert_eq!(ppu.framebuffer[0], PPU::shade(3)); // signé +$7F : tuile $97F0 → noire

    vram[BG_MAP_9800] = 0; // carte $9800 : tuile nulle → blanche
    ppu.lcdc |= LCDC_BG_MAP_9C00; // bit 3 du LCDC : carte $9C00-$9BFF
    vram[BG_MAP_9C00] = 0xFF; // cellule (0,0) of the carte $9C00 → tuile -1 → $8FF0 (noire)
    for row in 0..8 {
        vram[0x0FF0 + 2 * row] = 0xFF; // tuile $8FF0 : noire (index signé -$01) — MSB des pixels
        vram[0x0FF1 + 2 * row] = 0xFF; // LSB des pixels
    }
    ppu.render_frame(&vram, &oam);
    assert_eq!(ppu.framebuffer[0], PPU::shade(3)); // carte $9C00 : noire
}

#[test]
fn render_window_layer() {
    let mut ppu = PPU::new();
    // LCD allumé (bit 7) + fond activé (bit 0) + fenêtre activée (bit 5) ; bit 6 set : la carte de la fenêtre est en $9800-$9BFF,
    // and bit 3 set : the carte du fond is en $9C00-$9FFF — les deux couches lisent des cartes distinctes.
    ppu.write_register(0xFF40, 0xF9);
    ppu.bgp = 0xE4; // teinte v for a pixel value v
    ppu.wy = 32; // ligne du haut of the fenêtre
    ppu.wx = 7; // le pixel le plus à gauche est en colonne 8 (Pan Docs « Window »)

    let mut vram = [0u8; 0x2000]; // fond blanc : carte $9C00 nulle → tuile nulle → valeur 0
    for row in 0..8 {
        vram[5 * 16 + 2 * row] = 0xFF; // tuile 5 de $8000 (bit 4 set : index non signé) : noire
        vram[5 * 16 + 2 * row + 1] = 0xFF;
    }
    vram[BG_MAP_9800 + 1] = 5; // cellule (ligne 0, colonne 1) of the carte $9800 → tuile 5

    let oam = [0u8; 0xA0]; // OAM vide : aucun sprite

    ppu.render_frame(&vram, &oam);
    assert_eq!(ppu.framebuffer[32 * SCREEN_WIDTH + 16], PPU::shade(3)); // ligne WY : fenêtre noire (cellule (0,1) of the carte $9800)
    assert_eq!(ppu.framebuffer[(32 - 1) * SCREEN_WIDTH + 16], PPU::shade(0)); // ligne WY-1 : fond blanc
    assert_eq!(ppu.framebuffer[32 * SCREEN_WIDTH + 7], PPU::shade(0)); // colonne ≤ WX : pas de fenêtre
    assert_eq!(ppu.framebuffer[32 * SCREEN_WIDTH + 8], PPU::shade(0)); // cellule (0,0) nulle → transparente → fond blanc

    ppu.lcdc &= !LCDC_WIN_MAP_9800; // bit 6 du LCDC à 0 : la carte de la fenêtre passe en $9C00-$9FFF
    vram[BG_MAP_9C00 + 1] = 5; // cellule (ligne 0, colonne 1) of the carte $9C00 → tuile 5
    ppu.render_frame(&vram, &oam);
    assert_eq!(ppu.framebuffer[32 * SCREEN_WIDTH + 16], PPU::shade(3)); // fenêtre noire via la carte $9C00 (données de tuiles toujours en $8000)

    ppu.lcdc = 0xE9; // bit 6 set again (carte $9800) and bit 4 à 0 : les indices de tuiles sont signés (origine $9000) — partagé fond/fenêtre
    for row in 0..8 {
        vram[0x1050 + 2 * row] = 0xFF; // tuile 5 signé : $9000 + 5*16 = $9050 → noire (seules données posées)
        vram[0x1051 + 2 * row] = 0xFF;
    }
    ppu.render_frame(&vram, &oam);
    assert_eq!(ppu.framebuffer[32 * SCREEN_WIDTH + 16], PPU::shade(3)); // fenêtre noire via the tuile $9050 (index signé) — pas $8050
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
    ppu.write_register(0xFF40, 0xC3); // LCD allumé (bit 7) + fond activé (bit 0) + sprites activées (bit 1) ; bit 6 set : tuiles de la fenêtre non signées
    ppu.bgp = 0xE4; // teinte v for a pixel value v
    ppu.obp0 = 0xE4;

    let mut vram = [0u8; 0x2000];
    for row in 0..8 {
        vram[0x10 + 2 * row] = 0xFF; // tuile 1 ($8010-$801F) : toutes the values of pixel valent 3
        vram[0x11 + 2 * row] = 0xFF;
    }

    let mut oam = [0u8; 0xA0];
    oam[0] = 16; // Y = 16 : le sprite occupe the lines écran 15..22 (Y-1 .. Y+6)
    oam[1] = 32; // X = 32 : colonnes 32..39
    oam[2] = 1; // tuile 1, ensemble $8000-$8FFF

    ppu.render_frame(&vram, &oam);
    assert_eq!(ppu.framebuffer[15 * SCREEN_WIDTH + 32], PPU::shade(3)); // coin haut-gauche du sprite (ligne Y-1)
    assert_eq!(ppu.framebuffer[22 * SCREEN_WIDTH + 39], PPU::shade(3)); // coin bas-droite (ligne Y+6, colonne X+7)
    assert_eq!(ppu.framebuffer[14 * SCREEN_WIDTH + 32], PPU::shade(0)); // ligne Y-2 : pas de sprite → fond blanc
    assert_eq!(ppu.framebuffer[23 * SCREEN_WIDTH + 32], PPU::shade(0)); // ligne Y+7 : pas de sprite → fond blanc
    assert_eq!(ppu.framebuffer[15 * SCREEN_WIDTH + 40], PPU::shade(0)); // juste à droite of the sprite → fond white
}

#[test]
fn render_sprite_wraps_around_the_screen() {
    let mut ppu = PPU::new();
    ppu.write_register(0xFF40, 0xC3); // LCD allumé (bit 7) + fond activé (bit 0) + sprites activées (bit 1) ; bit 6 set : tuiles de la fenêtre non signées
    ppu.bgp = 0xE4; // teinte v for a pixel value v
    ppu.obp0 = 0xE4;

    let mut vram = [0u8; 0x2000];
    for row in 0..8 {
        vram[0x10 + 2 * row] = 0xFF; // tuile 1 ($8010-$801F) : toutes the values of pixel valent 3
        vram[0x11 + 2 * row] = 0xFF;
    }

    let mut oam = [0u8; 0xA0];
    oam[0] = 48; // Y = 48 : lignes écran 47..54
    oam[1] = 253; // X = 253 : colonnes 253..255 (hors écran) + repli vers 0..4
    oam[2] = 1;

    ppu.render_frame(&vram, &oam);
    assert_eq!(ppu.framebuffer[47 * SCREEN_WIDTH], PPU::shade(3)); // colonne 0 : la colonne 256 of the sprite repliée
    assert_eq!(ppu.framebuffer[47 * SCREEN_WIDTH + 4], PPU::shade(3)); // colonne 4 : dernière colonne visible of the sprite (j=7)
    assert_eq!(ppu.framebuffer[47 * SCREEN_WIDTH + 5], PPU::shade(0)); // colonne 5 : pas de sprite → fond white
}

#[test]
fn render_sprite_y_visibility_window() {
    let mut ppu = PPU::new();
    ppu.write_register(0xFF40, 0x83); // LCD allumé (bit 7) + fond activé (bit 0) + sprites activées (bit 1)
    ppu.bgp = 0xE4; // teinte v for a pixel value v
    ppu.obp0 = 0xE4;

    let mut vram = [0u8; 0x2000];
    for row in 0..8 {
        vram[0x10 + 2 * row] = 0xFF; // tuile 1 ($8010-$801F) : toutes the values of pixel valent 3
        vram[0x11 + 2 * row] = 0xFF;
    }

    let mut oam = [0u8; 0xA0];
    oam[0] = 143; // Y = 143 : le sprite occupe the lines écran 142, 143 and 144..149 (hors écran)
    oam[1] = 8;
    oam[2] = 1;

    ppu.render_frame(&vram, &oam);
    assert_eq!(ppu.framebuffer[(SCREEN_HEIGHT - 2) * SCREEN_WIDTH + 8], PPU::shade(3)); // ligne 142 : visible
    assert_eq!(ppu.framebuffer[(SCREEN_HEIGHT - 1) * SCREEN_WIDTH + 8], PPU::shade(3)); // ligne 143 : visible

    oam[0] = 150; // Y = 150 : le sprite occupe the lines écran 149..156 — toutes hors écran
    ppu.render_frame(&vram, &oam);
    assert!(ppu.framebuffer.iter().all(|&px| px == PPU::shade(0))); // aucun pixel of sprite visible → fond white
}

#[test]
fn render_sprite_horizontal_flip() {
    let mut ppu = PPU::new();
    ppu.write_register(0xFF40, 0x83); // LCD allumé (bit 7) + fond activé (bit 0) + sprites activées (bit 1)
    ppu.bgp = 0xE4; // teinte v for a pixel value v
    ppu.obp0 = 0xE4;

    let mut vram = [0u8; 0x2000];
    for row in 0..8 {
        vram[0x10 + 2 * row] = 0b11_11_00_00; // tuile 1 ($8010-$801F) : moitié gauche → valeur 3, moitié droite → valeur 0 (transparente) — MSB des pixels
        vram[0x11 + 2 * row] = 0b11_11_00_00; // LSB des pixels
    }

    let mut oam = [0u8; 0xA0];
    oam[0] = 16; // Y = 16 : lignes écran 15..22
    oam[1] = 32;
    oam[2] = 1;
    oam[3] = SPRITE_X_FLIP; // bit 6 des drapeaux : retournement horizontal

    ppu.render_frame(&vram, &oam);
    assert_eq!(ppu.framebuffer[15 * SCREEN_WIDTH + 32], PPU::shade(0)); // colonne X : moitié droite of the tuile (transparente) → fond white
    assert_eq!(ppu.framebuffer[15 * SCREEN_WIDTH + 39], PPU::shade(3)); // colonne X+7 : moitié gauche of the tuile
}

#[test]
fn render_sprite_vertical_flip() {
    let mut ppu = PPU::new();
    ppu.write_register(0xFF40, 0x83); // LCD allumé (bit 7) + fond activé (bit 0) + sprites activées (bit 1)
    ppu.bgp = 0xE4; // teinte v for a pixel value v
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
    assert_eq!(ppu.framebuffer[15 * SCREEN_WIDTH + 32], PPU::shade(0)); // ligne Y-1 : dernière line of the tuile (transparente) → fond white
    assert_eq!(ppu.framebuffer[22 * SCREEN_WIDTH + 32], PPU::shade(3)); // ligne Y+6 : première line of the tuile
}

#[test]
fn render_sprite_selects_tile_set_8800() {
    let mut ppu = PPU::new();
    ppu.write_register(0xFF40, 0x83); // LCD allumé (bit 7) + fond activé (bit 0) + sprites activées (bit 1)
    ppu.bgp = 0xE4; // teinte v for a pixel value v
    ppu.obp0 = 0xE4;

    let mut vram = [0u8; 0x2000];
    for row in 0..8 {
        vram[0x10 + 2 * row] = 0b11_11_00_00; // tuile 1 de $8000 : moitié gauche → valeur 3 (MSB des pixels)
        vram[0x11 + 2 * row] = 0b11_11_00_00; // LSB des pixels
        vram[TILE_SET_8800 + 0x10 + 2 * row] = 0b11_11_00_00; // tuile 1 de $8800 : moitié gauche → valeur 3 (MSB des pixels)
        vram[TILE_SET_8800 + 0x11 + 2 * row] = 0b11_11_00_00; // LSB des pixels
    }

    let mut oam = [0u8; 0xA0];
    oam[0] = 16; // Y = 16 : lignes écran 15..22
    oam[1] = 32;
    oam[2] = 1; // tuile 1, ensemble $8000-$8FFF

    ppu.render_frame(&vram, &oam);
    assert_eq!(ppu.framebuffer[15 * SCREEN_WIDTH + 32], PPU::shade(3)); // moitié gauche of the tuile 1 of $8000

    oam[3] = SPRITE_TILE_SET_8800; // bit 3 des drapeaux : ensemble $8800-$97FF (index non signé)
    ppu.render_frame(&vram, &oam);
    assert_eq!(ppu.framebuffer[15 * SCREEN_WIDTH + 32], PPU::shade(3)); // moitié gauche of the tuile 1 of $8800

    vram[TILE_SET_8800 + 0x10] = 0; // efface la première ligne de la tuile $8810 (MSB des pixels)
    vram[TILE_SET_8800 + 0x11] = 0; // ...et ses LSB : les pixels deviennent transparents
    ppu.render_frame(&vram, &oam);
    assert_eq!(ppu.framebuffer[15 * SCREEN_WIDTH + 32], PPU::shade(0)); // ligne Y-1 → transparente → fond white
}

#[test]
fn render_sprite_transparent_pixels() {
    let mut ppu = PPU::new();
    ppu.write_register(0xFF40, 0x83); // LCD allumé (bit 7) + fond activé (bit 0) + sprites activées (bit 1)
    ppu.bgp = 0xE4; // teinte v for a pixel value v

    let mut vram = [0u8; 0x2000];
    for row in 0..8 {
        vram[0x10 + 2 * row] = 0b11_11_00_00; // tuile 1 ($8010-$801F) : moitié gauche → valeur 3, moitié droite → valeur 0 (transparente) — MSB des pixels
        vram[0x11 + 2 * row] = 0b11_11_00_00; // LSB des pixels
    }

    let mut oam = [0u8; 0xA0];
    oam[0] = 16; // Y = 16 : lignes écran 15..22
    oam[1] = 32;
    oam[2] = 1;

    ppu.render_frame(&vram, &oam);
    assert_eq!(ppu.framebuffer[15 * SCREEN_WIDTH + 32], PPU::shade(3)); // moitié gauche : sprite visible (OBP0)
    assert_eq!(ppu.framebuffer[15 * SCREEN_WIDTH + 40], PPU::shade(0)); // moitié droite : transparente → fond white
}

#[test]
fn render_sprite_selects_obp_palette() {
    let mut ppu = PPU::new();
    ppu.write_register(0xFF40, 0x83); // LCD allumé (bit 7) + fond activé (bit 0) + sprites activées (bit 1)
    ppu.bgp = 0xE4; // teinte v for a pixel value v
    ppu.obp0 = 0xE4; // OBP0 : teinte v for a pixel value v
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
    ppu.write_register(0xFF40, 0x93); // LCD allumé (bit 7) + fond activé (bit 0) + sprites activées (bit 1), tuiles non signées $8000-$8FFF (bit 4 set)
    ppu.bgp = 0xE4; // teinte v for a pixel value v
    ppu.obp0 = 0xE4;

    let mut vram = [0u8; 0x2000];
    for row in 0..8 {
        vram[0x10 + 2 * row] = 0xFF; // tuile 1 : toutes the values of pixel valent 3 (fond noir)
        vram[0x11 + 2 * row] = 0xFF;
        vram[0x20 + 2 * row] = 0x00; // tuile 2 : toutes les valeurs de pixel valent 1 (sprite claire) — MSB des pixels à 0
        vram[0x21 + 2 * row] = 0xFF; // LSB des pixels à 1
    }
    vram[BG_MAP_9800..BG_MAP_9800 + 32 * 32].fill(1); // carte $9800-$9BFF : tuile 1 partout (fond noir)

    let mut oam = [0u8; 0xA0];
    oam[0] = 16; // Y = 16 : lignes écran 15..22
    oam[1] = 32; // X = 32 : colonnes 32..39
    oam[2] = 2; // tuile 2, mappée by OBP0

    ppu.render_frame(&vram, &oam);
    assert_eq!(ppu.framebuffer[15 * SCREEN_WIDTH + 32], PPU::shade(3)); // sans priorité : le fond noir masque the sprite
    assert_eq!(ppu.framebuffer[15 * SCREEN_WIDTH + 40], PPU::shade(3)); // hors of the sprite : fond noir

    oam[3] = SPRITE_PRIORITY; // bit 4 des drapeaux : le sprite is dessiné devant the fond
    ppu.render_frame(&vram, &oam);
    assert_eq!(ppu.framebuffer[15 * SCREEN_WIDTH + 32], PPU::shade(1)); // with priorité : the sprite (OBP0) passes devant

    ppu.lcdc &= !LCDC_BG; // fond éteint : les sprites sans priorité deviennent visibles on the base noire
    oam[3] = 0;
    ppu.render_frame(&vram, &oam);
    assert_eq!(ppu.framebuffer[15 * SCREEN_WIDTH + 32], PPU::shade(1)); // sprite visible (fond éteint)
}

#[test]
fn render_sprites_disabled_when_lcdc_bit_1_clear() {
    let mut ppu = PPU::new(); // LCDC = $91 : LCD allumé (bit 7) + fond activé (bit 0), sprites éteintes (bit 1 à 0), tuiles du fond non signées $8000-$8FFF (bit 4 set)
    ppu.bgp = 0xE4; // teinte v for a pixel value v
    ppu.obp0 = 0xE4;

    let mut vram = [0u8; 0x2000];
    for row in 0..8 {
        vram[0x10 + 2 * row] = 0xFF; // tuile 1 ($8010-$801F) : toutes the values of pixel valent 3
        vram[0x11 + 2 * row] = 0xFF;
    }

    let mut oam = [0u8; 0xA0];
    oam[0] = 16; // Y = 16 : lignes écran 15..22
    oam[1] = 32;
    oam[2] = 1;

    ppu.render_frame(&vram, &oam);
    assert_eq!(ppu.framebuffer[15 * SCREEN_WIDTH + 32], PPU::shade(0)); // bit 1 du LCDC à 0 : aucun sprite rendu → fond white

    ppu.lcdc |= LCDC_SPRITE_ON; // bit 1 du LCDC to 1
    ppu.render_frame(&vram, &oam);
    assert_eq!(ppu.framebuffer[15 * SCREEN_WIDTH + 32], PPU::shade(3)); // sprite rendu (OBP0)
}

#[test]
fn render_sprite_limit_of_ten_per_line() {
    let mut ppu = PPU::new();
    ppu.write_register(0xFF40, 0x83); // LCD allumé (bit 7) + fond activé (bit 0) + sprites activées (bit 1)
    ppu.bgp = 0xE4; // teinte v for a pixel value v
    ppu.obp0 = 0xE4;

    let mut vram = [0u8; 0x2000];
    for row in 0..8 {
        vram[0x10 + 2 * row] = 0xFF; // tuile 1 ($8010-$801F) : toutes the values of pixel valent 3
        vram[0x11 + 2 * row] = 0xFF;
    }

    let mut oam = [0u8; 0xA0];
    for i in 0..12 {
        oam[4 * i] = 16; // Y = 16 : toutes the entries recouvrent the line écran 15
        oam[4 * i + 1] = (i as u8) * 12; // X espacé of 12 columns : pas de chevauchement
        oam[4 * i + 2] = 1; // tuile 1
    }

    ppu.render_frame(&vram, &oam);
    assert_eq!(ppu.framebuffer[15 * SCREEN_WIDTH + 96], PPU::shade(3)); // entrée 8 (X = 96) : rendue
    assert_eq!(ppu.framebuffer[15 * SCREEN_WIDTH + 108], PPU::shade(3)); // entrée 9 (X = 108) : dernière rendue
    assert_eq!(ppu.framebuffer[15 * SCREEN_WIDTH + 120], PPU::shade(0)); // entrée 10 (X = 120) : supprimée by the limite of 10 → fond white
}

#[test]
fn render_bg_win_off_shows_black_base_and_objects() {
    let mut ppu = PPU::new();
    ppu.lcdc &= !LCDC_BG_WIN_ON; // bits 0 and 5 to 0 : background and window éteints
    ppu.lcdc |= LCDC_SPRITE_ON; // bit 1 to 1 : les objets restent activés, dessinés on the base noire
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
    oam[0] = 32; // Y = 32 : objet affiché on the lines écran 31..38 (Y-1)
    oam[1] = 40; // X = 40 : objet affiché on the columns écran 40..47
    oam[2] = 1; // tuile $8010-$801F
    ppu.render_frame(&vram, &oam);
    assert_eq!(ppu.framebuffer[31 * SCREEN_WIDTH + 40], PPU::shade(3)); // objet visible on the base noire
    assert_eq!(ppu.framebuffer[0], 0xFF00_0000); // le reste of l'écran : noir
}

#[test]
fn render_objects_16px_height_and_tile_selection() {
    let mut ppu = PPU::new();
    ppu.lcdc |= LCDC_OBJ_SIZE_16; // bit 2 du LCDC to 1 : objets 8×16 (PanDocs « OAM »)
    ppu.lcdc |= LCDC_SPRITE_ON; // bit 1 du LCDC to 1 : objets activés
    ppu.bgp = 0xE4; // teinte v for a pixel value v — le fond reste shade(0) (tuile nulle → valeur 0)
    ppu.obp0 = 0xE4;

    let mut vram = [0u8; 0x2000];
    let mut oam = [0u8; 0xA0];

    // Tuile 5 en $8050 : moitié gauche value 3, moitié droite value 0 ; tuile 4 ($8040) reste vide (value 0).
    for row in 0..8 {
        vram[0x0050 + 2 * row] = 0b11_11_00_00; // moitié gauche de la ligne `row` → value 3 (MSB des pixels)
        vram[0x0051 + 2 * row] = 0b11_11_00_00; // moitié droite → value 0 (LSB des pixels)
    }

    oam[0] = 32; // Y = 32 : objet affiché on the lines écran 16..31 (Y-16 .. Y-1)
    oam[1] = 32; // X = 32 : objet affiché on the columns écran 32..39
    oam[2] = 5; // tuile du haut « NN & $FE » = 4 → $8040 ; tuile du bas « NN | $01 » = 5 → $8050

    ppu.render_frame(&vram, &oam);
    assert_eq!(ppu.framebuffer[16 * SCREEN_WIDTH + 32], PPU::shade(0)); // moitié gauche of the tuile du haut (4) : vide → fond
    assert_eq!(ppu.framebuffer[24 * SCREEN_WIDTH + 32], PPU::shade(3)); // moitié gauche of the tuile du bas (5)
    assert_eq!(ppu.framebuffer[24 * SCREEN_WIDTH + 39], PPU::shade(0)); // right half of the tuile du bas : value 0 → fond
    assert_eq!(ppu.framebuffer[39 * SCREEN_WIDTH + 32], PPU::shade(0)); // ligne Y+7 : hors of the objet 8×16 (lignes Y-16..Y-1) → fond

    oam[3] = SPRITE_Y_FLIP; // bit 5 des drapeaux : retournement vertical — the two halves are swapped
    ppu.render_frame(&vram, &oam);
    assert_eq!(ppu.framebuffer[16 * SCREEN_WIDTH + 32], PPU::shade(3)); // moitié gauche of the tuile du bas (5) now at top
    assert_eq!(ppu.framebuffer[24 * SCREEN_WIDTH + 32], PPU::shade(0)); // moitié gauche of the tuile du haut (4) now at bottom : vide → fond
}

#[test]
fn render_objects_drawn_in_oam_order_later_overwrites_earlier() {
    let mut ppu = PPU::new();
    ppu.lcdc |= LCDC_SPRITE_ON; // sprites activées (bit 1 du LCDC)
    ppu.obp0 = 0xE4; // teinte v for a pixel value v
    ppu.obp1 = 0xE4;
    let mut vram = [0u8; 0x2000];
    let mut oam = [0u8; 0xA0];

    // Tuile 1 en $8010 : all pixels value 3 ; tuile 2 en $8020 : all pixels value 2.
    for row in 0..8 {
        vram[0x0010 + 2 * row] = 0b11_11_11_11; // tuile $8010 : MSB des pixels à 1
        vram[0x0011 + 2 * row] = 0b11_11_11_11; // LSB des pixels à 1 → valeur 3
        vram[0x0020 + 2 * row] = 0b10_10_10_10; // tuile $8020 : MSB des pixels à 1, LSB à 0 → valeur 2
        vram[0x0021 + 2 * row] = 0x00;
    }

    // L'objet B (X=48 → colonnes 48..55, tuile 2) is declared first dans l'OAM ; les sprites are
    // composés in the order OAM (PanDocs « Sprite »), donc the objet A déclaré ensuite (X=44 →
    // colonnes 44..51, tuile 3) écrase B on the zone of chevauchement (colonnes 48..51).
    oam[0] = 32; // Y = 32 : lignes écran 31..38 (Y-1)
    oam[1] = 48; // X = 48 : colonnes écran 48..55
    oam[2] = 2; // tuile $8020-$802F (value 2)

    oam[4] = 32; // Y = 32 : lignes écran 31..38
    oam[5] = 44; // X = 44 : colonnes écran 44..51 → chevauche B on the columns 48..51
    oam[6] = 1; // tuile $8010-$801F (value 3)

    ppu.render_frame(&vram, &oam);
    assert_eq!(ppu.framebuffer[31 * SCREEN_WIDTH + 46], PPU::shade(3)); // colonne of the objet A seul
    assert_eq!(ppu.framebuffer[31 * SCREEN_WIDTH + 50], PPU::shade(3)); // chevauchement : objet A (plus tardif in the OAM) devant
    assert_eq!(ppu.framebuffer[31 * SCREEN_WIDTH + 54], PPU::shade(2)); // colonne of the objet B seul
}

#[test]
fn step_tracks_ly_and_mode_per_line() {
    let mut ppu = PPU::new(); // ligne 0, mode_clock 0 : OAM Scan (LCD allumé)
    let vram = [0u8; 0x2000];
    let oam = [0u8; 0xA0];

    ppu.step(79, &vram, &oam); // mode_clock 1..79 : toujours in OAM Scan
    assert_eq!((ppu.mode_clock, ppu.mode, ppu.ly), (79, 2, 0));

    ppu.step(1, &vram, &oam); // mode_clock 80 : début of the Drawing
    assert_eq!((ppu.mode_clock, ppu.mode), (0, 3));

    ppu.step(176, &vram, &oam); // franchit the Drawing (172) : début of the HBlank with an overshoot of 4 dots
    assert_eq!((ppu.mode_clock, ppu.mode), (4, 0));

    ppu.step(199, &vram, &oam); // mode_clock 203 : dernier dot of the line 0 (HBlank)
    assert_eq!((ppu.mode_clock, ppu.ly), (203, 0));

    ppu.step(1, &vram, &oam); // début of the line 1 : OAM Scan à nouveau
    assert_eq!((ppu.mode_clock, ppu.mode, ppu.ly), (0, 2, 1));
}

#[test]
fn half_frame_is_at_line_77() {
    let mut ppu = PPU::new();
    let vram = [0u8; 0x2000];
    let oam = [0u8; 0xA0];

    // Demi-frame : exactly 77 lines × 456 dots.
    ppu.step(35_112, &vram, &oam);
    assert_eq!((ppu.mode_clock, ppu.ly), (0, 77));
}

#[test]
fn full_frame_returns_to_line_zero() {
    let mut ppu = PPU::new();
    let vram = [0u8; 0x2000];
    let oam = [0u8; 0xA0];

    assert!(ppu.step(FRAME_DOTS as u32, &vram, &oam)); // exactly one frame : frontière franchée
    assert_eq!((ppu.mode_clock, ppu.mode, ppu.ly), (0, 2, 0));
    assert!(!ppu.step(1, &vram, &oam)); // pas of new frame after a single dot
}

#[test]
fn vblank_spans_lines_144_to_153() {
    let mut ppu = PPU::new();
    let vram = [0u8; 0x2000];
    let oam = [0u8; 0xA0];

    ppu.step(VBLANK_START_DOT as u32, &vram, &oam); // début of the line 144 : le VBlank commence
    assert_eq!((ppu.mode_clock, ppu.mode, ppu.ly), (0, 1, 144));

    ppu.step(8 * DOTS_PER_LINE, &vram, &oam); // huit lines VBlank plus loin : début of the line 152
    assert_eq!((ppu.mode_clock, ppu.mode, ppu.ly), (0, 1, 152));

    ppu.step(DOTS_PER_LINE, &vram, &oam); // début of the line 153 : dernière line of the frame
    assert_eq!((ppu.mode_clock, ppu.mode, ppu.ly), (0, 1, 153));

    assert!(ppu.step(DOTS_PER_LINE, &vram, &oam)); // complète the line 153 and revient au début of the frame suivante
    assert_eq!((ppu.mode_clock, ppu.mode, ppu.ly), (0, 2, 0));
}

#[test]
fn lcd_off_freezes_the_ppu() {
    let mut ppu = PPU::new();
    let vram = [0u8; 0x2000];
    let oam = [0u8; 0xA0];

    ppu.write_register(0xFF40, 0x11); // LCD éteint (bit 7 to 0) : the transition on→off is gérée at the write of $FF40
    assert_eq!((ppu.mode_clock, ppu.mode, ppu.ly), (0, 0, 0)); // LY remis to 0, mode HBlank (Gekkio PDF §9) — immédiat au write

    ppu.step(10_000, &vram, &oam); // la PPU is gelée : aucun T-cycle consumed
    assert_eq!((ppu.mode_clock, ppu.mode, ppu.ly), (0, 0, 0));
    assert_eq!(ppu.take_interrupts(), 0);

    ppu.write_register(0xFF40, 0x91); // LCD rallumé : la PPU repart d'une frame complète depuis the line 0 (OAM Scan)
    ppu.step(DOTS_PER_LINE, &vram, &oam);
    assert_eq!((ppu.mode_clock, ppu.ly), (0, 1));
}

#[test]
fn vblank_interrupt_fires_on_entry() {
    let mut ppu = PPU::new();
    let vram = [0u8; 0x2000];
    let oam = [0u8; 0xA0];

    assert_eq!(ppu.take_interrupts(), 0); // rien in attente au power-on
    ppu.step(VBLANK_START_DOT as u32 - 1, &vram, &oam); // dernier dot of the line 143 (HBlank)
    assert_eq!((ppu.mode, ppu.ly), (0, 143));
    assert_eq!(ppu.take_interrupts(), 0); // pas encore in VBlank

    ppu.step(1, &vram, &oam); // début of the line 144 : le VBlank commence
    assert_eq!((ppu.ly, ppu.mode), (144, 1));
    assert_eq!(ppu.take_interrupts(), IRQ_VBLANK); // drapeau VBlank (bit 0 of IF) levé inconditionnellement

    ppu.step(3 * DOTS_PER_LINE, &vram, &oam); // reste in VBlank : pas of re-déclenchement
    assert_eq!(ppu.take_interrupts(), 0);
}

#[test]
fn vblank_interrupt_respects_its_enable_bit() {
    let vram = [0u8; 0x2000];
    let oam = [0u8; 0xA0];

    // Le drapeau VBlank (bit 0 of IF) is levé inconditionnellement à the entrée in VBlank, but the drapeau
    // STAT/LCD (bit 1 of IF) seulement if the interruption mode 1 is activée (bit 5 du STAT).
    let mut ppu = PPU::new();
    ppu.step(VBLANK_START_DOT as u32, &vram, &oam); // entre in VBlank sans bit d'activation posé
    assert_eq!(ppu.take_interrupts(), IRQ_VBLANK); // bit 0 levé inconditionnellement, bit 1 pas (STAT bit 5 to 0)

    let mut ppu = PPU::new();
    ppu.write_register(0xFF41, STAT_IRQ_VBLANK); // active the interruption mode 1 (bit 5 du STAT)
    ppu.step(VBLANK_START_DOT as u32, &vram, &oam); // entre in VBlank : les two drapeaux are levés
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
    // Le drapeau VBlank (bit 0) is levé à the entrée in VBlank ; le drapeau STAT (bit 1) pas encore (ly != lyc).
    assert_eq!(ppu.take_interrupts(), IRQ_VBLANK);

    ppu.step(1, &vram, &oam); // début of the line 145 : ly == lyc
    assert_eq!(ppu.ly, 145);
    assert_ne!(ppu.read_register(0xFF41) & (1 << 7), 0); // drapeau LYC==LY posé in bit 7 du STAT
    assert_ne!(ppu.take_interrupts() & IRQ_STAT, 0); // drapeau STAT (bit 1 of IF) levé

    ppu.step(DOTS_PER_LINE, &vram, &oam); // line 146 : ly != lyc à nouveau, pas of re-déclenchement
    assert_eq!(ppu.take_interrupts(), 0);
}

#[test]
fn mode_interrupts_fire_on_entry() {
    let mut ppu = PPU::new();
    let vram = [0u8; 0x2000];
    let oam = [0u8; 0xA0];

    // Interruptions mode 0 (HBlank) and mode 2 (OAM Scan) activées ; the mode 3 n'a not d'interruption.
    ppu.write_register(0xFF41, STAT_IRQ_MODE0 | STAT_IRQ_MODE2);

    ppu.step(MODE_OAM_CYCLES - 1, &vram, &oam); // toujours in OAM Scan of the line 0 (déjà entré au power-on)
    assert_eq!(ppu.take_interrupts(), 0); // pas of re-entrée : le drapeau n'est not levé à the power-on

    ppu.step(1, &vram, &oam); // dot 80 : début of the Drawing — the mode 3 n'a not d'interruption
    assert_eq!(ppu.take_interrupts(), 0);

    ppu.step(MODE_DRAW_CYCLES, &vram, &oam); // dot 256 : début of the HBlank (mode 0)
    assert_eq!((ppu.mode, ppu.ly), (0, 0));
    assert_ne!(ppu.take_interrupts() & IRQ_STAT, 0); // drapeau STAT levé à the entrée in mode 0

    ppu.step(MODE_HBLANK_CYCLES + MODE_OAM_CYCLES - 1, &vram, &oam); // fin of the line 0 and début of the line 1 : OAM Scan (mode 2)
    assert_eq!((ppu.mode_clock, ppu.mode), (MODE_OAM_CYCLES - 1, 2));
    assert_ne!(ppu.take_interrupts() & IRQ_STAT, 0); // drapeau STAT levé à the entrée in mode 2
}
