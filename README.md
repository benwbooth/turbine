# Turbine

**An AI-first code editor and IDE that looks like a 1990s DOS IDE.**

Turbine looks like Turbo Vision, QBasic, MS-DOS Edit and Visual Basic for
DOS: menu bar, double-line windows with drop shadows, scroll bars with
arrows, dialog boxes, F-key hints on the status line. It aims to work like
VS Code or Cursor: language servers, debuggers, syntax highlighting, AI tab
completion, an assistant and agents. The whole interface is made of
character cells, but windows and controls resize freely and the layout
reflows when the screen changes.

```
   File  Edit  Search  View  Run  AI  Options  Window  Help
┌──────────────── UNTITLED ──────────1──────┐╔═[■]════════════ main.rs ═══════════2══[↑]═╗
│  1                                        │║  1 fn main() {                            ▲
│                                           │║  2     let name = "Turbine";              ▒
│                                           │║  3     for i in 1..=3 {                   ▒
│                                           │║  4         println!("{i}: hello from {name▒
│                                           │║  5     }                                  ▒
│                                           │║  6 }                                      ■
│                                           │║                                           ▼
└───────────────────────────────────────────┘╚*◄■▒▒▒▒▒▒▒▒▒▒▒▒▒▒▒▒▒▒▒▒▒▒▒▒▒▒▒▒▒▒▒▒▒▒▒▒▒▒►═┘
 F1 Help  F2 Save  F3 Open  Alt+F3 Close  F5 Zoom  F6 Next  F10 Menu   7:1  UTF-8 LF  INS
```

## AI that works the screen like a person

Agents drive Turbine through its own widget tree and the same input path
you use. The pointer glides to its target, menus drop open, dialogs fill
in, and code is typed at a speed you can read, so you can watch what the AI
is doing. Pressing any key pauses it, **Space** pauses or resumes, and **Esc** stops it.
Try **AI > Agent Demo**.

```bash
turbine --automation /tmp/turbine.sock &
```

```bash
echo '{"cmd":"menu","path":"File/New"}
{"cmd":"type","text":"fn main() {}\n"}
{"cmd":"snapshot"}' | socat - UNIX-CONNECT:/tmp/turbine.sock
```

Commands: `snapshot` (widget tree, optionally `"screen":true` for the
screen as text), `menu`, `click` (by `target` id or `role:name`, or `x`/`y`),
`drag`, `key` (`"Ctrl+S"`), `type`, `paste`, `command`, `open`, `text`,
`speed` (characters per second, 0 = instant), `wait`, `stop`. Each reply
comes back once the action has finished on screen.

## What works today

- Terminal mode: truecolor with a 16-colour fallback, mouse (click, drag,
  wheel), bracketed paste, and the kitty keyboard protocol when the terminal
  has it
- Movable, resizable, zoomable windows; tile and cascade; Alt+1..9
- Full menu bar (F10, Alt+letter, Alt+Space for ≡, mouse) laid out like
  Cursor's, plus Borland's system menu, Search, Run/Compile/Debug items,
  Options and Window; dialogs; a window list (Alt+0)
- **Vim** (Options > Keymap, or `--keymap vim`): normal, insert, visual,
  visual line/block and replace modes; operators, motions, text objects and
  surround; registers, marks, dot repeat, macros; the `:` command line with
  ranges, `:s`, `:g`, `:norm`, `:sort`; `:e`/`:w`/`:q`/`:sp`/`:bn` and
  `Ctrl-W` work on Turbine's windows. The engine comes from tessh's editor.
- Editing: graphemes, wide characters, selection, clipboard, undo/redo,
  auto-indent, overwrite mode
- Unicode: grapheme clusters, East Asian wide characters (two cells each),
  combining marks, emoji, and bidirectional text with an RTL mode
- Encodings detected on open and kept on save: UTF-8/16 (with or without
  BOM), DOS code pages (CP437, CP850) by box-drawing heuristics, and every
  legacy encoding `chardetng` knows. Line endings are kept too.
- Colour schemes (Turbo, QBasic, Borland Cyan, Amber) and control styles
  (Turbo Vision, QBasic, ASCII), chosen separately
- The automation socket, plus `--headless --script` for tests and screenshots

The plan covers much more: LSP, DAP, tree-sitter, an embedded terminal
(libghostty-vt), WASI extensions, Emacs and VS Code keymaps, AI completion and an
assistant, hearth AI hub integration, a global menu, desktop and web front
ends, and retro platforms. See [docs/DESIGN.md](docs/DESIGN.md) and
[docs/STATUS.md](docs/STATUS.md).

## Build

```bash
cargo run --release -- [files...]
```

```bash
nix run github:benwbooth/turbine
```

## License

MIT
