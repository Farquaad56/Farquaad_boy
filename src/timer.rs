//! Timer : registres DIV ($FF04), TIMA ($FF05), TMA ($FF06) et TAC ($FF07).
//!
//! Partie 9 : émulé sur la base des T-cycles, d'après Pan Docs « Timer and Divider
//! Registers » et « Timer Obscure Behaviour » (périodes, rechargement TMA et comportements
//! d'écriture conformes) :
//! - Un compteur système interne de 16 bits s'incrémente à chaque T-cycle. DIV ($FF04) en est
//!   le haut (bits [15..8]) : il change de valeur toutes les 256 T-cycles, quelle que soit la
//!   valeur de TAC (« DIV is always counting »).
//! - Quand le timer est activé (bit 2 de TAC), TIMA ($FF05) s'incrémente à la période
//!   sélectionnée par les bits 1-0 de TAC :
//!     | TAC bits 1-0 | période      | fréquence  |
//!     |--------------|--------------|------------|
//!     | 00           | 1024 T-cycles| 4096 Hz    |
//!     | 01           | 16 T-cycles  | 262144 Hz  |
//!     | 10           | 64 T-cycles  | 65536 Hz   |
//!     | 11           | 256 T-cycles | 16384 Hz   |
//! - Quand TIMA déborde (passe de $FF à $00), il est rechargé depuis TMA ($FF06) et le
//!   drapeau Timer (bit 2 de IF, $FF0F) est levé un cycle plus tard.
//! - Comportement obscur : écrire $FF04 remet tout le compteur système à zéro ; si le bit
//!   sélectionné était à 1, la remise à zéro envoie un « timer tick » immédiat (TIMA peut
//!   s'incrémenter — et même déborder — sur une simple écriture de DIV). De même, écrire TAC
//!   peut envoyer un unique tick quand le bit sélectionné passe d'un état à 1 vers un état à 0.

/// Bits du compteur système sélectionnés par TAC (bits 1-0) pour l'incrément de TIMA.
const TAC_TRIGGER_BITS: [u16; 4] = [1 << 9, 1 << 3, 1 << 5, 1 << 7];

/// Timer de l'émulateur (registres $FF04-$FF07), synchronisé sur les T-cycles.
#[allow(clippy::upper_case_acronyms)]
#[derive(Default)]

pub struct Timer {
    /// Compteur système interne : s'incrémente à chaque T-cycle quelle que soit la valeur de TAC ;
    /// DIV = bits [15..8]. Placé à $AB00 dans l'état post-boot ROM (DIV se lit alors $AB — PanDocs « Power Up Sequence »).
    pub counter: u16,
    /// Registre TIMA ($FF05) : compteur de timer.
    tima: u8,
    /// Registre TMA ($FF06) : valeur rechargée dans TIMA au débordement.
    tma: u8,
    /// Registre TAC ($FF07) : seuls les bits 2-0 sont écriturables (bit 2 = enable).
    tac: u8,
    /// Accumulateur de phase : T-cycles écoulés depuis le dernier tick de TIMA.
    timer_counter: u32,
    /// Débordement en attente : l'interruption Timer sera levée au prochain `tick` (un cycle plus tard).
    pending_irq: bool,
}

impl Timer {
    /// Crée un timer à l'état power-on (DIV/TIMA/TMA/TAC = $00, timer désactivé).
    #[allow(dead_code)] // Utilisé uniquement par les tests dans la cible binaire (power_on utilise Timer::default())
    pub fn new() -> Self {
        Self::default()
    }

    /// Lecture de DIV ($FF04) : haut du compteur système (change toutes les 256 T-cycles).
    pub fn read_div(&self) -> u8 {
        (self.counter >> 8) as u8
    }

    /// Lecture de TIMA ($FF05).
    pub fn read_tima(&self) -> u8 {
        self.tima
    }

    /// Lecture de TMA ($FF06).
    pub fn read_tma(&self) -> u8 {
        self.tma
    }

