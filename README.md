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
`speed` (characters per second, 0 = instant), `wait` (with `"text"`: until
the active window shows it, e.g. a build's last line), `stop`. Terminal
windows report their screen text in `snapshot` and `text`. Each reply
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
- Syntax highlighting with tree-sitter in Borland's colours: 18 grammars
  built in, and any of Helix's ~250 fetched the first time a file needs it.
  Parsing runs off the UI thread.
- Language servers, started for you per project from ~25 known ones (or
  `TURBINE_LSP_<LANG>="cmd args"`): problems underlined with a ■ in the
  gutter and the message on the status line (F8 / Shift+F8 step through
  them), completion (Ctrl+Space, filtering as you type), go to definition
  (F12) and Topic Search (Ctrl+F1, the server's hover text)
- Debugging with Borland's keys over the Debug Adapter Protocol (lldb-dap,
  gdb, codelldb, debugpy, dlv, or `TURBINE_DAP_<LANG>`): Ctrl+F8 breakpoint,
  Ctrl+F9 run, F7 trace into, F8 step over, F4 go to cursor, Ctrl+F2 reset;
  red breakpoint bars, a cyan execution bar, Output and Debug (call stack,
  variables) windows along the bottom
- Help: F1 opens the guide for your file's language (one of ~230, from the
  language's own site, fetched once and kept for offline reading), Shift+F1
  lists them all, Ctrl+F1 asks the language server about the word under
  the cursor
- Unicode: grapheme clusters, East Asian wide characters (two cells each),
  combining marks, emoji, and bidirectional text with an RTL mode
- Encodings detected on open and kept on save: UTF-8/16 (with or without
  BOM), DOS code pages (CP437, CP850) by box-drawing heuristics, and every
  legacy encoding `chardetng` knows. Line endings are kept too.
- Colour schemes (Turbo, QBasic, Borland Cyan, Amber) and control styles
  (Turbo Vision, QBasic, ASCII), chosen separately
- **A built-in terminal** (Terminal > New Terminal, File > DOS Shell, Ctrl+\`)
  running Ghostty's terminal engine (libghostty-vt), drawn in Turbine's own
  cells, so it works inside terminal mode too. It speaks the kitty keyboard
  protocol to programs, does bracketed paste and scrollback, and uses the DOS
  16-colour palette. Every key goes to the program except F10 (menus),
  Alt+F3 (close) and Alt+0..9 (windows). Mouse clicks go to programs that
  ask for them (hold Shift to select instead); otherwise drag selects
  (double-click a word, triple-click a line, Alt for a block) and copies.
  Ctrl+Shift+C/V or Ctrl/Shift+Insert copy and paste, and a middle click
  pastes. Copies reach your system clipboard (OSC 52), even over SSH.
- The automation socket, plus `--headless --script` for tests and screenshots

The plan covers much more:
WASI extensions, Emacs and VS Code keymaps, AI completion and an
assistant, hearth AI hub integration, a global menu, desktop and web front
ends (a cxx-qt Qt app and a Leptos web app, no JavaScript), and retro
platforms. See [docs/DESIGN.md](docs/DESIGN.md) and
[docs/STATUS.md](docs/STATUS.md).

## Build

The terminal engine is Ghostty's, built with Zig 0.15. The flake's dev
shell has Zig:

```bash
nix develop -c cargo run --release -- [files...]
```

Without Nix, put Zig 0.15 and git on your PATH and use plain `cargo`.

```bash
nix run github:benwbooth/turbine
```

## License

MIT
