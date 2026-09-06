//! Memory Management Unit : pont entre le CPU et la mémoire.
//!
//! Implémente toute la carte d'adresses de la Game Boy (PanDocs « Memory Map »)
//! ainsi que le routage vers le contrôleur MBC unifié ([`mbc::Mbc`] : commutation des banques
//! ROM/SRAM, mode de banking MBC1 et registres RTC MBC3).
//!
//! Étape 1 : les registres PPU ($FF40-$FF4B, dont le DMA OAM $FF46) sont routés vers la structure
//! [`ppu::PPU`] embarquée dans le MMU ; partie 7 : les registres SCC ($FF01-$FF02) vers
//! [`serial::Serial`], et partie 9 : les registres Timer ($FF04-$FF07) vers [`timer::Timer`].
//! Étape 2 : la VRAM ($8000-$9FFF) est bloquée par la PPU only during the Drawing mode (3), and
//! l'OAM ($FE00-$FE9F) is blocked during OAM Scan (mode 2) and Drawing (mode 3) — in both cases, only when the LCD is on:
//! CPU reads return $FF, and writes are ignored (Pan Docs « PPU »).
//! Partie 10 : l'écriture du registre DMA OAM $FF46 démarre le transfert des 160 octets d'OAM
//! ($FE00-$FE9F) depuis l'adresse source `value << 8`, un octet par M-cycle pendant 160 M-cycles
//! (Pan Docs « OAM DMA Transfer ») : durant ce conflit de bus, le CPU ne peut plus accéder qu'à la
//! HRAM ($FF80-$FFFE) — les autres lectures renvoient $FF et les écritures sont ignorées. Le
//! registre Joypad $FF00 est routé vers [`joypad::Joypad`]. Le tableau `io` ne stocke que les
//! valeurs brutes des autres registres I/O.

use crate::cartridge::{CartridgeHeader, parse_header};
use crate::joypad::Joypad;
use crate::mbc::{HEADER_TYPE_ADDR, Mbc, MbcType};
use crate::ppu::PPU;
use crate::serial::Serial;
use crate::timer::Timer;

/// Durée du transfert OAM en M-cycles : 160 octets d'OAM ($FE00-$FE9F), un par cycle (Pan Docs « OAM DMA Transfer »).
pub const DMA_OAM_CYCLES: u32 = 160;

#[allow(clippy::upper_case_acronyms)]
pub struct MMU {
    /// ROM cartouche (fichier `.gb` à plat, banque 0 en tête). Vide tant qu'aucune ROM n'est chargée.
    rom: Vec<u8>,
    /// Video RAM (0x8000-0x9FFF), 8 KiB.
    pub vram: [u8; 0x2000],
    /// Work RAM (0xC000-0xDFFF) : banque 0 + banque 1 sur DMG, 8 KiB au total.
    pub wram: [u8; 0x2000],
    /// Object Attribute Memory (0xFE00-0xFE9F), 160 octets.
    pub oam: [u8; 0xA0],
    /// High RAM (0xFF80-0xFFFE), 127 octets.
    pub hram: [u8; 0x7F],
    /// Registres I/O (0xFF00-0xFF7F) : $FF00 est désormais routé vers le joypad, les autres conservent leur valeur brute.
    pub io: [u8; 0x80],
    /// Registre Interrupt Enable (0xFFFF).
    pub ie: u8,
    /// État HALT du CPU, synchronisé par `Emulator::step` : bit 5 en lecture seule du registre IF ($FF0F).
    pub cpu_halted: bool,
    /// Pixel Processing Unit (registres $FF40-$FF45), synchronisé sur les T-cycles.
    pub ppu: PPU,
    /// Serial Communication Controller (registres $FF01-$FF02), synchronisé sur les T-cycles.
    pub serial: Serial,
    /// Timer (registres $FF04-$FF07), synchronisé sur les T-cycles.
    pub timer: Timer,

    /// Joypad (registre $FF00, P1/JOYP), PanDocs « Joypad Input ».
    pub joypad: Joypad,

    // --- Contrôleur MBC cartouche (PanDocs « MBCs ») ---
    /// État unifié du contrôleur de mémoire : type détecté ($0147), banques ROM/SRAM,
    /// activation RAM, mode de banking MBC1 et registres RTC MBC3.
    pub mbc: Mbc,
    // --- En-tête de cartouche analysé (PanDocs « The Cartridge Header ») ---
    /// En-tête de cartouche ($0100-$014F) analysé au chargement ; None tant qu'aucune ROM n'est chargée.
    pub cartridge: Option<CartridgeHeader>,

    // --- Boot ROM (PanDocs « Boot ROM » / GBCTR Chapter 7) ---
    /// Image de la boot ROM DMG/MGB réelle (256 octets), mappée à $0000-$00FF au power-on : le CPU
    /// démarre à $0000 et l'exécute ; tant que `boot_rom_finished` est faux, les lectures de
    /// $0000-$00FF sont servies par cette image et les écritures de cette zone sont ignorées.
    pub boot_rom: [u8; 0x100],
    /// Registre rBANK ($FF50), bit 0 « BOOT_OFF » : une fois à 1 (écriture impaire), la boot ROM est
    /// définitivement dé-mappée jusqu'au prochain reset. Lecture de $FF50 = `0xFE | BOOT_OFF`.
    pub boot_rom_finished: bool,

    // --- Transfert DMA OAM en cours (Pan Docs « OAM DMA Transfer ») ---
    /// M-cycles restants du transfert OAM déclenché par l'écriture de $FF46 (0 = aucun transfert).
    pub dma_remaining: u32,
    /// Adresse source du transfert OAM en cours : `value << 8` ($0000-$DFFF), un octet lu par M-cycle.
    pub dma_source_addr: u16,
    /// Posé quand $FF46 est écrit pendant l'instruction courante : le transfert démarre après cette
    /// instruction (l'écriture a lieu sur son dernier M-cycle) — consommé par [`MMU::advance_dma`].
    dma_just_started: bool,
}

impl MMU {
    pub fn new() -> Self {
        Self::default()
    }