    /// Lecture de TAC ($FF07) : les bits 7-3 sont toujours lus à 1.
    pub fn read_tac(&self) -> u8 {
        0xF8 | (self.tac & 0x07)
    }

    /// Écriture de DIV ($FF04) : la valeur écrite est ignorée, le registre se lit $00 —
    /// en réalité tout le compteur système est remis à zéro.
    ///
    /// Comportement obscur (Pan Docs « Timer Obscure Behaviour ») : si le bit sélectionné par TAC
    /// était à 1 dans l'ancien compteur, la remise à zéro envoie un « timer tick » immédiat —
    /// TIMA peut donc s'incrémenter (et même déborder) sur une simple écriture de DIV.
    pub fn write_div(&mut self, _value: u8) {
        let old = self.counter;
        self.counter = 0;
        if self.is_enabled() && (old & Self::trigger_bit(self.tac)) != 0 {
            self.increment_tima();
        }
    }

    /// Écriture de TIMA ($FF05).
    pub fn write_tima(&mut self, value: u8) {
        self.tima = value;
    }

    /// Écriture de TMA ($FF06).
    pub fn write_tma(&mut self, value: u8) {
        self.tma = value;
    }

    /// Écriture de TAC ($FF07) : seuls les bits 2-0 sont pris en compte.
    ///
    /// Comportement obscur (Pan Docs « Timer Obscure Behaviour ») : si le timer était activé et que
    /// le bit sélectionné était à 1, changer la sélection vers un bit qui est à 0 (ou désactiver
    /// le timer) envoie un unique « timer tick ».
    pub fn write_tac(&mut self, value: u8) {
        let old_enabled = self.is_enabled();
        let old_bit = Self::trigger_bit(self.tac);
        self.tac = value & 0x07;
        if old_enabled && (self.counter & old_bit) != 0 {
            let new_bit = Self::trigger_bit(self.tac);
            if !self.is_enabled() || (self.counter & new_bit) == 0 {
                self.increment_tima();
            }
        }
    }

    /// Fait avancer le timer de `cycles` T-cycles.
    /// Renvoie true si l'interruption Timer doit être levée (l'appelant doit alors poser le bit 2 du registre IF).
    pub fn tick(&mut self, cycles: u32) -> bool {
        // Débordement en attente (depuis un tick précédent ou une écriture DIV/TAC) : levé maintenant,
        // soit « un cycle plus tard » par rapport au débordement — Pan Docs « Timer Obscure Behaviour ».
        let mut interrupt = false;
        if self.pending_irq {
            self.pending_irq = false;
            interrupt = true;
        }

        // DIV compte toujours (« DIV is always counting ») : le compteur système avance quelle que soit la valeur de TAC.
        self.counter = self.counter.wrapping_add(cycles as u16);

        if self.is_enabled() {
            let freq = match self.tac & 0x03 {
                0 => 1024,
                1 => 16,
                2 => 64,
                _ => 256,
            };

            self.timer_counter += cycles;
            while self.timer_counter >= freq {
                self.timer_counter -= freq;
                self.increment_tima();
            }
        }

        interrupt
    }

    /// Le timer est-il activé (bit 2 de TAC) ?
    fn is_enabled(&self) -> bool {
        self.tac & 0x04 != 0
    }

    /// Bit du compteur système sélectionné par TAC (bits 1-0).
    fn trigger_bit(tac: u8) -> u16 {
        TAC_TRIGGER_BITS[(tac & 0x03) as usize]
    }

    /// Incrémente TIMA ; un débordement le recharge depuis TMA et met l'interruption en attente (levée au prochain `tick`).
    fn increment_tima(&mut self) {
        if self.tima == 0xFF {
            log::debug!(
                "TIMA débordement : rechargé depuis TMA=${:02X}, interruption Timer en attente",
                self.tma
            );
            self.tima = self.tma;
            self.pending_irq = true;
        } else {
            self.tima += 1;
        }
    }
}

