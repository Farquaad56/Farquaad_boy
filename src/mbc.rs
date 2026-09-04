//! Contrôleur de mémoire MBC unifié — PanDocs « MBCs », « MBC1 », « MBC2 », « MBC3 », « MBC5 ».
//!
//! Modélise l'état des contrôleurs de mémoire de cartouche : commutation des banques ROM,
//! activation et commutation des banques SRAM, mode de banking MBC1 et registres RTC MBC3.
//! Le type de contrôleur est détecté depuis l'octet d'en-tête $0147 (PanDocs « The Cartridge Header »).

use std::time::{SystemTime, UNIX_EPOCH};

/// Taille d'une banque ROM (16 KiB).
pub const ROM_BANK_SIZE: usize = 0x4000;

/// Taille d'une banque SRAM/RTC (8 KiB).
pub const RAM_BANK_SIZE: usize = 0x2000;

/// Taille maximale de SRAM cartouche supportée (32 KiB = quatre banques de 8 KiB, max MBC1/MBC3).
pub const SRAM_SIZE: usize = 4 * RAM_BANK_SIZE;

/// Nombre de banques SRAM dans le tableau `sram` (une sélection hors plage rebrousse — PanDocs « MBCs »).
const SRAM_BANK_COUNT: usize = SRAM_SIZE / RAM_BANK_SIZE; // 4

/// Octet d'en-tête qui identifie le contrôleur de mémoire (PanDocs « The Cartridge Header »).
pub const HEADER_TYPE_ADDR: u16 = 0x0147;

/// Type de contrôleur de mémoire détecté depuis l'octet d'en-tête $0147.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MbcType {
    /// En-tête $00 « ROM ONLY » (ou valeur non reconnue) : pas de commutation de banque,
    /// les écritures dans $0000-$7FFF sont ignorées.
    RomOnly,
    /// Registre ROM 5 bits + mode de banking optionnel (banques SRAM ou bits supérieurs ROM).
    Mbc1,
    /// ROM petite (registre de banque 4 bits) + RAM interne de 512×4 bits (PanDocs « MBC2 »).
    Mbc2,
    /// Registre ROM 7 bits ; $4000-$5FFF sélectionne une banque SRAM ou un registre RTC (PanDocs « MBC3 »).
    Mbc3,
    /// Registre ROM 9 bits écrit en deux fois + registre de banque SRAM 4 bits (PanDocs « MBC5 »).
    Mbc5,
}

impl MbcType {
    /// Détecte le contrôleur de mémoire depuis l'octet d'en-tête $0147 : les 5 bits bas sélectionnent
    /// le type (tableau PanDocs « The Cartridge Header »).
    pub fn from_header_byte(byte: u8) -> Self {
        match byte & 0x1F {
            0x01..=0x03 => MbcType::Mbc1, // MBC1 / +RAM / +RAM+BATTERY
            0x05..=0x07 => MbcType::Mbc2, // MBC2 / +BATTERY / +RAM+BATTERY
            0x0B..=0x0D => MbcType::Mbc3, // MBC3 with TIMER (± battery/RAM)
            0x0F..=0x13 | 0x19..=0x1B => MbcType::Mbc5, // MBC5 (+RAM/+RUMBLE), dont variantes MBC6 non officielles
            _ => MbcType::RomOnly, // $00 (ROM ONLY), $20 (MBC7), autres valeurs inconnues
        }
    }
}

