# Test ROMs

This directory holds the test ROMs used by the automated acceptance tests in `src/`.

## Mooneye — Game Boy Timer suite (`Mooneye-gb/acceptance/timer/`)

The 13 Game Boy Timer test ROMs under `Mooneye-gb/acceptance/timer/*.gb` are from the
[Mooneye GB test suite](https://github.com/Gekkio/mooneye-test-suite) by Joonas Javanainen (Gekkio),
released under the [MIT License](https://opensource.org/licenses/MIT). See `Mooneye-gb/LICENSE`.

They validate the Timer behaviours covered by this project: TIMA increment, the `$00` reload window,
and DIV/TAC/TMA writes. The permanent non-regression guard is `mooneye_timer_roms_pass` in `src/emulator.rs`,
which runs all 13 ROMs headless and asserts each produces exactly the register values its source expects.
