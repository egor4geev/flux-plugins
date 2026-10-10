# Flux Tiles

File icons as colored tiles: each kind of file gets Flux's own glyph in white (near-black where
white wouldn't read) on a rounded tile of its color — Rust orange, TypeScript blue, Python blue
with a yellow snake, Go cyan, Docker blue, licenses gold. The tiles are full-color, so they look the
same on light and dark themes.

The set knows the same files as Flux Icons, the set Flux comes with. Folders keep Flux's own icons;
a file the set doesn't know gets a gray tile, unless its language plugin brings an icon of its own.

Choose it in **Settings → Appearance → File Icons**.

The icons are generated: `python3 src/gen-tiles.py <flux>/plugins/icons` reads Flux Icons and writes
`icons/` and `icon-themes/tiles.toml` (edit the script, not the generated files).