/// État unifié du contrôleur de mémoire de cartouche.
pub struct Mbc {
    /// Type de contrôleur détecté depuis l'en-tête $0147.
    pub mbc_type: MbcType,
    /// Taille de la ROM chargée en octets : les bits de banque au-delà de cette taille sont masqués (PanDocs « MBCs »).
    rom_size: usize,
    /// Valeur brute du registre de banque ROM ($2000-$3FFF ; pour MBC5 : bits 0-7 à $2000-$2FFF + bit 8 à $3000-$3FFF).
    /// Défaut power-up $00 (lu comme la banque $01, sauf sur MBC5 où il est lu comme $00).
    rom_bank: u16,
    /// Valeur brute du registre $4000-$5FFF. Le sens dépend du type/mode :
    /// MBC1 mode avancé → bits 5-6 = banques supérieures ROM et banque SRAM (mode simple : SRAM verrouillée sur la banque 0) ;
    /// MBC3/MBC5 → sélection banque SRAM ou registre RTC ($08..=$0C). Défaut power-up $00.
    ram_bank: u8,
    /// Mode de banking MBC1 (bit 7 du registre $6000-$7FFF) : false = simple (banking ROM), true = avancé (banking RAM).
    banking_mode: bool,
    /// La RAM cartouche est-elle activée ? Écriture $0A sur $0000-$1FFF pour l'activer.
    ram_enabled: bool,
    /// SRAM cartouche (jusqu'à 32 KiB en quatre banques de 8 KiB ; seule la première moitié basse est utilisée par MBC2).
    pub sram: [u8; SRAM_SIZE],
    // --- Registres RTC MBC3 ($A000-$A004 quand la sélection est $08..=$0C) ---
    rtc_seconds: u8,
    rtc_minutes: u8,
    rtc_hours: u8,
    /// Bits 0-6 : compteur de jours (octet bas).
    rtc_day_low: u8,
    /// Bit 0 : bit MSB du compteur de jours ; bit 6 : halt ; bit 7 : carry.
    rtc_day_high: u8,
    /// Les valeurs RTC sont-elles verrouillées (latch) ? ($6000-$7FFF = $01).
    rtc_latched: bool,
}

impl Mbc {
    /// État power-up du contrôleur : registres à $00, RAM désactivée, horloge initialisée sur l'heure système.
    pub fn new(mbc_type: MbcType, rom_size: usize) -> Self {
        let (seconds, minutes, hours) = rtc_initial_time();
        Self {
            mbc_type,
            rom_size,
            rom_bank: 0x00,
            ram_bank: 0x00,
            banking_mode: false,
            ram_enabled: false,
            sram: [0u8; SRAM_SIZE],
            rtc_seconds: seconds,
            rtc_minutes: minutes,
            rtc_hours: hours,
            rtc_day_low: 0x00,
            rtc_day_high: 0x00,
            rtc_latched: false,
        }
    }

    /// La RAM cartouche est-elle activée ? (PanDocs « MBCs » : $0A sur $0000-$1FFF)
    #[allow(dead_code)] // API publique du contrôleur — utilisée par les tests et les parties futures (sauvegarde d'état).
    pub fn ram_enabled(&self) -> bool {
        self.ram_enabled
    }

    /// Écriture de la région $0000-$1FFF : active/désactive la RAM cartouche.
    /// Toute valeur avec $A dans les 4 bits bas active la SRAM (PanDocs « MBC1 » / « MBC2 »).
    pub fn set_ram_enabled(&mut self, value: u8) {
        self.ram_enabled = (value & 0x0F) == 0x0A;
    }

    /// Écriture du registre de banque ROM ($2000-$3FFF ; pour MBC5 : $2000-$2FFF → bits 0-7, bit 8 conservé).
    pub fn set_rom_bank(&mut self, value: u8) {
        match self.mbc_type {
            MbcType::Mbc5 => self.rom_bank = (self.rom_bank & 0x0100) | value as u16,
            _ => self.rom_bank = value as u16,
        }
    }

    /// Écriture du registre du bit 8 de la banque ROM sur MBC5 ($3000-$3FFF). No-op pour les autres contrôleurs.
    pub fn set_rom_bank_bit8(&mut self, value: u8) {
        if self.mbc_type == MbcType::Mbc5 {
            self.rom_bank = (self.rom_bank & 0x00FF) | (((value & 0x01) as u16) << 8);
        }
    }

    /// Écriture de la région $4000-$5FFF : banque SRAM / bits supérieurs ROM / sélection registre RTC (valeur brute).
    pub fn set_ram_bank(&mut self, value: u8) {
        self.ram_bank = value;
    }

    /// Écriture de la région $6000-$7FFF : mode de banking MBC1 et latch horloge MBC3. No-op pour les autres contrôleurs.
    pub fn write_mode_register(&mut self, value: u8) {
        match self.mbc_type {
            MbcType::Mbc1 => self.banking_mode = value & 0x80 != 0,
            MbcType::Mbc3 => match value {
                0x00 => self.rtc_latched = false, // déverrouillage (PanDocs « MBC3 »)
                0x01 => self.rtc_latched = true,  // verrouille les valeurs RTC courantes
                _ => {}
            },
            _ => {} // ROM ONLY / MBC2 / MBC5 : aucun registre dans $6000-$7FFF
        }
    }

