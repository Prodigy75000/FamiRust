# Permission to distribute the FamiRust Demo Cart

**Work:** FamiRust Demo Cart (`famirust-demo.nes`), a Nintendo Entertainment
System / Famicom cartridge image, together with its complete source code.

**Author and sole rights holder:** Prodigy75000 (`https://github.com/Prodigy75000`).

**Source of record:** `https://github.com/Prodigy75000/FamiRust`, directory
`roms/famirust-demo/`.

---

## Statement

I am the sole author of the FamiRust Demo Cart. I wrote every line of its 6502
assembly source, drew every graphic in it, and composed the music in it. It was
assembled by `nes-asm`, a 6502 assembler that I also wrote and that lives in the
same repository, so no third-party build tool contributed any content to the
resulting file.

The cartridge contains no code, artwork, audio, text, font, logo, trademark or
data taken from any commercial video game, from any other emulator project, or
from any other third party. It is not derived from, traced from, or measured
against any existing cartridge. Its character set, its graphics and its music
were made for this cartridge and exist nowhere else.

No permission from any other party is required to copy, distribute, modify or
bundle this work, because no other party holds any rights in it.

## Grant

I dedicate the FamiRust Demo Cart, both the assembled `.nes` image and its
complete source, to the public domain under **CC0 1.0 Universal**. The full text
of that dedication is in the `LICENSE` file beside this one.

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

## Verification

Anyone can confirm that the distributed binary is exactly what the published
source produces. From a clone of the repository:

```sh
cargo test -p nes-asm --test demo_rom_reproduces
```

That test reassembles `roms/famirust-demo/src/main.s` and compares the result
byte for byte with the committed `famirust-demo.nes`. The published SHA-256 of
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

Name: _______________________________________

Signature: __________________________________

Date: _______________________________________

GitHub: https://github.com/Prodigy75000
