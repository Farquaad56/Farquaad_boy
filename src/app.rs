//! Application eframe : interface utilisateur et rendu.
//!
//! Mise en page professionnelle à 4 zones — barre de menu supérieure (chargement ROM + option
//! « ⏭ Skip Boot ROM », Run/Pause/Stop, menu « 🔍 Debug » activant les widgets de débogage), panneau gauche
//! « Output » (framebuffer PPU mis à l'échelle pour remplir la zone), zone centrale avec
//! Processor / ROM Info / IO Map activables depuis le menu Debug, panneau droit « Pattern
//! Table » (tuiles VRAM et banques ROM 0/1 colorées par palette BGP/OBP0/OBP1, grille optionnelle). Le mappage
//! clavier des touches du joypad est persisté entre les sessions (fichier bincode à côté de
//! l'exécutable).

use crate::cartridge::is_nintendo_logo_valid;
use crate::cpu::flags::Flags;
use crate::emulator::Emulator;
use crate::joypad::{KEY_A, KEY_B, KEY_DOWN, KEY_LEFT, KEY_RIGHT, KEY_SELECT, KEY_START, KEY_UP};
use crate::ppu::{self, SCREEN_HEIGHT, SCREEN_WIDTH};
use egui::{Color32, Visuals};
use serde::{Deserialize, Serialize};

/// État de l'émulation : Running (une frame par update UI), Paused (état gelé) ou Stopped
/// (réinitialisation power-on, la ROM chargée est conservée).
#[derive(Clone, Copy, Debug, PartialEq)]
enum EmulationState {
    /// Exécution en cours : une frame PPU par update UI (~60 FPS).
    Running,
    /// Exécution en pause : l'état de l'émulateur est gelé.
    Paused,
    /// Émulateur réinitialisé à l'état power-on (la ROM chargée est conservée).
    Stopped,
}

impl EmulationState {
    /// Libellé affiché dans la barre de menu et le panneau Output.
    fn label(self) -> &'static str {
        match self {
            EmulationState::Running => "RUNNING",
            EmulationState::Paused => "PAUSED",
            EmulationState::Stopped => "STOPPED",
        }
    }

    /// Couleur d'accent de l'état (vert = en cours, jaune = pause, rouge = stoppé).
    fn color(self) -> Color32 {
        match self {
            EmulationState::Running => Color32::GREEN,
            EmulationState::Paused => Color32::YELLOW,
            EmulationState::Stopped => Color32::RED,
        }
    }
}

/// Palette used to colorize the Pattern Table tiles (VRAM $8000-$9FFF, 512 tiles).
/// BCP0/OCP0/OCP1 read the hardware registers live (BGP $FF47 / OBP0 $FF48 / OBP1 $FF49);
/// the other slots are fixed preview palettes (no hardware source) to inspect the tiles under
/// different background/object colorizations.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum TilePalette {
    /// BCP0: live BGP register ($FF47) - background/window palette.
    Bcp0,
    /// BCP1..BCP7: background preview palettes (no hardware source).
    Bcp1,
    Bcp2,
    Bcp3,
    Bcp4,
    Bcp5,
    Bcp6,
    Bcp7,
    /// OCP0: live OBP0 register ($FF48) - sprite 0 palette (default).
    Ocp0,
    /// OCP1: live OBP1 register ($FF49) - sprite 1 palette.
    Ocp1,
    /// OCP2..OCP7: object preview palettes (no hardware source).
    Ocp2,
    Ocp3,
    Ocp4,
    Ocp5,
    Ocp6,
    Ocp7,
}

impl TilePalette {
    /// All palettes, in display order of the selector (BCP0..BCP7 then OCP0..OCP7).
    const ALL: [TilePalette; 16] = [
        Self::Bcp0, Self::Bcp1, Self::Bcp2, Self::Bcp3, Self::Bcp4, Self::Bcp5, Self::Bcp6, Self::Bcp7,
        Self::Ocp0, Self::Ocp1, Self::Ocp2, Self::Ocp3, Self::Ocp4, Self::Ocp5, Self::Ocp6, Self::Ocp7,
    ];

    /// Label displayed in the palette selector.
    fn label(self) -> &'static str {
        match self {
            Self::Bcp0 => "BCP0 - BGP $FF47",
            Self::Bcp1 => "BCP1 - classic (v0->v3 light->dark)",
            Self::Bcp2 => "BCP2 - inverted (v0->v3 dark->light)",
            Self::Bcp3 => "BCP3 - two-tone (BGP power-on)",
            Self::Bcp4 => "BCP4 - two-tone inverted",
            Self::Bcp5 => "BCP5 - outline",
            Self::Bcp6 => "BCP6 - X-ray",
            Self::Bcp7 => "BCP7 - medium contrast",
            Self::Ocp0 => "OCP0 - OBP0 $FF48",
            Self::Ocp1 => "OCP1 - OBP1 $FF49",
            Self::Ocp2 => "OCP2 - all dark (OBP power-on)",
            Self::Ocp3 => "OCP3 - all light",
            Self::Ocp4 => "OCP4 - outline",
            Self::Ocp5 => "OCP5 - medium contrast",
            Self::Ocp6 => "OCP6 - classic (v0->v3 light->dark)",
            Self::Ocp7 => "OCP7 - inverted (v0->v3 dark->light)",
        }
    }

    /// Resolves the palette to an 8-bit byte: live hardware register for BCP0/OCP0/OCP1, fixed preview
    /// value otherwise. Each pair of bits selects the DMG shade of pixels with value 0..3
    /// (bits 7-6 -> v0, 5-4 -> v1, 3-2 -> v2, 1-0 -> v3).
    fn resolve(self, ppu: &crate::ppu::PPU) -> u8 {
        match self {
            Self::Bcp0 => ppu.bgp,
            Self::Ocp0 => ppu.obp0,
            Self::Ocp1 => ppu.obp1,
            // Preview palettes (no hardware source): DMG shades for v0..v3.
            Self::Bcp1 => 0x0B, // classic: v0 light -> v3 dark
            Self::Bcp2 => 0xF4, // inverted: v0 dark -> v3 light
            Self::Bcp3 => 0xFC, // two-tone (power-on BGP): v0,v1 dark; v2,v3 light
            Self::Bcp4 => 0x03, // two-tone inverted: v0,v1 light; v2,v3 dark
            Self::Bcp5 => 0x13, // outline: light / medium / medium / dark
            Self::Bcp6 => 0xC0, // X-ray: only v3 visible (dark)
            Self::Bcp7 => 0x5A, // medium contrast: v0,v1 = shade 1; v2,v3 = shade 2
            Self::Ocp2 => 0xFF, // all dark (power-on OBP)
            Self::Ocp3 => 0x00, // all light
            Self::Ocp4 => 0x13, // outline: light / medium / medium / dark
            Self::Ocp5 => 0x5A, // medium contrast: v0,v1 = shade 1; v2,v3 = shade 2
            Self::Ocp6 => 0x0B, // classic: v0 light -> v3 dark
            Self::Ocp7 => 0xF4, // inverted: v0 dark -> v3 light
        }
    }
}

/// Tile size in pixels (8x8).
const TILE_SIZE: usize = 8;
/// Number of tiles displayed: VRAM $8000-$9FFF (512 tiles).
const TILE_COUNT: usize = 512;
/// Pattern Table grid: 32 columns x 16 rows of tiles.
const TILE_COLS: usize = 32;
const TILE_ROWS: usize = 16;
/// Pattern Table image size in pixels for the VRAM view (32x8 wide, 16x8 tall).
const TILE_IMAGE_W: usize = TILE_COLS * TILE_SIZE; // 256
const TILE_IMAGE_H: usize = TILE_ROWS * TILE_SIZE; // 128
/// Number of tiles in a ROM bank view: one bank = 16 KiB (`crate::mbc::ROM_BANK_SIZE`) = 2048 tiles of 8x8.
const BANK_TILE_COUNT: usize = crate::mbc::ROM_BANK_SIZE / (TILE_SIZE * TILE_SIZE); // 2048
/// Pattern Table grid for the ROM bank views: 64 columns x 32 rows of tiles.
const BANK_TILE_COLS: usize = 64;
const BANK_TILE_ROWS: usize = 32;
/// Largest Pattern Table image in pixels (ROM bank views): 512 wide, 256 tall — the pixel buffer is sized for it.
const TILE_BUFFER_W: usize = BANK_TILE_COLS * TILE_SIZE; // 512
const TILE_BUFFER_H: usize = BANK_TILE_ROWS * TILE_SIZE; // 256

/// View of the Pattern Table panel: VRAM tiles or raw ROM banks rendered as tile grids.
#[derive(Clone, Copy, Debug, PartialEq)]
enum PatternView {
    /// VRAM $8000-$9FFF (512 tiles 8x8).
    Vram,
    /// ROM bank 0: cartridge bytes $0000-$3FFF (2048 tiles 8x8).
    Bank0,
    /// ROM bank 1: cartridge bytes $4000-$7FFF (2048 tiles 8x8).
    Bank1,
}

