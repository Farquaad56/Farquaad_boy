//! Application eframe : interface utilisateur et rendu.
//!
//! Mise en page professionnelle à 4 zones — barre de menu supérieure (chargement ROM,
//! Run/Pause/Stop, menu « 🔍 Debug » activant les widgets de débogage), panneau gauche
//! « Output » (framebuffer PPU mis à l'échelle pour remplir la zone), zone centrale avec
//! Processor / ROM Info / IO Map activables depuis le menu Debug, panneau droit « Pattern
//! Table » (tuiles VRAM colorées par palette BGP/OBP0/OBP1, grille optionnelle). Le mappage
//! clavier des touches du joypad est persisté entre les sessions (fichier bincode à côté de
//! l'exécutable).

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

/// Palette utilisée pour coloriser les tuiles du Pattern Table (VRAM $8000-$97FF).
#[derive(Clone, Copy, Debug, PartialEq)]
enum TilePalette {
    /// BGP ($FF47) : palette du fond/fenêtre.
    Bgp,
    /// OBP0 ($FF48) : palette sprite 0 (par défaut).
    Obp0,
    /// OBP1 ($FF49) : palette sprite 1.
    Obp1,
}

/// Taille d'une tuile en pixels (8×8).
const TILE_SIZE: usize = 8;
/// Nombre de tuiles affichées : VRAM $8000-$97FF (256 tuiles).
const TILE_COUNT: usize = 256;
/// Grille du Pattern Table : 16×16 tuiles.
const TILE_GRID: usize = 16;
/// Taille de l'image du Pattern Table en pixels (16 tuiles × 8 px).
const TILE_IMAGE_SIZE: usize = TILE_GRID * TILE_SIZE; // 128

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
    /// Widgets de débogage activables depuis le menu « 🔍 Debug » de la barre supérieure.
    show_cpu_registers: bool, // panneau Processor (registres CPU) — zone centrale
    show_rom_info: bool,      // panneau ROM Info — zone centrale
    show_io_map: bool,        // carte IO ($FF00–$FF7F) — bas de la zone centrale
    show_pattern_table: bool, // panneau droit « Pattern Table » (tuiles VRAM)
    /// Fenêtre « ⌨️ Input Mapping » (mappage clavier → joypad $FF00).
    show_input_mapping: bool,
    /// Mappage clavier des huit boutons du joypad, dans the order of the indices
    /// (`crate::joypad::KEY_A`…`crate::joypad::KEY_DOWN`) ; `None` = non mappé.
    key_map: [Option<egui::Key>; 8],
    /// Bouton du joypad en cours de mappage (en attente d'un appui clavier) ; `None` = pas de capture.
    capturing_button: Option<usize>,
    /// Texture écran (framebuffer PPU, NEAREST pour le pixel art).
    texture: Option<egui::TextureHandle>,
    /// Texture Pattern Table (256 tuiles VRAM colorées, NEAREST).
    tile_texture: Option<egui::TextureHandle>,
    /// Tampon réutilisable des pixels du Pattern Table (128×128 u32, R dans l'octet le plus bas — compatible egui/bytemuck).
    tile_pixels: Vec<u32>,
    /// Palette de colorisation du Pattern Table.
    tile_palette: TilePalette,
    /// Affichage de la grille 8×8 entre les tuiles du Pattern Table.
    show_tile_grid: bool,
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
            emulator: Emulator::new(),
            rom_name: None,
            state: EmulationState::Stopped,
            // Vue épurée par défaut : seul le panneau ROM Info est actif (les autres widgets se
            // réactivent depuis le menu « 🔍 Debug »).
            show_cpu_registers: false,
            show_rom_info: true,
            show_io_map: false,
            show_pattern_table: false,
            show_input_mapping: false,
            key_map: [None; 8],
            capturing_button: None,
            texture: None,
            tile_texture: None,
            tile_pixels: vec![0xFF00_0000; TILE_IMAGE_SIZE * TILE_IMAGE_SIZE],
            tile_palette: TilePalette::Bgp,
            show_tile_grid: true,
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
    /// et the argument de ligne de commande).
    fn load_rom_file(&mut self, path: &std::path::Path) {
        match std::fs::read(path) {
            Ok(data) => {
                self.emulator.load_rom(data);
                self.rom_name = path.file_name().and_then(|n| n.to_str()).map(String::from);
                self.state = EmulationState::Running; // démarrage immédiat (séquence de boot + jeu)
                log::info!("ROM chargée : {:?}", path);
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

    /// Reconstruit la texture du Pattern Table : 256 tuiles VRAM ($8000-$97FF) colorées par the palette
    /// choisie (BGP/OBP0/OBP1), grille optionnelle entre les tuiles.
    fn update_tile_texture(&mut self, ctx: &egui::Context) {
        let vram = &self.emulator.mmu.vram;
        // Octet de palette : chaque paire de bits sélectionne la teinte DMG des pixels de valeur 0..3.
        let palette_byte = match self.tile_palette {
            TilePalette::Bgp => self.emulator.mmu.ppu.bgp,
            TilePalette::Obp0 => self.emulator.mmu.ppu.obp0,
            TilePalette::Obp1 => self.emulator.mmu.ppu.obp1,
        };
        let colors =
            [0u8, 1, 2, 3].map(|index| ppu::PPU::shade((palette_byte >> (6 - 2 * index)) & 3));

        // Remplissage du tampon : tuile par tuile, ligne par ligne (deux octets de 8 bits chacune).
        let pixels = &mut self.tile_pixels;
        for tile in 0..TILE_COUNT {
            let base = tile * TILE_SIZE * 2; // $8000 + tuile × 16 octets
            for row in 0..TILE_SIZE {
                let b0 = vram[base + row * 2];
                let b1 = vram[base + row * 2 + 1];
                let py = (tile / TILE_GRID) * TILE_SIZE + row; // position de la tuile dans la grille 16×16
                for col in 0..TILE_SIZE {
                    // Bits 7→0 de chaque octet : bit 7 = teinte 1, bit 6 = teinte 2 (index = b7 + 2×b6).
                    let bit = 7 - col;
                    let shade_index = ((b0 >> bit) & 1) | (((b1 >> bit) & 1) << 1);
                    pixels[py * TILE_IMAGE_SIZE + (tile % TILE_GRID) * TILE_SIZE + col] =
                        colors[shade_index as usize];
                }
            }
        }

        // Grille optionnelle : lignes sombres toutes les 8 px entre les tuiles.
        if self.show_tile_grid {
            let grid_color: u32 = 0xFF_5A_5A_5A;
            for i in (0..TILE_IMAGE_SIZE).step_by(TILE_SIZE) {
                for j in 0..TILE_IMAGE_SIZE {
                    pixels[i * TILE_IMAGE_SIZE + j] = grid_color; // ligne horizontale
                    pixels[j * TILE_IMAGE_SIZE + i] = grid_color; // colonne verticale
                }
            }
        }

        let image = egui::ColorImage::from_rgba_unmultiplied(
            [TILE_IMAGE_SIZE, TILE_IMAGE_SIZE],
            bytemuck::cast_slice(&self.tile_pixels),
        );
        match &mut self.tile_texture {
            Some(texture) => texture.set(image, egui::TextureOptions::NEAREST),
            None => {
                self.tile_texture =
                    Some(ctx.load_texture("gb_pattern_table", image, egui::TextureOptions::NEAREST))
            }
        }
    }
    /// Barre de menu supérieure : chargement ROM, Run/Pause/Stop (colorés selon l'état),
    /// menu « 🔍 Debug » (activation des widgets de débogage) et libellé de la ROM.
    fn show_menu_bar(&mut self, ctx: &egui::Context) {
        egui::TopBottomPanel::top("menu").show(ctx, |ui| {
            egui::menu::bar(ui, |ui| {
                if ui.button("📂 Ouvrir ROM").clicked() {
                    self.load_rom();
                }

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
                // Menu « 🔍 Debug » : activation des widgets de débogage (panneaux et fenêtre).
                ui.menu_button("🔍 Debug", |ui| {
                    ui.checkbox(&mut self.show_cpu_registers, "CPU Registers");
                    ui.checkbox(&mut self.show_rom_info, "ROM Info");
                    ui.checkbox(&mut self.show_io_map, "IO Map");
                    ui.separator();
                    ui.checkbox(&mut self.show_pattern_table, "Pattern Table");
                    ui.separator();
                    ui.checkbox(&mut self.show_input_mapping, "Input Mapping");
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

    /// Panneau droit « Pattern Table » : 256 tuiles VRAM colorées par palette BGP/OBP0/OBP1, grille optionnelle.
    fn show_pattern_table_panel(&mut self, ctx: &egui::Context) {
        // ~20 % de la largeur fenêtre par défaut (1920) ; egui borne la largeur à l'espace disponible.
        egui::SidePanel::right("pattern_table")
            .default_width(384.0)
            .min_width(256.0)
            .resizable(true)
            .show(ctx, |ui| {
                ui.label(egui::RichText::new("PATTERN TABLE").strong());
                ui.weak("VRAM $8000–$97FF (256 tuiles 8×8)");
                ui.separator();

                // Palette de colorisation : BGP / OBP0 / OBP1.
                ui.radio_value(&mut self.tile_palette, TilePalette::Bgp, "BGP ($FF47)");
                ui.radio_value(&mut self.tile_palette, TilePalette::Obp0, "OBP0 ($FF48)");
                ui.radio_value(&mut self.tile_palette, TilePalette::Obp1, "OBP1 ($FF49)");
                ui.checkbox(&mut self.show_tile_grid, "Grille 8×8");
                ui.separator();

                // Image carrée (128×128) mise à l'échelle pour remplir la zone.
                let available = ui.available_size().max(egui::vec2(1.0, 1.0));
                if let Some(texture) = &self.tile_texture {
                    let scale = available.x.min(available.y) / TILE_IMAGE_SIZE as f32;
                    let size = egui::vec2(
                        (TILE_IMAGE_SIZE as f32 * scale).max(1.0),
                        (TILE_IMAGE_SIZE as f32 * scale).max(1.0),
                    );
                    ui.vertical_centered(|ui| {
                        ui.add(egui::Image::new(texture).fit_to_exact_size(size));
                    });
                }
            });
    }

    /// Zone centrale, haut gauche : processeur (drapeaux, PC/SP, registres A/F B/C D/E H/L, IME/HALT, BOOTROM).
    fn show_processor_panel(&self, ui: &mut egui::Ui) {
        // Le contenu (cadre des registres, titre « 🧠 Processor » inclus) défile si la zone est trop petite.
        egui::ScrollArea::vertical()
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
    /// Informations CPU en style « table » : titre « 🧠 Processor », section drapeaux Z N H C, PC/SP et
    /// paires de registres A|F B|C D|E H|L affichés en hexadécimal + expansion binaire, état IME/HALT
    /// et indicateur BOOTROM. Toutes les lignes utilisent la police monospace avec des colonnes fixes
    /// (étiquette 2 caractères + valeur) : le tableau reste parfaitement aligné quelle que soit la
    /// valeur affichée.
    fn show_cpu_info(&self, ui: &mut egui::Ui) {
        let cpu = &self.emulator.cpu;

        // Le cadre s'étend sur toute la largeur du panneau (les séparateurs internes sont des widgets expansifs).
        egui::Frame::default()
            .stroke(egui::Stroke::new(1.0_f32, Color32::from_rgb(96, 96, 96)))
            .inner_margin(8.0)
            .show(ui, |ui| {
                // Titre du panneau (centré).
                ui.vertical_centered(|ui| {
                    ui.label(egui::RichText::new("🧠 Processor").strong());
                });

                ui.separator();

                // Section drapeaux Z N H C : étiquettes orange, valeur allumée si levée.
                ui.vertical_centered(|ui| {
                    ui.label(egui::RichText::new("🚦 Flags CPU"));
                });
                ui.add_space(4.0);
                ui.vertical_centered(|ui| {
                    ui.horizontal(|ui| {
                        let f = cpu.flags();
                        for (name, flag) in [
                            ('Z', Flags::Z),
                            ('N', Flags::N),
                            ('H', Flags::H),
                            ('C', Flags::C),
                        ] {
                            Self::show_flag(ui, name, f.contains(flag));
                        }
                    });
                });

                ui.separator();

                // PC et SP : hexadécimal + expansion binaire 16 bits.
                Self::show_pointer_row(ui, "PC", cpu.pc);
                ui.separator();
                Self::show_pointer_row(ui, "SP", cpu.sp);

                // Paires de registres : grille 2 colonnes (A|F, B|C, D|E, H|L), hexadécimal + binaire.
                let pairs = [
                    ('A', cpu.a, 'F', cpu.f),
                    ('B', cpu.b, 'C', cpu.c),
                    ('D', cpu.d, 'E', cpu.e),
                    ('H', cpu.h, 'L', cpu.l),
                ];
                for (i, (n1, v1, n2, v2)) in pairs.iter().enumerate() {
                    if i > 0 {
                        ui.separator();
                    }
                    ui.horizontal(|ui| {
                        // Moitié de la largeur de la ligne (moins le séparateur central), bornée à une
                        // valeur positive : `set_min_width` ne doit jamais recevoir une valeur négative,
                        // même si la zone est extrêmement étroite.
                        let half = ((ui.available_width() - 14.0) * 0.5).max(30.0);
                        ui.scope(|ui| {
                            ui.set_min_width(half);
                            Self::show_register_cell(ui, *n1, *v1);
                        });
                        ui.add(egui::Separator::default().vertical());
                        ui.scope(|ui| {
                            ui.set_min_width(half);
                            Self::show_register_cell(ui, *n2, *v2);
                        });
                    });
                }

                ui.separator();

                // IME et HALT (allumés quand actifs).
                ui.vertical_centered(|ui| {
                    ui.horizontal(|ui| {
                        let ime_color = if cpu.ime {
                            Color32::GREEN
                        } else {
                            Self::DIM_GRAY
                        };
                        ui.label(egui::RichText::new("IME").color(ime_color));
                        ui.add_space(48.0);
                        let halt_color = if cpu.halted {
                            Color32::GREEN
                        } else {
                            Self::DIM_GRAY
                        };
                        ui.label(egui::RichText::new("HALT").color(halt_color));
                    });
                });

                // BOOTROM : allumé quand le PC est dans la région du boot ROM ($0100-$014F).
                let in_bootrom = (0x0100..=0x014F).contains(&cpu.pc);
                ui.vertical_centered(|ui| {
                    ui.label(egui::RichText::new("BOOTROM").color(if in_bootrom {
                        Color32::GREEN
                    } else {
                        Self::DIM_GRAY
                    }));
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

    /// Ligne PC/SP (structure fixe) : étiquette monospace calée sur 2 caractères + valeur hexadécimale,
    /// puis expansion binaire 16 bits en gris foncé. La colonne `$` est alignée avec les cellules de
    /// registres ci-dessous.
    fn show_pointer_row(ui: &mut egui::Ui, name: &str, value: u16) {
        ui.horizontal(|ui| {
            ui.label(
                egui::RichText::new(format!("{name:<2}"))
                    .monospace()
                    .color(Color32::YELLOW),
            );
            ui.monospace(format!(" ${value:04X}"));
        });
        let binary = format!("{:08b} {:08b}", (value >> 8) as u8, value & 0xFF);
        ui.label(
            egui::RichText::new(binary)
                .monospace()
                .color(Self::DIM_GRAY),
        );
    }

    /// Cellule de registre (structure fixe) : nom monospace calé sur 2 caractères + valeur hexadécimale,
    /// puis expansion binaire en gris foncé. La colonne `$` est alignée with the lines PC/SP au-dessus.
    fn show_register_cell(ui: &mut egui::Ui, name: char, value: u8) {
        ui.horizontal(|ui| {
            ui.label(
                egui::RichText::new(format!("{name:<2}"))
                    .monospace()
                    .color(Color32::CYAN),
            );
            ui.monospace(format!(" ${value:02X}"));
        });
        let binary = format!("{:04b} {:04b}", value >> 4, value & 0x0F);
        ui.label(
            egui::RichText::new(binary)
                .monospace()
                .color(Self::DIM_GRAY),
        );
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

        // Right panel « Pattern Table » : colorized VRAM tiles (BGP/OBP0/OBP1) — activable via the Debug menu.
        if self.show_pattern_table {
            self.show_pattern_table_panel(ctx);
        }

        // Central zone : debug widgets activable from the Debug menu (Processor, ROM Info, IO Map).
        egui::CentralPanel::default().show(ctx, |ui| {
            let cpu = self.show_cpu_registers;
            let rom = self.show_rom_info;
            let io = self.show_io_map;

            if !cpu && !rom && !io {
                // No active widget : centered hint.
                ui.vertical_centered(|ui| {
                    ui.add_space(ui.available_height() * 0.4);
                    ui.weak("No active debug widgets — open the « 🔍 Debug » menu to enable some.");
                });
            } else if cpu && rom {
                // Processor | ROM Info side by side (top), IO Map below when enabled.
                let available = ui.available_size();
                let top_height = (available.y * 0.5).round().max(120.0);
                // Processor column: 55% of the width when there is enough room, otherwise almost all of it
                // (the ROM Info panel takes the rest) — never wider than the zone itself.
                let left_width = if available.x > 480.0 {
                    (available.x * 0.55).round()
                } else {
                    (available.x - 24.0).max(120.0)
                };
                ui.horizontal(|ui| {
                    ui.scope(|ui| {
                        ui.set_min_width(left_width);
                        ui.set_max_width(left_width);
                        if io {
                            ui.set_max_height(top_height); // IO Map takes the lower half
                        }
                        self.show_processor_panel(ui);
                    });
                    ui.add(egui::Separator::default().vertical());
                    ui.scope(|ui| {
                        if io {
                            ui.set_max_height(top_height);
                        }
                        self.show_rom_info_panel(ui);
                    });
                });
                if io {
                    // Lower half : IO Map (fills the remaining space).
                    self.show_io_map(ui);
                }
            } else if cpu || rom {
                // Single widget (Processor or ROM Info) : full width, IO Map below when enabled.
                let available = ui.available_size();
                if io {
                    let top_height = (available.y * 0.5).round().max(120.0);
                    ui.scope(|ui| {
                        ui.set_max_height(top_height);
                        if cpu {
                            self.show_processor_panel(ui);
                        } else {
                            self.show_rom_info_panel(ui);
                        }
                    });
                    self.show_io_map(ui);
                } else if cpu {
                    self.show_processor_panel(ui);
                } else {
                    self.show_rom_info_panel(ui);
                }
            } else {
                // IO Map only : fills the entire zone.
                self.show_io_map(ui);
            }
        });

        // Window « ⌨️ Input Mapping » (keyboard mapping → joypad $FF00).
        self.show_input_mapping_window(ctx);
    }
}
