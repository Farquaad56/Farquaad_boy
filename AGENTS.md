# Consigne de travail (en vigueur)

**Chantier : comprendre et corriger l'ordre tick/accès bus dans `Bus::read`/`Bus::write`, en deux phases strictement séparées.**

## Acquis (terminé, committé, ne pas retoucher sans mandat explicite)
- Étape 1 : wrapper `Bus` + `Cpu::tick` + `MicroOp` + latches W/Z + table de cycles depuis `data/Opcodes.json` (`d2701a1`).
- Étape 2, point 1 — famille `(HL) écriture` (`$70-75, $77, $36, $22, $32`) portée, `$36` corrigé à 3 M-cycles (`12c107b`).
- Rebranchement de `Emulator::step()` sur `cpu.tick()` en boucle jusqu'à complétion d'une instruction portée — neutre côté timing, confirmé A/B.
- Étape 2, point 2 — famille `(HL) lecture` (`$46-7E, $0A, $1A, $F2, $F0, $FA, CB xx,(HL) lecture seule`) portée, dispatch vérifié complet dans `ported_steps()` (`e6d662c`).

## Constat qui motive ce chantier
Les deux familles portées ci-dessus sont structurellement correctes (M-cycles validés contre `Opcodes.json` ET GBCTR chapitre 6, tests d'entrelacement avec vérification d'adresse bus). Pourtant, `02-write_timing.gb` et `01-read_timing.gb` de Blargg restent tous les deux en échec, avec un écart résiduel constant. Une expérience antérieure (inversion globale de l'ordre tick/write dans `Bus::write`) a fait disparaître la liste d'échecs affichée mais a empêché le ROM de terminer (boucle de resynchronisation `sync_tima_64` qui ne sort plus jamais, TIMA pinné à 0). Conclusion : l'ordre compte, mais une inversion globale et naïve est fausse. La cause exacte n'est pas comprise.

## Phase 1 — Investigation SEULEMENT. Aucune modification de `Bus::read`/`Bus::write` pendant cette phase.
Objectif : comprendre précisément, au niveau du front d'horloge, quand un accès bus prend effet relativement à l'avancement du Timer/PPU/DMA à l'intérieur d'un même M-cycle — avant d'écrire le moindre correctif.

Sources à exploiter, dans cet ordre :
1. **Mooneye-gb test suite** — télécharger/localiser les ROMs `acceptance/timer/*` et `acceptance/ppu/*` orientées M-cycle (notamment tout ce qui touche `tima_write_reloading`, `tim00`/`tim01`/etc., et les tests `mem_timing`/`mem_timing-2` de la suite Mooneye elle-même, distincts de ceux de Blargg). Les faire tourner tel quel (headless, capture de sortie) pour voir lesquels passent/échouent déjà avec le code actuel — ça donne une cartographie plus fine que Blargg seul.
2. **GBCTR, Figure 2.2 "Clock edges in a machine cycle" (page 14)** — cette figure n'a jamais été lue visuellement (seulement son titre extrait en texte). Rastériser la page et la lire pour voir si elle précise sur quel front (montant/descendant de PHI) un accès mémoire est échantillonné par rapport au reste du système.
3. **GBCTR, chapitre Timer** (si un tel chapitre existe dans cette révision — à vérifier, la table des matières n'en montrait pas explicitement lors de la dernière recherche) ou toute section décrivant le compteur système (`DIV`) au niveau du front d'horloge.
4. Si GBCTR ne suffit pas : chercher des sources tierces reconnues dans la communauté gbdev (ex. les notes de SameBoy ou de Mooneye-gb elles-mêmes sur ce sujet précis) — mais seulement en dernier recours, et en citant la source exacte.

Livrable de la phase 1 : un rapport écrit, avant tout code, qui répond précisément à : « à quel moment, dans un M-cycle donné, un accès bus (lecture ou écriture) doit-il être visible pour un composant qui compte les cycles en parallèle (Timer notamment) ? », avec citations. Si la réponse reste incertaine après ces sources, le dire explicitement plutôt que de deviner.

## Phase 2 — Correctif, seulement après feu vert explicite sur le rapport de phase 1
- Le correctif doit s'appuyer sur ce que la phase 1 a établi, pas sur un essai-erreur global comme la première tentative.
- Avant tout correctif : réexpliquer pourquoi l'inversion globale précédente a cassé `sync_tima_64`, à la lumière de ce que la phase 1 a appris — si cette explication ne tient pas, le correctif proposé a probablement le même défaut.
- Les tests d'entrelacement des familles `(HL)` écriture et lecture (déjà committés) doivent rester verts après le correctif — ce sont le filet de sécurité de ce chantier.
- Vérification finale : `cargo test` A/B (ensembles d'échecs, pas totaux), `cpu_instrs` (11/11), puis `01-read_timing.gb` ET `02-write_timing.gb` de Blargg — les deux doivent progresser, pas seulement un des deux.
- Ne committer qu'après mon feu vert explicite.

## Interdictions explicites
- Ne pas toucher à `Bus::read`/`Bus::write` avant la fin de la phase 1 et mon feu vert sur son rapport.
- Ne pas porter de nouvelle famille d'instructions pendant ce chantier — c'est une pause volontaire du portage, pas un abandon (les points 3-5 de l'Étape 2 restent au plan, après ce chantier).
- Ne pas toucher au PPU.
- Ne jamais modifier ce fichier (`AGENTS.md`) toi-même.
- Ne pas committer sans mon feu vert explicite à chaque phase.