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
| AI service: hearth AI hub (hearth-ai JSON lines: models, loaded state, load through the hub's VRAM arbiter) or a local Ollama / `TURBINE_AI_URL` | done |
| AI Chat (Ctrl+L): streams the answer into a Markdown window, with the selection or nearby code as context; reasoning (<think>) dropped | done (live-tested with the hub and qwen3.5:4b) |
| Tab completion: fill-in-the-middle model (e.g. qwen2.5-coder base) after a 350 ms pause, dim ghost text, Tab accepts; AI > Tab Completion toggles | done (tested with a fake server; not yet live) |
| AI > Models: pick the chat model | done |
| Agent (AI > Agent, Ctrl+I): the model works the UI with tools (look, menu, click, type, key, read, wait, open) through the visible driver; steps logged in AI Chat; terminal commands need Allow/Always/Deny; Esc stops | done (live: qwen3.5:4b via the hub opened, typed, saved) |
| Automation socket on by default; TURBINE_SOCKET in terminals; tool results say what has focus | done |
| `turbine mcp`: MCP stdio server so Claude Code, Codex or hearth's assistant drive a running Turbine with the same tools | done |
| TURBINE_AI_LOG: log of every agent turn | done |
| Inline edit (Ctrl+K), Claude/Codex/Gemini through the hub's CLIs, hearth assistant configured to use `turbine mcp` | planned |
| hearth AI hub as model provider | done |
| MCP server (`turbine mcp`) | done; hearth's assistant still needs it in its config |
| Docs service: Tessera's ~230 language guides fetched from official sites, converted to Markdown, kept offline (turbine/guides) | done |
| Help windows (read-only, Markdown highlighting): F1 guide for the file's language, Shift+F1 index, Download All Guides | done |
| QBasic-style rendered help with links (<Contents> <Index> <Back>), search across guides | planned |
| File viewers (hex, images, tables, archives, PDF) | planned |
| Desktop app: cxx-qt Qt Quick, QPainter cell rows, native menus (global menu via Qt's dbusmenu) | planned |
| Web front end: Leptos (wasm, no hand-written JS) and `turbine serve` (Leptos server) | planned |
| Windows 9x (rust9x) TUI build; DOS / Win 3.1 thin clients | planned |
| Settings file + Settings dialog | planned |