    /// Écriture d'un registre de la région ROM ($0000-$7FFF) — le routage dépend du type de contrôleur (PanDocs).
    pub fn write_register(&mut self, addr: u16, value: u8) {
        match self.mbc_type {
            MbcType::RomOnly => {} // « ROM ONLY » : les écritures dans $0000-$7FFF sont ignorées
            MbcType::Mbc2 => match addr {
                // PanDocs « MBC2 » : le bit 8 de l'adresse sélectionne l'activation RAM (bit à 0) ou la banque ROM (bit à 1).
                0x0000..=0x3FFF if addr & 0x0100 == 0 => self.set_ram_enabled(value),
                0x0000..=0x3FFF => self.set_rom_bank(value), // 4 bits bas → banque ROM ($00 lu comme $01)
                _ => {} // $4000-$7FFF : aucun registre sur MBC2
            },
            MbcType::Mbc5 => match addr {
                0x0000..=0x1FFF => self.set_ram_enabled(value),
                0x2000..=0x2FFF => self.set_rom_bank(value), // bits 0-7 de la banque ROM (PanDocs « MBC5 »)
                0x3000..=0x3FFF => self.set_rom_bank_bit8(value), // bit 8 de la banque ROM
                0x4000..=0x5FFF => self.set_ram_bank(value), // banque SRAM (bits 0-3)
                _ => {} // $6000-$7FFF : aucun registre sur MBC5
            },
            MbcType::Mbc1 | MbcType::Mbc3 => match addr {
                0x0000..=0x1FFF => self.set_ram_enabled(value),
                0x2000..=0x3FFF => self.set_rom_bank(value), // bits 0-4 (MBC1) / 7 bits entiers (MBC3)
                0x4000..=0x5FFF => self.set_ram_bank(value), // banque SRAM / bits supérieurs ROM / sélection RTC
                _ => self.write_mode_register(value), // $6000-$7FFF : mode select (MBC1) / latch (MBC3)
            },
        }
    }

    /// Index à plat dans le fichier ROM pour une adresse $0000-$7FFF.
    pub fn get_rom_addr(&self, addr: u16) -> usize {
        if self.mbc_type == MbcType::RomOnly {
            return addr as usize; // ROM plate : aucune commutation de banque
        }
        let offset = (addr & 0x3FFF) as usize;
        (self.effective_rom_bank(addr) as usize) * ROM_BANK_SIZE + offset
    }

    /// Lecture de la SRAM cartouche / du registre RTC à $A000-$BFFF, ou None si la RAM est désactivée/absente.
    pub fn read_ram(&self, addr: u16) -> Option<u8> {
        if !self.ram_enabled || self.mbc_type == MbcType::RomOnly {
            return None; // open bus (PanDocs « MBCs »)
        }
        let offset = (addr - 0xA000) as usize;
        Some(match self.mbc_type {
            MbcType::Mbc2 => self.sram[(addr & 0x1FF) as usize], // RAM interne 512×4 bits : seuls les 9 bits bas d'adresse sont utilisés
            MbcType::Mbc3 => match self.ram_bank & 0x0F {
                sel @ 8..=12 => self.rtc_read(sel - 8), // registre RTC $08-$0C (accessible à toute adresse de la région)
                sel => self.sram[sel as usize % SRAM_BANK_COUNT * RAM_BANK_SIZE + offset],
            },
            MbcType::Mbc1 if self.banking_mode => {
                let bank = ((self.ram_bank >> 5) & 0x03) as usize; // bits 5-6 : même registre que les banques supérieures ROM (PanDocs « MBC1 »)
                self.sram[bank * RAM_BANK_SIZE + offset]
            }
            MbcType::Mbc5 => {
                let bank = (self.ram_bank & 0x0F) as usize % SRAM_BANK_COUNT; // bits 0-3, rebroussement
                self.sram[bank * RAM_BANK_SIZE + offset]
            }
            _ => self.sram[offset], // MBC1 mode 0 : la SRAM reste verrouillée sur la banque 0
        })
    }