impl PatternView {
    /// All views, in display order.
    const ALL: [PatternView; 3] = [Self::Vram, Self::Bank0, Self::Bank1];

    /// Short label for the view selector.
    fn short_label(self) -> &'static str {
        match self {
            Self::Vram => "VRAM",
            Self::Bank0 => "Bank 0",
            Self::Bank1 => "Bank 1",
        }
    }

    /// Full label with the address range.
    fn label(self) -> &'static str {
        match self {
            Self::Vram => "VRAM $8000-$9FFF (512 tuiles 8x8)",
            Self::Bank0 => "ROM Bank 0 — $0000-$3FFF (2048 tuiles 8x8)",
            Self::Bank1 => "ROM Bank 1 — $4000-$7FFF (2048 tuiles 8x8)",
        }
    }

    /// Number of tiles in the view.
    fn tile_count(self) -> usize {
        match self {
            Self::Vram => TILE_COUNT,
            Self::Bank0 | Self::Bank1 => BANK_TILE_COUNT,
        }
    }

    /// Grid dimensions (columns x rows of tiles).
    fn grid(self) -> (usize, usize) {
        match self {
            Self::Vram => (TILE_COLS, TILE_ROWS),
            Self::Bank0 | Self::Bank1 => (BANK_TILE_COLS, BANK_TILE_ROWS),
        }
    }

    /// Image size in pixels for the view (VRAM 256x128, ROM banks 512x256).
    fn image_size(self) -> (usize, usize) {
        match self {
            Self::Vram => (TILE_IMAGE_W, TILE_IMAGE_H),
            Self::Bank0 | Self::Bank1 => (TILE_BUFFER_W, TILE_BUFFER_H),
        }
    }
}

/// Tabs of the central debug zone: one view at a time (no more widgets stacked side by side).
#[derive(Clone, Copy, Debug, PartialEq)]
enum DebugTab {
    /// CPU registers (flags Z/N/H/C, PC/SP, A/B/C/D/E/H/L + 16-bit combos, IME/HALT/EI delay).
    Cpu,
    /// Cartridge header (title, MBC type, sizes, CGB/SGB flags, versions and checksums).
    RomInfo,
    /// IO map ($FF00-$FF7F): address, register name and live value.
    IoMap,
    /// Memory editor: 7 sub-tabs (ROM0/ROM1/VRAM/WRAM/OAM/IO/HIRAM).
    Memory,
}

impl DebugTab {
    /// All tabs, in display order.
    const ALL: [DebugTab; 4] = [Self::Cpu, Self::RomInfo, Self::IoMap, Self::Memory];

    /// Tab label.
    fn label(self) -> &'static str {
        match self {
            Self::Cpu => "CPU",
            Self::RomInfo => "ROM Info",
            Self::IoMap => "IO Map",
            Self::Memory => "Memory",
        }
    }
}

/// Memory regions of the editor (sub-tabs) - PanDocs "Memory Map" nomenclature.
#[derive(Clone, Copy, Debug, PartialEq)]
enum MemoryRegion {
    /// ROM bank 0 ($0000-$3FFF).
    Rom0,
    /// ROM bank 1+ ($4000-$7FFF) - routed through the MBC controller.
    Rom1,
    /// Video RAM ($8000-$9FFF).
    Vram,
    /// Work RAM ($C000-$DFFF).
    Wram,
    /// Object Attribute Memory ($FE00-$FE9F).
    Oam,
    /// I/O registers ($FF00-$FF7F).
    Io,
    /// High RAM ($FF80-$FFFE).
    Hiram,
}

impl MemoryRegion {
    /// All sub-tabs, in display order.
    const ALL: [MemoryRegion; 7] = [
        Self::Rom0, Self::Rom1, Self::Vram, Self::Wram, Self::Oam, Self::Io, Self::Hiram,
    ];

    /// Sub-tab label.
    fn label(self) -> &'static str {
        match self {
            Self::Rom0 => "ROM0",
            Self::Rom1 => "ROM1",
            Self::Vram => "VRAM",
            Self::Wram => "WRAM",
            Self::Oam => "OAM",
            Self::Io => "IO",
            Self::Hiram => "HIRAM",
        }
    }

    /// Address range of the region: (base address, number of bytes).
    fn range(self) -> (u16, usize) {
        match self {
            Self::Rom0 => (0x0000, 0x4000), // $0000-$3FFF
            Self::Rom1 => (0x4000, 0x4000), // $4000-$7FFF
            Self::Vram => (0x8000, 0x2000), // $8000-$9FFF
            Self::Wram => (0xC000, 0x2000), // $C000-$DFFF
            Self::Oam => (0xFE00, 0xA0),     // $FE00-$FE9F
            Self::Io => (0xFF00, 0x80),      // $FF00-$FF7F
            Self::Hiram => (0xFF80, 0x7F),   // $FF80-$FFFE
        }
    }
}

/// Nom du registre IO à l'adresse $FF00-$FF7F (nomenclature PanDocs, alignée sur les modules
/// `timer`/`serial`/`ppu` de ce projet) ; `None` pour les adresses non documentées.
fn io_register_name(offset: u16) -> Option<&'static str> {
    match offset & 0x7F {
        0x00 => Some("P1 (JOYPAD)"),
        0x01 => Some("SB (SERIAL DATA)"),
        0x02 => Some("SC (SERIAL CONTROL)"),
        0x04 => Some("DIV (DIVIDER)"),
        0x05 => Some("TIMA (TIMER COUNTER)"),
        0x06 => Some("TMA (TIMER MODULO)"),
        0x07 => Some("TAC (TIMER CONTROL)"),
        0x08 => Some("CHAN_A (SOUND CH.1)"),
        0x09 => Some("CHAN_B (SOUND CH.2)"),
        0x0A => Some("CHAN_C (SOUND CH.3)"),
        0x0B => Some("CHAN_D (SOUND CH.4)"),
        0x0F => Some("IF (INTERRUPT FLAG)"),
        0x10 => Some("SWR1 (WAVE FORM DATA)"),
        0x11 => Some("SWR2 (WAVE FORM DATA)"),
        0x12 => Some("SWR3 (WAVE FORM DATA)"),
        0x13 => Some("SWR4 (WAVE FORM DATA)"),
        0x14 => Some("NSSO (NOISE SHIFT STEP OUT)"),
        0x15 => Some("WAVE (WAVE CONTROL)"),
        0x17 => Some("C1ST (SOUND CH.1 STATUS)"),
        0x18 => Some("C1SV (SOUND CH.1 VOLUME ENVELOPE)"),
        0x40 => Some("LCDC (LCD CONTROL)"),
        0x41 => Some("STAT (PPU STATUS)"),
        0x42 => Some("SCY (BG Y-SCROLL)"),
        0x43 => Some("SCX (BG X-SCROLL)"),
        0x44 => Some("LY (SCANLINE COUNTER)"),
        0x45 => Some("LYC (LINE COMPARE)"),
        0x46 => Some("DMA (OAM TRANSFER)"),
        0x47 => Some("BGP (BG PALETTE)"),
        0x48 => Some("OBP0 (SPRITE PALETTE 0)"),
        0x49 => Some("OBP1 (SPRITE PALETTE 1)"),
        0x4A => Some("WX (WINDOW X-POSITION)"),
        0x4B => Some("WY (WINDOW Y-POSITION)"),
        _ => None,
    }
}

/// Parses a hex byte from user input ("FF", "ff", "$FF" or "0xFF"); `None` if invalid or out of range.
fn parse_hex_u8(input: &str) -> Option<u8> {
    let s = input.trim();
    let s = s.strip_prefix('$').unwrap_or(s);
    let s = s.strip_prefix("0x").or_else(|| s.strip_prefix("0X")).unwrap_or(s);
    u8::from_str_radix(s, 16).ok()
}

/// Parses a hex 16-bit address from user input ("8000", "$8000" or "0x8000"); `None` if invalid.
fn parse_hex_u16(input: &str) -> Option<u16> {
    let s = input.trim();
    let s = s.strip_prefix('$').unwrap_or(s);
    let s = s.strip_prefix("0x").or_else(|| s.strip_prefix("0X")).unwrap_or(s);
    u16::from_str_radix(s, 16).ok()
}

/// Mappage clavier persisté entre les sessions (bincode, fichier à côté de l'exécutable).
#[derive(Serialize, Deserialize)]
struct PersistedInput {
    /// Version du format (permet d'ignorer les fichiers obsolètes).
    version: u32,
    /// Mappage des huit boutons du joypad (`None` = non mappé), dans l'ordre de `JOYPAD_KEY_INDICES`.
    key_map: [Option<egui::Key>; 8],
}

