# Claude Usage

Menu bar monitor for Claude subscription usage limits.

## Build

```bash
pnpm install
pnpm tauri build
```

Rust tests: `cargo test --all` (from `src-tauri`). Frontend checks:
`pnpm check && pnpm test`.

### Tray icons

`src-tauri/icons/tray-*.png` are generated, not drawn — Ferris the crab,
recoloured per severity, rendered from geometric primitives so the build
needs no image library or external binary. Regenerate them with:

```bash
python3 scripts/generate-tray-icons.py
```

Edit the `COLOURS` table or the `inside()` geometry at the top of that
script, then re-run it and open one of the output PNGs enlarged to confirm
it still reads as a crab before committing.