impl std::fmt::Debug for Timer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Timer")
            .field("div", &self.read_div())
            .field("tima", &self.tima)
            .field("tma", &self.tma)
            .field("tac", &self.read_tac())
            .finish()
    }
}
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn power_on_state() {
        let mut t = Timer::new();
        assert_eq!(t.read_div(), 0); // DIV à l'état power-on
        assert_eq!(t.read_tima(), 0); // TIMA à l'état power-on
        assert_eq!(t.read_tma(), 0); // TMA à l'état power-on
        assert_eq!(t.read_tac(), 0xF8); // bits 7-3 toujours lus à 1 ; timer désactivé (bits 2-0 = $00)
        t.tick(4);
        assert_eq!(t.read_tima(), 0); // timer désactivé : TIMA ne s'incrémente pas…
    }

    #[test]
    fn div_increments_every_256_t_cycles() {
        let mut t = Timer::new();
        t.tick(255);
        assert_eq!(t.read_div(), 0); // pas encore de passage à $01…
        t.tick(1); // …le 256e T-cycle passe DIV de $00 à $01
        assert_eq!(t.read_div(), 1);
        t.tick(255);
        assert_eq!(t.read_div(), 1); // toujours $01…
        t.tick(1); // …le 512e T-cycle passe DIV de $01 à $02
        assert_eq!(t.read_div(), 2);

        // Une frame vidéo entière (70224 T-cycles) : le compteur système (16 bits) a tourné.
        t = Timer::new();
        t.tick(70_224);
        assert_eq!(t.read_div(), (((70_224 % 0x10000) as u16) >> 8) as u8); // = 18
    }

    #[test]
    fn div_keeps_counting_while_the_timer_is_disabled() {
        let mut t = Timer::new();
        t.tick(512);
        assert_eq!(t.read_div(), 2); // « DIV is always counting », même avec TAC = $00
        assert_eq!(t.read_tima(), 0);
    }

    #[test]
    fn tima_periods_per_tac_select() {
        // (valeur de TAC, période en T-cycles) — Pan Docs « Timer and Divider Registers ».
        for (tac, period) in [(0x04u8, 1024u32), (0x05, 16), (0x06, 64), (0x07, 256)] {
            let mut t = Timer::new();
            t.write_tac(tac);
            t.tick(period - 1);
            assert_eq!(t.read_tima(), 0, "TAC=${tac:02X} : pas encore d'incrément");
            t.tick(1);
            assert_eq!(
                t.read_tima(),
                1,
                "TAC=${tac:02X} : incrémenté à la période exacte"
            );
        }
    }

    #[test]
    fn overflow_reloads_tma_and_raises_interrupt_one_cycle_later() {
        let mut t = Timer::new();
        t.write_tac(0x07); // select 11 : tick toutes les 256 T-cycles
        t.write_tma(0x33);
        t.write_tima(0xFF);

        assert!(!t.tick(256)); // le bit sélectionné passe à 0 → TIMA déborde, rechargé depuis TMA…
        assert_eq!(t.read_tima(), 0x33);
        assert!(t.tick(1)); // …mais l'interruption n'est demandée que le cycle suivant
        assert!(!t.tick(1)); // et ne se répète pas (état de rechargement terminé)
    }

    #[test]
    fn tma_ff_divides_the_selected_clock() {
        // TMA = $FF : every increment is an overflow → interruption à chaque tick du timer.
        let mut t = Timer::new();
        t.write_tac(0x05); // select 01 : tick toutes les 16 T-cycles
        t.write_tma(0xFF);

        assert!(!t.tick(4_096)); // le 256e incrément (TIMA $FF → débordement) met l'interruption en attente…
        assert_eq!(t.read_tima(), 0xFF); // …rechargé depuis TMA ($FF)
        assert!(t.tick(1)); // …et demandée un cycle plus tard
    }

    #[test]
    fn div_write_resets_the_system_counter_and_can_tick_tima() {
        let mut t = Timer::new();
        t.write_tac(0x05); // select 01 : bit 3 du compteur système
        t.tick(8); // compteur = 8 : le bit sélectionné est à 1

        t.write_div(0x42); // la valeur écrite est ignorée…
        assert_eq!(t.read_div(), 0); // …tout le compteur système est remis à zéro
        assert_eq!(t.read_tima(), 1); // …et le bit qui venait de passer à 0 envoie un « timer tick »

        t.tick(4); // compteur = 4 : le bit sélectionné est à 0
        t.write_div(0x00); // pas de tick cette fois
        assert_eq!(t.read_tima(), 1);
    }

    #[test]
    fn div_write_can_overflow_tima() {
        let mut t = Timer::new();
        t.write_tac(0x05);
        t.write_tma(0x55);
        t.write_tima(0xFF);
        t.tick(8); // bit 3 du compteur à 1

        t.write_div(0x00); // le « timer tick » envoyé par la remise à zéro déborde TIMA…
        assert_eq!(t.read_tima(), 0x55); // …rechargé depuis TMA, interruption en attente
        assert!(t.tick(1)); // …demandée un cycle plus tard
    }

    #[test]
    fn tac_write_can_tick_tima() {
        let mut t = Timer::new();
        // Le bit sélectionné (bit 7) est à 0 : changer la selection n'envoie pas de tick.
        t.write_tac(0x07); // activé, select 11 (bit 7)
        t.write_tac(0x05); // select 01 (bit 3) : l'ancien bit sélectionné est à 0 → pas de tick
        assert_eq!(t.read_tima(), 0);

        // Le bit sélectionné (bit 7) est à 1 et the new one (bit 3) is 0 : single tick.
        t.write_tac(0x07); // select 11 (bit 7) : l'ancien bit sélectionné (bit 3, compteur = 0) est à 0 → pas de tick
        assert_eq!(t.read_tima(), 0);
        t.tick(128); // compteur = 128 : bit 7 à 1, bit 3 à 0
        t.write_tac(0x05); // select 01 (bit 3) : l'ancien bit sélectionné (bit 7) est à 1 et the new one is 0 → tick unique
        assert_eq!(t.read_tima(), 1);
    }

    #[test]
    fn tac_write_disabling_the_timer_ticks_once() {
        let mut t = Timer::new();
        t.write_tac(0x05); // activé, select 01 (bit 3)
        t.tick(8); // bit 3 à 1

        t.write_tac(0x01); // timer désactivé alors que le bit sélectionné est à 1 → tick unique
        assert_eq!(t.read_tima(), 1);
        assert_eq!(t.read_tac(), 0xF9); // bits 7-3 toujours lus à 1, enable effacé

        t.write_tac(0x05); // réactivé : l'ancien état était désactivé → pas de tick
        assert_eq!(t.read_tima(), 1);
    }

    #[test]
    fn tac_writable_bits_and_readback() {
        let mut t = Timer::new();
        t.write_tac(0x06); // seuls les bits 2-0 sont pris en compte (0b110)
        assert_eq!(t.read_tac(), 0xFE); // 0xF8 | 6
        t.write_tac(0x07);
        assert_eq!(t.read_tac(), 0xFF);
    }

    #[test]
    fn overflow_reload_then_plain_writes() {
        let mut t = Timer::new();
        t.write_tac(0x07); // select 11 : tick toutes les 256 T-cycles
        t.write_tma(0x33);
        t.write_tima(0xFF);

        assert!(!t.tick(256)); // débordement : TIMA rechargé depuis TMA, interruption en attente…
        assert_eq!(t.read_tima(), 0x33);
        assert!(t.tick(1)); // …levée un cycle plus tard

        t.write_tma(0x44); // écriture simple : pas de fenêtre de rechargement particulière…
        assert_eq!(t.read_tma(), 0x44);
        assert_eq!(t.read_tima(), 0x33); // …TIMA n'est pas recopié depuis TMA avant le prochain tick
        t.write_tima(0x55); // écriture simple : prise en compte immédiatement
        assert_eq!(t.read_tima(), 0x55);
    }
}