/// État de l'application FarquaadGB.
pub struct FarquaadGBApp {
    emulator: Emulator,
    /// Nom du fichier ROM chargé (pour affichage).
    rom_name: Option<String>,
    /// État d'émulation : Running / Paused / Stopped.
    state: EmulationState,
    /// Skip the boot ROM on load: start directly at $0100 (post-boot state) instead of running the real DMG
    /// boot ROM — indispensable for homebrews and debugging (toggled from the menu bar). Also set automatically
    /// when the cartridge's Nintendo logo ($0104-$0133) is invalid: on the matériel réel, la boot ROM se verrouillerait en boucle infinie.
    skip_boot_rom: bool,
    /// Avertissement affiché dans la barre de menu quand la cartouche chargée a un logo Nintendo invalide ($0104-$0133) :
    /// la boot ROM réelle se verrouillerait en boucle infinie (écran noir), donc le bypass est activé automatiquement.
    boot_warning: Option<String>,
    /// Active tab of the central debug zone (CPU / ROM Info / IO Map / Memory).
    debug_tab: DebugTab,
    /// Active sub-tab of the memory editor (ROM0/ROM1/VRAM/WRAM/OAM/IO/HIRAM).
    memory_region: MemoryRegion,
    show_pattern_table: bool, // panneau droit « Pattern Table » (tuiles VRAM / banques ROM 0-1)
    /// Active view of the Pattern Table panel (VRAM $8000-$9FFF / ROM bank 0 / ROM bank 1).
    pattern_view: PatternView,
    /// Fenêtre « ⌨️ Input Mapping » (mappage clavier → joypad $FF00).
    show_input_mapping: bool,
    /// Fenêtre « 🔌 Serial Monitor » : sortie du port série (SB/SC + transcript) pour le debug des ROMs de test.
    show_serial_monitor: bool,
    /// Cache du transcript série rendu en texte (régénéré uniquement quand un nouvel octet est capturé).
    serial_text_cache: String,
    /// Nombre d'octets contenus dans `serial_text_cache` (détection des nouveaux octets capturés).
    serial_cached_len: usize,
    /// Mappage clavier des huit boutons du joypad, dans the order of the indices
    /// (`crate::joypad::KEY_A`…`crate::joypad::KEY_DOWN`) ; `None` = non mappé.
    key_map: [Option<egui::Key>; 8],
    /// Bouton du joypad en cours de mappage (en attente d'un appui clavier) ; `None` = pas de capture.
    capturing_button: Option<usize>,
    /// Texture écran (framebuffer PPU, NEAREST pour le pixel art).
    texture: Option<egui::TextureHandle>,
    /// Texture Pattern Table (VRAM or ROM bank tiles colorées, NEAREST).
    tile_texture: Option<egui::TextureHandle>,
    /// Reusable pixel buffer for the Pattern Table (TILE_BUFFER_W x TILE_BUFFER_H u32, R in the lowest byte - egui/bytemuck compatible);
    /// only the first `width * height` pixels of the active view are used.
    tile_pixels: Vec<u32>,
    /// Palette de colorisation du Pattern Table.
    tile_palette: TilePalette,
    /// Affichage de la grille 8×8 entre les tuiles du Pattern Table.
    show_tile_grid: bool,
    /// Memory editor: hex address field for writing a byte (e.g. "8000" or "$8000").
    mem_edit_addr: String,
    /// Memory editor: hex value field for writing a byte (e.g. "FF").
    mem_edit_value: String,
}
impl FarquaadGBApp {
    /// Crée l'application et configure le contexte egui (thème sombre, fond #1a1a1a).
    /// Si `initial_rom` est fourni (argument de ligne de commande), la ROM est chargée au démarrage.
    pub fn new(cc: &eframe::CreationContext<'_>, initial_rom: Option<String>) -> Self {
        let mut visuals = Visuals::dark();
        visuals.window_fill = Color32::from_rgb(26, 26, 26); // #1a1a1a (fond de la fenêtre)
        cc.egui_ctx.set_style(egui::Style {
            visuals,
            ..Default::default()
        });

        let mut this = Self {
            // Séquence de démarrage par défaut : la boot ROM (rom/Boot_room.gb, repli embarqué) est mappée à
            // $0000-$00FF et le CPU démarre à $0000 — comme sur le matériel réel. Sans cartouche valide, elle
            // échoue sa vérification du logo et tourne en boucle infinie (écran noir), comme une console vide.
            emulator: Emulator::new_with_boot(),
            rom_name: None,
            state: EmulationState::Stopped,
            // Default : la séquence de boot réelle est exécutée au chargement (case « ⏭ Skip Boot ROM » pour la sauter).
            skip_boot_rom: false,
            // Aucun avertissement pour l'instant : posé quand la cartouche chargée a un logo Nintendo invalide.
            boot_warning: None,
            // Central debug zone: tabs CPU / ROM Info / IO Map / Memory (one at a time).
            debug_tab: DebugTab::Cpu,
            memory_region: MemoryRegion::Rom0,
            show_pattern_table: false,
            pattern_view: PatternView::Vram,
            show_input_mapping: false,
            // Vue épurée par défaut : la fenêtre Serial Monitor se réactive depuis the menu « 🔍 Debug ».
            show_serial_monitor: false,
            serial_text_cache: String::new(),
            serial_cached_len: 0,
            key_map: [None; 8],
            capturing_button: None,
            texture: None,
            tile_texture: None,
            tile_pixels: vec![0xFF00_0000; TILE_BUFFER_W * TILE_BUFFER_H],
            tile_palette: TilePalette::Bcp0,
            show_tile_grid: true,
            mem_edit_addr: String::from("8000"),
            mem_edit_value: String::from("FF"),
        };

        // Chargement automatique depuis la ligne de commande (ex. `cargo run -- assets/cpu_instrs.gb`).
        if let Some(path) = initial_rom {
            this.load_rom_file(std::path::Path::new(&path));
        }

        // Restauration du mappage clavier persisté lors des sessions précédentes.
        this.load_input_config();
        this
    }

    /// Ouvre une boîte de dialogue et charge la ROM sélectionnée dans l'émulateur.
    fn load_rom(&mut self) {
        if let Some(path) = rfd::FileDialog::new()
            .add_filter("Game Boy ROM", &["gb", "gbc"])
            .pick_file()
        {
            self.load_rom_file(&path);
        }
    }

    /// Charge une ROM depuis the disque dans l'émulateur (partagée entre la boîte de dialogue
    /// et the argument de ligne de commande). Par défaut, la séquence de démarrage est la boot ROM réelle :
    /// elle s'exécute d'abord à $0000, valide le logo de la cartouche puis dé-mappe via rBANK ($FF50) avant
    /// que le jeu ne démarre à $0100. Si « ⏭ Skip Boot ROM » est cochée, the démarrage saute cette séquence :
    /// the système démarre directement à l'état post-boot (PC = $0100), indispensable pour les homebrews et le débogage.
    /// Quand le logo Nintendo de la cartouche ($0104-$0133) est invalide — ou que la ROM est trop courte pour
    /// contenir un en-tête complet —, la boot ROM réelle se verrouillerait en boucle infinie sur the matériel réel :
    /// the bypass est alors activé automatiquement (`skip_boot_rom`) et un avertissement est affiché dans la barre de menu.
    fn load_rom_file(&mut self, path: &std::path::Path) {
        match std::fs::read(path) {
            Ok(data) => {
                // Logo Nintendo invalide ($0104-$0133), ou ROM trop courte pour un en-tête complet ? Sur the matériel réel,
                // la boot ROM se verrouillerait en boucle infinie (écran noir) : on active automatiquement the bypass.
                let logo_valid = is_nintendo_logo_valid(&data);
                self.boot_warning = None; // efface l'avertissement du chargement précédent
                if !self.skip_boot_rom && !logo_valid {
                    log::warn!(
                        "⚠️ Logo Nintendo invalide ($0104-$0133) : la Boot ROM bouclerait à l'infini — bypass activé automatiquement."
                    );
                    self.skip_boot_rom = true; // Activation automatique du bypass
                    self.boot_warning = Some(
                        "⚠️ Logo Nintendo invalide ($0104-$0133) : Boot ROM sautée automatiquement".to_string(),
                    );
                }
                if self.skip_boot_rom {
                    // Démarre directement à l'état post-boot ROM (PC = $0100).
                    self.emulator.load_rom(data);
                } else {
                    // Exécute la séquence de boot réelle.
                    self.emulator.load_rom_with_boot(data);
                }
                self.rom_name = path.file_name().and_then(|n| n.to_str()).map(String::from);
                self.state = EmulationState::Running; // démarrage immédiat (séquence de boot + jeu, ou direct à $0100)
                log::info!(
                    "ROM chargée : {:?}{}",
                    path,
                    if self.boot_warning.is_some() {
                        " (Boot ROM sautée — logo Nintendo invalide)"
                    } else if self.skip_boot_rom {
                        " (Boot ROM sautée)"
                    } else {
                        ""
                    }
                );
            }
            Err(err) => log::error!("Échec de la lecture de {:?} : {err}", path),
        }
    }

    /// Version du format de configuration des touches (à augmenter si la structure change).
    const INPUT_CONFIG_VERSION: u32 = 1;

    /// Chemin du fichier de configuration des touches : `farquaadgb_input.bin` à côté de l'exécutable.
    fn input_config_path() -> Option<std::path::PathBuf> {
        std::env::current_exe()
            .ok()?
            .parent()
            .map(|dir| dir.join("farquaadgb_input.bin"))
    }

