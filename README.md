# rmc

A lightweight dual-pane file manager for the terminal, inspired by Midnight Commander. The name stands for **R**ust **M**idnight **C**ommander.

## Features

- **Dual-pane navigation** - Browse two directories side by side
- **Background file operations** - Copy, move, and delete run in background threads with progress tracking
- **File viewer** - View files with multiple modes:
  - Text and hex dump
  - Binary analysis: disassembly, strings, ELF headers, sections, symbols, shared libraries
  - JSON pretty-printing
  - Archive contents
  - EXIF metadata
- **Keyboard-driven** - Vim-style navigation (hjkl) plus traditional keys

## Installation

```sh
cargo build --release
cp target/release/rmc ~/.local/bin/
```

## Key Bindings

### Navigation

| Key | Action |
|-----|--------|
| `j` / `↓` | Move down |
| `k` / `↑` | Move up |
| `h` / `←` | Go to parent directory |
| `l` / `→` / `Enter` | Enter directory |
| `Tab` | Switch pane |
| `PageUp` | Page up |
| `PageDown` | Page down |

### File Operations

| Key | Action |
|-----|--------|
| `Insert` | Toggle selection |
| `F2` | Rename |
| `F3` | View file |
| `e` / `F4` | Edit file in $EDITOR |
| `c` / `F5` | Copy to other pane |
| `m` / `F6` | Move to other pane |
| `F7` | Create directory |
| `Delete` / `F8` | Delete |

### Other

| Key | Action |
|-----|--------|
| `J` | Show job list |
| `Ctrl+S` | Search |
| `H` | Toggle hidden files |
| `S` | Cycle size display (off → quick → full) |
| `U` | Swap panes |
| `:` | Command line |
| `q` / `Esc` | Quit |

### Job List

| Key | Action |
|-----|--------|
| `K` | Kill selected job |
| `P` | Pause/resume job |
| `d` | Dismiss completed job |
| `Esc` / `J` | Close job list |

### File Viewer

| Key | Action |
|-----|--------|
| `t` | Text mode |
| `x` | Hex mode |
| `d` | Disassembly |
| `s` | Strings |
| `h` | ELF header |
| `S` | Sections |
| `y` | Symbols |
| `l` | Shared libraries (ldd) |
| `i` | File info |
| `e` | EXIF data |
| `a` | Archive listing |
| `J` | JSON pretty-print |
| `q` / `Esc` | Close viewer |

## Requirements

- Rust 2024 edition (to build)
- Optional external tools for file viewer modes:
  - `objdump` - Disassembly
  - `readelf` - ELF analysis
  - `ldd` - Shared library dependencies
  - `strings` - Extract strings from binaries
  - `file` - File type detection
  - `exiftool` - EXIF metadata
  - `jq` - JSON formatting
  - `tar`, `unzip`, `7z`, etc. - Archive listing

## License

MIT
