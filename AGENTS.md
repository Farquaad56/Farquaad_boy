# Consigne de travail (en vigueur)

**Exécuter l'Étape 2, point 3 du plan de refactoring CPU/Bus : porter la famille `(HL)` lecture-modification-écriture en micro-op.**

## Acquis (terminé, committé, ne pas retoucher sans mandat explicite)
- Étape 1 : wrapper `Bus` + `Cpu::tick` + `MicroOp` + latches W/Z + table de cycles depuis `data/Opcodes.json` (`d2701a1`).
- Étape 2 point 1 — `(HL)` écriture (`$70-75, $77, $36, $22, $32`), `$36` corrigé à 3 M-cycles (`12c107b`).
- Étape 2 point 2 — `(HL)` lecture (`$46-7E, $0A, $1A, $F2, $F0, $FA, CB xx,(HL)` lecture seule/`BIT`) (`e6d662c`).
- Chantier Timer : détection de front sur compteur partagé + fenêtre de rechargement `$00`, 13 ROMs Mooneye `acceptance/timer/` comme garde-fou permanent (`ca1aa54`, `0393a24`, `9ec315f`).

## Mandat actuel
Porter la famille `(HL)` lecture-modification-écriture :
- **`INC (HL)`** (`$34`), **`DEC (HL)`** (`$35`) — 3 M-cycles chacun (fetch, lecture, écriture).
- **Tous les `CB xx,(HL)` sauf `BIT`** (déjà portés au point 2) — c'est-à-dire les rotations/décalages et `RES`/`SET` sur `(HL)` : `RLC/RRC/RL/RR/SLA/SRA/SWAP/SRL (HL)` (`CB 06/0E/16/1E/26/2E/36/3E`) et `RES b,(HL)`/`SET b,(HL)` (`CB 86-BE` pairs, `CB C6-FE` pairs) — 4 M-cycles chacun (fetch CB, fetch sous-opcode, lecture, écriture).

Avant d'écrire le moindre code : cite le tableau M-cycle GBCTR chapitre 6 pour chaque opcode de cette liste (comme pour les deux familles précédentes) et croise avec `Opcodes.json`. Signale toute divergence entre le compte legacy et la référence — comme `$36` en son temps — avant de l'implémenter, ne la corrige pas silencieusement.

Point d'attention particulier : `INC (HL)`/`DEC (HL)` et les `CB (HL)` de cette famille modifient les flags **en plus** de lire-puis-écrire — vérifie que le calcul des flags a lieu au bon M-cycle (généralement au moment de la lecture, avant l'écriture) et ne dépend pas de la valeur déjà écrite.

Méthode imposée, identique aux points précédents :
1. Micro-programme + test d'entrelacement dédié par opcode (M-cycles séparés observables, adresse bus vérifiée où pertinent).
2. A/B `cargo test` (ensembles d'échecs, pas totaux).
3. `cpu_instrs` (11/11 attendu) + `mooneye_timer_roms_pass` (13/13, ne doit pas régresser).
4. Relancer `03-modify_timing.gb` de Blargg — sortie brute avant/après. Rappel : le résidu peut rester stable même si le portage est correct, comme observé sur les deux familles précédentes ; ne pas chercher à forcer un passage au vert à tout prix sans en comprendre la cause si ça ne bouge pas.
5. Ne committer qu'après mon feu vert explicite.

## Interdictions explicites
- Ne pas porter d'autre famille au-delà de `(HL)` lecture-modification-écriture (PUSH/POP/CALL/RET/interruptions restent pour un mandat futur).
- Ne pas toucher au PPU.
- Ne pas retoucher `timer.rs`, `Bus::read`/`Bus::write` sans mandat explicite séparé.
- Ne jamais modifier ce fichier (`AGENTS.md`) toi-même.
- Ne pas committer sans mon feu vert explicite.