    /// Restaure le mappage clavier depuis le disque (silencieux si le fichier est absent ou corrompu).
    fn load_input_config(&mut self) {
        let path = match Self::input_config_path() {
            Some(path) => path,
            None => return,
        };
        // Pas de fichier (premier lancement) : on garde le mappage par défaut.
        let Ok(bytes) = std::fs::read(&path) else {
            return;
        };
        // Fichier corrompu ou format inconnu : on garde le mappage courant.
        let Ok(config) = bincode::deserialize::<PersistedInput>(&bytes) else {
            return;
        };
        if config.version == Self::INPUT_CONFIG_VERSION {
            self.key_map = config.key_map;
            log::info!("Mappage clavier restauré depuis {:?}", path);
        }
    }

    /// Sauvegarde le mappage clavier sur le disque (appelé à chaque modification du mappage).
    fn save_input_config(&self) {
        let path = match Self::input_config_path() {
            Some(path) => path,
            None => return,
        };
        let config = PersistedInput {
            version: Self::INPUT_CONFIG_VERSION,
            key_map: self.key_map,
        };
        match bincode::serialize(&config) {
            Ok(bytes) => {
                if let Err(err) = std::fs::write(&path, bytes) {
                    log::error!(
                        "Échec de la sauvegarde du mappage clavier {:?} : {err}",
                        path
                    );
                }
            }
            Err(err) => log::error!("Échec de la sérialisation du mappage clavier : {err}"),
        }
    }

    /// Lit le titre du jeu (16 octets) dans l'en-tête cartouche à 0x0134.
    fn rom_title(&self) -> String {
        (0..16u16)
            .map(|i| self.emulator.mmu.read(0x0134 + i))
            .map(|b| {
                if (0x20..=0x7E).contains(&b) {
                    b as char
                } else {
                    ' '
                }
            })
            .collect::<String>()
            .trim_end()
            .to_string()
    }

    /// Reconstruit la texture écran : upload zero-copy du framebuffer PPU (bytemuck, NEAREST pour le pixel art).
    fn update_screen_texture(&mut self, ctx: &egui::Context) {
        let image = egui::ColorImage::from_rgba_unmultiplied(
            [SCREEN_WIDTH, SCREEN_HEIGHT],
            bytemuck::cast_slice(&self.emulator.mmu.ppu.framebuffer),
        );
        match &mut self.texture {
            Some(texture) => texture.set(image, egui::TextureOptions::NEAREST),
            None => {
                self.texture =
                    Some(ctx.load_texture("gb_screen", image, egui::TextureOptions::NEAREST))
            }
        }
    }

    /// Rebuilds the Pattern Table texture for the active view (VRAM $8000-$9FFF or raw ROM bank 0/1):
    /// tiles colored with the selected palette (BCP/OCP - live register or fixed preview), optional grid.
    fn update_tile_texture(&mut self, ctx: &egui::Context) {
        let view = self.pattern_view;
        let tile_count = view.tile_count();
        let (cols, rows) = view.grid();
        let width = cols * TILE_SIZE;
        let height = rows * TILE_SIZE;

        // Octet de palette : chaque paire de bits sélectionne la teinte DMG des pixels de valeur 0..3.
        let ppu = &self.emulator.mmu.ppu;
        let palette_byte = self.tile_palette.resolve(ppu);
        let colors = [0u8, 1, 2, 3].map(|index| ppu::PPU::shade((palette_byte >> (6 - 2 * index)) & 3));

        // Remplissage du tampon : tuile par tuile, ligne par ligne (deux octets de 8 bits chacune).
        let vram = &self.emulator.mmu.vram;
        let pixels = &mut self.tile_pixels[..width * height];
        for tile in 0..tile_count {
            // Flat offset of the first byte of the tile: VRAM from $8000, ROM banks from the start of the file.
            let base = match view {
                PatternView::Vram => tile * TILE_SIZE * 2,
                PatternView::Bank0 => tile * TILE_SIZE * 2,
                PatternView::Bank1 => crate::mbc::ROM_BANK_SIZE + tile * TILE_SIZE * 2,
            };
            for row in 0..TILE_SIZE {
                let (b0, b1) = match view {
                    PatternView::Vram => (vram[base + row * 2], vram[base + row * 2 + 1]),
                    _ => (
                        self.emulator.mmu.rom_raw(base + row * 2),
                        self.emulator.mmu.rom_raw(base + row * 2 + 1),
                    ),
                };
                let py = (tile / cols) * TILE_SIZE + row; // tile position in the grid
                let px = (tile % cols) * TILE_SIZE; // tile position in the grid
                for col in 0..TILE_SIZE {
                    // Bits 7→0 de chaque octet : b0 contient les MSB des pixels, b1 les LSB (index = 2×MSB + LSB).
                    let bit = 7 - col;
                    let shade_index = (((b0 >> bit) & 1) << 1) | ((b1 >> bit) & 1);
                    pixels[py * width + px + col] = colors[shade_index as usize];
                }
            }
        }

        // Grille optionnelle : lignes sombres toutes les 8 px entre les tuiles.
        if self.show_tile_grid {
            let grid_color: u32 = 0xFF_5A_5A_5A;
            // Horizontal grid lines: one dark line at each tile row (y = 0, 8, ... < height).
            for i in (0..height).step_by(TILE_SIZE) {
                for j in 0..width {
                    pixels[i * width + j] = grid_color; // ligne horizontale
                }
            }
            // Vertical grid lines: one dark column at each tile column (x = 0, 8, ... < width).
            for i in (0..width).step_by(TILE_SIZE) {
                for j in 0..height {
                    pixels[j * width + i] = grid_color; // colonne verticale
                }
            }
        }

        let image = egui::ColorImage::from_rgba_unmultiplied(
            [width, height],
            bytemuck::cast_slice(&self.tile_pixels[..width * height]),
        );
        match &mut self.tile_texture {
            Some(texture) => texture.set(image, egui::TextureOptions::NEAREST),
            None => {
                self.tile_texture =
                    Some(ctx.load_texture("gb_pattern_table", image, egui::TextureOptions::NEAREST))
            }
        }
    }
    /// Barre de menu supérieure : chargement ROM (+ option « ⏭ Skip Boot ROM »), Run/Pause/Stop (colorés selon l'état),
    /// menu « 🔍 Debug » (activation des widgets de débogage) et libellé de la ROM, plus un avertissement quand le logo
    /// Nintendo de la cartouche est invalide (bypass de la Boot ROM activé automatiquement).
    fn show_menu_bar(&mut self, ctx: &egui::Context) {
        egui::TopBottomPanel::top("menu").show(ctx, |ui| {
            egui::menu::bar(ui, |ui| {
                if ui.button("📂 Ouvrir ROM").clicked() {
                    self.load_rom();
                }
                // ⏭ Skip Boot ROM : au chargement, démarre directement à $0100 (état post-boot) au lieu de
                // la séquence de boot réelle — indispensable pour les homebrews et le débogage.
                ui.checkbox(&mut self.skip_boot_rom, "⏭ Skip Boot ROM");

                ui.separator();

                // ▶ Run / ⏸ Pause / ⏹ Stop : l'état actif est mis en avant.
                let idle = Color32::from_rgb(170, 170, 170);
                if ui
                    .button(egui::RichText::new("▶ Run").color(
                        if self.state == EmulationState::Running {
                            Color32::GREEN
                        } else {
                            idle
                        },
                    ))
                    .clicked()
                {
                    self.state = EmulationState::Running; // démarre ou reprend l'exécution (une frame par update UI)
                }
                if ui
                    .button(egui::RichText::new("⏸ Pause").color(
                        if self.state == EmulationState::Paused {
                            Color32::YELLOW
                        } else {
                            idle
                        },
                    ))
                    .clicked()
                {
                    self.state = EmulationState::Paused; // gèle l'état de l'émulateur
                }
                if ui.button(egui::RichText::new("⏹ Stop")).clicked() {
                    // Retour à l'état power-on post-boot ROM (la ROM chargée est conservée).
                    self.emulator.reset();
                    self.state = EmulationState::Stopped;
                }
                if ui.button(egui::RichText::new("🔄 Reset CPU")).clicked() {
                    // Re-exécute depuis $0100 avec les registres post-boot ; la mémoire et les I/O sont conservés.
                    self.emulator.reset_cpu();
                }

                // État d'émulation (pastille colorée + libellé).
                ui.add_space(8.0);
                let state = self.state;
                ui.colored_label(state.color(), format!("● {}", state.label()));

                ui.separator();
                // Menu « 🔍 Debug » : activation des panneaux/fenêtres de débogage. Les vues CPU / ROM Info /
                // IO Map / Memory sont toujours disponibles en onglets dans la zone centrale.
                ui.menu_button("🔍 Debug", |ui| {
                    ui.checkbox(&mut self.show_pattern_table, "Pattern Table");
                    ui.separator();
                    ui.checkbox(&mut self.show_input_mapping, "Input Mapping");
                    ui.separator();
                    // Fenêtre « 🔌 Serial Monitor » : sortie du port série pour le debug des ROMs de test.
                    ui.checkbox(&mut self.show_serial_monitor, "Serial Monitor");
                });
                ui.separator();
                match &self.rom_name {
                    Some(name) => {
                        let title = self.rom_title();
                        let size_kib = self.emulator.mmu.rom_size() / 1024;
                        if title.is_empty() {
                            ui.label(format!("{name} ({size_kib} KiB)"));
                        } else {
                            ui.label(format!("{name} ({size_kib} KiB) — « {title} »"));
                        }
                    }
                    None => {
                        ui.weak("Aucune ROM chargée");
                    }
                }
                // Avertissement : logo Nintendo invalide ($0104-$0133) → la Boot ROM a été sautée automatiquement
                // (sur le matériel réel, elle se verrouillerait en boucle infinie).
                if let Some(warning) = &self.boot_warning {
                    ui.colored_label(Color32::from_rgb(255, 170, 60), warning.as_str());
                }
            });
        });
    }