    /// Écriture de la SRAM cartouche / du registre RTC à $A000-$BFFF (ignorée si la RAM est désactivée/absente).
    pub fn write_ram(&mut self, addr: u16, value: u8) {
        if !self.ram_enabled || self.mbc_type == MbcType::RomOnly {
            return; // écritures ignorées (PanDocs « MBCs »)
        }
        let offset = (addr - 0xA000) as usize;
        match self.mbc_type {
            MbcType::Mbc2 => self.sram[(addr & 0x1FF) as usize] = value & 0x0F, // seuls les 4 bits bas sont conservés
            MbcType::Mbc3 => match self.ram_bank & 0x0F {
                sel @ 8..=12 => self.rtc_write(sel - 8, value),
                sel => self.sram[sel as usize % SRAM_BANK_COUNT * RAM_BANK_SIZE + offset] = value,
            },
            MbcType::Mbc1 if self.banking_mode => {
                let bank = ((self.ram_bank >> 5) & 0x03) as usize; // bits 5-6 (PanDocs « MBC1 »)
                self.sram[bank * RAM_BANK_SIZE + offset] = value;
            }
            MbcType::Mbc5 => {
                let bank = (self.ram_bank & 0x0F) as usize % SRAM_BANK_COUNT;
                self.sram[bank * RAM_BANK_SIZE + offset] = value;
            }
            _ => self.sram[offset] = value, // MBC1 mode 0 : la SRAM reste verrouillée sur la banque 0
        }
    }

    /// Index du registre RTC sélectionné (0..=4 → $A000..=$A004), ou None si inapplicable.
    #[allow(dead_code)] // API publique du contrôleur — utilisée par les tests et les parties futures (sauvegarde d'état).
    pub fn rtc_register(&self) -> Option<u8> {
        if self.mbc_type == MbcType::Mbc3 && self.ram_enabled {
            let sel = self.ram_bank & 0x0F;
            ((8..=12).contains(&sel)).then(|| sel - 8)
        } else {
            None
        }
    }

    /// Lit le registre RTC sélectionné ($08..=$0C).
    pub fn rtc_read(&self, reg: u8) -> u8 {
        match reg {
            0 => self.rtc_seconds,
            1 => self.rtc_minutes,
            2 => self.rtc_hours,
            3 => self.rtc_day_low,
            _ => self.rtc_day_high,
        }
    }

    /// Écrit le registre RTC sélectionné ($08..=$0C).
    pub fn rtc_write(&mut self, reg: u8, value: u8) {
        match reg {
            0 => self.rtc_seconds = value,
            1 => self.rtc_minutes = value,
            2 => self.rtc_hours = value,
            3 => self.rtc_day_low = value,
            _ => self.rtc_day_high = value,
        }
    }

    /// Les valeurs RTC sont-elles verrouillées (latch) ?
    #[allow(dead_code)] // API publique du contrôleur — utilisée par les tests et les parties futures (sauvegarde d'état).
    pub fn rtc_latched(&self) -> bool {
        self.rtc_latched
    }

    /// Calcule le nombre de banques ROM disponibles et masque les bits de banque invalides.
    /// PanDocs : si le jeu sélectionne une banque qui n'existe pas, les bits supérieurs sont ignorés.
    fn mask_rom_bank(&self, bank: u32) -> u32 {
        let max_banks = (self.rom_size / ROM_BANK_SIZE) as u32;
        if max_banks <= 1 {
            return 0; // ROM plate ou une seule banque
        }
        // Trouver le masque de bits valide (ex: 2 banques = 1 bit, 4 banques = 2 bits, etc.)
        let mask = max_banks.next_power_of_two() - 1;
        bank & mask
    }

