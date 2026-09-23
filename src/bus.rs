//! Bus système : wrapper minimal autour du MMU existant (étape 1 du plan de refactoring CPU/Bus).
//!
//! Le [`Bus`] emprunte le [`MMU`] et en expose l'API D1 : une lecture ou une écriture CPU sur le bus
//! (`read` / `write`) combine l'accès mémoire avec l'avancement d'un M-cycle (4 T-cycles) du matériel.
//! Les méthodes [`tick_m_cycle`](Bus::tick_m_cycle) et [`idle_m_cycle`](Bus::idle_m_cycle) avancent le
//! matériel d'un M-cycle, avec ou sans accès bus. C'est ici que vit l'avancement PPU / Timer / Série /
//! DMA OAM et la pose des bits IF qui quittaient `Emulator::step` pour entrer dans le Bus (méthode
//! [`advance`](Bus::advance)). Le chemin atomique legacy du CPU (étape 1) n'utilise pas encore les accès
//! par M-cycle : il exécute l'instruction en un bloc puis appelle [`advance`](Bus::advance) une fois.

use std::ops::{Deref, DerefMut};

use crate::mmu::MMU;
use crate::ppu::{IRQ_STAT, IRQ_VBLANK};

/// Bus système : wrapper minimal autour du MMU existant (étape 1).
///
/// Le [`Bus`] emprunte le [`MMU`] (`Deref`/`DerefMut`) et ajoute l'API D1 d'accès par M-cycle. Les accès
/// bruts au MMU (diagnostics, initialisation, tests) passent par le champ public `mmu`, qui ne déclenche
/// aucun avancement du matériel — seuls [`read`](Bus::read)/[`write`](Bus::write) combinent l'accès avec un tick.
pub struct Bus<'a> {
    /// MMU emprunté : la carte d'adresses complète (ROM, VRAM, WRAM, OAM, HRAM, I/O, PPU, Série, Timer…).
    pub mmu: &'a mut MMU,
}

impl<'a> Bus<'a> {
    /// Crée un [`Bus`] autour du MMU emprunté.
    pub fn new(mmu: &'a mut MMU) -> Self {
        Self { mmu }
    }

    /// D1 : lecture CPU sur le bus — accès mémoire + avancement d'un M-cycle (4 T-cycles).
    #[allow(dead_code)] // API de l'étape 2 : les familles portées en micro-ops utiliseront ce chemin.
    pub fn read(&mut self, addr: u16) -> u8 {
        let value = self.mmu.read(addr);
        self.tick_m_cycle();
        value
    }

    /// D1 : écriture CPU sur le bus — accès mémoire + avancement d'un M-cycle (4 T-cycles).
    #[allow(dead_code)] // API de l'étape 2 : les familles portées en micro-ops utiliseront ce chemin.
    pub fn write(&mut self, addr: u16, val: u8) {
        self.mmu.write(addr, val);
        self.tick_m_cycle();
    }

    /// Avance tout le matériel d'un M-cycle (4 T-cycles) : PPU, Série, DMA OAM, Timer + bits IF.
    #[allow(dead_code)] // API de l'étape 2 : un micro-op ReadMem/WriteMem/FetchOpcode consomme ce pas.
    pub fn tick_m_cycle(&mut self) {
        self.advance(4);
    }

    /// M-cycle « idle » : aucun accès bus, mais le matériel avance quand même d'un M-cycle (4 T-cycles).
    #[allow(dead_code)] // API de l'étape 2 : un micro-op Internal consomme ce pas.
    pub fn idle_m_cycle(&mut self) {
        self.advance(4);
    }

    /// Avance tout le matériel de `t_cycles` T-cycles (en bloc) et pose les bits IF correspondants —
    /// l'avancement PPU / Série / DMA OAM / Timer qui quittait `Emulator::step` pour entrer dans le Bus.
    /// Renvoie true quand la frontière de frame est franchie (LY > 153 → 0), pour le traceur d'`Emulator`.
    pub fn advance(&mut self, t_cycles: u32) -> bool {
        let frame_done = self.mmu.ppu.step(t_cycles, &self.mmu.vram, &self.mmu.oam);

        // Les requêtes d'interruption PPU en attente lèvent les bits correspondants du registre IF ($FF0F).
        let ppu_irq = self.mmu.ppu.take_interrupts();
        if ppu_irq != 0 {
            log::debug!(
                "[PPU] IRQ raised: ${:02X}, IF before: ${:02X}, LY={}, mode={}",
                ppu_irq,
                self.mmu.io[0x0F],
                self.mmu.ppu.ly,
                self.mmu.ppu.mode
            );
        }
        if ppu_irq & IRQ_VBLANK != 0 {
            self.mmu.io[0x0F] |= 0x01; // bit 0 de IF : interruption VBlank demandée (vecteur $40)
            log::debug!("[VBlank] IF bit 0 set, new IF=${:02X}", self.mmu.io[0x0F]);
        }
        if ppu_irq & IRQ_STAT != 0 {
            self.mmu.io[0x0F] |= 0x02; // bit 1 de IF : interruption STAT/LCD demandée (vecteur $48)
        }

        // La SCC avance du même nombre de T-cycles ; un transfert achevé lève le drapeau IF série.
        if self.mmu.serial.tick(t_cycles) {
            self.mmu.io[0x0F] |= 0x08; // bit 3 de IF ($FF0F) : interruption série demandée (Pan Docs « Interrupt Sources »)
        }

        // Le DMA OAM avance du même nombre de T-cycles.
        self.mmu.advance_dma(t_cycles);

        // Le Timer avance ; un débordement de TIMA lève le drapeau IF Timer.
        if self.mmu.timer.tick(t_cycles) {
            self.mmu.io[0x0F] |= 0x04; // bit 2 de IF ($FF0F) : interruption Timer demandée (Pan Docs « Interrupt Sources »)
        }

        frame_done
    }
}

impl Deref for Bus<'_> {
    type Target = MMU;
    fn deref(&self) -> &Self::Target {
        self.mmu
    }
}

impl DerefMut for Bus<'_> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.mmu
    }
}
