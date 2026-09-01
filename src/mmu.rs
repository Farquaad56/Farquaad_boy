//! Memory Management Unit : pont entre le CPU et la mémoire.
//!
//! Implémente toute la carte d'adresses de la Game Boy (PanDocs « Memory Map »)
//! ainsi que la logique MBC1 de base (commutation des banques ROM, SRAM).
//!
//! Étape 1 : les registres PPU ($FF40-$FF4B, dont le DMA OAM $FF46) sont routés vers la structure
//! [`ppu::PPU`] embarquée dans le MMU ; partie 7 : les registres SCC ($FF01-$FF02) vers
//! [`serial::Serial`], et partie 9 : les registres Timer ($FF04-$FF07) vers [`timer::Timer`].
//! Le tableau `io` ne stocke que les valeurs brutes des autres registres (joypad… — parties 7+).

use crate::ppu::PPU;
use crate::serial::Serial;
use crate::timer::Timer;

/// Taille d'une banque ROM (16 KiB).
const ROM_BANK_SIZE: usize = 0x4000;

/// Taille maximale de SRAM cartouche supportée (32 KiB = quatre banques de 8 KiB, MBC1 max).
const SRAM_SIZE: usize = 0x8000;

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
    /// Registres I/O (0xFF00-0xFF7F).
    pub io: [u8; 0x80],
    /// Registre Interrupt Enable (0xFFFF).
    pub ie: u8,
    /// Pixel Processing Unit (registres $FF40-$FF45), synchronisé sur les T-cycles.
    pub ppu: PPU,
    /// Serial Communication Controller (registres $FF01-$FF02), synchronisé sur les T-cycles.
    pub serial: Serial,
    /// Timer (registres $FF04-$FF07), synchronisé sur les T-cycles.
    pub timer: Timer,

    // --- État MBC1 cartouche ---
    /// SRAM cartouche (jusqu'à 32 KiB en quatre banques de 8 KiB).
    sram: [u8; SRAM_SIZE],
    /// La RAM cartouche est-elle activée (écriture $0A sur le registre d'activation) ?
    ram_enabled: bool,
    /// Banque ROM sélectionnée pour la région 0x4000-0x7FFF ($00 se comporte comme $01).
    rom_bank: u8,
    /// Banque SRAM sélectionnée (cartouches avec plus de 8 KiB de RAM).
    ram_bank: u8,
}

impl MMU {
    pub fn new() -> Self {
        Self::default()
    }

    /// Charge une ROM `.gb` et réinitialise la mémoire à l'état power-on.
    pub fn load_rom(&mut self, data: Vec<u8>) {
        let mut fresh = Self::new();
        fresh.rom = data;
        *self = fresh;
    }

    /// Taille de la ROM chargée en octets (0 si aucune ROM n'est chargée).
    pub fn rom_size(&self) -> usize {
        self.rom.len()
    }

    /// Lit un octet sur toute la carte d'adresses 16 bits.
    /// Les zones non mappées renvoient $FF, comme sur le matériel réel.
    pub fn read(&self, addr: u16) -> u8 {
        match addr {
            // Banque ROM 0 (fixe).
            0x0000..=0x3FFF => self.read_rom_at(addr as u32),
            // Région ROM à banques (MBC) : $00 se comporte comme la banque $01.
            0x4000..=0x7FFF => {
                let bank = if self.rom_bank == 0 { 1 } else { self.rom_bank };
                self.read_rom_at(bank as u32 * ROM_BANK_SIZE as u32 + (addr - 0x4000) as u32)
            }
            // Video RAM.
            0x8000..=0x9FFF => self.vram[(addr - 0x8000) as usize],
            // SRAM cartouche (accessible uniquement si activée).
            0xA000..=0xBFFF => {
                if self.ram_enabled {
                    let bank = (self.ram_bank & 0x03) as usize;
                    self.sram[bank * 0x2000 + (addr - 0xA000) as usize]
                } else {
                    0xFF // open bus
                }
            }
            // Work RAM.
            0xC000..=0xDFFF => self.wram[(addr - 0xC000) as usize],
            // Echo RAM : miroir de C000-DDFF (seuls les 13 bits bas d'adresse sont connectés).
            0xE000..=0xFDFF => self.read(addr - 0x2000),
            // Object Attribute Memory.
            0xFE00..=0xFE9F => self.oam[(addr - 0xFE00) as usize],
            // Non utilisable (FEA0-FEFF) : $00 sur DMG hors bloc OAM ;
            // TODO(partie 4) : renvoyer $FF pendant le bloc de DMA OAM.
            0xFEA0..=0xFEFF => 0x00,
            // Serial Communication Controller (partie 7).
            0xFF01 => self.serial.read_sb(),
            0xFF02 => self.serial.sc,
            // Registres Timer (partie 9).
            0xFF04 => self.timer.read_div(),
            0xFF05 => self.timer.read_tima(),
            0xFF06 => self.timer.read_tma(),
            0xFF07 => self.timer.read_tac(),
            // Registres PPU (étape 1) : $FF40-$FF4B, dont le DMA OAM ($FF46).
            0xFF40..=0xFF4B => self.ppu.read_register(addr),
            // Autres registres I/O.
            0xFF00..=0xFF7F => self.io[(addr - 0xFF00) as usize],
            // High RAM.
            0xFF80..=0xFFFE => self.hram[(addr - 0xFF80) as usize],
            // Registre Interrupt Enable.
            0xFFFF => self.ie,
        }
    }