    /// Banque ROM effective pour une adresse $0000-$7FFF (PanDocs « MBC1 » / « MBC2 » / « MBC3 » / « MBC5 »).
    fn effective_rom_bank(&self, addr: u16) -> u32 {
        match self.mbc_type {
            MbcType::Mbc1 => {
                if self.banking_mode {
                    let upper = ((self.ram_bank >> 5) & 0x03) as u32; // bits 5-6 du registre $4000-$5FFF
                    if addr < 0x4000 {
                        self.mask_rom_bank(upper << 5) // les banques $20/$40/$60 deviennent accessibles dans cette région
                    } else {
                        let lower = (self.rom_bank & 0x1F) as u32;
                        self.mask_rom_bank((upper << 5) | if lower == 0 { 1 } else { lower }) // la banque $00 se comporte comme $01
                    }
                } else {
                    let bank = (self.rom_bank & 0x1F) as u32; // bits 0-4 du registre ROM
                    if addr < 0x4000 {
                        0
                    } else {
                        self.mask_rom_bank(bank).max(1)
                    }
                }
            }
            MbcType::Mbc2 => {
                let bank = (self.rom_bank & 0x0F) as u32; // registre 4 bits (PanDocs « MBC2 »)
                if addr < 0x4000 {
                    0
                } else {
                    self.mask_rom_bank(bank).max(1)
                } // écrire $00 sélectionne la banque $01
            }
            MbcType::Mbc3 => {
                let bank = (self.rom_bank & 0x7F) as u32; // les 7 bits entiers du registre ROM (PanDocs « MBC3 »)
                if addr < 0x4000 {
                    0
                } else {
                    self.mask_rom_bank(bank).max(1)
                } // écrire $00 sélectionne la banque $01
            }
            MbcType::Mbc5 => {
                let bank = (self.rom_bank & 0x1FF) as u32; // bits 0-8 du registre ROM (PanDocs « MBC5 »)
                if addr < 0x4000 {
                    0
                } else {
                    self.mask_rom_bank(bank)
                } // sur MBC5, la banque $00 est réellement la banque $00
            }
            MbcType::RomOnly => unreachable!(), // géré dans get_rom_addr (ROM plate, sans commutation)
        }
    }
}