    /// Charge une ROM `.gb` et réinitialise la mémoire à l'état power-on.
    /// Analyse et valide l'en-tête de cartouche ($0100-$014F) — PanDocs « The Cartridge Header » :
    /// un logo Nintendo ou une somme de contrôle d'en-tête invalide ne produit qu'un avertissement
    /// (certains homebrews n'ont pas le logo), la ROM est chargée quand même.
    pub fn load_rom(&mut self, data: Vec<u8>) {
        let mut fresh = Self::new();
        match parse_header(&data) {
            Ok(header) => {
                if !header.logo_valid {
                    log::warn!(
                        "[MMU] Logo Nintendo invalide ($0104-$0133) : la boot ROM se verrouillerait sur le matériel réel — continuation quand même (homebrew ?)."
                    );
                }
                if !header.header_checksum_valid {
                    log::warn!(
                        "[MMU] Somme de contrôle d'en-tête invalide : $014D = ${:02X} mais calculée ${:02X} sur $0134-$014C — continuation quand même.",
                        header.header_checksum,
                        header.computed_header_checksum
                    );
                }
                log::debug!(
                    "[MMU] Cartouche : « {} » — {:?}, taille ROM {:?} octets, RAM externe {} octets, code de destination ${:02X}",
                    header.title,
                    header.mbc_type,
                    header.rom_size_bytes,
                    header.ram_size_bytes,
                    header.destination_code
                );
                let mbc_type = header.mbc_type; // type détecté depuis $0147 (PanDocs « The Cartridge Header »)
                let rom_size = data.len(); // taille réelle de la ROM : masque les bits de banque invalides (PanDocs « MBCs »)
                fresh.cartridge = Some(header);
                fresh.rom = data;
                fresh.mbc = Mbc::new(mbc_type, rom_size); // état power-up du contrôleur détecté
            }
            Err(err) => {
                log::error!(
                    "[MMU] En-tête de cartouche invalide : {err} — chargement tel quel (ROM ONLY)."
                );
                let header_byte = data.get(HEADER_TYPE_ADDR as usize).copied().unwrap_or(0x00);
                let rom_size = data.len(); // taille réelle de la ROM : masque les bits de banque invalides (PanDocs « MBCs »)
                fresh.rom = data;
                fresh.mbc = Mbc::new(MbcType::from_header_byte(header_byte), rom_size); // repli : type détecté depuis $0147 si lisible
            }
        }
        *self = fresh;
    }

    /// En-tête de cartouche analysé au chargement (None tant qu'aucune ROM n'est chargée).
    #[allow(dead_code)] // API publique du MMU — utilisée par les tests et les parties futures (affichage UI, sauvegarde d'état).
    pub fn cartridge(&self) -> Option<&CartridgeHeader> {
        self.cartridge.as_ref()
    }

    /// Réinitialise les régions de mémoire à leurs valeurs au power-on (PanDocs « Power Up Sequence ») :
    /// VRAM $FF (DMG), WRAM banque 0 = $11 / banque 1 = $FF, OAM $FF, HRAM $FF.
    /// La pile du CPU (SP=$FFFE au hand-off) pointe dans la HRAM, qui contient donc $FF comme sur le matériel réel.
    pub fn reset_memory(&mut self) {
        self.vram.fill(0xFF); // $8000-$9FFF : $FF sur DMG (aléatoire sur GB).
        for byte in &mut self.wram[..0x1000] {
            *byte = 0x11; // $C000-$CFFF : banque 0 WRAM.
        }
        for byte in &mut self.wram[0x1000..] {
            *byte = 0xFF; // $D000-$DFFF : banque 1 WRAM.
        }
        self.oam.fill(0xFF); // $FE00-$FE9F.
        self.hram.fill(0xFF); // $FF80-$FFFE : la pile est ici (SP=$FFFE).
    }

    /// Taille de la ROM chargée en octets (0 si aucune ROM n'est chargée).
    pub fn rom_size(&self) -> usize {
        self.rom.len()
    }

    /// Lit un octet sur toute la carte d'adresses 16 bits (accès CPU).
    /// Les zones non mappées renvoient $FF, comme sur le matériel réel.
    /// Pendant le DMA OAM, seule la HRAM ($FF80-$FFFE) est accessible au CPU : les autres lectures
    /// renvoient $FF (conflit de bus — Pan Docs « OAM DMA Transfer »).
    pub fn read(&self, addr: u16) -> u8 {
        // Boot ROM mapping (PanDocs « Boot ROM » / GBCTR Chapter 7): while BOOT_OFF=0 ($FF50 bit 0),
        // $0000-$00FF is served by the boot ROM image instead of the cartridge; reads of $0100+ still reach it.
        if !self.boot_rom_finished && addr < 0x100 {
            return self.boot_rom[addr as usize];
        }
        if self.dma_active() && !Self::is_hram(addr) {
            return 0xFF; // Bus occupé par le transfert OAM : lecture CPU bloquée.
        }
        self.read_plain(addr)
    }

    /// Debug read (memory editor): reads a byte across the full 16-bit address map without PPU/DMA bus blocking;
    /// a debugger must always show actual RAM contents, even during OAM Scan / Drawing modes or DMA transfers.
    pub fn read_debug(&self, addr: u16) -> u8 {
        if !self.boot_rom_finished && addr < 0x100 {
            return self.boot_rom[addr as usize];
        }
        self.read_plain(addr)
    }

    /// La PPU bloque-t-elle la VRAM ($8000-$9FFF) au CPU ? Seulement pendant le mode Drawing (3), et
    /// uniquement quand le LCD est allumé (bit 7 du LCDC) — Pan Docs « PPU ».
    fn vram_blocked(&self) -> bool {
        self.ppu.lcdc & crate::ppu::LCDC_LCD_ON != 0 && self.ppu.mode == 3
    }

    /// La PPU bloque-t-elle l'OAM ($FE00-$FE9F) au CPU ? Pendant les modes OAM Scan (2) et Drawing (3),
    /// et uniquement quand le LCD est allumé — Pan Docs « PPU ».
    fn oam_blocked(&self) -> bool {
        self.ppu.lcdc & crate::ppu::LCDC_LCD_ON != 0 && (self.ppu.mode == 2 || self.ppu.mode == 3)
    }

    /// Lit un octet sur toute la carte d'adresses sans appliquer le conflit de bus du DMA OAM —
    /// utilisé par l'unité DMA elle-même, qui n'est pas affectée par ce conflit (Pan Docs « OAM DMA Transfer »).
    fn read_plain(&self, addr: u16) -> u8 {
        match addr {
            // Région ROM : banque active selon le contrôleur MBC (PanDocs « MBC1 » / « MBC3 »).
            0x0000..=0x7FFF => self.read_rom_at(self.mbc.get_rom_addr(addr) as u32),
            // Video RAM : inaccessible au CPU pendant le mode Drawing (3) de la PPU — la lecture renvoie $FF,
            // comme sur le matériel réel (Pan Docs « PPU »). Pendant l'OAM Scan (mode 2), seule l'OAM est bloquée.
            0x8000..=0x9FFF => {
                if self.vram_blocked() {
                    return 0xFF; // Bus bloqué par la PPU.
                }
                self.vram[(addr - 0x8000) as usize]
            }
            // SRAM cartouche / registres RTC (open bus $FF si la RAM est désactivée ou absente).
            0xA000..=0xBFFF => self.mbc.read_ram(addr).unwrap_or(0xFF),
            // Work RAM.
            0xC000..=0xDFFF => self.wram[(addr - 0xC000) as usize],
            // Echo RAM : miroir de C000-DDFF (seuls les 13 bits bas d'adresse sont connectés).
            0xE000..=0xFDFF => self.read_plain(addr - 0x2000),
            // Object Attribute Memory : inaccessible au CPU pendant les modes OAM Scan (2) and Drawing (3) of the PPU.
            0xFE00..=0xFE9F => {
                if self.oam_blocked() {
                    return 0xFF; // Bus bloqué par la PPU en mode OAM Scan / Drawing.
                }
                self.oam[(addr - 0xFE00) as usize]
            }
            // Non utilisable (FEA0-FEFF) : $00 sur DMG hors bloc OAM ; pendant le DMA OAM, la lecture
            // CPU est bloquée par le conflit de bus et renvoie $FF (voir `read`).
            0xFEA0..=0xFEFF => 0x00,
            // Serial Communication Controller (partie 7).
            0xFF01 => self.serial.read_sb(),
            0xFF02 => self.serial.sc,
            // Registres Timer (partie 9).
            0xFF04 => self.timer.read_div(),
            0xFF05 => self.timer.read_tima(),
            0xFF06 => self.timer.read_tma(),
            0xFF07 => self.timer.read_tac(),
            // Registre LY ($FF44) : routé explicitement vers la PPU — ne doit jamais tomber dans un cas par défaut.
            0xFF44 => self.ppu.read_register(0xFF44),
            // Registres PPU (étape 1) : $FF40-$FF4B, dont le DMA OAM ($FF46).
            0xFF40..=0xFF4B => self.ppu.read_register(addr),
            // Registre Joypad (partie 10).
            0xFF00 => self.joypad.read(),
            // Registre Interrupt Flag ($FF0F) : bits 0–4 = drapeaux d'interruption, bit 5 = halted (read-only),
            // bits 7-6 non utilisés.
            0xFF0F => self.io[0x0F] | if self.cpu_halted { 0x20 } else { 0 },
            // Registre rBANK ($FF50, PanDocs « Boot ROM »): bits 7-1 se lisent à 1 ; bit 0 = BOOT_OFF.
            0xFF50 => 0xFE | u8::from(self.boot_rom_finished),
            // Autres registres I/O.
            0xFF00..=0xFF7F => self.io[(addr - 0xFF00) as usize],
            // High RAM.
            0xFF80..=0xFFFE => self.hram[(addr - 0xFF80) as usize],
            // Registre Interrupt Enable.
            0xFFFF => self.ie,
        }
    }