    /// Écrit un octet sur toute la carte d'adresses 16 bits.
    #[allow(dead_code)] // Utilisé à partir de la partie 3 (le CPU écrit en mémoire).
    pub fn write(&mut self, addr: u16, value: u8) {
        match addr {
            // Région ROM : les écritures vont aux registres de contrôle du MBC (la ROM est en lecture seule).
            0x0000..=0x3FFF => self.mbc_write(addr, value),
            // Région ROM à banques : aucun registre MBC1 ici ; écritures ignorées.
            0x4000..=0x7FFF => {}
            // Video RAM.
            0x8000..=0x9FFF => self.vram[(addr - 0x8000) as usize] = value,
            // SRAM cartouche (ignorée si non activée).
            0xA000..=0xBFFF => {
                if self.ram_enabled {
                    let bank = (self.ram_bank & 0x03) as usize;
                    self.sram[bank * 0x2000 + (addr - 0xA000) as usize] = value;
                }
            }
            // Work RAM.
            0xC000..=0xDFFF => self.wram[(addr - 0xC000) as usize] = value,
            // Echo RAM : les écritures sont miroirées vers C000-DDFF.
            0xE000..=0xFDFF => self.write(addr - 0x2000, value),
            // Object Attribute Memory.
            0xFE00..=0xFE9F => self.oam[(addr - 0xFE00) as usize] = value,
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
            0xFF40..=0xFF4B => self.ppu.write_register(addr, value),
            // Autres registres I/O.
            0xFF00..=0xFF7F => self.io[(addr - 0xFF00) as usize] = value,
            // High RAM.
            0xFF80..=0xFFFE => self.hram[(addr - 0xFF80) as usize] = value,
            // Registre Interrupt Enable.
            0xFFFF => self.ie = value,
        }
    }

    /// Lit dans le fichier ROM ; $FF au-delà de sa fin (open bus).
    fn read_rom_at(&self, idx: u32) -> u8 {
        match self.rom.get(idx as usize) {
            Some(byte) => *byte,
            None => 0xFF,
        }
    }

    /// Registres de contrôle MBC1 (écritures dans la région ROM).
    ///
    /// Les jeux écrivent conventionnellement : $0A à `0x0000` (activation RAM),
    /// le numéro de banque à `0x0100`, et la banque SRAM à `0x0200`. PanDocs décrit
    /// les registres physiques comme 0x0000-0x1FFF / 0x2000-0x3FFF ; nous acceptons
    /// les deux plages d'adresses pour que tout logiciel fonctionne.
    #[allow(dead_code)] // Utilisé à partir de la partie 3 (le CPU écrit en mémoire).
    fn mbc_write(&mut self, addr: u16, value: u8) {
        match addr {
            // Activation RAM : toute valeur avec $A dans les 4 bits bas active la SRAM.
            0x0000..=0x00FF => self.ram_enabled = (value & 0x0F) == 0x0A,
            // Numéro de banque ROM ($00 se comporte comme $01 à la lecture).
            0x0100..=0x01FF | 0x2000..=0x3FFF => self.rom_bank = value & 0x7F,
            // Numéro de banque SRAM (cartouches avec plus de 8 KiB de RAM).
            0x0200..=0x03FF => self.ram_bank = value & 0x03,
            _ => {}
        }
    }
}

impl Default for MMU {
    fn default() -> Self {
        Self {
            rom: Vec::new(),
            vram: [0; 0x2000],
            wram: [0; 0x2000],
            oam: [0; 0xA0],
            hram: [0; 0x7F],
            io: [0; 0x80],
            ie: 0,
            ppu: PPU::new(),
            serial: Serial::default(),
            timer: Timer::default(),
            sram: [0; SRAM_SIZE],
            ram_enabled: false,
            rom_bank: 0, // $00 au power-up → se comporte comme la banque $01.
            ram_bank: 0,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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

        let mut mmu = MMU::new();
        mmu.load_rom(rom);

        assert_eq!(mmu.read(0x3FFF), 0x99); // région banque 0 fixe
        assert_eq!(mmu.read(0x4000), 0x11); // power-up : $00 se comporte comme la banque $01
        mmu.write(0x0100, 0x02); // les jeux écrivent la banque à 0x0100
        assert_eq!(mmu.read(0x4000), 0x22); // maintenant la banque 2
        mmu.write(0x0100, 0x00);
        assert_eq!(mmu.read(0x4000), 0x11); // $00 → retour à la banque $01
    }

    #[test]
    fn mbc_cartridge_ram_requires_enable() {
        let mut mmu = MMU::new();
        mmu.load_rom(vec![0xFF; 0x4000]);

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

        // STAT ($FF41) : seuls les bits 3..6 sont écrits ; la lecture renvoie les bits d'activation + drapeaux.
        mmu.write(0xFF41, 0x78);
        assert_eq!(mmu.read(0xFF41), 0x7C); // bits écrits 0x78 + drapeau LYC==LY (0==0) ; le mode 2 ne pose aucun bit de drapeau.

        // Le DMA OAM ($FF46) est routé vers la PPU, pas vers `io`.
        mmu.write(0xFF46, 0xC0);
        assert_eq!(mmu.read(0xFF46), 0xC0);
    }
}