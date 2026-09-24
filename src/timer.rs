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
//!   drapeau Timer (bit 2 de IF, $FF0F) est levé un cycle plus tard. Le rechargement n'est pas lisible
//!   immédiatement : TIMA se lit `$00` pendant une fenêtre de rechargement d'1 M-cycle (4 T-cycles),
//!   au cours de laquelle une écriture sur TIMA est ignorée et une écriture sur TMA change la valeur qui sera
//!   effectivement chargée dans TIMA (Pan Docs « Timer » / Mooneye `tima_reload.s`,
//!   `tima_write_reloading.s`, `tma_write_reloading.s`).
//! - Comportement obscur : écrire $FF04 remet tout le compteur système à zéro ; si le bit
//!   sélectionné était à 1, la remise à zéro envoie un « timer tick » immédiat (TIMA peut
//!   s'incrémenter — et même déborder — sur une simple écriture de DIV). De même, écrire TAC
//!   peut envoyer un tick unique quand le bit sélectionné passe d'un état à 1 vers un état à 0.

/// Bits du compteur système sélectionnés par TAC (bits 1-0) pour l'incrément de TIMA.
const TAC_TRIGGER_BITS: [u16; 4] = [1 << 9, 1 << 3, 1 << 5, 1 << 7];

/// Durée de la fenêtre de rechargement en T-cycles : exactement 1 M-cycle (4 T). Cette même constante est
/// le décalage structurel d'1 M-cycle par lequel l'action d'un front (l'incrément de TIMA) est retardée
/// relativement au wrap physique du bit sélectionné.
const RELOAD_WINDOW_LEN: u32 = 4;

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
    /// Valeur du compteur système (à la fin du tick) à laquelle le dernier débordement PÉRIODIQUE de TIMA a été appliqué.
    /// La fenêtre de rechargement `$00` est active tant que `(counter - last_overflow) mod 2^16 < RELOAD_WINDOW_LEN`.
    last_overflow: Option<u32>,
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

    /// Lecture de TIMA ($FF05) : se lit `$00` pendant la fenêtre de rechargement.
    pub fn read_tima(&self) -> u8 {
        if self.in_reload_window() {
            0
        } else {
            self.tima
        }
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
            if self.increment_tima() {
                self.pending_irq = true;
            }
        }
    }

    /// Écriture de TIMA ($FF05) : ignorée pendant la fenêtre de rechargement.
    pub fn write_tima(&mut self, value: u8) {
        if !self.in_reload_window() {
            self.tima = value;
        }
    }

    /// Écriture de TMA ($FF06) : toujours effective — y compris pendant la fenêtre de rechargement, où elle change
    /// la valeur qui sera effectivement chargée dans TIMA (Mooneye `tma_write_reloading.s`).
    pub fn write_tma(&mut self, value: u8) {
        self.tma = value;
        if self.in_reload_window() {
            // TIMA suit la nouvelle valeur de TMA pendant la fenêtre : au débordement suivant (ou à la fin de la
            // fenêtre), TIMA est rechargé avec cette valeur.
            self.tima = value;
        }
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
                if self.increment_tima() {
                    self.pending_irq = true;
                }
            }
        }
    }

    /// Fait avancer le timer de `cycles` T-cycles.
    /// Renvoie true si l'interruption Timer doit être levée (l'appelant doit alors poser le bit 2 du registre IF).
    pub fn tick(&mut self, cycles: u32) -> bool {
        // Débordement en attente (depuis un tick précédent ou une écriture DIV/TAC) : levé maintenant, soit « un cycle plus tard »
        // par rapport au débordement — Pan Docs « Timer Obscure Behaviour ».
        let mut interrupt = false;
        if self.pending_irq {
            self.pending_irq = false;
            interrupt = true;
        }

        // DIV compte toujours (« DIV is always counting ») : le compteur système avance quelle que soit la valeur de TAC.
        let c0 = self.counter as u32;
        let c1 = c0 + cycles;
        self.counter = (c1 % 0x10000) as u16;

        if self.is_enabled() {
            let freq = match self.tac & 0x03 {
                0 => 1024,
                1 => 16,
                2 => 64,
                _ => 256,
            };

            // Fronts 1→0 du bit sélectionné : les wraps w = k·freq (k ≥ 1) dont le point effectif e = w + RELOAD_WINDOW_LEN
            // tombe dans [c0, c1). Le décalage d'1 M-cycle (+RELOAD_WINDOW_LEN) retarde l'action de chaque front relativement au
            // wrap physique ; la borne haute stricte (e < c1) est l'off-by-one qui retarde l'incrément de 1 T-cycle.
            // Le compte couvre tous les fronts franchis par ce tick — a priori un seul (la granularité d'appel est ≥ 4 T-cycles
            // et la période minimale est de 16 T), mais la formule gère correctement le cas où une seule avance en franchit plusieurs.
            let a = c0.saturating_sub(RELOAD_WINDOW_LEN).max(freq);
            let b = c1.saturating_sub(RELOAD_WINDOW_LEN);
            let fronts = if b > a {
                ((b - 1) / freq).saturating_sub((a - 1) / freq)
            } else {
                0
            };

            for _ in 0..fronts {
                if self.increment_tima() {
                    // Le débordement périodique démarre la fenêtre de rechargement `$00` (active tant que le compteur système
                    // est à moins de RELOAD_WINDOW_LEN T-cycles de c1, la fin de ce tick).
                    self.last_overflow = Some(c1 % 0x10000);
                    self.pending_irq = true;
                }
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

    /// Incrémente TIMA de 1 ; renvoie `true` si cela a débordé (TIMA rechargé depuis TMA).
    fn increment_tima(&mut self) -> bool {
        if self.tima == 0xFF {
            log::debug!("TIMA débordement : rechargé depuis TMA=${:02X}", self.tma);
            self.tima = self.tma;
            true
        } else {
            self.tima += 1;
            false
        }
    }

    /// La fenêtre de rechargement `$00` est-elle active (TIMA se lit `$00`, écriture de TIMA ignorée) ?
    fn in_reload_window(&self) -> bool {
        match self.last_overflow {
            Some(ov) => (self.counter as u32).wrapping_sub(ov) < RELOAD_WINDOW_LEN,
            None => false,
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
            // L'action du front est retardée par un décalage structurel d'1 M-cycle (+RELOAD_WINDOW_LEN) relativement au
            // wrap physique : l'incrément ne devient visible que lorsque le compteur système a dépassé period + RELOAD_WINDOW_LEN.
            t.tick(period + 4);
            assert_eq!(t.read_tima(), 0, "TAC=${tac:02X} : pas encore d'incrément (décalage structurel d'1 M-cycle)");
            t.tick(1);
            assert_eq!(
                t.read_tima(),
                1,
                "TAC=${tac:02X} : incrémenté à la période + décalage structurel d'1 M-cycle"
            );
        }
    }

    #[test]
    fn overflow_reloads_tma_and_raises_interrupt_one_cycle_later() {
        let mut t = Timer::new();
        t.write_tac(0x07); // sélection 11 : tick toutes les 256 T-cycles
        t.write_tma(0x33);
        t.write_tima(0xFF);

        assert!(!t.tick(261)); // compteur = 261 > e = 260 : le front déborde, rechargé depuis TMA ; interruption en attente (pas encore levée)
        assert_eq!(t.read_tima(), 0x00); // …mais TIMA se lit `$00` pendant la fenêtre de rechargement d'1 M-cycle…
        assert!(t.tick(4)); // …lève l'interruption (un cycle plus tard que le débordement) et passe la fenêtre
        assert_eq!(t.read_tima(), 0x33); // …rechargé depuis TMA une fois la fenêtre passée
        assert!(!t.tick(1)); // et ne se répète pas (état de rechargement terminé)
    }

    #[test]
    fn tma_ff_divides_the_selected_clock() {
        // TMA = $FF : chaque incrément est un débordement → interruption à chaque tick du timer.
        let mut t = Timer::new();
        t.write_tac(0x05); // sélection 01 : tick toutes les 16 T-cycles
        t.write_tma(0xFF);

        assert!(!t.tick(4_101)); // compteur = 4101 > e = 4100 : le 256e incrément (w = 4096) déborde → interruption en attente, pas encore levée
        assert_eq!(t.read_tima(), 0x00); // …TIMA se lit `$00` pendant la fenêtre de rechargement…
        assert!(t.tick(4)); // …lève l'interruption (un cycle plus tard) et passe la fenêtre
        assert_eq!(t.read_tima(), 0xFF); // …rechargé depuis TMA ($FF) une fois la fenêtre passée
    }

    #[test]
    fn div_write_resets_the_system_counter_and_can_tick_tima() {
        let mut t = Timer::new();
        t.write_tac(0x05); // sélection 01 : bit 3 du compteur système
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
        assert_eq!(t.read_tima(), 0x55); // …rechargé depuis TMA, interruption en attente (pas de fenêtre : quirk immédiat)
        assert!(t.tick(1)); // …demandée un cycle plus tard
    }

    #[test]
    fn tac_write_can_tick_tima() {
        let mut t = Timer::new();
        // Le bit sélectionné (bit 7) est à 0 : changer la sélection n'envoie pas de tick.
        t.write_tac(0x07); // activé, sélection 11 (bit 7)
        t.write_tac(0x05); // sélection 01 (bit 3) : l'ancien bit sélectionné est à 0 → pas de tick
        assert_eq!(t.read_tima(), 0);

        // Le bit sélectionné (bit 7) est à 1 et le nouveau (bit 3) est à 0 : tick unique.
        t.write_tac(0x07); // sélection 11 (bit 7) : l'ancien bit sélectionné (bit 3, compteur = 0) est à 0 → pas de tick
        assert_eq!(t.read_tima(), 0);
        t.tick(128); // compteur = 128 : bit 7 à 1, bit 3 à 0
        t.write_tac(0x05); // sélection 01 (bit 3) : l'ancien bit sélectionné (bit 7) est à 1 et le nouveau est à 0 → tick unique
        assert_eq!(t.read_tima(), 1);
    }

    #[test]
    fn tac_write_disabling_the_timer_ticks_once() {
        let mut t = Timer::new();
        t.write_tac(0x05); // activé, sélection 01 (bit 3)
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
        t.write_tac(0x07); // sélection 11 : tick toutes les 256 T-cycles
        t.write_tma(0x33);
        t.write_tima(0xFF);

        assert!(!t.tick(261)); // débordement (e = 260 franchi) : TIMA rechargé depuis TMA, interruption en attente…
        assert_eq!(t.read_tima(), 0x00); // …se lit `$00` pendant la fenêtre de rechargement d'1 M-cycle
        assert!(t.tick(4)); // …levée un cycle plus tard et passe la fenêtre

        t.write_tma(0x44); // écriture simple hors fenêtre : pas de recopie…
        assert_eq!(t.read_tma(), 0x44);
        assert_eq!(t.read_tima(), 0x33); // …TIMA n'est pas recopié depuis TMA avant le prochain tick
        t.write_tima(0x55); // écriture simple hors fenêtre : prise en compte immédiatement
        assert_eq!(t.read_tima(), 0x55);
    }
}
