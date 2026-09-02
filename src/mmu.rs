//! Memory Management Unit : pont entre le CPU et la mémoire.
//!
//! Implémente toute la carte d'adresses de la Game Boy (PanDocs « Memory Map »)
//! ainsi que le routage vers le contrôleur MBC unifié ([`mbc::Mbc`] : commutation des banques
//! ROM/SRAM, mode de banking MBC1 et registres RTC MBC3).
//!
//! Étape 1 : les registres PPU ($FF40-$FF4B, dont le DMA OAM $FF46) sont routés vers la structure
//! [`ppu::PPU`] embarquée dans le MMU ; partie 7 : les registres SCC ($FF01-$FF02) vers
//! [`serial::Serial`], et partie 9 : les registres Timer ($FF04-$FF07) vers [`timer::Timer`].
//! Partie 10 : l'écriture du registre DMA OAM $FF46 déclenche le transfert des 160 octets d'OAM
//! ($FE00-$FE9F) depuis l'adresse source `value << 8`, lue via la carte d'adresses (Pan Docs « DMA »).
//! Le tableau `io` ne stocke que les valeurs brutes des autres registres (joypad… — parties 7+).

use crate::cartridge::{parse_header, CartridgeHeader};
use crate::mbc::{HEADER_TYPE_ADDR, Mbc, MbcType};
use crate::ppu::PPU;
use crate::serial::Serial;
use crate::timer::Timer;

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
    /// PC de l'instruction en cours d'exécution par le CPU — maintenu par `CPU::step`, traçage debug uniquement.
    pub cpu_pc: u16,

    // --- Contrôleur MBC cartouche (PanDocs « MBCs ») ---
    /// État unifié du contrôleur de mémoire : type détecté ($0147), banques ROM/SRAM,
    /// activation RAM, mode de banking MBC1 et registres RTC MBC3.
    pub mbc: Mbc,
    // --- En-tête de cartouche analysé (PanDocs « The Cartridge Header ») ---
    /// En-tête de cartouche ($0100-$014F) analysé au chargement ; None tant qu'aucune ROM n'est chargée.
    pub cartridge: Option<CartridgeHeader>,
}

impl MMU {
    pub fn new() -> Self {
        Self::default()
    }

