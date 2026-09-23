#!/usr/bin/env python3
"""Génère `src/cpu/expected_cycles.rs` à partir de `data/Opcodes.json`.

Source : https://gbdev.io/gb-opcodes/Opcodes.json (référence canonique des cycles Game Boy).

Chaque table est indexée par l'opcode (position = valeur de l'opcode) et contient, en M-cycles
(1 M-cycle = 4 T-cycles) :
  - la 1re composante : les cycles quand la branche est PRISE (ou le cycle count unique pour une
    instruction non conditionnelle),
  - la 2e composante (`Option`) : les cycles quand la branche n'est PAS prise ; `None` = pas de
    branche.

Usage :
    python scripts/gen_cycle_table.py            # régénère src/cpu/expected_cycles.rs
    python scripts/gen_cycle_table.py --check    # échoue si le fichier généré a dérivé
"""

import json
import os
import sys

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.dirname(HERE)
SRC_JSON = os.path.join(ROOT, "data", "Opcodes.json")
OUT_RS = os.path.join(ROOT, "src", "cpu", "expected_cycles.rs")


def m_cycles(t_cycles: int) -> int:
    assert t_cycles % 4 == 0, f"T-cycle count {t_cycles} is not a multiple of 4"
    return t_cycles // 4


def short_mnemonic(entry: dict) -> str:
    """Ex. 'JR NZ,e8' / 'LD BC,n16' — lisible pour les commentaires de la table."""
    ops = ",".join(o["name"] for o in entry.get("operands", []))
    return f"{entry['mnemonic']} {ops}".strip()


def build_table(section: dict) -> list[tuple[int, int | None]]:
    """Indexée par opcode : (taken_m_cycles, not_taken_m_cycles | None)."""
    table = []
    for i in range(256):
        key = f"0x{i:02X}"
        entry = section[key]
        cycles = entry["cycles"]
        taken = m_cycles(cycles[0])
        not_taken = m_cycles(cycles[1]) if len(cycles) >= 2 else None
        table.append((taken, not_taken))
    return table


def main() -> int:
    with open(SRC_JSON, encoding="utf-8") as f:
        data = json.load(f)

    unprefixed_raw = data["unprefixed"]
    cbprefixed_raw = data["cbprefixed"]

    unprefixed = build_table(unprefixed_raw)
    cbprefixed = build_table(cbprefixed_raw)

    def render(name: str, doc: str, raw: dict, table: list) -> str:
        lines = [f"/// {doc}"]
        lines.append("#[allow(dead_code)] // consommé uniquement par le test exhaustif (étape 1) ; la table sert de référence pour les étapes suivantes")
        lines.append(f"pub const {name}: [(u8, Option<u8>); 256] = [")
        for i in range(256):
            taken, not_taken = table[i]
            nt = "None" if not_taken is None else f"Some({not_taken})"
            comment = short_mnemonic(raw[f"0x{i:02X}"])
            lines.append(f"    ({taken}, {nt}), // ${i:02X} {comment}")
        lines.append("];")
        return "\n".join(lines)

    header = (
        "//! Table des cycles attendus par opcode — GÉNÉRÉ par `scripts/gen_cycle_table.py` à partir de\n"
        "//! `data/Opcodes.json` (source : https://gbdev.io/gb-opcodes/Opcodes.json).\n"
        "//! NE PAS éditer à la main : relancer le script. Chaque table est indexée par l'opcode\n"
        "//! (position = valeur de l'opcode) et contient, en M-cycles (1 M-cycle = 4 T-cycles),\n"
        "//! `(taken_m_cycles, not_taken_m_cycles)` : la 2e composante est `None` quand il n'y a pas\n"
        "//! de branche (instruction non conditionnelle).\n"
    )

    unprefixed_rs = render("EXPECTED_UNPREFIXED",
                           "Cycles attendus (M-cycles) des opcodes sans préfixe, indexés par l'opcode.",
                           unprefixed_raw, unprefixed)
    cbprefixed_rs = render("EXPECTED_CBPREFIXED",
                           "Cycles attendus (M-cycles) des opcodes préfixées CB ($CB xx), indexées par l'octet suivant $CB.",
                           cbprefixed_raw, cbprefixed)

    helpers = (
        "\n"
        "/// Cycles attendus pour un opcode non préfixé et une branche donnée (`taken` = la condition est vraie).\n"
        "#[allow(dead_code)] // consommé uniquement par le test exhaustif (étape 1) ; sert de référence pour les étapes suivantes\n"
        "pub fn expected_unprefixed_m_cycles(opcode: u8, taken: bool) -> u8 {\n"
        "    let (taken_c, not_taken_c) = EXPECTED_UNPREFIXED[opcode as usize];\n"
        "    match not_taken_c {\n"
        "        None => taken_c,\n"
        "        Some(nt) if !taken => nt,\n"
        "        _ => taken_c,\n"
        "    }\n"
        "}\n"
        "\n"
        "/// Cycles attendus pour un opcode préfixé CB ($CB xx).\n"
        "#[allow(dead_code)] // consommé uniquement par le test exhaustif (étape 1) ; sert de référence pour les étapes suivantes\n"
        "pub fn expected_cb_m_cycles(cb: u8) -> u8 {\n"
        "    EXPECTED_CBPREFIXED[cb as usize].0\n"
        "}\n"
    )

    content = header + "\n" + unprefixed_rs + "\n\n" + cbprefixed_rs + helpers + "\n"

    if "--check" in sys.argv:
        with open(OUT_RS, encoding="utf-8") as f:
            existing = f.read()
        if existing != content:
            print("ERROR: src/cpu/expected_cycles.rs has drifted from data/Opcodes.json — re-run the generator.")
            return 1
        print("OK: expected_cycles.rs matches data/Opcodes.json.")
        return 0

    with open(OUT_RS, "w", encoding="utf-8") as f:
        f.write(content)

    n_cond = sum(1 for _, nt in unprefixed if nt is not None)
    print(f"Wrote {os.path.relpath(OUT_RS, ROOT)} : 256 opcodes non préfixés ({n_cond} conditionnelles), "
          f"256 opcodes CB. Valeurs en M-cycles (taken / not-taken).")
    return 0


if __name__ == "__main__":
    sys.exit(main())
