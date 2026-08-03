# Core Debug Harness — app ↔ core contract

The Android debug frontend builds the on-device "Core Debug" overlay (frame-step,
counters, layer/OAM/tile viewers, share). This core owns the PPU decode; the app
owns display, PNG encoding, gallery and share. So the split is: **the core hands
over decoded pixels + structured data through custom exports; the app pulls,
encodes and displays.** No PNG encoder or file I/O in the core, and the app
reuses its existing framebuffer-pull (`nativeRenderInto`) and save/share plumbing.

Same contract covers SuperRust later — only the layer set widens (BG1–4 + OBJ).

## Transport

The app's native bridge already `dlopen`s the core `.so`, so it calls these
extra C exports directly (alongside the standard `retro_*`). All image buffers
are **app-allocated**; the core copies into them. Pixels are `XRGB8888`
(`0xAARRGGBB`, alpha ignored) — the same format as the main framebuffer.

Enable capture only while the overlay is open (`nes_dbg_enable(1)`); it is off by
default and costs nothing in normal play. Call the getters **after** a frame
(`retro_run` / frame-step) so they reflect the frame just rendered.

## Exports

| Export | Signature | Buffer size | Notes |
|---|---|---|---|
| `nes_dbg_enable` | `(on: u32)` | — | 1 = capture per-layer buffers each frame, 0 = off |
| `nes_dbg_set_layer_mask` | `(mask: u32)` | — | Composite toggle: bit0=BG, bit1=OBJ. `0b11`=normal. Live. |
| `nes_dbg_layer` | `(which: u32, out: *mut u32)` | 256×240 u32 | `which` 0=BG-only (backdrop fill), 1=OBJ-only (**magenta `0xFFFF00FF` = transparent**) |
| `nes_dbg_tiles` | `(out: *mut u32)` | 128×256 u32 | Both pattern tables, 16 tiles wide × 32 tall, coloured with BG palette 0 |
| `nes_dbg_palette` | `(out: *mut u32)` | 32 u32 | `[0..16]` = BG palettes, `[16..32]` = sprite palettes |
| `nes_dbg_oam_json` | `(buf: *mut u8, cap: u32) -> u32` | ≥ 8 KiB | Writes UTF-8 JSON, returns byte length (0 if no core) |

Frame dims are fixed for NES: main/layer = **256×240**, tiles = **128×256**
(`nes_core::{FRAME_W, FRAME_H, DBG_TILES_W, DBG_TILES_H}`).

### `nes_dbg_oam_json` shape

```json
[{"index":0,"x":0,"y":248,"tile":1,"palette":0,"priority":0,"flipH":0,"flipV":0,"size":"8x16"}, ...]
```
64 entries. `priority` 0 = in front of BG, 1 = behind. `size` follows PPUCTRL bit 5
("8x8" or "8x16"). Coordinates are the raw OAM bytes (sprite Y is top-1, as on hardware).

## App-side responsibilities (recap)

* **Frame-step + whole-frame counters** — fully app-side off the main framebuffer
  (`nativeRunFrames(1)` + distinct-colors / non-black-px / delta-vs-previous). No
  core dependency; this is the Contra-flash signal at a glance.
* **Layer / tiles / palette PNGs** — call the getter, encode + save to the debug
  gallery dir, share. (BG uses backdrop fill; OBJ uses magenta as the transparency key.)
* **OAM inspector** — parse `nes_dbg_oam_json` into the sprite list; overlay boxes
  on the frame using x/y/size if desired.
* **Layer toggles** — write `nes_dbg_set_layer_mask`; the visible frame updates on
  the next `retro_run`. Sprite-0 hit stays on the real pixels, so toggling never
  changes game logic.

## Notes / invariants

* Debug fields are **not serialized** and the default mask (`0b11`) renders
  identically to release — verified: save-state bytes and render output are
  unchanged with the harness present.
* Getters are cheap; `nes_dbg_tiles` re-reads CHR each call (fine for on-demand).
* SuperRust maps the same exports with `which` ∈ {BG1..BG4, OBJ} and a wider
  palette/tiles sheet; the app viewer code is unchanged bar the layer labels.