    /// Écrit un octet sur toute la carte d'adresses 16 bits (accès CPU).
    /// Pendant le DMA OAM, seule la HRAM ($FF80-$FFFE) est accessible au CPU : les autres écritures
    /// sont ignorées car le bus est occupé par l'unité DMA (Pan Docs « OAM DMA Transfer »).
    #[allow(dead_code)] // Utilisé à partir de la partie 3 (le CPU écrit en mémoire).
    pub fn write(&mut self, addr: u16, value: u8) {
        if self.dma_active() && !Self::is_hram(addr) {
            return; // Bus occupé par le transfert OAM : écriture CPU ignorée.
        }
        // Boot ROM mapping (PanDocs « Boot ROM » / GBCTR Chapter 7): while BOOT_OFF=0, writes to
        // $0000-$00FF are ignored (they cannot reach the cartridge/MBC) — only reads are intercepted.
        if !self.boot_rom_finished && addr < 0x100 {
            return;
        }
        // Registre rBANK ($FF50, PanDocs « Boot ROM »): bit 0 « BOOT_OFF » only transitions 0→1 (once the
        // boot ROM is unmapped it stays so until reset); bits 7-1 are ignored on write. Reading $FF50 = 0xFE|BOOT_OFF.
        if addr == 0xFF50 {
            self.boot_rom_finished |= value & 1 != 0;
            return;
        }
        match addr {
            // Région ROM : les écritures vont aux registres du contrôleur MBC actif (la ROM est en lecture seule).
            // Le routage exact des adresses dépend du type détecté ($0147) — PanDocs « MBCs » / « MBC2 ».
            0x0000..=0x7FFF => self.mbc.write_register(addr, value),
            // Video RAM : les écritures sont ignorées pendant le mode Drawing (3) de la PPU (LCD allumé).
            0x8000..=0x9FFF => {
                if self.vram_blocked() {
                    return; // Bus bloqué par la PPU : écriture ignorée.
                }
                self.vram[(addr - 0x8000) as usize] = value;
            }
            // SRAM cartouche / registres RTC (ignorées si la RAM est désactivée ou absente).
            0xA000..=0xBFFF => self.mbc.write_ram(addr, value),
            // Work RAM.
            0xC000..=0xDFFF => self.wram[(addr - 0xC000) as usize] = value,
            // Echo RAM : les écritures sont miroirées vers C000-DDFF.
            0xE000..=0xFDFF => self.write(addr - 0x2000, value),
            // Object Attribute Memory : les écritures sont ignorées pendant the modes OAM Scan (2) and Drawing (3) of the PPU.
            0xFE00..=0xFE9F => {
                if self.oam_blocked() {
                    return; // Bus bloqué par la PPU en mode OAM Scan / Drawing : écriture ignorée.
                }
                self.oam[(addr - 0xFE00) as usize] = value;
            }
            // Non utilisable : écritures ignorées sur DMG.
            0xFEA0..=0xFEFF => {}
            // Serial Communication Controller (partie 7).
            0xFF01 => self.serial.write_sb(value),
            0xFF02 => self.serial.write_sc(value),
            // Registres Timer (partie 9).
            0xFF04 => self.timer.write_div(value),
            0xFF05 => self.timer.write_tima(value),
            0xFF06 => self.timer.write_tma(value),
            0xFF07 => self.timer.write_tac(value),
            // Registres PPU (étape 1) : $FF40-$FF4B ; LY ($FF44) est en lecture seule — écriture ignorée.
            0xFF46 => {
                // DMA OAM (Pan Docs « OAM DMA Transfer ») : l'écriture de $FF46 démarre le transfert des
                // 160 octets d'OAM ($FE00-$FE9F) depuis l'adresse source `value << 8`, un octet par M-cycle
                // pendant 160 M-cycles. Le registre mémorise la valeur ; seules les valeurs $00-$DF
                // sélectionnent une adresse source valide (ROM ou RAM uniquement).
                self.ppu.write_register(addr, value); // enregistre l'adresse source dans le registre DMA
                if value < 0xE0 {
                    self.dma_source_addr = (value as u16) << 8;
                    self.dma_remaining = DMA_OAM_CYCLES;
                    self.dma_just_started = true; // le transfert démarre après cette instruction.
                }
            }
            0xFF40..=0xFF4B => self.ppu.write_register(addr, value),
            // Registre Joypad (partie 10) : seuls les bits 4-5 sont écriturables.
            0xFF00 => self.joypad.write(value),
            // Registre Interrupt Flag ($FF0F) : write-1-to-clear for bits 0–4 (Pan Docs « Interrupt Sources ») —
            // c'est au jeu, dans its ISR, d'acknowledge an interrupt by writing a 1 in the bit corresponding to $FF0F.
            0xFF0F => self.io[0x0F] &= !(value & 0x1F),
            // Autres registres I/O.
            0xFF00..=0xFF7F => self.io[(addr - 0xFF00) as usize] = value,
            // High RAM.
            0xFF80..=0xFFFE => self.hram[(addr - 0xFF80) as usize] = value,
            // Registre Interrupt Enable.
            0xFFFF => self.ie = value,
        }
    }

    /// Indique si un transfert DMA OAM est en cours (Pan Docs « OAM DMA Transfer »).
    pub fn dma_active(&self) -> bool {
        self.dma_remaining > 0
    }

    /// Réinitialise l'état du transfert DMA OAM (power-on / reset — Pan Docs « OAM DMA Transfer »).
    pub fn reset_dma(&mut self) {
        self.dma_remaining = 0;
        self.dma_just_started = false;
    }

