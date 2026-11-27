# Status

| Area | State |
|---|---|
| Cell surface, wide chars, clipping, diff rendering | done |
| Terminal front end (crossterm, kitty keys, SGR mouse, paste) | done |
| Menu bar (Cursor's menus + Borland's ≡, Search, Run/Compile/Debug, Options, Window), drop-downs, status bar | done; unbuilt features shown disabled |
| Responsive menu bar (tightens; overflow stays keyboard-reachable) | done; overflow "»" menu for the mouse planned |
| Windows: move, resize from edges/corner, zoom, tile, cascade | done |
| Dialogs: label, input, button, list | done |
| Editor: graphemes, selection, clipboard, undo, auto-indent, bidi | done |
| Encodings: UTF-8/16, DOS CP437/850, chardetng, line endings | done |
| Colour schemes + control styles | 4 + 3 built in |
| Automation tree + JSON-lines socket + paced agent driver | done |
| Agents read terminal screens (snapshot/text) and `wait` for text to appear | done |
| Headless mode with scripts and text screenshots | done |
| Find/replace | planned |
| Splitters inside windows, file tree panel | planned |
| Vim mode (turbine-vi, from tessh): modes, operators, text objects, surround, registers, marks, dot, `:` ex with ranges/:s/:g/:norm/:sort | done |
| Vim additions: macros (q/@/@@/@:), Ctrl-W windows, :e :w :q :wq :x :wa :qa :sp/:vs {file} :new :only :bn :bp, :set nu/rl | done |
| Vim: two views of one buffer (:sp with no file), vimrc, more :set options, `"+` via system clipboard | planned |
| Keymaps: CUA (DOS) and Vim, Options > Keymap or `--keymap` | done |
| Emacs / VS Code keymaps | planned |
| tree-sitter highlighting (Helix grammars on demand) | planned |
| LSP client | planned |
| DAP client | planned |
| Embedded terminal (turbine-term: libghostty-vt on a PTY; works in TUI mode too) | done |
| Terminal: kitty/xterm key encoding, bracketed paste, wheel scrollback, DOS palette, title, restart on Enter after exit | done |
| Terminal: mouse reporting, selection/copy, find, OSC 133 marks, images, split terminals, event-driven wakeups (now ~60 Hz ticks while open) | planned |
| WASI extension host (wasmtime, `wit/turbine.wit`) | interface drafted |
| AI providers, tab completion, assistant, agent tools | planned |
| hearth AI hub provider + MCP server for hearth's assistant | planned |
| Markdown help window, language guides download | planned |
| File viewers (hex, images, tables, archives, PDF) | planned |
| Desktop app (winit + wgpu) with dbusmenu global menu | planned |
| Web front end (wasm canvas) and `turbine serve` | planned |
| Windows 9x (rust9x) TUI build; DOS / Win 3.1 thin clients | planned |
| Settings file + Settings dialog | planned |
