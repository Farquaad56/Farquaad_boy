//! Table des cycles attendus par opcode — GÉNÉRÉ par `scripts/gen_cycle_table.py` à partir de
//! `data/Opcodes.json` (source : https://gbdev.io/gb-opcodes/Opcodes.json).
//! NE PAS éditer à la main : relancer le script. Chaque table est indexée par l'opcode
//! (position = valeur de l'opcode) et contient, en M-cycles (1 M-cycle = 4 T-cycles),
//! `(taken_m_cycles, not_taken_m_cycles)` : la 2e composante est `None` quand il n'y a pas
//! de branche (instruction non conditionnelle).

/// Cycles attendus (M-cycles) des opcodes sans préfixe, indexés par l'opcode.
#[allow(dead_code)] // consommé uniquement par le test exhaustif (étape 1) ; la table sert de référence pour les étapes suivantes
pub const EXPECTED_UNPREFIXED: [(u8, Option<u8>); 256] = [
    (1, None), // $00 NOP
    (3, None), // $01 LD BC,n16
    (2, None), // $02 LD BC,A
    (2, None), // $03 INC BC
    (1, None), // $04 INC B
    (1, None), // $05 DEC B
    (2, None), // $06 LD B,n8
    (1, None), // $07 RLCA
    (5, None), // $08 LD a16,SP
    (2, None), // $09 ADD HL,BC
    (2, None), // $0A LD A,BC
    (2, None), // $0B DEC BC
    (1, None), // $0C INC C
    (1, None), // $0D DEC C
    (2, None), // $0E LD C,n8
    (1, None), // $0F RRCA
    (1, None), // $10 STOP n8
    (3, None), // $11 LD DE,n16
    (2, None), // $12 LD DE,A
    (2, None), // $13 INC DE
    (1, None), // $14 INC D
    (1, None), // $15 DEC D
    (2, None), // $16 LD D,n8
    (1, None), // $17 RLA
    (3, None), // $18 JR e8
    (2, None), // $19 ADD HL,DE
    (2, None), // $1A LD A,DE
    (2, None), // $1B DEC DE
    (1, None), // $1C INC E
    (1, None), // $1D DEC E
    (2, None), // $1E LD E,n8
    (1, None), // $1F RRA
    (3, Some(2)), // $20 JR NZ,e8
    (3, None), // $21 LD HL,n16
    (2, None), // $22 LD HL,A
    (2, None), // $23 INC HL
    (1, None), // $24 INC H
    (1, None), // $25 DEC H
    (2, None), // $26 LD H,n8
    (1, None), // $27 DAA
    (3, Some(2)), // $28 JR Z,e8
    (2, None), // $29 ADD HL,HL
    (2, None), // $2A LD A,HL
    (2, None), // $2B DEC HL
    (1, None), // $2C INC L
    (1, None), // $2D DEC L
    (2, None), // $2E LD L,n8
    (1, None), // $2F CPL
    (3, Some(2)), // $30 JR NC,e8
    (3, None), // $31 LD SP,n16
    (2, None), // $32 LD HL,A
    (2, None), // $33 INC SP
    (3, None), // $34 INC HL
    (3, None), // $35 DEC HL
    (3, None), // $36 LD HL,n8
    (1, None), // $37 SCF
    (3, Some(2)), // $38 JR C,e8
    (2, None), // $39 ADD HL,SP
    (2, None), // $3A LD A,HL
    (2, None), // $3B DEC SP
    (1, None), // $3C INC A
    (1, None), // $3D DEC A
    (2, None), // $3E LD A,n8
    (1, None), // $3F CCF
    (1, None), // $40 LD B,B
    (1, None), // $41 LD B,C
    (1, None), // $42 LD B,D
    (1, None), // $43 LD B,E
    (1, None), // $44 LD B,H
    (1, None), // $45 LD B,L
    (2, None), // $46 LD B,HL
    (1, None), // $47 LD B,A
    (1, None), // $48 LD C,B
    (1, None), // $49 LD C,C
    (1, None), // $4A LD C,D
    (1, None), // $4B LD C,E
    (1, None), // $4C LD C,H
    (1, None), // $4D LD C,L
    (2, None), // $4E LD C,HL
    (1, None), // $4F LD C,A
    (1, None), // $50 LD D,B
    (1, None), // $51 LD D,C
    (1, None), // $52 LD D,D
    (1, None), // $53 LD D,E
    (1, None), // $54 LD D,H
    (1, None), // $55 LD D,L
    (2, None), // $56 LD D,HL
    (1, None), // $57 LD D,A
    (1, None), // $58 LD E,B
    (1, None), // $59 LD E,C
    (1, None), // $5A LD E,D
    (1, None), // $5B LD E,E
    (1, None), // $5C LD E,H
    (1, None), // $5D LD E,L
    (2, None), // $5E LD E,HL
    (1, None), // $5F LD E,A
    (1, None), // $60 LD H,B
    (1, None), // $61 LD H,C
    (1, None), // $62 LD H,D
    (1, None), // $63 LD H,E
    (1, None), // $64 LD H,H
    (1, None), // $65 LD H,L
    (2, None), // $66 LD H,HL
    (1, None), // $67 LD H,A
    (1, None), // $68 LD L,B
    (1, None), // $69 LD L,C
    (1, None), // $6A LD L,D
    (1, None), // $6B LD L,E
    (1, None), // $6C LD L,H
    (1, None), // $6D LD L,L
    (2, None), // $6E LD L,HL
    (1, None), // $6F LD L,A
    (2, None), // $70 LD HL,B
    (2, None), // $71 LD HL,C
    (2, None), // $72 LD HL,D
    (2, None), // $73 LD HL,E
    (2, None), // $74 LD HL,H
    (2, None), // $75 LD HL,L
    (1, None), // $76 HALT
    (2, None), // $77 LD HL,A
    (1, None), // $78 LD A,B
    (1, None), // $79 LD A,C
    (1, None), // $7A LD A,D
    (1, None), // $7B LD A,E
    (1, None), // $7C LD A,H
    (1, None), // $7D LD A,L
    (2, None), // $7E LD A,HL
    (1, None), // $7F LD A,A
    (1, None), // $80 ADD A,B
    (1, None), // $81 ADD A,C
    (1, None), // $82 ADD A,D
    (1, None), // $83 ADD A,E
    (1, None), // $84 ADD A,H
    (1, None), // $85 ADD A,L
    (2, None), // $86 ADD A,HL
    (1, None), // $87 ADD A,A
    (1, None), // $88 ADC A,B
    (1, None), // $89 ADC A,C
    (1, None), // $8A ADC A,D
    (1, None), // $8B ADC A,E
    (1, None), // $8C ADC A,H
    (1, None), // $8D ADC A,L
    (2, None), // $8E ADC A,HL
    (1, None), // $8F ADC A,A
    (1, None), // $90 SUB A,B
    (1, None), // $91 SUB A,C
    (1, None), // $92 SUB A,D
    (1, None), // $93 SUB A,E
    (1, None), // $94 SUB A,H
    (1, None), // $95 SUB A,L
    (2, None), // $96 SUB A,HL
    (1, None), // $97 SUB A,A
    (1, None), // $98 SBC A,B
    (1, None), // $99 SBC A,C
    (1, None), // $9A SBC A,D
    (1, None), // $9B SBC A,E
    (1, None), // $9C SBC A,H
    (1, None), // $9D SBC A,L
    (2, None), // $9E SBC A,HL
    (1, None), // $9F SBC A,A
    (1, None), // $A0 AND A,B
    (1, None), // $A1 AND A,C
    (1, None), // $A2 AND A,D
    (1, None), // $A3 AND A,E
    (1, None), // $A4 AND A,H
    (1, None), // $A5 AND A,L
    (2, None), // $A6 AND A,HL
    (1, None), // $A7 AND A,A
    (1, None), // $A8 XOR A,B
    (1, None), // $A9 XOR A,C
    (1, None), // $AA XOR A,D
    (1, None), // $AB XOR A,E
    (1, None), // $AC XOR A,H
    (1, None), // $AD XOR A,L
    (2, None), // $AE XOR A,HL
    (1, None), // $AF XOR A,A
    (1, None), // $B0 OR A,B
    (1, None), // $B1 OR A,C
    (1, None), // $B2 OR A,D
    (1, None), // $B3 OR A,E
    (1, None), // $B4 OR A,H
    (1, None), // $B5 OR A,L
    (2, None), // $B6 OR A,HL
    (1, None), // $B7 OR A,A
    (1, None), // $B8 CP A,B
    (1, None), // $B9 CP A,C
    (1, None), // $BA CP A,D
    (1, None), // $BB CP A,E
    (1, None), // $BC CP A,H
    (1, None), // $BD CP A,L
    (2, None), // $BE CP A,HL
    (1, None), // $BF CP A,A
    (5, Some(2)), // $C0 RET NZ
    (3, None), // $C1 POP BC
    (4, Some(3)), // $C2 JP NZ,a16
    (4, None), // $C3 JP a16
    (6, Some(3)), // $C4 CALL NZ,a16
    (4, None), // $C5 PUSH BC
    (2, None), // $C6 ADD A,n8
    (4, None), // $C7 RST $00
    (5, Some(2)), // $C8 RET Z
    (4, None), // $C9 RET
    (4, Some(3)), // $CA JP Z,a16
    (1, None), // $CB PREFIX
    (6, Some(3)), // $CC CALL Z,a16
    (6, None), // $CD CALL a16
    (2, None), // $CE ADC A,n8
    (4, None), // $CF RST $08
    (5, Some(2)), // $D0 RET NC
    (3, None), // $D1 POP DE
    (4, Some(3)), // $D2 JP NC,a16
    (1, None), // $D3 ILLEGAL_D3
    (6, Some(3)), // $D4 CALL NC,a16
    (4, None), // $D5 PUSH DE
    (2, None), // $D6 SUB A,n8
    (4, None), // $D7 RST $10
    (5, Some(2)), // $D8 RET C
    (4, None), // $D9 RETI
    (4, Some(3)), // $DA JP C,a16
    (1, None), // $DB ILLEGAL_DB
    (6, Some(3)), // $DC CALL C,a16
    (1, None), // $DD ILLEGAL_DD
    (2, None), // $DE SBC A,n8
    (4, None), // $DF RST $18
    (3, None), // $E0 LDH a8,A
    (3, None), // $E1 POP HL
    (2, None), // $E2 LDH C,A
    (1, None), // $E3 ILLEGAL_E3
    (1, None), // $E4 ILLEGAL_E4
    (4, None), // $E5 PUSH HL
    (2, None), // $E6 AND A,n8
    (4, None), // $E7 RST $20
    (4, None), // $E8 ADD SP,e8
    (1, None), // $E9 JP HL
    (4, None), // $EA LD a16,A
    (1, None), // $EB ILLEGAL_EB
    (1, None), // $EC ILLEGAL_EC
    (1, None), // $ED ILLEGAL_ED
    (2, None), // $EE XOR A,n8
    (4, None), // $EF RST $28
    (3, None), // $F0 LDH A,a8
    (3, None), // $F1 POP AF
    (2, None), // $F2 LDH A,C
    (1, None), // $F3 DI
    (1, None), // $F4 ILLEGAL_F4
    (4, None), // $F5 PUSH AF
    (2, None), // $F6 OR A,n8
    (4, None), // $F7 RST $30
    (3, None), // $F8 LD HL,SP,e8
    (2, None), // $F9 LD SP,HL
    (4, None), // $FA LD A,a16
    (1, None), // $FB EI
    (1, None), // $FC ILLEGAL_FC
    (1, None), // $FD ILLEGAL_FD
    (2, None), // $FE CP A,n8
    (4, None), // $FF RST $38
];