    /// PC de l'instruction en cours d'exécution (maintenu par `CPU::step`) — traçage debug uniquement.
    fn cpu_pc_for_debug(&self) -> u16 {
        self.cpu_pc
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
                fresh.cartridge = Some(header);
                fresh.rom = data;
                fresh.mbc = Mbc::new(mbc_type); // état power-up du contrôleur détecté
            }
            Err(err) => {
                log::error!("[MMU] En-tête de cartouche invalide : {err} — chargement tel quel (ROM ONLY).");
                let header_byte = data.get(HEADER_TYPE_ADDR as usize).copied().unwrap_or(0x00);
                fresh.rom = data;
                fresh.mbc = Mbc::new(MbcType::from_header_byte(header_byte)); // repli : type détecté depuis $0147 si lisible
            }
        }
        *self = fresh;
    }

    /// En-tête de cartouche analysé au chargement (None tant qu'aucune ROM n'est chargée).
    #[allow(dead_code)] // API publique du MMU — utilisée par les tests et les parties futures (affichage UI, sauvegarde d'état).
    pub fn cartridge(&self) -> Option<&CartridgeHeader> {
        self.cartridge.as_ref()
    }

    /// Taille de la ROM chargée en octets (0 si aucune ROM n'est chargée).
    pub fn rom_size(&self) -> usize {
        self.rom.len()
    }

    /// Lit un octet sur toute la carte d'adresses 16 bits.
    /// Les zones non mappées renvoient $FF, comme sur le matériel réel.
    pub fn read(&self, addr: u16) -> u8 {
        match addr {
            // Région ROM : banque active selon le contrôleur MBC (PanDocs « MBC1 » / « MBC3 »).
            0x0000..=0x7FFF => self.read_rom_at(self.mbc.get_rom_addr(addr) as u32),
            // Video RAM.
            0x8000..=0x9FFF => self.vram[(addr - 0x8000) as usize],
            // SRAM cartouche / registres RTC (open bus $FF si la RAM est désactivée ou absente).
            0xA000..=0xBFFF => self.mbc.read_ram(addr).unwrap_or(0xFF),
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
        // TEMP (diagnostic) : trace TOUTES les écritures du CPU vers les registres I/O ($FF00-$FF4B et $FFFF),
        // avec le PC de l'instruction en cours — pour reconstituer la séquence d'initialisation complète du jeu.
        if (0xFF00..=0xFF4B).contains(&addr) || addr == 0xFFFF {
            log::debug!(
                "[IO TRACE] CPU (PC=${:04X}) écrit ${:02X} → ${:04X}",
                self.cpu_pc_for_debug(),
                value,
                addr
            );
        }
        match addr {
            // Région ROM : les écritures vont aux registres du contrôleur MBC actif (la ROM est en lecture seule).
            // Le routage exact des adresses dépend du type détecté ($0147) — PanDocs « MBCs » / « MBC2 ».
            0x0000..=0x7FFF => self.mbc.write_register(addr, value),
            // Video RAM.
            0x8000..=0x9FFF => self.vram[(addr - 0x8000) as usize] = value,
            // SRAM cartouche / registres RTC (ignorées si la RAM est désactivée ou absente).
            0xA000..=0xBFFF => self.mbc.write_ram(addr, value),
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
            0xFF41 => {
                // STAT ($FF41) : trace INCONDITIONNELLE de TOUTES les écritures du CPU (même la valeur $00),
                // avec le PC de l'instruction en cours — pour identifier qui efface le bit d'activation
                // VBlank (bit 5) et condamnerait le jeu à une boucle infinie réveillée par le Timer.
                log::debug!(
                    "[MMU TRACE] CPU (PC=${:04X}) écrit dans STAT ($FF41) : valeur=${:02X} (VBlank IRQ enable: {})",
                    self.cpu_pc_for_debug(),
                    value,
                    (value & 0x20) != 0
                );
                self.ppu.write_register(addr, value); // bits d'activation des interruptions (bits 3..6) routés vers la PPU
            }
            0xFF46 => {
                // DMA OAM : l'écriture de $FF46 copie les 160 octets d'OAM depuis l'adresse source (value << 8).
                self.ppu.write_register(addr, value); // enregistre l'adresse source dans le registre DMA
                self.dma_transfer(value);
            }
            0xFF40..=0xFF4B => self.ppu.write_register(addr, value),
            // Autres registres I/O.
            0xFF00..=0xFF7F => self.io[(addr - 0xFF00) as usize] = value,
            // High RAM.
            0xFF80..=0xFFFE => self.hram[(addr - 0xFF80) as usize] = value,
            // Registre Interrupt Enable.
            0xFFFF => self.ie = value,
        }
    }

    /// Transfert DMA OAM (Pan Docs « DMA ») : l'écriture de $FF46 copie les 160 octets d'OAM
    /// ($FE00-$FE9F) depuis l'adresse source `source << 8` ($0000-$FFFF), lue via la carte d'adresses.
    fn dma_transfer(&mut self, source: u8) {
        let base = (source as usize) << 8; // adresse source : $0000-$FFFF
        for i in 0..self.oam.len() {
            let byte = self.read((base + i) as u16);
            self.oam[i] = byte;
        }
    }

    /// Lit dans le fichier ROM ; $FF au-delà de sa fin (open bus).
    fn read_rom_at(&self, idx: u32) -> u8 {
        match self.rom.get(idx as usize) {
            Some(byte) => *byte,
            None => 0xFF,
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
            cpu_pc: 0, // maintenu par `CPU::step` (traçage debug uniquement) ; $0000 à l'initialisation.
            mbc: Mbc::new(MbcType::RomOnly), // état power-up ; remplacé par le type détecté dans `load_rom`.
            cartridge: None, // remplacé par l'en-tête analysé dans `load_rom`.
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
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

        // STAT ($FF41) : seuls les bits 3..6 sont écrits ; la lecture renvoie les bits d'activation + le drapeau LYC==LY (bit 7).
        mmu.write(0xFF41, 0x78);
        assert_eq!(mmu.read(0xFF41), 0xF8); // bits écrits 0x78 + drapeau LYC==LY en bit 7 (ly == lyc == 0) ; les bits 0-2 se lisent à 0.

        // Le DMA OAM ($FF46) est routé vers la PPU, pas vers `io`.
        mmu.write(0xFF46, 0xC0);
        assert_eq!(mmu.read(0xFF46), 0xC0);
    }

    #[test]
    fn dma_oam_transfer_copies_from_source() {
        let mut mmu = MMU::new();
        // Source en WRAM : les 160 octets à $C000-$C09F (source byte $C0 → base $C000).
        for i in 0..0xA0 {
            mmu.write(0xC000 + i, (i as u8) ^ 0x5A);
        }
        // L'écriture de $FF46 = $C0 copie les 160 octets depuis $C000 vers OAM ($FE00-$FE9F).
        mmu.write(0xFF46, 0xC0);

        for i in 0..0xA0 {
            assert_eq!(mmu.read(0xFE00 + i), (i as u8) ^ 0x5A); // OAM = contenu de $C000-$C09F
        }
        assert_eq!(mmu.read(0xFF46), 0xC0); // le registre DMA mémorise l'adresse source
    }

    #[test]
    fn dma_oam_transfer_reads_through_full_address_map() {
        let mut mmu = MMU::new();
        // Source en VRAM : les 160 octets à $8200-$829F (source byte $82 → base $8200).
        for i in 0..0xA0 {
            mmu.write(0x8200 + i, 0x3C);
        }
        mmu.write(0xFF46, 0x82);

        assert_eq!(mmu.read(0xFE00), 0x3C); // premier octet OAM = $8200
        assert_eq!(mmu.read(0xFE9F), 0x3C); // dernier octet OAM = $829F
    }

    #[test]
    fn mbc3_header_enables_rom_and_rtc_banking() {
        let mut rom = vec![0x00; 0x8000 * 4]; // 128 KiB (MBC3)
        rom[0x147] = 0x11; // MBC3 (PanDocs « The Cartridge Header »)
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
        use crate::cartridge::{compute_header_checksum, HEADER_CHECKSUM_ADDR, LOGO_LEN, LOGO_START, NINTENDO_LOGO};
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

        let header = mmu.cartridge().expect("l'en-tête doit être analysé au chargement");
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
}