    /// HRAM ($FF80-$FFFE) : la seule région accessible au CPU pendant le DMA OAM (Pan Docs « OAM DMA Transfer »).
    fn is_hram(addr: u16) -> bool {
        (0xFF80..=0xFFFE).contains(&addr)
    }

    /// Fait avancer le transfert DMA OAM de `cycles` M-cycles : un octet est copié par cycle, depuis
    /// l'adresse source (`value << 8`) vers l'OAM ($FE00-$FE9F), lu via la carte d'adresses complète —
    /// l'unité DMA n'est pas affectée par le conflit de bus qu'elle provoque (Pan Docs « OAM DMA Transfer »).
    /// Si $FF46 a été écrit pendant ces mêmes cycles, aucun octet n'est copié : l'écriture a lieu sur le
    /// dernier M-cycle de son instruction, donc la fenêtre de 160 M-cycles démarre après celle-ci.
    pub fn advance_dma(&mut self, cycles: u32) {
        if self.dma_just_started {
            self.dma_just_started = false; // ces cycles ont déjà eu lieu avant (ou pendant) l'écriture de $FF46.
            return;
        }
        if self.dma_remaining == 0 {
            return; // aucun transfert en cours.
        }
        let n = cycles.min(self.dma_remaining) as usize;
        for _ in 0..n {
            let idx = self.oam.len() - self.dma_remaining as usize; // prochain slot OAM ($FE00 + idx)
            self.oam[idx] = self.read_plain(self.dma_source_addr + idx as u16);
            self.dma_remaining -= 1;
        }
    }

    /// Lit dans le fichier ROM ; $FF au-delà de sa fin (open bus).
    fn read_rom_at(&self, idx: u32) -> u8 {
        match self.rom.get(idx as usize) {
            Some(byte) => *byte,
            None => 0xFF,
        }
    }

    /// Raw cartridge ROM byte at a flat offset (bank 0 first), $FF beyond the end of the file
    /// (open bus) — used by the Pattern Table widget to render raw ROM banks as tile grids.
    pub fn rom_raw(&self, idx: usize) -> u8 {
        self.read_rom_at(idx as u32)
    }
}

