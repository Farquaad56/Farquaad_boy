# Consigne de travail (en vigueur)

**Exécuter l'Étape 2, point 5 du plan de refactoring CPU/Bus : porter les formes simples restantes en micro-op — dernière famille avant l'Étape 3.**

## Acquis (terminé, committé, ne pas retoucher sans mandat explicite)
- Étape 1 : wrapper `Bus` + `Cpu::tick` + `MicroOp` + latches W/Z + table de cycles depuis `data/Opcodes.json` (`d2701a1`).
- Étape 2 point 1 — `(HL)` écriture (`12c107b`).
- Étape 2 point 2 — `(HL)` lecture (`e6d662c`).
- Étape 2 point 3 — `(HL)` lecture-modification-écriture, 26 opcodes (`46f0f8f`).
- Étape 2 point 4 — PUSH/POP/CALL/RET/RST + dispatch d'interruption (`1f392d9`) ; `pop_timing.gb` (Mooneye) confirmé corrigé par le portage (A/C/E : `00/00/00`→`01/01/01`, conforme aux attentes du ROM).
- Chantier Timer : détection de front + fenêtre de rechargement, 13 ROMs Mooneye `acceptance/timer/` en garde-fou permanent (`ca1aa54`, `0393a24`, `9ec315f`).

## Mandat actuel
Porter tout ce qui reste, hors `(HL)`/`CB (HL)` déjà couverts :
- **`LD r,r'`** (registre-registre, `$40-7F` sauf `HALT`/`(HL)` déjà faits) — 1 M-cycle.
- **`LD r,n8`** (`$06,0E,16,1E,26,2E,3E`) — 2 M-cycles.
- **ALU `A,r` / `A,n8`** (`ADD/ADC/SUB/SBC/AND/XOR/OR/CP`, formes registre et immédiat, hors `(HL)` déjà fait) — 1 ou 2 M-cycles.
- **`INC/DEC r8`** (registre, hors `(HL)`) — 1 M-cycle.
- **`INC/DEC r16`** (`$03,0B,13,1B,23,2B,33,3B`) — 2 M-cycles.
- **`LD r16,n16`** (`$01,11,21,31`) — 3 M-cycles.
- **`LD (a16),SP`** (`$08`) — 5 M-cycles.
- **`LD SP,HL`** (`$F9`) — 2 M-cycles.
- **`ADD SP,e8`** (`$E8`) / **`LD HL,SP+e8`** (`$F8`) — 4/3 M-cycles.
- **`JR e8`** (`$18`) et **`JP a16`** (`$C3`) inconditionnels, si pas déjà portés — vérifie d'abord s'ils le sont.
- **`JP HL`** (`$E9`) — 1 M-cycle (pas d'accès mémoire, juste `PC←HL`).
- **`CB` registre** : rotations/décalages/`BIT`/`RES`/`SET` sur registre (pas `(HL)`, déjà fait) — 2 M-cycles.
- **`EI`/`DI`/`NOP`/`CCF`/`SCF`/`CPL`/`DAA`** et autres 1-M-cycle sans accès mémoire, si pas déjà portés.

Avant tout code : dresse d'abord la liste exacte des opcodes qui *restent* réellement non portés (grep `ported_steps()` pour voir ce qui manque, plutôt que de re-porter par erreur ce qui l'est déjà). Ensuite cite le tableau M-cycle GBCTR pour chaque forme non couverte, croisé avec `Opcodes.json`.

Point d'attention : `ADD SP,e8`/`LD HL,SP+e8` ont un calcul de flags particulier (H/C calculés sur l'addition 8-bit basse, pas sur l'addition 16-bit complète) — vérifie ce détail contre le comportement legacy actuel avant de porter, ne le suppose pas.

Méthode imposée, identique aux points précédents :
1. Micro-programme + test d'entrelacement dédié par opcode (assertions individuelles).
2. A/B `cargo test` (ensembles d'échecs).
3. `cpu_instrs` (11/11) + `mooneye_timer_roms_pass` (13/13).
4. Relancer `instr_timing.gb` de Blargg (englobe désormais toutes les familles portées) — sortie brute avant/après.
5. Sonder les ROMs Mooneye `acceptance/instr/` restantes non encore testées (`push_timing`/`pop_timing`/`call_timing`/etc. déjà faites au point 4 — chercher les autres : `add_sp_e_timing`, `ld_hl_sp_e_timing`, `jp_timing`, `jp_cc_timing`, `daa`, etc.) — rapporter lesquelles passent déjà.
6. Ne committer qu'après mon feu vert explicite.

## Interdictions explicites
- Ne pas toucher au PPU, ni au rendu des sprites.
- Ne pas retoucher `timer.rs`, `Bus::read`/`Bus::write`, ni la boot ROM sans mandat explicite séparé.
- Ne pas commencer l'Étape 3 (rebranchement complet, suppression du mode legacy) — ce sera un mandat séparé une fois ce point terminé.
- Ne jamais modifier ce fichier (`AGENTS.md`) toi-même.
- Ne pas committer sans mon feu vert explicite.