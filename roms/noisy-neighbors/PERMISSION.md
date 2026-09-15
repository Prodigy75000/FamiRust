# Permission to distribute NOISY NEIGHBORS

**Work:** NOISY NEIGHBORS (`noisy-neighbors.nes`), a Nintendo Entertainment
System / Famicom cartridge image, together with its complete source code.

**Author and sole rights holder:** Armand Bireaud, publishing as Prodigy75000
(`https://github.com/Prodigy75000`).

**Source of record:** `https://github.com/Prodigy75000/FamiRust`, directory
`roms/noisy-neighbors/`.

---

## Statement

I am the sole author of NOISY NEIGHBORS. I wrote every line of its 6502
assembly source, drew every graphic in it, designed every room in it, and
composed the music and the sound effects in it. The tune is an original
composition; it is not transcribed, arranged or adapted from any existing
piece. It was assembled by `nes-asm`, a 6502 assembler that I also wrote and
that lives in the same repository, so no third-party build tool contributed any
content to the resulting file.

The cartridge contains no code, artwork, audio, text, font, logo, trademark or
data taken from any commercial video game, from any other emulator project, or
from any other third party. It is not derived from, traced from, or measured
against any existing cartridge.

**Shared material, stated plainly.** This cartridge began as a fork of MONKEY
FARCE in the same repository, and it still shares a good deal with it: the
character set, most of the block and sprite graphics, the music, the sound
effect table, and the movement code underneath the game. MONKEY FARCE is also
my own work and is also dedicated to the public domain under CC0 1.0, as is the
FamiRust Demo Cart whose font both of them use. Nothing here came from anyone
else, and nothing here is encumbered by anyone else's rights. This paragraph
exists because the sentence above about drawing every graphic would otherwise
imply that all of it was drawn fresh for this cartridge, and some of it was
drawn for the one next door instead. Both are mine.

No permission from any other party is required to copy, distribute, modify or
bundle this work, because no other party holds any rights in it.

## Grant

I dedicate NOISY NEIGHBORS, both the assembled `.nes` image and its complete
source, to the public domain under **CC0 1.0 Universal**. The full text of that
dedication is in the `LICENSE` file beside this one.

To the extent that anything further is useful, I additionally and expressly
permit any person or organisation, without charge, condition, notice or further
permission, to:

- download, copy, host, redistribute and mirror the cartridge image;
- include the cartridge image inside another product, including a commercial
  product and including an application submitted to or distributed through the
  Apple App Store, the Google Play Store, or any other channel;
- use the cartridge image to test, demonstrate, review, benchmark or certify
  emulator software;
- modify the cartridge or its source and distribute the result.

This permission is irrevocable and is not limited by territory, medium, format
or duration.

## Relationship to the surrounding repository

The FamiRust emulator core that this cartridge lives beside is licensed
GPL-3.0-or-later. The cartridge is not. A repository is not a single work, and
these are two separate works distributed together: the core is a program, and
the cartridge is a data file with its own source, built by a separate tool and
executed by the emulated machine rather than linked into anything.

The assembler that produces the cartridge, `nes-asm`, is itself GPL-3.0, and
that has no effect on the cartridge. A compiler's licence does not attach to
what it compiles, and this assembler in particular injects nothing of its own
into its output: every byte of `noisy-neighbors.nes` other than the sixteen-byte
iNES container header and the unused padding comes from the cartridge source
listed above.

CC0 imposes no conditions on anyone, so nothing about it can conflict with the
GPL in either direction. In any case I hold the copyright in both works and am
free to license each as I choose.

## Verification

Anyone can confirm that the distributed binary is exactly what the published
source produces. From a clone of the repository:

```sh
cargo test -p nes-asm --test noisy_neighbors_reproduces
```

That test reassembles `roms/noisy-neighbors/src/main.s` and compares the result
byte for byte with the committed `noisy-neighbors.nes`. The published SHA-256 of
the current build is recorded in `SHA256SUMS` in this directory.

## Disclaimer of warranty

The work is provided as-is, without warranty of any kind, as set out in section
4 of the CC0 text in `LICENSE`.

## Note on trademarks

"Nintendo Entertainment System" and "Famicom" are trademarks of Nintendo. This
cartridge and the FamiRust project are independent works and are not affiliated
with, endorsed by, or sponsored by Nintendo. Nothing in this document claims any
right in anyone else's trademark, and CC0 does not purport to license trademark
rights (see section 4(a) of `LICENSE`).

---

Signed,

**Armand Bireaud**

Date: 15 September 2026

GitHub: https://github.com/Prodigy75000

If a handwritten signature is required, print this page and sign below.

Signature: __________________________________