    /// Panneau gauche « Output » : framebuffer PPU mis à l'échelle pour remplir la zone (ratio 160×144 conservé).
    fn show_output_panel(&self, ctx: &egui::Context) {
        // ~40 % de la largeur fenêtre par défaut (1920) ; egui borne la largeur à l'espace disponible.
        egui::SidePanel::left("output")
            .default_width(768.0)
            .min_width(320.0)
            .resizable(true)
            .show(ctx, |ui| {
                let state = self.state;
                ui.horizontal(|ui| {
                    ui.label(egui::RichText::new("OUTPUT").strong());
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.colored_label(state.color(), format!("● {}", state.label()));
                    });
                });
                ui.separator();

                // Framebuffer mis à l'échelle pour remplir la zone (ratio 160:144 conservé, centré).
                let available = ui.available_size().max(egui::vec2(1.0, 1.0));
                if let Some(texture) = &self.texture {
                    let scale =
                        (available.x / SCREEN_WIDTH as f32).min(available.y / SCREEN_HEIGHT as f32);
                    let size = egui::vec2(
                        (SCREEN_WIDTH as f32 * scale).max(1.0),
                        (SCREEN_HEIGHT as f32 * scale).max(1.0),
                    );
                    ui.vertical_centered(|ui| {
                        ui.add(egui::Image::new(texture).fit_to_exact_size(size));
                    });
                } else {
                    ui.heading("…");
                }
                if self.rom_name.is_none() {
                    ui.weak("Chargez une ROM to start (l'écran montre l'état power-on)");
                }
            });
    }

    /// Right panel "Pattern Table": tile views (VRAM $8000-$9FFF or raw ROM banks 0/1) colored with the
    /// selected palette (BCP/OCP - live register or fixed preview), optional grid.
    fn show_pattern_table_panel(&mut self, ctx: &egui::Context) {
        // ~25 % de la largeur fenêtre par défaut (1920) ; egui borne la largeur à l'espace disponible.
        egui::SidePanel::right("pattern_table")
            .default_width(480.0)
            .min_width(320.0)
            .resizable(true)
            .show(ctx, |ui| {
                ui.label(egui::RichText::new("PATTERN TABLE").strong());
                // View selector: VRAM tiles or raw ROM banks (bank 0 = $0000-$3FFF, bank 1 = $4000-$7FFF).
                ui.horizontal(|ui| {
                    for view in PatternView::ALL {
                        let selected = self.pattern_view == view;
                        if ui.selectable_label(selected, view.short_label()).clicked() {
                            self.pattern_view = view;
                        }
                    }
                });
                ui.weak(self.pattern_view.label());
                ui.separator();

                // Palette de colorisation : BCP0..BCP7 (fond) + OCP0..OCP7 (sprites).
                ui.label(egui::RichText::new("Palette").color(Color32::YELLOW));
                egui::ComboBox::new("pattern_table_palette", "Palette")
                    .selected_text(self.tile_palette.label())
                    .show_ui(ui, |ui| {
                        for palette in TilePalette::ALL {
                            ui.selectable_value(&mut self.tile_palette, palette, palette.label());
                        }
                    });
                ui.checkbox(&mut self.show_tile_grid, "Grille 8x8");
                ui.separator();

                // Image mise a l'echelle pour remplir la zone (ratio conserve), sized per view.
                let (img_w, img_h) = self.pattern_view.image_size();
                let available = ui.available_size().max(egui::vec2(1.0, 1.0));
                if let Some(texture) = &self.tile_texture {
                    let scale = (available.x / img_w as f32).min(available.y / img_h as f32);
                    let size = egui::vec2(
                        (img_w as f32 * scale).max(1.0),
                        (img_h as f32 * scale).max(1.0),
                    );
                    ui.vertical_centered(|ui| {
                        ui.add(egui::Image::new(texture).fit_to_exact_size(size));
                    });
                }
            });
    }

    /// Zone centrale, haut gauche : processeur (drapeaux, PC/SP, registres A/F B/C D/E H/L + combinaisons 16 bits,
    /// IME/HALT/EI delay, compteurs instructions/T-cycles, BOOTROM).
    fn show_processor_panel(&self, ui: &mut egui::Ui) {
        // Le contenu (cadre des registres, titre « 🧠 Processor » inclus) défile si la zone est trop petite ;
        // le scroll horizontal évite que le tableau des registres soit écrêté/déformé quand la zone est étroite.
        egui::ScrollArea::both()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                self.show_cpu_info(ui);
            });
    }
    /// Zone centrale, haut droit : informations de l'en-tête cartouche (titre, type MBC, tailles,
    /// destination, drapeaux CGB/SGB, version et sommes de contrôle).
    fn show_rom_info_panel(&self, ui: &mut egui::Ui) {
        ui.label(egui::RichText::new("ROM INFO").strong());
        ui.separator();
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| match self.emulator.mmu.cartridge() {
                Some(header) => {
                    let title = self.rom_title();
                    let title_value = if title.is_empty() {
                        String::from("—")
                    } else {
                        title
                    };
                    Self::show_rom_info_row(ui, "Title", &title_value);

                    let mbc_value =
                        format!("{:?} (raw ${:02X})", header.mbc_type, header.cartridge_type);
                    Self::show_rom_info_row(ui, "Cartridge type", &mbc_value);

                    let rom_size = self.emulator.mmu.rom_size();
                    let rom_value = if rom_size > 0 {
                        format!("{} KiB", rom_size / 1024)
                    } else {
                        String::from("—")
                    };
                    Self::show_rom_info_row(ui, "ROM size", &rom_value);

                    // Nombre de banques ROM (une banque = 16 KiB — PanDocs « MBCs »).
                    let rom_banks_value = if rom_size > 0 {
                        format!("{}", rom_size / crate::mbc::ROM_BANK_SIZE)
                    } else {
                        String::from("—")
                    };
                    Self::show_rom_info_row(ui, "ROM banks", &rom_banks_value);

                    let ram_size = header.ram_size_bytes;
                    let ram_value = if ram_size > 0 {
                        format!("{} KB", ram_size / 1024)
                    } else {
                        String::from("—")
                    };
                    Self::show_rom_info_row(ui, "RAM size", &ram_value);

                    // Nombre de banques SRAM (une banque = 8 KiB — PanDocs « MBCs »).
                    let ram_banks_value = if ram_size > 0 {
                        format!("{}", ram_size / crate::mbc::RAM_BANK_SIZE)
                    } else {
                        String::from("—")
                    };
                    Self::show_rom_info_row(ui, "RAM banks", &ram_banks_value);

                    Self::show_rom_info_row(
                        ui,
                        "Destination",
                        if header.is_japanese() {
                            "Japan"
                        } else {
                            "Overseas"
                        },
                    );

                    let cgb_value = format!(
                        "${:02X} ({})",
                        header.cgb_flag,
                        if header.supports_cgb() {
                            "CGB compatible"
                        } else {
                            "DMG only"
                        }
                    );
                    Self::show_rom_info_row(ui, "CGB flag", &cgb_value);

                    Self::show_rom_info_row(
                        ui,
                        "SGB supported",
                        if header.sgb_supported { "yes" } else { "no" },
                    );

                    let version_value = format!("${:02X}", header.mask_rom_version);
                    Self::show_rom_info_row(ui, "Version", &version_value);

                    Self::show_rom_info_row(
                        ui,
                        "Header checksum",
                        if header.header_checksum_valid {
                            "valid"
                        } else {
                            "invalid"
                        },
                    );

                    let global_value = format!("${:04X}", header.global_checksum);
                    Self::show_rom_info_row(ui, "Global checksum", &global_value);
                }
                None => {
                    ui.weak("Aucune ROM chargée");
                }
            });
    }

    /// Ligne étiquette/valeur du panneau ROM Info (colonne d'étiquettes à largeur fixe pour l'alignement).
    fn show_rom_info_row(ui: &mut egui::Ui, label: &str, value: &str) {
        ui.horizontal(|ui| {
            ui.scope(|ui| {
                ui.set_min_width(130.0);
                ui.label(egui::RichText::new(label).color(Color32::YELLOW));
            });
            ui.monospace(value);
        });
    }

    /// Zone centrale, bas : carte IO ($FF00-$FF7F) — adresse, nom du registre et valeur en direct.
    fn show_io_map(&self, ui: &mut egui::Ui) {
        ui.label(egui::RichText::new("IO MAP ($FF00–$FF7F)").strong());
        ui.separator();
        let io = self.emulator.mmu.io; // copie du tableau de 128 octets (coût négligeable)
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                for offset in 0..io.len() as u16 {
                    let addr = 0xFF00 + offset;
                    ui.horizontal(|ui| {
                        ui.scope(|ui| {
                            ui.set_min_width(64.0);
                            ui.label(
                                egui::RichText::new(format!("${addr:04X}"))
                                    .monospace()
                                    .color(Color32::CYAN),
                            );
                        });
                        ui.scope(|ui| {
                            ui.set_min_width(190.0);
                            match io_register_name(offset) {
                                Some(name) => {
                                    ui.label(egui::RichText::new(name).color(Color32::YELLOW))
                                }
                                None => ui.weak("—"),
                            }
                        });
                        ui.scope(|ui| {
                            ui.set_min_width(48.0);
                            ui.monospace(format!("{:02X}", io[offset as usize]));
                        });
                    });
                }
            });
    }

    /// Central zone: memory editor - sub-tabs per region of the memory map (ROM0/ROM1/VRAM/WRAM/OAM/IO/HIRAM),
    /// live hex dump (16 bytes per line, address column) read through the MMU so that ROM banking (MBC), VRAM,
    /// WRAM, OAM, IO and HIRAM are always up to date, plus a byte-write field (hex address + value).
    fn show_memory_editor(&mut self, ui: &mut egui::Ui) {
        // Sub-tabs : the 7 regions of the memory map.
        ui.horizontal(|ui| {
            for region in MemoryRegion::ALL {
                let selected = self.memory_region == region;
                if ui.selectable_label(selected, region.label()).clicked() {
                    self.memory_region = region;
                }
            }
        });

        // Header: address range of the active region.
        let (base, size) = self.memory_region.range();
        let end = base.wrapping_add(size as u16).wrapping_sub(1);
        ui.label(
            egui::RichText::new(format!(
                "MEMORY EDITOR - {} (${:04X}-${:04X}, {} bytes)",
                self.memory_region.label(),
                base,
                end,
                size
            ))
            .strong(),
        );

        // Byte write: hex address + value (applied on the "Write" button).
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new("Write").color(Color32::YELLOW));
            ui.scope(|ui| {
                ui.set_width(80.0);
                ui.add(
                    egui::TextEdit::singleline(&mut self.mem_edit_addr)
                        .hint_text("$8000")
                        .font(egui::FontId::monospace(14.0)),
                );
            });
            ui.scope(|ui| {
                ui.set_width(56.0);
                ui.add(
                    egui::TextEdit::singleline(&mut self.mem_edit_value)
                        .hint_text("FF")
                        .font(egui::FontId::monospace(14.0)),
                );
            });
            if ui.button("Write").clicked() {
                let addr = parse_hex_u16(&self.mem_edit_addr);
                let value = parse_hex_u8(&self.mem_edit_value);
                if let (Some(addr), Some(value)) = (addr, value) {
                    self.emulator.mmu.write(addr, value);
                }
            }
        });

        ui.separator();

        // Hex dump: 16 bytes per line with an address column - read live through the MMU.
        // While the boot ROM is still mapped (BOOT_OFF=0), $0000-$00FF are served by the boot image, not
        // the cartridge (GBCTR Chapter 7) : annotate those rows so they don't look like ROM0 contents.
        let boot_rom_mapped = !self.emulator.mmu.boot_rom_finished;
        if boot_rom_mapped {
            ui.label(
                egui::RichText::new("⚠ $0000-$00FF are currently masked by the Boot ROM (BOOT_OFF=0) : these bytes come from the boot image, not the cartridge.")
                    .color(Color32::YELLOW),
            );
        }
        let mut dump = String::with_capacity((size / 16 + 1) * 48);
        for offset in (0..size).step_by(16) {
            let row_base = base.wrapping_add(offset as u16);
            let mut line = format!("${row_base:04X} | ");
            for i in 0..16u16 {
                if offset + (i as usize) < size {
                    let byte = self.emulator.mmu.read_debug(row_base.wrapping_add(i));
                    line.push_str(&format!("{byte:02X} "));
                } else {
                    line.push_str(".. "); // padding for the last incomplete row (HIRAM)
                }
            }
            if boot_rom_mapped && row_base < 0x100 {
                line.push_str("← BOOT ROM");
            }
            dump.push_str(line.trim_end());
            dump.push('\n');
        }

        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                ui.monospace(dump);
            });
    }

    /// Informations CPU en style « table » : titre « 🧠 Processor », section drapeaux Z N H C (+ registre F),
    /// tableau complet des registres (PC|SP, paires de bytes A|B F|C D|E H|L, combinaisons 16 bits BC|DE HL|AF)
    /// en hexadécimal + expansion binaire, état IME/HALT/EI delay, compteurs instructions/T-cycles et indicateur
    /// BOOTROM. Les tableaux sont construits avec `egui_extras::TableBuilder` (colonnes auto-dimensionnées sur le
    /// contenu monospace) : l'alignement est conservé quelle que soit la largeur du panneau — plus aucune
    /// déformation quand la zone centrale redimensionne ou qu'un autre widget de débogage est activé à côté.
    fn show_cpu_info(&self, ui: &mut egui::Ui) {
        use egui_extras::{Column, TableBuilder};

        let cpu = &self.emulator.cpu;

        // Le cadre est centré dans le panneau avec une largeur minimale : le tableau ne peut plus être écrêté.
        ui.vertical_centered(|ui| {
            egui::Frame::default()
                .stroke(egui::Stroke::new(1.0_f32, Color32::from_rgb(96, 96, 96)))
                .inner_margin(8.0)
                .show(ui, |ui| {
                    // egui 0.31 : `Frame` n'a pas de methode `min_size` ; la largeur minimale est
                    // imposee sur le ui interne (le cadre s'adapte a `content_ui.min_rect()`).
                    ui.set_min_size(egui::vec2(470.0, 0.0));
                    // Titre du panneau (centré).
                    ui.vertical_centered(|ui| {
                        ui.label(egui::RichText::new("🧠 Processor").strong());
                    });

                    ui.separator();

                    // Section drapeaux Z N H C : étiquettes orange, valeur allumée si levée + registre F.
                    ui.vertical_centered(|ui| {
                        ui.label(egui::RichText::new("🚦 Flags CPU"));
                    });
                    ui.add_space(4.0);
                    let f = cpu.flags();
                    ui.horizontal(|ui| {
                        for (name, flag) in [
                            ('Z', Flags::Z),
                            ('N', Flags::N),
                            ('H', Flags::H),
                            ('C', Flags::C),
                        ] {
                            Self::show_flag(ui, name, f.contains(flag));
                        }
                        ui.add_space(16.0);
                        ui.label(egui::RichText::new("F").color(Color32::ORANGE));
                        ui.monospace(format!("=${:02X}", cpu.f & 0xF0));
                    });

                    ui.separator();

                    // Tableau complet des registres : PC|SP, puis paires de bytes A|B F|C D|E H|L,
                    // puis combinaisons 16 bits BC|DE HL|AF. Valeur hexadécimale + expansion binaire gris foncé.
                    let table = TableBuilder::new(ui)
                        .striped(false)
                        .cell_layout(egui::Layout::left_to_right(egui::Align::TOP))
                        .column(Column::auto()) // Registre (moitié gauche)
                        .column(Column::auto()) // Valeur hexadécimale (gauche)
                        .column(Column::auto()) // Expansion binaire (gauche)
                        .column(Column::exact(18.0)) // Séparation entre les deux moitiés
                        .column(Column::auto()) // Registre (moitié droite)
                        .column(Column::auto()) // Valeur hexadécimale (droite)
                        .column(Column::auto()); // Expansion binaire (droite)

                    let rows16: [(&str, u16, &str, u16); 3] = [
                        ("PC", cpu.pc, "SP", cpu.sp),
                        (
                            "BC",
                            ((cpu.b as u16) << 8) | cpu.c as u16,
                            "DE",
                            ((cpu.d as u16) << 8) | cpu.e as u16,
                        ),
                        ("HL", cpu.hl(), "AF", cpu.af()),
                    ];

                    // Toutes les lignes sont ajoutées dans le corps de la table : en egui_extras 0.31,
                    // `row()` vit sur `TableBody` (obtenu via `TableBuilder::body`).
                    table.body(|mut body| {
                        // Ligne PC|SP (pointeurs 16 bits).
                        let (n1, v1, n2, v2) = rows16[0];
                        body.row(20.0, |mut row| {
                            Self::show_register_half(
                                &mut row,
                                n1,
                                format!("${v1:04X}"),
                                format!("{:08b} {:08b}", (v1 >> 8) as u8, v1 as u8),
                            );
                            row.col(|_| {}); // Séparation entre les deux moitiés.
                            Self::show_register_half(
                                &mut row,
                                n2,
                                format!("${v2:04X}"),
                                format!("{:08b} {:08b}", (v2 >> 8) as u8, v2 as u8),
                            );
                        });

                        // Ligne d'espacement.
                        body.row(6.0, |mut row| {
                            row.col(|_| {});
                        });

                        // Paires de bytes A|B F|C D|E H|L.
                        let pairs8: [(&str, u8, &str, u8); 4] = [
                            ("A", cpu.a, "B", cpu.b),
                            ("F", cpu.f, "C", cpu.c),
                            ("D", cpu.d, "E", cpu.e),
                            ("H", cpu.h, "L", cpu.l),
                        ];
                        for (n1, v1, n2, v2) in pairs8 {
                            body.row(20.0, |mut row| {
                                Self::show_register_half(
                                    &mut row,
                                    n1,
                                    format!("${v1:02X}"),
                                    format!("{:04b} {:04b}", v1 >> 4, v1 & 0x0F),
                                );
                                row.col(|_| {}); // Séparation entre les deux moitiés.
                                Self::show_register_half(
                                    &mut row,
                                    n2,
                                    format!("${v2:02X}"),
                                    format!("{:04b} {:04b}", v2 >> 4, v2 & 0x0F),
                                );
                            });
                        }

                        // Ligne d'espacement.
                        body.row(6.0, |mut row| {
                            row.col(|_| {});
                        });

                        // Combinaisons 16 bits BC|DE HL|AF (les deux dernières lignes de `rows16`).
                        for &(n1, v1, n2, v2) in &rows16[1..] {
                            body.row(20.0, |mut row| {
                                Self::show_register_half(
                                    &mut row,
                                    n1,
                                    format!("${v1:04X}"),
                                    format!("{:08b} {:08b}", (v1 >> 8) as u8, v1 as u8),
                                );
                                row.col(|_| {}); // Séparation entre les deux moitiés.
                                Self::show_register_half(
                                    &mut row,
                                    n2,
                                    format!("${v2:04X}"),
                                    format!("{:08b} {:08b}", (v2 >> 8) as u8, v2 as u8),
                                );
                            });
                        }
                    });

                    ui.separator();

                    // État du processeur : IME / HALT / EI delay + compteurs instructions et T-cycles.
                    let status = TableBuilder::new(ui)
                        .striped(false)
                        .cell_layout(egui::Layout::left_to_right(egui::Align::TOP))
                        .column(Column::auto()) // Étiquette (groupe 1)
                        .column(Column::auto()) // Valeur (groupe 1)
                        .column(Column::exact(18.0))
                        .column(Column::auto()) // Étiquette (groupe 2)
                        .column(Column::auto()) // Valeur (groupe 2)
                        .column(Column::exact(18.0))
                        .column(Column::auto()) // Étiquette (groupe 3)
                        .column(Column::auto()); // Valeur (groupe 3)

                    let t_cycles = self.emulator.t_cycles;
                    let frames_elapsed = t_cycles / crate::emulator::FRAME_TCYCLES;
                    let dot_in_frame = t_cycles % crate::emulator::FRAME_TCYCLES;

                    status.body(|mut body| {
                        body.row(20.0, |mut row| {
                            Self::show_state_cell(
                                &mut row,
                                "IME",
                                if cpu.ime { "ON" } else { "OFF" }.to_owned(),
                                if cpu.ime { Color32::GREEN } else { Self::DIM_GRAY },
                            );
                            row.col(|_| {});
                            Self::show_state_cell(
                                &mut row,
                                "HALT",
                                if cpu.halted { "ACTIVE" } else { "—" }.to_owned(),
                                if cpu.halted { Color32::YELLOW } else { Self::DIM_GRAY },
                            );
                            row.col(|_| {});
                            Self::show_state_cell(
                                &mut row,
                                "EI delay",
                                cpu.ei_delay.to_string(),
                                if cpu.ei_delay != 0 { Color32::GREEN } else { Self::DIM_GRAY },
                            );
                        });

                        body.row(20.0, |mut row| {
                            Self::show_state_cell(
                                &mut row,
                                "Instr",
                                format!("#{}", self.emulator.instructions),
                                Color32::from_rgb(190, 190, 190),
                            );
                            row.col(|_| {});
                            Self::show_state_cell(
                                &mut row,
                                "TCyc",
                                format!(
                                    "#{t_cycles} · frame {frames_elapsed} · dot {dot_in_frame}/{}",
                                    crate::emulator::FRAME_TCYCLES
                                ),
                                Color32::from_rgb(190, 190, 190),
                            );
                            row.col(|_| {}); // Groupe 3 vide (structure de colonnes conservée).
                        });
                    });

                    ui.separator();

                    // BOOTROM : allumé tant que la boot ROM DMG est encore mappée (séquence de démarrage en cours).
                    let in_bootrom = !self.emulator.mmu.boot_rom_finished;
                    ui.vertical_centered(|ui| {
                        ui.label(egui::RichText::new("BOOTROM").color(if in_bootrom {
                            Color32::GREEN
                        } else {
                            Self::DIM_GRAY
                        }));
                    });
                });
        });
    }

    /// Gris foncé pour les expansions binaires et les indicateurs inactifs.
    const DIM_GRAY: Color32 = Color32::from_rgb(110, 110, 110);

    /// Drapeau Z N H C : étiquette orange au-dessus de la valeur (verte si levée).
    fn show_flag(ui: &mut egui::Ui, name: char, set: bool) {
        ui.scope(|ui| {
            ui.set_min_width(44.0);
            ui.vertical_centered(|ui| {
                ui.label(egui::RichText::new(name.to_string()).color(Color32::ORANGE));
                let color = if set {
                    Color32::GREEN
                } else {
                    Color32::from_rgb(190, 190, 190)
                };
                ui.label(
                    egui::RichText::new(if set { "1" } else { "0" })
                        .monospace()
                        .color(color),
                );
            });
        });
    }

    /// Moitié d'une ligne du tableau des registres : nom monospace cyan + valeur hexadécimale, puis expansion
    /// binaire gris foncé. Les colonnes auto-dimensionnées de `TableBuilder` gardent l'alignement quel que soit
    /// le contenu (largeurs fixes en police monospace).
    fn show_register_half(row: &mut egui_extras::TableRow, name: &str, hex: String, bin: String) {
        row.col(|ui| {
            ui.label(egui::RichText::new(name).monospace().color(Color32::CYAN));
        });
        row.col(|ui| {
            ui.monospace(hex);
        });
        row.col(|ui| {
            ui.label(egui::RichText::new(bin).monospace().color(Self::DIM_GRAY));
        });
    }

    /// Cellule d'état (étiquette monospace jaune + valeur colorée) pour la section IME/HALT/EI delay/compteurs.
    fn show_state_cell(row: &mut egui_extras::TableRow, name: &str, value: String, color: Color32) {
        row.col(|ui| {
            ui.label(egui::RichText::new(name).monospace().color(Color32::YELLOW));
        });
        row.col(|ui| {
            ui.label(egui::RichText::new(value).monospace().color(color));
        });
    }
    /// Noms des huit boutons du joypad, dans the order of the indices (`crate::joypad::KEY_A`…`crate::joypad::KEY_DOWN`).
    const JOYPAD_BUTTON_NAMES: [&str; 8] =
        ["A", "B", "Select", "Start", "Right", "Left", "Up", "Down"];

    /// Indices des huit boutons du joypad dans the order d'affichage (identique à `crate::joypad`).
    const JOYPAD_KEY_INDICES: [u8; 8] = [
        KEY_A, KEY_B, KEY_SELECT, KEY_START, KEY_RIGHT, KEY_LEFT, KEY_UP, KEY_DOWN,
    ];

    /// Dessine la fenêtre « ⌨️ Input Mapping » : pour chacun des huit boutons du joypad, un clic on son
    /// mappage actuel capture the next key pressed (stored in `egui::Key`) ; the button « ✕ » clears
    /// the mapping. The mapped keys are then polled in [`Self::update`] to $FF00 (P1). Any change of
    /// the mapping is persisted on disk after the window content has been rendered.
    fn show_input_mapping_window(&mut self, ctx: &egui::Context) {
        // Le mappage a-t-il été modifié ? La sauvegarde se fait après la fermeture de la closure
        // (le champ `show_input_mapping` est encore emprunté par `.open(...)` en son sein).
        let mut mapping_changed = false;
        egui::Window::new("⌨️ Input Mapping")
            .open(&mut self.show_input_mapping)
            .resizable(false)
            .show(ctx, |ui| {
                ui.label(
                    "Cliquez sur the mappage d'un bouton, puis appuyez on the key to assign it.",
                );
                if let Some(idx) = self.capturing_button {
                    ui.colored_label(
                        Color32::YELLOW,
                        format!(
                            "Mappage de {} : appuyez on any key… (Échap cancels)",
                            Self::JOYPAD_BUTTON_NAMES[idx]
                        ),
                    );
                }
                ui.separator();

                for idx in 0..Self::JOYPAD_KEY_INDICES.len() {
                    let name = Self::JOYPAD_BUTTON_NAMES[idx];
                    let key_index = Self::JOYPAD_KEY_INDICES[idx];
                    let current = self.key_map[idx]
                        .map(|key| key.name().to_owned())
                        .unwrap_or_else(|| "—".to_owned());
                    ui.horizontal(|ui| {
                        // Live state of the button (green = pressed) : immediate visual feedback of the mapping.
                        let pressed = self.emulator.mmu.joypad.is_key_pressed(key_index);
                        let dot_color = if pressed {
                            Color32::GREEN
                        } else {
                            Color32::GRAY
                        };
                        ui.label(
                            egui::RichText::new(if pressed { "●" } else { "○" }).color(dot_color),
                        );
                        ui.label(egui::RichText::new(name).color(Color32::CYAN));
                        let button = egui::Button::new(egui::RichText::new(current))
                            .min_size(egui::vec2(100.0, 0.0));
                        if ui.add(button).clicked() {
                            self.capturing_button = Some(idx); // waiting for the next key press
                        }
                        if self.key_map[idx].is_some() && ui.small_button("✕").clicked() {
                            self.key_map[idx] = None;
                            mapping_changed = true;
                        }
                    });
                }

                ui.separator();
                if ui.button("Réinitialiser tous les mappages").clicked() {
                    self.key_map = [None; 8];
                    self.capturing_button = None;
                    mapping_changed = true;
                }
            });

        // Persiste le mappage modifié entre les sessions (fichier bincode à côté de l'exécutable).
        if mapping_changed {
            self.save_input_config();
        }
    }

    /// Dessine la fenêtre « 🔌 Serial Monitor » : sortie en direct du port série (registres SB/SC,
    /// dernière ligne complète et transcript des octets émis) pour le debug des ROMs de test
    /// (GBCTR / cpu_instrs). La ROM n'est pas décompilée : seules les octes transmis sur le câble
    /// link sont affichés, avec la même convention que l'affichage stdout (`serial.rs`).
    fn show_serial_monitor_window(&mut self, ctx: &egui::Context) {
        // Régénère le transcript rendu uniquement quand de nouveaux octets ont été capturés (ou vidés).
        let serial = &self.emulator.mmu.serial;
        if serial.transcript_len() != self.serial_cached_len {
            self.serial_text_cache = serial.rendered_transcript();
            self.serial_cached_len = serial.transcript_len();
        }

        // Valeurs d'en-tête copiées (la closure ci-dessous emprunte `self` mutablement pour le bouton « Clear »).
        let sb = self.emulator.mmu.serial.sb;
        let sc = self.emulator.mmu.serial.sc;
        let last_line = self.emulator.mmu.serial.last_line().to_owned();

        egui::Window::new("🔌 Serial Monitor")
            .open(&mut self.show_serial_monitor)
            .default_size([600.0, 420.0])
            .show(ctx, |ui| {
                // En-tête : registres SB/SC + compteur d'octets + bouton « Clear » (à droite).
                ui.horizontal(|ui| {
                    ui.label(egui::RichText::new("SB ($FF01)").color(Color32::YELLOW));
                    ui.monospace(format!("{sb:02X}"));
                    ui.add_space(8.0);
                    let transfer_active = sc & 0x80 != 0; // bit 7 : transfert en cours (PanDocs)
                    ui.label(egui::RichText::new("SC ($FF02)").color(Color32::YELLOW));
                    ui.colored_label(
                        if transfer_active { Color32::GREEN } else { Self::DIM_GRAY },
                        format!("{sc:02X}"),
                    );
                    if transfer_active {
                        ui.weak("(transfert en cours)");
                    }
                    ui.add_space(8.0);
                    ui.label(
                        egui::RichText::new(format!("{} octets", self.serial_cached_len))
                            .color(Color32::CYAN),
                    );
                    // Bouton « Clear » à droite de la ligne (vide le transcript, pas les registres).
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.button("🗑 Clear").clicked() {
                            self.emulator.mmu.serial.clear_transcript();
                            self.serial_text_cache.clear();
                            self.serial_cached_len = 0;
                        }
                    });
                });

                // Dernière ligne complète reçue (statut d'un coup d'œil, ex. « All tests passed! »).
                ui.horizontal(|ui| {
                    ui.label(egui::RichText::new("Dernière ligne").color(Color32::YELLOW));
                    if last_line.is_empty() {
                        ui.weak("—");
                    } else {
                        ui.monospace(&last_line);
                    }
                });

                ui.separator();

                // Transcript complet (monospace, défilement calé sur le bas comme un terminal).
                egui::ScrollArea::vertical()
                    .stick_to_bottom(true)
                    .show(ui, |ui| {
                        if self.serial_text_cache.is_empty() {
                            ui.weak(
                                "Aucune sortie série — exécutez une ROM de test (GBCTR / cpu_instrs) pour voir ses résultats ici.",
                            );
                        } else {
                            ui.monospace(&self.serial_text_cache);
                        }
                    });
            });
    }
}