/// Cycles attendus (M-cycles) des opcodes préfixées CB ($CB xx), indexées par l'octet suivant $CB.
#[allow(dead_code)] // consommé uniquement par le test exhaustif (étape 1) ; la table sert de référence pour les étapes suivantes
pub const EXPECTED_CBPREFIXED: [(u8, Option<u8>); 256] = [
    (2, None), // $00 RLC B
    (2, None), // $01 RLC C
    (2, None), // $02 RLC D
    (2, None), // $03 RLC E
    (2, None), // $04 RLC H
    (2, None), // $05 RLC L
    (4, None), // $06 RLC HL
    (2, None), // $07 RLC A
    (2, None), // $08 RRC B
    (2, None), // $09 RRC C
    (2, None), // $0A RRC D
    (2, None), // $0B RRC E
    (2, None), // $0C RRC H
    (2, None), // $0D RRC L
    (4, None), // $0E RRC HL
    (2, None), // $0F RRC A
    (2, None), // $10 RL B
    (2, None), // $11 RL C
    (2, None), // $12 RL D
    (2, None), // $13 RL E
    (2, None), // $14 RL H
    (2, None), // $15 RL L
    (4, None), // $16 RL HL
    (2, None), // $17 RL A
    (2, None), // $18 RR B
    (2, None), // $19 RR C
    (2, None), // $1A RR D
    (2, None), // $1B RR E
    (2, None), // $1C RR H
    (2, None), // $1D RR L
    (4, None), // $1E RR HL
    (2, None), // $1F RR A
    (2, None), // $20 SLA B
    (2, None), // $21 SLA C
    (2, None), // $22 SLA D
    (2, None), // $23 SLA E
    (2, None), // $24 SLA H
    (2, None), // $25 SLA L
    (4, None), // $26 SLA HL
    (2, None), // $27 SLA A
    (2, None), // $28 SRA B
    (2, None), // $29 SRA C
    (2, None), // $2A SRA D
    (2, None), // $2B SRA E
    (2, None), // $2C SRA H
    (2, None), // $2D SRA L
    (4, None), // $2E SRA HL
    (2, None), // $2F SRA A
    (2, None), // $30 SWAP B
    (2, None), // $31 SWAP C
    (2, None), // $32 SWAP D
    (2, None), // $33 SWAP E
    (2, None), // $34 SWAP H
    (2, None), // $35 SWAP L
    (4, None), // $36 SWAP HL
    (2, None), // $37 SWAP A
    (2, None), // $38 SRL B
    (2, None), // $39 SRL C
    (2, None), // $3A SRL D
    (2, None), // $3B SRL E
    (2, None), // $3C SRL H
    (2, None), // $3D SRL L
    (4, None), // $3E SRL HL
    (2, None), // $3F SRL A
    (2, None), // $40 BIT 0,B
    (2, None), // $41 BIT 0,C
    (2, None), // $42 BIT 0,D
    (2, None), // $43 BIT 0,E
    (2, None), // $44 BIT 0,H
    (2, None), // $45 BIT 0,L
    (3, None), // $46 BIT 0,HL
    (2, None), // $47 BIT 0,A
    (2, None), // $48 BIT 1,B
    (2, None), // $49 BIT 1,C
    (2, None), // $4A BIT 1,D
    (2, None), // $4B BIT 1,E
    (2, None), // $4C BIT 1,H
    (2, None), // $4D BIT 1,L
    (3, None), // $4E BIT 1,HL
    (2, None), // $4F BIT 1,A
    (2, None), // $50 BIT 2,B
    (2, None), // $51 BIT 2,C
    (2, None), // $52 BIT 2,D
    (2, None), // $53 BIT 2,E
    (2, None), // $54 BIT 2,H
    (2, None), // $55 BIT 2,L
    (3, None), // $56 BIT 2,HL
    (2, None), // $57 BIT 2,A
    (2, None), // $58 BIT 3,B
    (2, None), // $59 BIT 3,C
    (2, None), // $5A BIT 3,D
    (2, None), // $5B BIT 3,E
    (2, None), // $5C BIT 3,H
    (2, None), // $5D BIT 3,L
    (3, None), // $5E BIT 3,HL
    (2, None), // $5F BIT 3,A
    (2, None), // $60 BIT 4,B
    (2, None), // $61 BIT 4,C
    (2, None), // $62 BIT 4,D
    (2, None), // $63 BIT 4,E
    (2, None), // $64 BIT 4,H
    (2, None), // $65 BIT 4,L
    (3, None), // $66 BIT 4,HL
    (2, None), // $67 BIT 4,A
    (2, None), // $68 BIT 5,B
    (2, None), // $69 BIT 5,C
    (2, None), // $6A BIT 5,D
    (2, None), // $6B BIT 5,E
    (2, None), // $6C BIT 5,H
    (2, None), // $6D BIT 5,L
    (3, None), // $6E BIT 5,HL
    (2, None), // $6F BIT 5,A
    (2, None), // $70 BIT 6,B
    (2, None), // $71 BIT 6,C
    (2, None), // $72 BIT 6,D
    (2, None), // $73 BIT 6,E
    (2, None), // $74 BIT 6,H
    (2, None), // $75 BIT 6,L
    (3, None), // $76 BIT 6,HL
    (2, None), // $77 BIT 6,A
    (2, None), // $78 BIT 7,B
    (2, None), // $79 BIT 7,C
    (2, None), // $7A BIT 7,D
    (2, None), // $7B BIT 7,E
    (2, None), // $7C BIT 7,H
    (2, None), // $7D BIT 7,L
    (3, None), // $7E BIT 7,HL
    (2, None), // $7F BIT 7,A
    (2, None), // $80 RES 0,B
    (2, None), // $81 RES 0,C
    (2, None), // $82 RES 0,D
    (2, None), // $83 RES 0,E
    (2, None), // $84 RES 0,H
    (2, None), // $85 RES 0,L
    (4, None), // $86 RES 0,HL
    (2, None), // $87 RES 0,A
    (2, None), // $88 RES 1,B
    (2, None), // $89 RES 1,C
    (2, None), // $8A RES 1,D
    (2, None), // $8B RES 1,E
    (2, None), // $8C RES 1,H
    (2, None), // $8D RES 1,L
    (4, None), // $8E RES 1,HL
    (2, None), // $8F RES 1,A
    (2, None), // $90 RES 2,B
    (2, None), // $91 RES 2,C
    (2, None), // $92 RES 2,D
    (2, None), // $93 RES 2,E
    (2, None), // $94 RES 2,H
    (2, None), // $95 RES 2,L
    (4, None), // $96 RES 2,HL
    (2, None), // $97 RES 2,A
    (2, None), // $98 RES 3,B
    (2, None), // $99 RES 3,C
    (2, None), // $9A RES 3,D
    (2, None), // $9B RES 3,E
    (2, None), // $9C RES 3,H
    (2, None), // $9D RES 3,L
    (4, None), // $9E RES 3,HL
    (2, None), // $9F RES 3,A
    (2, None), // $A0 RES 4,B
    (2, None), // $A1 RES 4,C
    (2, None), // $A2 RES 4,D
    (2, None), // $A3 RES 4,E
    (2, None), // $A4 RES 4,H
    (2, None), // $A5 RES 4,L
    (4, None), // $A6 RES 4,HL
    (2, None), // $A7 RES 4,A
    (2, None), // $A8 RES 5,B
    (2, None), // $A9 RES 5,C
    (2, None), // $AA RES 5,D
    (2, None), // $AB RES 5,E
    (2, None), // $AC RES 5,H
    (2, None), // $AD RES 5,L
    (4, None), // $AE RES 5,HL
    (2, None), // $AF RES 5,A
    (2, None), // $B0 RES 6,B
    (2, None), // $B1 RES 6,C
    (2, None), // $B2 RES 6,D
    (2, None), // $B3 RES 6,E
    (2, None), // $B4 RES 6,H
    (2, None), // $B5 RES 6,L
    (4, None), // $B6 RES 6,HL
    (2, None), // $B7 RES 6,A
    (2, None), // $B8 RES 7,B
    (2, None), // $B9 RES 7,C
    (2, None), // $BA RES 7,D
    (2, None), // $BB RES 7,E
    (2, None), // $BC RES 7,H
    (2, None), // $BD RES 7,L
    (4, None), // $BE RES 7,HL
    (2, None), // $BF RES 7,A
    (2, None), // $C0 SET 0,B
    (2, None), // $C1 SET 0,C
    (2, None), // $C2 SET 0,D
    (2, None), // $C3 SET 0,E
    (2, None), // $C4 SET 0,H
    (2, None), // $C5 SET 0,L
    (4, None), // $C6 SET 0,HL
    (2, None), // $C7 SET 0,A
    (2, None), // $C8 SET 1,B
    (2, None), // $C9 SET 1,C
    (2, None), // $CA SET 1,D
    (2, None), // $CB SET 1,E
    (2, None), // $CC SET 1,H
    (2, None), // $CD SET 1,L
    (4, None), // $CE SET 1,HL
    (2, None), // $CF SET 1,A
    (2, None), // $D0 SET 2,B
    (2, None), // $D1 SET 2,C
    (2, None), // $D2 SET 2,D
    (2, None), // $D3 SET 2,E
    (2, None), // $D4 SET 2,H
    (2, None), // $D5 SET 2,L
    (4, None), // $D6 SET 2,HL
    (2, None), // $D7 SET 2,A
    (2, None), // $D8 SET 3,B
    (2, None), // $D9 SET 3,C
    (2, None), // $DA SET 3,D
    (2, None), // $DB SET 3,E
    (2, None), // $DC SET 3,H
    (2, None), // $DD SET 3,L
    (4, None), // $DE SET 3,HL
    (2, None), // $DF SET 3,A
    (2, None), // $E0 SET 4,B
    (2, None), // $E1 SET 4,C
    (2, None), // $E2 SET 4,D
    (2, None), // $E3 SET 4,E
    (2, None), // $E4 SET 4,H
    (2, None), // $E5 SET 4,L
    (4, None), // $E6 SET 4,HL
    (2, None), // $E7 SET 4,A
    (2, None), // $E8 SET 5,B
    (2, None), // $E9 SET 5,C
    (2, None), // $EA SET 5,D
    (2, None), // $EB SET 5,E
    (2, None), // $EC SET 5,H
    (2, None), // $ED SET 5,L
    (4, None), // $EE SET 5,HL
    (2, None), // $EF SET 5,A
    (2, None), // $F0 SET 6,B
    (2, None), // $F1 SET 6,C
    (2, None), // $F2 SET 6,D
    (2, None), // $F3 SET 6,E
    (2, None), // $F4 SET 6,H
    (2, None), // $F5 SET 6,L
    (4, None), // $F6 SET 6,HL
    (2, None), // $F7 SET 6,A
    (2, None), // $F8 SET 7,B
    (2, None), // $F9 SET 7,C
    (2, None), // $FA SET 7,D
    (2, None), // $FB SET 7,E
    (2, None), // $FC SET 7,H
    (2, None), // $FD SET 7,L
    (4, None), // $FE SET 7,HL
    (2, None), // $FF SET 7,A
];
/// Cycles attendus pour un opcode non préfixé et une branche donnée (`taken` = la condition est vraie).
#[allow(dead_code)] // consommé uniquement par le test exhaustif (étape 1) ; sert de référence pour les étapes suivantes
pub fn expected_unprefixed_m_cycles(opcode: u8, taken: bool) -> u8 {
    let (taken_c, not_taken_c) = EXPECTED_UNPREFIXED[opcode as usize];
    match not_taken_c {
        None => taken_c,
        Some(nt) if !taken => nt,
        _ => taken_c,
    }
}

/// Cycles attendus pour un opcode préfixé CB ($CB xx).
#[allow(dead_code)] // consommé uniquement par le test exhaustif (étape 1) ; sert de référence pour les étapes suivantes
pub fn expected_cb_m_cycles(cb: u8) -> u8 {
    EXPECTED_CBPREFIXED[cb as usize].0
}