impl Default for MMU {
    fn default() -> Self {
        let mut mmu = Self {
            rom: Vec::new(),
            vram: [0; 0x2000],
            wram: [0; 0x2000],
            oam: [0; 0xA0],
            hram: [0; 0x7F],
            io: [0; 0x80],
            ie: 0,
            cpu_halted: false, // le CPU n'est pas halté au power-on (bit 5 de IF lu à $00).
            ppu: PPU::new(),
            serial: Serial::default(),
            timer: Timer::default(),
            joypad: Joypad::default(),
            mbc: Mbc::new(MbcType::RomOnly, 0), // état power-up (aucune ROM chargée) ; remplacé par le type détecté dans `load_rom`.
            cartridge: None,                    // remplacé par l'en-tête analysé dans `load_rom`.

            // Boot ROM (PanDocs « Boot ROM ») : image DMG réelle ; `boot_rom_finished` = rBANK ($FF50) bit 0.
            // Default à true (dé-mappée) pour que les tests de la carte d'adresses/MBC voient $0000-$00FF comme
            // une ROM cartouche normale ; `Emulator::load_rom_with_boot` le met à false pour exécuter la boot ROM.
            boot_rom: crate::bootrom::DMG_BOOT_ROM,
            boot_rom_finished: true,
            dma_remaining: 0,                   // aucun transfert OAM en cours au power-on.
            dma_source_addr: 0,
            dma_just_started: false,
        };
        mmu.reset_memory(); // valeurs au power-on (PanDocs « Power Up Sequence ») : VRAM/OAM/HRAM = $FF, WRAM banque 0 = $11 / banque 1 = $FF.
        mmu
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cpu::CPU;
    use crate::mbc::ROM_BANK_SIZE;

    /// Construit une ROM minimale de 32 KiB (sans MBC) avec `title` à l'emplacement du titre (0x0134).
    fn rom_with_title(title: &[u8]) -> Vec<u8> {
        let mut rom = vec![0xFF; 0x4000];
        rom[0x0134..0x0134 + title.len()].copy_from_slice(title);
        rom
    }

    #[test]
    fn read_game_title_at_0x0134() {
        let mut mmu = MMU::new();
        mmu.load_rom(rom_with_title(b"FARQUAADGB TEST"));

        let title: Vec<u8> = (0..16).map(|i| mmu.read(0x0134 + i)).collect();
        assert_eq!(&title[..15], b"FARQUAADGB TEST");
    }

    #[test]
    fn reads_beyond_rom_size_return_ff() {
        let mut mmu = MMU::new();
        mmu.load_rom(vec![0x42; 0x100]); // ROM minuscule
        assert_eq!(mmu.read(0x0000), 0x42);
        assert_eq!(mmu.read(0x00FF), 0x42);
        assert_eq!(mmu.read(0x0100), 0xFF); // open bus au-delà du fichier
    }

    #[test]
    fn small_32kib_roms_map_the_whole_file_without_open_bus() {
        // Une ROM de 32 KiB a des indices valides de 0 à 32767 : quel que soit le type d'en-tête (ROM ONLY, MBC1 ou MBC5)
        // et quelle que soit la valeur écrite dans les registres de banque, une lecture $0000-$7FFF renvoie un octet du fichier.
        let base: Vec<u8> = (0..32 * 1024).map(|i| if i < ROM_BANK_SIZE { 0x11 } else { 0x22 }).collect(); // banque 0 en $11, banque 1 en $22

        // Cas 1 : en-tête $0147 = $00 → ROM ONLY : cartographie plate du fichier entier.
        let mut rom = base.clone();
        rom[HEADER_TYPE_ADDR as usize] = 0x00;
        let mut mmu = MMU::new();
        mmu.load_rom(rom);
        assert_eq!(mmu.read(0x3FFF), 0x11); // dernier octet de la première moitié
        assert_eq!(mmu.read(0x4000), 0x22); // seconde moitié du fichier à $4000-$7FFF
        assert_eq!(mmu.read(0x7FFF), 0x22); // dernier octet du fichier

        // Cas 2 : en-tête $0147 = $01 → MBC1 : seul le bit 0 du registre de banque est valide (deux banques).
        let mut rom = base.clone();
        rom[HEADER_TYPE_ADDR as usize] = 0x01;
        let mut mmu = MMU::new();
        mmu.load_rom(rom);
        for reg in 0..=0xFFu8 {
            mmu.write(0x2000, reg); // registre de banque ROM
            assert_eq!(mmu.read(0x4000), 0x22); // règle du zéro + bit unique valide → toujours la banque 1
            assert_eq!(mmu.read(0x7FFF), 0x22); // dernier octet du fichier, jamais l'open bus
        }
        mmu.write(0x6000, 0x80); // mode avancé : les bits supérieurs sont aussi masqués vers les deux banques existantes
        for ram_reg in 0..=0xFFu8 {
            mmu.write(0x4000, ram_reg);
            mmu.write(0x2000, 0x00);
            assert_eq!(mmu.read(0x4000), 0x22); // règle du zéro : registre $00 → banque 1
            mmu.write(0x2000, 0x02);
            assert_eq!(mmu.read(0x4000), 0x11); // seul le bit 0 valide : pair non nul → banque 0
        }

        // Cas 3 : en-tête $0147 = $11 → MBC5 : au power-up, le registre $00 sélectionne réellement la banque $00 ; seul le bit 0 est valide.
        let mut rom = base;
        rom[HEADER_TYPE_ADDR as usize] = 0x11;
        let mut mmu = MMU::new();
        mmu.load_rom(rom);
        assert_eq!(mmu.read(0x4000), 0x11); // power-up : registre $00 → réellement la banque $00 (PanDocs « MBC5 »)
        for reg in 0..=0xFFu8 {
            mmu.write(0x2000, reg); // bits 0-7 de la banque ROM
            let expected = if reg & 1 == 1 { 0x22 } else { 0x11 };
            assert_eq!(mmu.read(0x4000), expected); // seul le bit 0 valide, jamais hors plage du fichier
        }
    }

    #[test]
    fn echo_ram_mirrors_wram() {
        let mut mmu = MMU::new();
        mmu.write(0xC000, 0xAB);
        assert_eq!(mmu.read(0xE000), 0xAB); // miroir de C000
        mmu.write(0xDDFF, 0xCD);
        assert_eq!(mmu.read(0xFDFF), 0xCD); // miroir de DDFF (haut de la plage)
        // Les écritures via l'Echo RAM atteignent aussi la WRAM.
        mmu.write(0xE123, 0x78);
        assert_eq!(mmu.read(0xC123), 0x78);
    }

    #[test]
    fn hram_and_ie_register() {
        let mut mmu = MMU::new();
        mmu.write(0xFF80, 0x11);
        assert_eq!(mmu.read(0xFF80), 0x11);
        mmu.write(0xFFFE, 0x22); // dernier octet HRAM
        assert_eq!(mmu.read(0xFFFE), 0x22);
        mmu.write(0xFFFF, 0b1010_0000);
        assert_eq!(mmu.read(0xFFFF), 0b1010_0000); // registre IE
    }

    #[test]
    fn oam_region() {
        let mut mmu = MMU::new();
        mmu.ppu.mode = 0; // HBlank : l'OAM est accessible au CPU (la PPU ne le bloque qu'en modes 2/3, LCD allumé).
        mmu.write(0xFE00, 0x90);
        assert_eq!(mmu.read(0xFE00), 0x90);
        mmu.write(0xFE9F, 0x12); // dernier octet OAM
        assert_eq!(mmu.read(0xFE9F), 0x12);
        assert_eq!(mmu.read(0xFEA0), 0x00); // non utilisable → $00 sur DMG (hors bloc OAM)
    }

    #[test]
    fn mbc_rom_bank_switching() {
        let mut rom = vec![0x00; 0xC000]; // 48 KiB : banques 0-2
        rom[0x3FFF] = 0x99; // banque 0, dernier octet (toujours mappé)
        rom[0x4000] = 0x11; // banque 1, premier octet
        rom[0x8000] = 0x22; // banque 2, premier octet
        rom[HEADER_TYPE_ADDR as usize] = 0x01; // en-tête $0147 : MBC1

        let mut mmu = MMU::new();
        mmu.load_rom(rom);

        assert_eq!(mmu.read(0x3FFF), 0x99); // région banque 0 fixe
        assert_eq!(mmu.read(0x4000), 0x11); // power-up : $00 se comporte comme la banque $01
        mmu.write(0x2000, 0x02); // les jeux écrivent le registre de banque à 0x2000 (PanDocs « MBC1 »)
        assert_eq!(mmu.read(0x4000), 0x22); // maintenant la banque 2
        mmu.write(0x2000, 0x00);
        assert_eq!(mmu.read(0x4000), 0x11); // $00 → retour à la banque $01
    }

    #[test]
    fn rom_only_cart_ignores_writes() {
        let mut rom = vec![0x00; 0x8000]; // ROM ONLY (en-tête $0147 = $00)
        rom[0x4000] = 0x11;

        let mut mmu = MMU::new();
        mmu.load_rom(rom);

        assert_eq!(mmu.read(0x4000), 0x11); // ROM plate : aucune commutation de banque
        mmu.write(0x2000, 0x05); // « ROM ONLY » : les écritures dans $0000-$7FFF sont ignorées
        assert_eq!(mmu.read(0x4000), 0x11);
    }

    #[test]
    fn mbc_cartridge_ram_requires_enable() {
        let mut rom = vec![0xFF; 0x8000]; // MBC1 + RAM (en-tête $0147 = $02)
        rom[HEADER_TYPE_ADDR as usize] = 0x02;

        let mut mmu = MMU::new();
        mmu.load_rom(rom);

        // Désactivée par défaut : écritures ignorées, lectures open bus $FF.
        mmu.write(0xA000, 0x42);
        assert_eq!(mmu.read(0xA000), 0xFF);

        // Activée avec $0A (toute valeur avec $A dans les 4 bits bas).
        mmu.write(0x0000, 0x0A);
        mmu.write(0xA000, 0x42);
        assert_eq!(mmu.read(0xA000), 0x42);

        // Toute autre valeur la désactive à nouveau.
        mmu.write(0x0000, 0x00);
        assert_eq!(mmu.read(0xA000), 0xFF);
    }

    #[test]
    fn mbc5_two_part_rom_bank() {
        let mut rom = vec![0x00; ROM_BANK_SIZE * 0x180]; // 6 MiB : banques 0-383 (MBC5)
        rom[0x4000] = 0x11; // banque 1, premier octet
        rom[0x7F * ROM_BANK_SIZE] = 0x22; // banque $7F
        rom[0x17F * ROM_BANK_SIZE] = 0x33; // banque $17F
        rom[HEADER_TYPE_ADDR as usize] = 0x19; // en-tête $0147 : MBC5

        let mut mmu = MMU::new();
        mmu.load_rom(rom);

        assert_eq!(mmu.read(0x4000), 0); // power-up : registre $00 → réellement la banque $00 (PanDocs « MBC5 »)
        mmu.write(0x2000, 0x01); // bits 0-7 de la banque ROM
        assert_eq!(mmu.read(0x4000), 0x11);
        mmu.write(0x2000, 0x7F);
        assert_eq!(mmu.read(0x4000), 0x22);
        mmu.write(0x3000, 0x01); // bit 8 → banque $17F
        assert_eq!(mmu.read(0x4000), 0x33);
    }

    #[test]
    fn mbc2_bit8_of_address_selects_register() {
        let mut rom = vec![0x00; 0x4000 * 16]; // 256 KiB : banques 0-15 (MBC2)
        rom[0x4000] = 0x11; // banque 1, premier octet
        rom[0x8000] = 0x33; // banque 2, premier octet
        rom[HEADER_TYPE_ADDR as usize] = 0x05; // en-tête $0147 : MBC2

        let mut mmu = MMU::new();
        mmu.load_rom(rom);

        assert_eq!(mmu.read(0x4000), 0x11); // power-up : registre $00 → se comporte comme la banque $01
        mmu.write(0x0100, 0x02); // bit 8 à 1 → registre de banque ROM (PanDocs « MBC2 »)
        assert_eq!(mmu.read(0x4000), 0x33);
        mmu.write(0x0200, 0x0A); // bit 8 à 0 → activation RAM
        assert_eq!(mmu.read(0xA000), 0x00); // la SRAM est maintenant accessible (open bus $FF avant)
    }

    #[test]
    fn serial_registers_are_routed() {
        let mut mmu = MMU::new();
        assert_eq!(mmu.read(0xFF01), 0); // SB à l'état power-on
        assert_eq!(mmu.read(0xFF02), 0); // SC à l'état power-on

        mmu.write(0xFF01, b'A');
        assert_eq!(mmu.read(0xFF01), b'A'); // SB mémorise le prochain octet à émettre

        mmu.write(0xFF02, 0x81); // transfert master (horloge interne) démarré
        assert_eq!(mmu.read(0xFF02) & 0x81, 0x81); // bit 7 à 1 : transfert en cours

        // Les autres registres I/O restent routés vers le tableau `io`.
        mmu.write(0xFF03, 0x42);
        assert_eq!(mmu.read(0xFF03), 0x42);
    }

    #[test]
    fn timer_registers_are_routed() {
        let mut mmu = MMU::new();
        assert_eq!(mmu.read(0xFF04), 0); // DIV à l'état power-on
        assert_eq!(mmu.read(0xFF05), 0); // TIMA à l'état power-on
        assert_eq!(mmu.read(0xFF06), 0); // TMA à l'état power-on
        assert_eq!(mmu.read(0xFF07), 0xF8); // TAC : bits 7-3 toujours lus à 1, timer désactivé

        mmu.write(0xFF05, 0x42);
        assert_eq!(mmu.read(0xFF05), 0x42); // TIMA mémorise la valeur écrite
        mmu.write(0xFF06, 0x33);
        assert_eq!(mmu.read(0xFF06), 0x33); // TMA mémorise la valeur de rechargement

        mmu.write(0xFF07, 0x83); // seuls les bits 2-0 sont pris en compte
        assert_eq!(mmu.read(0xFF07) & 0x07, 0x03);
        assert_eq!(mmu.read(0xFF07) & 0xF8, 0xF8);

        // Les autres registres I/O restent routés vers le tableau `io`.
        mmu.write(0xFF03, 0x42);
        assert_eq!(mmu.read(0xFF03), 0x42);
    }

    #[test]
    fn ppu_registers_are_routed() {
        let mut mmu = MMU::new();
        // OBP0/OBP1/WX/WY ($FF48-$FF4B) sont routés vers la PPU, pas vers `io`.
        mmu.write(0xFF48, 0xFC);
        assert_eq!(mmu.read(0xFF48), 0xFC);
        mmu.write(0xFF49, 0xEF);
        assert_eq!(mmu.read(0xFF49), 0xEF);
        mmu.write(0xFF4A, 48);
        assert_eq!(mmu.read(0xFF4A), 48);
        mmu.write(0xFF4B, 112);
        assert_eq!(mmu.read(0xFF4B), 112);

        // LY ($FF44) est en lecture seule : l'écriture est ignorée.
        mmu.write(0xFF44, 99);
        assert_eq!(mmu.read(0xFF44), 0);

        // STAT ($FF41) : seuls les bits 3..6 sont écrits ; la lecture renvoie le mode PPU courant (bits 0-1, OAM Scan au power-on),
        // les bits d'activation + le drapeau LYC==LY (bit 7).
        mmu.write(0xFF41, 0x78);
        assert_eq!(mmu.read(0xFF41), 0xFA); // bits écrits 0x78 + mode OAM Scan en bits 0-1 (power-on) + drapeau LYC==LY en bit 7 (ly == lyc == 0).

        // Le DMA OAM ($FF46) est routé vers la PPU, pas vers `io`. L'écriture démarre le transfert de
        // 160 M-cycles : pendant le conflit de bus, la lecture de $FF46 (hors HRAM) renvoie $FF.
        mmu.write(0xFF46, 0xC0);
        assert_eq!(mmu.read(0xFF46), 0xFF); // Bus occupé par le DMA → lecture bloquée...
        mmu.advance_dma(8); // cycles de l'instruction ayant écrit $FF46 : aucun octet copié.
        mmu.advance_dma(DMA_OAM_CYCLES); // ...jusqu'à la fin du transfert.
        assert_eq!(mmu.read(0xFF46), 0xC0); // le registre DMA mémorise l'adresse source
    }

    #[test]
    fn dma_oam_transfer_copies_from_source() {
        let mut mmu = MMU::new();
        mmu.ppu.mode = 0; // HBlank : l'OAM est accessible au CPU pour la lecture finale (la PPU ne le bloque qu'en modes 2/3, LCD allumé).
        // Source en WRAM : les 160 octets à $C000-$C09F (source byte $C0 → base $C000).
        for i in 0..0xA0 {
            mmu.write(0xC000 + i, (i as u8) ^ 0x5A);
        }
        // L'écriture de $FF46 = $C0 démarre le transfert : rien n'est copié tant que les M-cycles ne se sont pas écoulés.
        mmu.write(0xFF46, 0xC0);
        assert!(mmu.dma_active()); // le transfert est en cours...
        assert_eq!(mmu.oam[0], 0xFF); // ...et l'OAM n'est pas encore modifiée (valeur power-on $FF ; la lecture CPU de $FE00 renvoie $FF).

        mmu.advance_dma(8); // cycles de l'instruction ayant écrit $FF46 : aucun octet copié.
        assert!(mmu.dma_active());
        assert_eq!(mmu.oam[0], 0xFF);

        mmu.advance_dma(DMA_OAM_CYCLES); // le transfert s'achève (exactement 160 M-cycles).
        assert!(!mmu.dma_active());

        for i in 0..0xA0 {
            assert_eq!(mmu.read(0xFE00 + i), (i as u8) ^ 0x5A); // OAM = contenu de $C000-$C09F
        }
    }

    #[test]
    fn dma_oam_transfer_copies_one_byte_per_cycle() {
        let mut mmu = MMU::new();
        for i in 0..0xA0 {
            mmu.write(0xC000 + i, i as u8);
        }
        mmu.write(0xFF46, 0xC0);
        mmu.advance_dma(8); // cycles de l'instruction ayant écrit $FF46 : aucun octet copié.

        for k in 1..=0xA0 {
            mmu.advance_dma(1);
            assert_eq!(mmu.oam[k - 1], (k - 1) as u8); // l'octet k-1 vient d'être copié...
            if k < 0xA0 {
                assert_eq!(mmu.oam[k], 0xFF); // ...le suivant ne l'est pas encore (valeur power-on $FF).
            }
        }
        assert!(!mmu.dma_active()); // le transfert s'achève exactement après le 160e M-cycle.
    }

    #[test]
    fn dma_oam_transfer_reads_through_full_address_map() {
        let mut mmu = MMU::new();
        mmu.ppu.mode = 0; // HBlank : la VRAM est accessible au CPU (la PPU ne la bloque qu'en mode 3, LCD allumé).
        // Source en VRAM : les 160 octets à $8200-$829F (source byte $82 → base $8200).
        for i in 0..0xA0 {
            mmu.write(0x8200 + i, 0x3C);
        }
        mmu.write(0xFF46, 0x82);
        mmu.advance_dma(8); // cycles de l'instruction ayant écrit $FF46 : aucun octet copié.
        mmu.advance_dma(DMA_OAM_CYCLES);

        assert_eq!(mmu.read(0xFE00), 0x3C); // premier octet OAM = $8200
        assert_eq!(mmu.read(0xFE9F), 0x3C); // dernier octet OAM = $829F
    }

    #[test]
    fn dma_oam_transfer_restricts_cpu_access_for_160_cycles() {
        let mut mmu = MMU::new();
        for i in 0..0xA0 {
            mmu.write(0xC000 + i, (i as u8) ^ 0x5A);
        }
        // La HRAM reste accessible pendant le transfert.
        mmu.write(0xFF80, 0x11);
        mmu.write(0xFFFE, 0x22);

        mmu.write(0xFF46, 0xC0);
        mmu.advance_dma(8); // cycles de l'instruction ayant écrit $FF46 : aucun octet copié.

        for _ in 0..DMA_OAM_CYCLES - 1 {
            mmu.advance_dma(1);
            assert!(mmu.dma_active()); // la fenêtre dure exactement 160 M-cycles...
            assert_eq!(mmu.read(0xC000), 0xFF); // ...les lectures WRAM sont bloquées → $FF.
            assert_eq!(mmu.read(0x8000), 0xFF); // VRAM aussi.
            assert_eq!(mmu.read(0xFFFF), 0xFF); // même IE ($FFFF) — hors HRAM.
            mmu.write(0xC000, 0x77); // les écritures sont ignorées (bus occupé par l'unité DMA).
            assert_eq!(mmu.read(0xFF80), 0x11); // ...sauf la HRAM, toujours accessible.
            assert_eq!(mmu.read(0xFFFE), 0x22);
        }

        mmu.advance_dma(1); // le 160e M-cycle : le transfert s'achève.
        assert!(!mmu.dma_active());
        assert_eq!(mmu.read(0xC000), 0x5A); // l'écriture faite pendant le DMA a été ignorée...
        assert_eq!(mmu.read(0xFF80), 0x11); // ...et la HRAM conserve les valeurs écrites pendant le DMA.
    }

    #[test]
    fn dma_oam_write_outside_valid_source_range_does_not_start_transfer() {
        let mut mmu = MMU::new();
        for i in 0..0xA0 {
            mmu.write(0xC000 + i, (i as u8) ^ 0x5A);
        }

        // $E0 et au-delà : l'adresse source n'est ni en ROM ni en RAM → aucun transfert
        // (Pan Docs « OAM DMA Transfer » : XX = $00 à $DF).
        for value in [0xE0u8, 0xFE, 0xFF] {
            mmu.write(0xFF46, value);
            assert!(!mmu.dma_active()); // aucun transfert démarré.
            assert_eq!(mmu.read(0xFF46), value); // le registre mémorise la valeur écrite.
        }

        // Une écriture valide démarre bien un transfert de 160 M-cycles.
        mmu.write(0xFF46, 0xC0);
        assert!(mmu.dma_active());
    }

    #[test]
    fn mbc3_header_enables_rom_and_rtc_banking() {
        let mut rom = vec![0x00; 0x8000 * 4]; // 128 KiB (MBC3)
        rom[0x147] = 0x0B; // MBC3 with TIMER (PanDocs « The Cartridge Header »)
        rom[0x4000] = 0x11; // banque $01, premier octet
        rom[0x8000] = 0x22; // banque $02, premier octet

        let mut mmu = MMU::new();
        mmu.load_rom(rom);

        assert_eq!(mmu.read(0x4000), 0x11); // power-up : registre ROM $00 → banque $01
        mmu.write(0x2000, 0x02);
        assert_eq!(mmu.read(0x4000), 0x22); // maintenant la banque $02

        // Registres RTC : RAM activée + sélection $08 → secondes à $A000.
        mmu.write(0x0000, 0x0A);
        mmu.write(0x4000, 0x08);
        assert!(mmu.read(0xA000) < 60); // secondes initialisées depuis l'heure système
        mmu.write(0xA001, 59);
        assert_eq!(mmu.read(0xA001), 59); // minutes

        // RAM désactivée → open bus $FF même avec un registre RTC sélectionné.
        mmu.write(0x0000, 0x00);
        assert_eq!(mmu.read(0xA000), 0xFF);

        // Banques SRAM : sélection $01 → banque 1 (RAM réactivée).
        mmu.write(0x0000, 0x0A);
        mmu.write(0x4000, 0x01);
        mmu.write(0xA000, 0xAB);
        assert_eq!(mmu.read(0xA000), 0xAB);
    }

    #[test]
    fn mbc1_mode1_upper_bits_apply_to_both_regions() {
        let mut rom = vec![0x00; 0x10_0000]; // 1 MiB : banques $00-$3F
        rom[0x147] = 0x03; // MBC1+RAM+BATTERY (PanDocs « The Cartridge Header »)
        rom[0x0000] = 0x10; // banque $00, premier octet
        rom[0x80000] = 0xA2; // banque $20, premier octet (32 × 16 KiB)
        rom[0x84000] = 0xB3; // banque $21, premier octet

        let mut mmu = MMU::new();
        mmu.load_rom(rom);

        assert_eq!(mmu.read(0x0000), 0x10); // mode 0 (power-up) : région $0000-$3FFF en banque $00
        mmu.write(0x6000, 0x80); // mode avancé (PanDocs « MBC1 »)
        assert_eq!(mmu.read(0x0000), 0x10); // bits supérieurs = 0 → toujours la banque $00 ici
        mmu.write(0x4000, 0x20); // bits supérieurs = 1 → banques $20-$3F
        assert_eq!(mmu.read(0x0000), 0xA2); // la région $0000-$3FFF suit les bits supérieurs (mode 1)
        assert_eq!(mmu.read(0x4000), 0xB3); // registre ROM $00 → se comporte comme $21 ($20 + 1)
    }

    /// Construit une ROM de 32 KiB avec le logo officiel et la somme de contrôle d'en-tête valide.
    fn rom_with_valid_header() -> Vec<u8> {
        use crate::cartridge::{
            HEADER_CHECKSUM_ADDR, LOGO_LEN, LOGO_START, NINTENDO_LOGO, compute_header_checksum,
        };
        let mut rom = vec![0u8; 0x4000];
        rom[LOGO_START..LOGO_START + LOGO_LEN].copy_from_slice(&NINTENDO_LOGO); // logo officiel ($0104-$0133)
        rom[0x0134..0x0134 + 16].copy_from_slice(b"FARQUAADGB\0\0\0\0\0\0"); // titre bourgé de $00
        rom[HEADER_CHECKSUM_ADDR as usize] = compute_header_checksum(&rom); // $014D valide sur $0134-$014C
        rom
    }

    #[test]
    fn load_rom_parses_cartridge_header() {
        let mut mmu = MMU::new();
        mmu.load_rom(rom_with_valid_header());

        let header = mmu
            .cartridge()
            .expect("l'en-tête doit être analysé au chargement");
        assert!(header.logo_valid); // logo officiel présent ($0104-$0133)
        assert_eq!(header.title, "FARQUAADGB");
        assert!(header.header_checksum_valid); // $014D == valeur calculée sur $0134-$014C
        assert_eq!(header.mbc_type, MbcType::RomOnly); // $0147 = $00 → ROM ONLY
        assert_eq!(header.rom_size_bytes, Some(32 * 1024)); // $0148 = $00 → 32 KiB
        assert_eq!(mmu.mbc.mbc_type, MbcType::RomOnly); // le contrôleur est initialisé sur le type détecté
    }

    #[test]
    fn load_rom_without_complete_header_still_works() {
        let mut mmu = MMU::new();
        mmu.load_rom(vec![0x42; 0x100]); // trop court pour un en-tête complet ($0100-$014F)

        assert!(mmu.cartridge().is_none()); // pas d'en-tête analysé (avertissement affiché)
        assert_eq!(mmu.read(0x0000), 0x42); // la ROM est chargée quand même, telle quelle
    }

    /// Audit de la carte d'adresses complète (Gekkio Appendix B / PanDocs « Memory Map ») :
    /// chaque plage est lue et écrite à sa place exacte, y compris la HRAM où vit la pile.
    #[test]
    fn full_memory_map_read_write_audit() {
        let mut rom = vec![0x00; 0xC000]; // MBC1, 48 KiB (banques $00-$02)
        rom[HEADER_TYPE_ADDR as usize] = 0x01;
        rom[0x0000] = 0xA0; // banque 0, premier octet
        rom[0x3FFF] = 0xB0; // banque 0, dernier octet (toujours mappé)
        rom[0x4000] = 0xC1; // banque 1, premier octet

        let mut mmu = MMU::new();
        mmu.load_rom(rom);
        mmu.ppu.mode = 0; // HBlank : la VRAM/OAM est accessible au CPU (la PPU ne les bloque que pendant the modes 2/3, LCD allumé).

        // --- ROM : la région $0000-$3FFF reste en banque 0 ; $4000-$7FFF suit le registre de banque.
        assert_eq!(mmu.read(0x0000), 0xA0);
        assert_eq!(mmu.read(0x3FFF), 0xB0);
        assert_eq!(mmu.read(0x4000), 0xC1); // power-up : le registre $00 se comporte comme la banque $01
        mmu.write(0x2000, 0x02); // sélection de la banque 2 (registre ROM)
        assert_eq!(mmu.read(0x4000), 0x00); // banque 2 (remplie de $00 dans ce test)
        assert_eq!(mmu.read(0x3FFF), 0xB0); // la région banque 0 reste inchangée

        // --- VRAM.
        mmu.write(0x8000, 0x12);
        assert_eq!(mmu.read(0x8000), 0x12);
        mmu.write(0x9FFF, 0x34);
        assert_eq!(mmu.read(0x9FFF), 0x34);

        // --- SRAM cartouche : désactivée → open bus $FF ; activée ($0A) → lisible/écrivable.
        assert_eq!(mmu.read(0xA000), 0xFF);
        mmu.write(0x0000, 0x0A); // activation de la RAM
        mmu.write(0xBFFF, 0x56);
        assert_eq!(mmu.read(0xBFFF), 0x56);

        // --- WRAM + Echo RAM (miroir C000-DDFF dans les deux sens).
        mmu.write(0xC000, 0x78);
        assert_eq!(mmu.read(0xE000), 0x78); // lecture via le miroir
        mmu.write(0xFDFF, 0x9A);
        assert_eq!(mmu.read(0xDDFF), 0x9A); // écriture via le miroir

        // --- OAM + zone inutilisable.
        mmu.write(0xFE9F, 0xBC);
        assert_eq!(mmu.read(0xFE9F), 0xBC);
        assert_eq!(mmu.read(0xFEA0), 0x00); // $FEA0-$FEFF : $00 sur DMG

        // --- HRAM (la pile est ici !) + registre IE.
        mmu.write(0xFF80, 0xDE);
        assert_eq!(mmu.read(0xFF80), 0xDE);
        mmu.write(0xFFFE, 0xAD); // vérification cruciale : le dernier octet de HRAM est bien écrit (LD SP,$FFFE)
        assert_eq!(mmu.read(0xFFFE), 0xAD);
        mmu.write(0xFFFF, 0b1010_0000);
        assert_eq!(mmu.read(0xFFFF), 0b1010_0000); // registre IE
    }

    /// Valeurs au power-on des régions de mémoire (PanDocs « Power Up Sequence ») :
    /// VRAM $FF, WRAM banque 0 = $11 / banque 1 = $FF, OAM $FF, HRAM $FF.
    #[test]
    fn memory_regions_power_up_values() {
        let mmu = MMU::new();
        assert_eq!(mmu.read(0x8000), 0xFF); // VRAM : $FF sur DMG
        assert_eq!(mmu.read(0x9FFF), 0xFF);
        assert_eq!(mmu.read(0xC000), 0x11); // WRAM banque 0 ($C000-$CFFF) : $11
        assert_eq!(mmu.read(0xD000), 0xFF); // WRAM banque 1 ($D000-$DFFF) : $FF
        assert_eq!(mmu.read(0xFE00), 0xFF); // OAM : $FF
        assert_eq!(mmu.read(0xFF80), 0xFF); // HRAM : $FF (SP=$FFFE pointe ici au hand-off)
        assert_eq!(mmu.read(0xFFFE), 0xFF);
    }

    /// La pile du CPU gère correctement la HRAM via de vraies instructions :
    /// LD SP,$FFFE puis PUSH/POP traversent le dernier octet de HRAM sans perte.
    #[test]
    fn stack_operations_through_hram_top_byte() {
        let mut rom = vec![0xFF; 0x4000];
        let code: &[u8] = &[
            0x31, 0xFE, 0xFF, // LD SP,$FFFE (vérification cruciale : le dernier octet de HRAM est utilisé)
            0x01, 0x34, 0x12, // LD BC,$1234
            0xC5, // PUSH BC → $FFFD = $12, $FFFC = $34 (SP devient $FFFC)
            0xC1, // POP BC
        ];
        rom[0x0100..0x0100 + code.len()].copy_from_slice(code);

        let mut mmu = MMU::new();
        mmu.load_rom(rom);
        let mut cpu = CPU::new();
        for _ in 0..4 { // exactement les 4 instructions (LD SP, LD BC, PUSH, POP)
            cpu.step(&mut mmu);
        }

        assert_eq!(cpu.sp, 0xFFFE); // la pile est de retour à $FFFE après le POP
        assert_eq!(mmu.read(0xFFFC), 0x34); // l'octet bas a été poussé dans la HRAM...
        assert_eq!(mmu.read(0xFFFD), 0x12); // ...ainsi que l'octet haut (le dernier octet $FFFE n'est pas écrasé)
        assert_eq!(cpu.b, 0x12); // le POP a restauré BC depuis la HRAM
        assert_eq!(cpu.c, 0x34);
    }
}