impl eframe::App for FarquaadGBApp {
    /// Never persist egui memory (panel widths, window positions, etc.) :
    /// each launch must open with exactly the same layout ; a state restored from a previous session could otherwise widen a panel and hide the game screen.
    fn persist_egui_memory(&self) -> bool {
        false
    }

    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // Capture of the next key press for the joypad button being mapped
        // (window « ⌨️ Input Mapping ») ; Échap cancels the capture.
        if let Some(idx) = self.capturing_button {
            if !self.show_input_mapping {
                self.capturing_button = None; // window closed : stop capturing
            } else if let Some(key) = ctx.input(|i| {
                i.events.iter().find_map(|event| match event {
                    egui::Event::Key {
                        key,
                        pressed: true,
                        repeat: false,
                        ..
                    } => Some(*key),
                    _ => None,
                })
            }) {
                if key == egui::Key::Escape {
                    self.capturing_button = None; // Échap cancels the capture
                } else {
                    self.key_map[idx] = Some(key);
                    self.capturing_button = None;
                    self.save_input_config(); // persiste le mappage entre les sessions
                }
            }
        }

        // Joypad : keyboard → $FF00 (P1), polled on each UI update (~60 Hz). A press transition
        // (released → pressed) raises the joypad interrupt flag (bit 4 of IF — PanDocs « Interrupt Sources »).
        for idx in 0..Self::JOYPAD_KEY_INDICES.len() {
            let key_index = Self::JOYPAD_KEY_INDICES[idx];
            let pressed = self.key_map[idx].is_some_and(|key| ctx.input(|i| i.key_down(key)));
            if self.emulator.mmu.joypad.set_key(key_index, pressed) {
                self.emulator.mmu.io[0x0F] |= 0x10; // IF bit 4 : joypad interrupt pending
            }
        }

