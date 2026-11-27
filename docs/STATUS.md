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
| tree-sitter highlighting on a worker thread: 18 built-in grammars + Helix's ~250 on demand (Tessera's loader, grammars trial-loaded in a child) | done |
| Syntax colours per scheme (Borland style: white keywords, grey comments, cyan strings, green numbers/preprocessor) | done |
| Language detection by name, extension, shebang | done |
| turbine-services: Content-Length JSON-RPC over stdio (reader/writer threads, never blocks the UI); in-process fake server for tests | done |
| LSP: one server per language + project root (auto-detected from ~25 known servers, or `TURBINE_LSP_<LANG>`), full-text sync, didSave | done |
| LSP: diagnostics (underline + gutter ■ + status bar message, F8/Shift+F8), completion popup (Ctrl+Space, filters as you type), go to definition (F12), hover (Ctrl+F1 Topic Search) | done |
| LSP: references, rename, code actions, formatting, signature help, inlay hints, semantic tokens, incremental sync, Problems window | planned |
| DAP client on the same RPC layer: initialize/launch/setBreakpoints/configurationDone, stops fetch stack, scopes and variables; fake adapter for tests | done |
| Debugging UI (Borland keys): Ctrl+F8 breakpoint (red bar), Ctrl+F9 run/continue, F7 trace into, F8 step over, F4 go to cursor, Ctrl+F2 reset, Ctrl+F3 call stack; cyan execution bar; Output and Debug windows; Run > Arguments | done |
| Adapters: lldb-dap, codelldb, gdb -i dap, debugpy, dlv, or `TURBINE_DAP_<LANG>` | done (none installed here: tested with the fake adapter only) |
| Debugging: watches, evaluate/modify, expanding nested variables, threads, conditional breakpoints, attach, build-before-run | planned |
| Embedded terminal (turbine-term: libghostty-vt on a PTY; works in TUI mode too) | done |
| Terminal: kitty/xterm key encoding, bracketed paste, wheel scrollback, DOS palette, title, restart on Enter after exit | done |
| Terminal: mouse reporting (any mode Ghostty encodes), selection (drag, double-click word, triple-click line, Alt for a block), copy on select, Ctrl+Shift+C/V, Ctrl/Shift+Ins, middle-click paste, wheel = arrows in full-screen programs | done |
| System clipboard from terminal mode via OSC 52 (editor and terminal copies) | done |
| Terminal: find, OSC 133 marks, images, split terminals, event-driven wakeups (now ~60 Hz ticks while open) | planned |
| WASI extension host (wasmtime, `wit/turbine.wit`) | interface drafted |
| AI providers, tab completion, assistant, agent tools | planned |
| hearth AI hub provider + MCP server for hearth's assistant | planned |
| Markdown help window, language guides download | planned |
| File viewers (hex, images, tables, archives, PDF) | planned |
| Desktop app: cxx-qt Qt Quick, QPainter cell rows, native menus (global menu via Qt's dbusmenu) | planned |
| Web front end: Leptos (wasm, no hand-written JS) and `turbine serve` (Leptos server) | planned |
| Windows 9x (rust9x) TUI build; DOS / Win 3.1 thin clients | planned |
| Settings file + Settings dialog | planned |
