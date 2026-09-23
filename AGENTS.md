# Consigne de travail (en vigueur)

**Chantier : corriger `timer.rs` (incrément TIMA et fenêtre de rechargement), phase 2 — correctif, sous condition.**

## Acquis (terminé, committé, ne pas retoucher sans mandat explicite)
- Étape 1 (`d2701a1`), Étape 2 points 1 et 2 — familles `(HL)` écriture et lecture portées en micro-op (`12c107b`, `e6d662c`), rebranchement `step()` neutre.
- Phase 1 de ce chantier (investigation) : cause racine identifiée et vérifiée indépendamment sur le code réel (clone du HEAD, lecture directe de `timer.rs`), deux bugs précis, pas une question d'ordre `Bus::read`/`Bus::write` global comme supposé initialement :
  1. `tick()` fait avancer TIMA via un accumulateur de phase séparé (`timer_counter: u32`), découplé du compteur système partagé (`counter: u16`) que lit `DIV`. Une écriture sur `DIV` (`write_div`) remet `counter` à zéro mais ne touche jamais `timer_counter` → confirmé cause du fail de `div_write.s`.
  2. `increment_tima()` recharge TMA dans TIMA **immédiatement** au débordement, sans jamais laisser TIMA lisible à `$00` pendant la fenêtre d'1 M-cycle documentée par Pan Docs/Mooneye → confirmé cause des fails de `tima_reload.s`, `tima_write_reloading.s`, `tma_write_reloading.s`.
- Diagnostic Mooneye avec le code actuel (13 ROMs `acceptance/timer/`) : seul `rapid_toggle.s` passe ; les 12 autres échouent, tous rattachés aux deux causes ci-dessus.

## Mandat actuel
Corriger `timer.rs`, et uniquement `timer.rs`, pour :

1. **Dériver l'incrément de TIMA d'une détection de front sur le compteur système partagé (`counter`)**, pas d'un accumulateur séparé. Le mécanisme existe déjà et fonctionne pour les quirks d'écriture (`TAC_TRIGGER_BITS`, utilisé par `write_div`/`write_tac`) — il faut l'appliquer aussi au tick périodique normal : à chaque avancement de `counter`, détecter la transition 1→0 du bit sélectionné par `TAC_TRIGGER_BITS[tac & 0x03]` et déclencher `increment_tima()` sur cette transition, pas sur un seuil de cycles accumulés. Vérifie s'il est possible qu'un seul appel à `tick(cycles)` franchisse plusieurs fronts (selon la granularité d'appel réelle — M-cycle = 4 T, période minimale 16 T, donc a priori un seul front par appel, mais démontre-le plutôt que de le supposer) ; si oui, la détection doit gérer ce cas correctement.
2. **Ajouter la fenêtre de rechargement `$00`** : après un débordement, TIMA doit rester lisible à `$00` pendant exactement 1 M-cycle avant que TMA n'y soit copié. Pendant cette fenêtre précise :
   - une écriture sur TIMA doit être ignorée (`tima_write_reloading.s`) ;
   - une écriture sur TMA doit modifier la valeur qui sera effectivement chargée (`tma_write_reloading.s`).

Avant d'écrire le moindre code : relis les 5 sources assembleur Mooneye déjà lues en phase 1 (`div_write.s`, `tim00-11(_div_trigger).s`, `tima_reload.s`, `tima_write_reloading.s`, `tma_write_reloading.s`) et extrais-en un tableau exact des transitions d'état attendues (quel registre, quel M-cycle, quelle valeur), comme on l'a fait pour les tableaux M-cycle GBCTR — pas d'implémentation à l'aveugle.

## Critère de sortie
- Les 12 ROMs Mooneye `acceptance/timer/` actuellement en échec doivent passer (les 13, `rapid_toggle.s` inclus, doivent rester verts).
- Les tests unitaires Rust existants de `timer.rs` (`div_increments_every_256_t_cycles`, `tima_periods_per_tac_select`, `overflow_reloads_tma_and_raises_interrupt_one_cycle_later`, `tac_write_can_tick_tima`, etc.) doivent rester verts — comparaison A/B, ensembles d'échecs, pas seulement totaux.
- `cpu_instrs.gb` : 11/11 (ne doit pas être affecté, mais à vérifier).
- Relancer `01-read_timing.gb` et `02-write_timing.gb` de Blargg : rapporter si le résidu observé sur les deux familles déjà portées (`$F0/$FA/CB xx,(HL)` et le groupe `$46..$7E/$F2/$0A/$1A`) évolue — c'est la confirmation croisée qu'on attend depuis le début de ce chantier.

## Interdictions explicites
- Ne toucher qu'à `timer.rs`. Ne pas modifier `Bus::read`/`Bus::write`, `mmu.rs`, ni le dispatch des micro-op CPU.
- Ne pas porter de nouvelle famille d'instructions pendant ce chantier.
- Ne pas toucher au PPU.
- Ne jamais modifier ce fichier (`AGENTS.md`) toi-même.
- Ne pas committer sans mon feu vert explicite. Rapport d'abord (tableau des transitions attendues extrait des sources Mooneye), implémentation ensuite, seulement après confirmation que le tableau est correct.