        // Automatic execution : one frame per UI update (~60 FPS), only in the Running state.
        if self.state == EmulationState::Running && self.emulator.mmu.rom_size() > 0 {
            self.emulator.run_frame();
            // Force egui to redraw without waiting for a mouse/keyboard event :
            // otherwise the application goes idle and the CPU only advances on UI events.
            ctx.request_repaint();
        }

        // Textures (screen + Pattern Table).
        self.update_screen_texture(ctx);
        self.update_tile_texture(ctx);

        // Top menu bar : ROM loading, Run/Pause/Stop, input mapping.
        self.show_menu_bar(ctx);

        // Left panel « Output » : PPU framebuffer scaled to fill the zone.
        self.show_output_panel(ctx);

        // Right panel "Pattern Table": colorized tile views - VRAM or raw ROM banks 0/1 (BCP/OCP palettes) - activable from the Debug menu.
        if self.show_pattern_table {
            self.show_pattern_table_panel(ctx);
        }

        // Central zone : debug views as tabs (CPU / ROM Info / IO Map / Memory) - one at a time, each in its
        // own scroll area (no more cramming of widgets side by side nor nested scroll areas).
        egui::CentralPanel::default().show(ctx, |ui| {
            ui.horizontal(|ui| {
                for tab in DebugTab::ALL {
                    let selected = self.debug_tab == tab;
                    if ui.selectable_label(selected, tab.label()).clicked() {
                        self.debug_tab = tab;
                    }
                }
            });
            ui.separator();

            match self.debug_tab {
                DebugTab::Cpu => self.show_processor_panel(ui),
                DebugTab::RomInfo => self.show_rom_info_panel(ui),
                DebugTab::IoMap => self.show_io_map(ui),
                DebugTab::Memory => self.show_memory_editor(ui),
            }
        });

        // Window « ⌨️ Input Mapping » (keyboard mapping → joypad $FF00).
        self.show_input_mapping_window(ctx);

        // Window « 🔌 Serial Monitor » : live output of the serial port (SB/SC + transcript) for debugging test ROMs.
        self.show_serial_monitor_window(ctx);
    }
}