/// Heure RTC initiale depuis l'horloge système (UTC), pour que l'horloge démarre à une valeur plausible.
fn rtc_initial_time() -> (u8, u8, u8) {
    match SystemTime::now().duration_since(UNIX_EPOCH) {
        Ok(d) => {
            let s = d.as_secs();
            (
                (s % 60) as u8,
                ((s / 60) % 60) as u8,
                ((s / 3600) % 24) as u8,
            )
        }
        Err(_) => (0, 0, 0),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn header_byte_maps_to_mbc_type() {
        assert_eq!(MbcType::from_header_byte(0x00), MbcType::RomOnly); // ROM ONLY
        assert_eq!(MbcType::from_header_byte(0xFF), MbcType::RomOnly); // valeur inconnue (unknown value)
        assert_eq!(MbcType::from_header_byte(0x20), MbcType::RomOnly); // MBC7 : pas de variante dédiée → repli ROM ONLY
        for b in [0x01u8, 0x02, 0x03] {
            assert_eq!(MbcType::from_header_byte(b), MbcType::Mbc1);
        }
        for b in [0x05u8, 0x06, 0x07] {
            assert_eq!(MbcType::from_header_byte(b), MbcType::Mbc2);
        }
        for b in [0x0Bu8, 0x0C, 0x0D] {
            assert_eq!(MbcType::from_header_byte(b), MbcType::Mbc3); // MBC3 with TIMER (± battery/RAM)
        }
        for b in [0x0Fu8, 0x10, 0x11, 0x12, 0x13] {
            assert_eq!(MbcType::from_header_byte(b), MbcType::Mbc5); // MBC5 (+RAM/+RUMBLE)
        }
        for b in [0x19u8, 0x1A, 0x1B] {
            assert_eq!(MbcType::from_header_byte(b), MbcType::Mbc5); // variantes MBC6 non officielles (MBC5+RAM)
        }
    }

    #[test]
    fn rom_only_is_flat_and_ignores_writes() {
        let mut mbc = Mbc::new(MbcType::RomOnly, 0x8000);
        assert_eq!(mbc.get_rom_addr(0x4000), 0x4000); // ROM plate : aucune commutation de banque (flat ROM: no bank switching)
        mbc.write_register(0x2000, 0x05); // les écritures dans $0000-$7FFF sont ignorées (writes in the range $0000-$7FFF are ignored)
        assert_eq!(mbc.get_rom_addr(0x4000), 0x4000);
        mbc.set_ram_enabled(0x0A); // aucune SRAM du tout sur une cartouche ROM ONLY (no SRAM at all on a ROM ONLY cartridge)
        assert_eq!(mbc.read_ram(0xA000), None);
    }

    #[test]
    fn mbc1_mode0_rom_banking() {
        let mut mbc = Mbc::new(MbcType::Mbc1, 0x20000); // ROM de 128 KiB (8 banques)
        assert_eq!(mbc.get_rom_addr(0x0000), 0); // la région $0000-$3FFF reste toujours en banque 0 (the region $0000-$3FFF always remains on bank 0)
        assert_eq!(mbc.get_rom_addr(0x4000), ROM_BANK_SIZE); // le registre $00 se comporte comme $01 (register $00 behaves as $01)
        mbc.set_rom_bank(0x05);
        assert_eq!(mbc.get_rom_addr(0x4000), 5 * ROM_BANK_SIZE);
        assert_eq!(mbc.get_rom_addr(0x7FFF), 5 * ROM_BANK_SIZE + 0x3FFF);
    }

    #[test]
    fn mbc1_mode1_upper_bits_and_zero_rule() {
        let mut mbc = Mbc::new(MbcType::Mbc1, 0x200000); // ROM de 2 MiB (128 banques)
        mbc.write_mode_register(0x80); // mode avancé (PanDocs « MBC1 ») (advanced mode)
        mbc.set_ram_bank(0x20); // bits supérieurs = 1 → banques $20-$3F (upper bits = 1 → banks $20-$3F)
        assert_eq!(mbc.get_rom_addr(0x0000), 0x20 * ROM_BANK_SIZE);
        assert_eq!(mbc.get_rom_addr(0x4000), 0x21 * ROM_BANK_SIZE); // bits bas $00 → se comporte comme $01 (lower bits $00 → behaves as $01)
        mbc.set_rom_bank(0x03);
        assert_eq!(mbc.get_rom_addr(0x4000), (0x20 | 0x03) * ROM_BANK_SIZE);
    }

    #[test]
    fn mbc1_ram_banking_in_mode1() {
        let mut mbc = Mbc::new(MbcType::Mbc1, 0x200000); // ROM de 2 MiB (128 banques)
        mbc.set_ram_enabled(0x0A);
        assert_eq!(mbc.read_ram(0xA000), Some(0)); // mode 0 : la SRAM reste verrouillée sur la banque 0 (mode 0: SRAM remains locked on bank 0)
        mbc.write_mode_register(0x80); // mode avancé (PanDocs « MBC1 ») (advanced mode)
        mbc.set_ram_bank(0x40); // bits 5-6 = 01 → banque SRAM 1, même registre que les banques supérieures ROM (bits 5-6 = 01 → SRAM bank 1, same register as the upper ROM banks)
        mbc.write_ram(0xA000, 0xAB);
        assert_eq!(mbc.read_ram(0xA000), Some(0xAB));
        mbc.set_ram_bank(0x80); // bits 5-6 = 10 → banque SRAM 2 (bits 5-6 = 10 → SRAM bank 2)
        assert_eq!(mbc.read_ram(0xA000), Some(0)); // la banque 2 est inchangée (bank 2 is unchanged)
        mbc.set_ram_bank(0x40);
        assert_eq!(mbc.read_ram(0xA000), Some(0xAB)); // la banque 1 est conservée (bank 1 is retained)
    }

    #[test]
    fn ram_disabled_is_open_bus() {
        let mut mbc = Mbc::new(MbcType::Mbc1, 0x20000); // ROM de 128 KiB (8 banques)
        assert_eq!(mbc.read_ram(0xA000), None); // désactivée par défaut (disabled by default)
        assert_eq!(mbc.rtc_register(), None);
        mbc.set_ram_enabled(0x0A);
        assert_eq!(mbc.read_ram(0xBFFF), Some(0));
    }

    #[test]
    fn mbc2_bit8_of_address_selects_register() {
        let mut mbc = Mbc::new(MbcType::Mbc2, 0x10000); // ROM de 64 KiB (4 banques)
        assert_eq!(mbc.get_rom_addr(0x4000), ROM_BANK_SIZE); // power-up : registre $00 → banque $01 (power-up: register $00 → bank $01)
        mbc.write_register(0x0100, 0x03); // bit 8 à 1 → banque ROM (PanDocs « MBC2 ») (bit 8 set to 1 → ROM bank)
        assert_eq!(mbc.get_rom_addr(0x4000), 3 * ROM_BANK_SIZE);
        assert!(!mbc.ram_enabled()); // l'activation RAM n'est pas affectée par une écriture de banque ROM (RAM enable is not affected by a ROM bank write)
        mbc.write_register(0x0200, 0x0A); // bit 8 à 0 → activation RAM (bit 8 set to 0 → RAM enable)
        assert!(mbc.ram_enabled());
        assert_eq!(mbc.get_rom_addr(0x4000), 3 * ROM_BANK_SIZE); // la banque ROM est inchangée (ROM bank is unchanged)
        mbc.write_register(0x0100, 0x00); // $00 → se comporte comme la banque $01 ($00 → behaves as bank $01)
        assert_eq!(mbc.get_rom_addr(0x4000), ROM_BANK_SIZE);
    }

    #[test]
    fn mbc2_internal_ram_is_512_x_4_bits() {
        let mut mbc = Mbc::new(MbcType::Mbc2, 0x8000); // ROM de 32 KiB (2 banques)
        mbc.write_register(0x0000, 0x0A); // active la RAM (bit 8 à 0) (enables the RAM: bit 8 set to 0)
        mbc.write_ram(0xA1FF, 0xAB); // dernier octet de la première région (last byte of the first region)
        assert_eq!(mbc.read_ram(0xBFFF), Some(0x0B)); // écho : seuls les 9 bits bas d'adresse + 4 bits bas conservés (echo: only the lower 9 address bits and lower 4 data bits are retained)
    }

    #[test]
    fn mbc3_rom_bank_uses_seven_bits() {
        let mut mbc = Mbc::new(MbcType::Mbc3, 0x200000); // ROM de 2 MiB (128 banques)
        assert_eq!(mbc.get_rom_addr(0x4000), ROM_BANK_SIZE); // le registre $00 se comporte comme $01 (register $00 behaves as $01)
        mbc.set_rom_bank(0x7F);
        assert_eq!(mbc.get_rom_addr(0x4000), 0x7F * ROM_BANK_SIZE);
        assert_eq!(mbc.get_rom_addr(0x0000), 0); // la région $0000-$3FFF reste toujours en banque 0 (the region $0000-$3FFF always remains on bank 0)
    }

    #[test]
    fn mbc3_rtc_registers_and_ram_banks() {
        let mut mbc = Mbc::new(MbcType::Mbc3, 0x10000); // ROM de 64 KiB (4 banques)
        mbc.set_ram_enabled(0x0A);
        // Sélection $00 → banque SRAM 0 ; pas de registre RTC. (selection $00 → SRAM bank 0; no RTC register)
        assert_eq!(mbc.read_ram(0xA000), Some(0));
        assert_eq!(mbc.rtc_register(), None);
        // Sélection $01 → banque SRAM 1. (selection $01 → SRAM bank 1)
        mbc.set_ram_bank(0x01);
        mbc.write_ram(0xA000, 0xCD);
        assert_eq!(mbc.read_ram(0xA000), Some(0xCD));
        // Sélections $08..=$0C → registres RTC (lisibles/écrivables à toute adresse de la région). (selections $08..=$0C → RTC registers, readable/writable at any address in the region)
        for (sel, reg) in [(0x08u8, 0u8), (0x09, 1), (0x0A, 2), (0x0B, 3), (0x0C, 4)] {
            mbc.set_ram_bank(sel);
            assert_eq!(mbc.rtc_register(), Some(reg));
        }
        mbc.set_ram_bank(0x08);
        mbc.write_ram(0xA000, 42); // secondes RTC via write_ram (RTC seconds via write_ram)
        assert_eq!(mbc.read_ram(0xA000), Some(42));
        mbc.rtc_write(3, 7);
        assert_eq!(mbc.rtc_read(3), 7);
    }

    #[test]
    fn mbc3_latch_sequence() {
        let mut mbc = Mbc::new(MbcType::Mbc3, 0x10000); // ROM de 64 KiB (4 banques)
        assert!(!mbc.rtc_latched());
        mbc.write_mode_register(0x01); // verrouille l'horloge (PanDocs « MBC3 ») (locks the clock)
        assert!(mbc.rtc_latched());
        mbc.write_mode_register(0x00); // déverrouille (unlocks)
        assert!(!mbc.rtc_latched());
    }

    #[test]
    fn mbc5_rom_bank_written_in_two_parts() {
        let mut mbc = Mbc::new(MbcType::Mbc5, 0x800000); // ROM de 8 MiB (512 banques) : le bit 8 du registre est valide
        assert_eq!(mbc.get_rom_addr(0x4000), 0); // power-up : registre $00 → réellement la banque $00 (PanDocs « MBC5 ») (power-up: register $00 → actually bank $00)
        mbc.set_rom_bank(0x7F);
        assert_eq!(mbc.get_rom_addr(0x4000), 0x7F * ROM_BANK_SIZE);
        mbc.set_rom_bank_bit8(0x01); // bit 8 → banque $17F (bit 8 → bank $17F)
        assert_eq!(mbc.get_rom_addr(0x4000), 0x17F * ROM_BANK_SIZE);
        mbc.set_rom_bank_bit8(0x00);
        assert_eq!(mbc.get_rom_addr(0x4000), 0x7F * ROM_BANK_SIZE); // les bits 0-7 sont conservés (bits 0-7 are retained)
    }

    #[test]
    fn mbc5_ram_banking_wraps_around() {
        let mut mbc = Mbc::new(MbcType::Mbc5, 0x200000); // ROM de 2 MiB (128 banques)
        mbc.set_ram_enabled(0x0A);
        mbc.set_rom_bank(0x7F);
        assert_eq!(mbc.get_rom_addr(0x4000), 0x7F * ROM_BANK_SIZE);
        mbc.set_ram_bank(0x05); // bits 0-3 → banque 5, rebroussement vers la banque 1 (SRAM de 32 KiB) (bits 0-3 → bank 5, wraps around to bank 1: 32 KiB SRAM)
        mbc.write_ram(0xA000, 0xAB);
        assert_eq!(mbc.read_ram(0xA000), Some(0xAB));
        mbc.set_ram_bank(0x01);
        assert_eq!(mbc.read_ram(0xA000), Some(0xAB)); // rebroussement (PanDocs « MBCs ») (wraps around)
    }

    #[test]
    fn rom_bank_bits_beyond_rom_size_are_masked() {
        // ROM de 128 KiB (8 banques) : les bits au-delà du bit 2 sont ignorés (PanDocs « MBCs »).
        let mut mbc = Mbc::new(MbcType::Mbc3, 0x20000);
        mbc.set_rom_bank(0x47); // bits supérieurs inexistants → masqués vers la banque $07
        assert_eq!(mbc.get_rom_addr(0x4000), 7 * ROM_BANK_SIZE);
        // ROM de 64 KiB (4 banques) : écrire $0C sélectionne la banque $00, lue comme $01.
        let mut mbc = Mbc::new(MbcType::Mbc3, 0x10000);
        mbc.set_rom_bank(0x0C);
        assert_eq!(mbc.get_rom_addr(0x4000), ROM_BANK_SIZE);
    }

    #[test]
    fn mbc1_32kib_rom_never_selects_missing_bank() {
        // Une ROM de 32 KiB a exactement deux banques ($0000-$3FFF et $4000-$7FFF) : les bits de banque au-delà du bit 0 sont masqués (PanDocs « MBCs »),
        // donc aucune valeur de registre ne peut sélectionner une banque inexistante ni lire au-delà de la fin du fichier.
        let mut mbc = Mbc::new(MbcType::Mbc1, 32 * 1024);
        for reg in 0..=0xFFu8 {
            mbc.set_rom_bank(reg);
            assert_eq!(mbc.get_rom_addr(0x0000), 0); // la région $0000-$3FFF reste toujours en banque 0
            assert_eq!(mbc.get_rom_addr(0x4000), ROM_BANK_SIZE); // mode simple : règle du zéro + bit unique valide → toujours la banque 1
            assert_eq!(mbc.get_rom_addr(0x7FFF), ROM_BANK_SIZE + 0x3FFF); // dernier octet du fichier, jamais hors plage
        }
        // Mode avancé : les bits supérieurs sont aussi masqués vers les deux banques existantes.
        mbc.write_mode_register(0x80);
        for ram_reg in 0..=0xFFu8 {
            mbc.set_ram_bank(ram_reg);
            assert_eq!(mbc.get_rom_addr(0x0000), 0); // bits supérieurs masqués → banque 0
            let addr = mbc.get_rom_addr(0x4000);
            assert!(addr == ROM_BANK_SIZE || addr == 0, "registre $4000=${:02X} → index {} hors plage", ram_reg, addr);
        }
    }

    #[test]
    fn rtc_is_initialized_from_system_time() {
        let mbc = Mbc::new(MbcType::Mbc3, 0x10000); // ROM de 64 KiB (4 banques)
        assert!(mbc.rtc_read(0) < 60); // secondes : 0-59 (seconds: 0-59)
        assert!(mbc.rtc_read(1) < 60); // minutes : 0-59 (minutes: 0-59)
        assert!(mbc.rtc_read(2) < 24); // heures : 0-23 (hours: 0-23)
    }
}
