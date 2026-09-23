# Consigne de travail (en vigueur)

**Exécuter l'étape 2 du plan de refactoring CPU/Bus : porter les familles d'instructions vers des programmes micro-op, famille par famille, une famille à la fois.**

L'étape 1 (wrapper `Bus` + `Cpu::tick(&mut self, bus)` + enum `MicroOp` + latches W/Z sur le CPU + table de cycles générée depuis `data/Opcodes.json` + test exhaustif) est terminée et commitée (`d2701a1`).

Le portage de la famille `(HL) en écriture` (`$70-75, $77, $36, $22, $32`) est terminé et commité (`12c107b`), micro-op vérifié par test d'entrelacement dédié.

**Mandat actuel, et rien de plus :** rebrancher `Emulator::step()` pour router uniquement les opcodes de la famille `(HL) écriture` déjà portée vers `cpu.tick()`, tous les autres opcodes restant sur le dispatch atomique legacy. Objectif : obtenir une confirmation Blargg (`02-write_timing.gb`) de bout en bout sur cette seule famille avant d'investir dans le portage des familles suivantes.

**Interdictions explicites tant que ce point n'est pas validé et confirmé par moi :**
- Ne pas porter d'autre famille d'instructions.
- Ne pas toucher au PPU.
- Ne pas faire le rebranchement complet d'`Emulator::step()` (Étape 3 entière) — seulement les opcodes déjà portés.
- Ne jamais modifier ce fichier (`AGENTS.md`) toi-même, même pour refléter une extension de scope qui semble découler naturellement de la conversation. Toute extension de mandat doit être écrite par moi, dans mon propre commit.
- Ne pas committer sans mon feu vert explicite sur le résultat de chaque étape.