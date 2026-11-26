# Turbine design

Turbine is an AI-first code editor and IDE that looks and feels like a
1990s DOS IDE (Turbo Vision, QBasic, MS-DOS Edit, Visual Basic for DOS) and
works like a modern one (VS Code, Cursor). Everything on screen is a grid of
character cells, but windows, splitters and controls resize freely and the
layout reacts to the size of the screen.

This document is the plan. `docs/STATUS.md` tracks what exists.

## Goals

1. **Retro look, modern behaviour.** Menu bar, double-line framed windows,
   drop shadows, scroll bars with arrows, dialog boxes with `< OK >`
   buttons, a status bar with key hints. Under that: LSP, DAP, tree-sitter,
   AI completion, an agent, a real terminal, Unicode everywhere.
2. **AI-first.** An AI can drive the whole interface the way a person does,
   through the same input path, and you can watch it: the pointer moves,
   menus drop down, text is typed at a readable speed, focus moves between
   windows. Nothing the AI does is invisible.
3. **Almost everything is an extension.** The core is a cell renderer, a
   widget toolkit, a text model and an extension host. Languages, themes,
   AI providers, viewers, the file tree, git, even the vim mode are
   extensions (built-in ones are compiled in, but use the same API).
4. **One program, three front ends.** Terminal (TUI), desktop app, web.
   The same widget tree and the same cell grid feed all three.
5. **Runs nearly everywhere.** Linux, macOS, Windows natively; DOS, Windows
   3.1 and Windows 9x as far as is honestly possible (see Retro platforms).

## Architecture

```
               +-------------------------------------------------+
               |                 turbine (binary)                |
               +-------------------------------------------------+
   front ends  | turbine-tui | turbine-app (winit+wgpu) | web     |
               +-------------+--------------------------+---------+
               |           turbine-ui  (widgets, layout,         |
               |           themes, input, automation tree)       |
               +-------------------------------------------------+
               | turbine-text | turbine-vi | turbine-term (VT)   |
               +-------------------------------------------------+
               | turbine-ext (WASI component host) | services:   |
               |  extensions/*.wasm                | lsp dap ai  |
               +-------------------------------------------------+
```

### Cells: the one rendering model

`turbine-ui::Surface` is a grid of `Cell { text: grapheme, width, fg, bg,
attrs }`. Widgets draw into a clipped `Canvas` over the surface. Every
front end does the same thing each frame: diff the new surface against the
last one and draw what changed.

- **TUI** writes the changes with crossterm (truecolor, falling back to 256
  and 16 colours, and to CP437/ASCII line drawing on terminals that can't
  show box characters).
- **App** draws cells with a real font on the GPU (winit + wgpu, glyphon for
  shaping). Because it owns the pixels it can do what a terminal can't:
  half-cell smooth scrolling, sub-cell mouse positions, a pixel-perfect VGA
  font option, images in cells, fractional scaling without blur.
- **Web** compiles the same core to `wasm32` and draws cells to a canvas.
  A second web mode, `turbine serve`, runs the editor on a machine and
  streams surface diffs over a WebSocket to a thin page (remote editing).

Cell content is a grapheme cluster, not a `char`, so combining marks, emoji
ZWJ sequences and East Asian wide characters all take the right number of
columns (`unicode-segmentation` + `unicode-width`). A wide character fills
two cells (the second is a continuation cell); overwriting either half
clears the other, clipping never splits one, and the cursor, selection and
mouse all move by whole graphemes.

### Widgets and layout

A retained widget tree. Each widget has an id, a role, a rectangle, and
handles events. Layout is a constraint pass every frame (sizes are cheap to
compute in cells), so everything reflows when the screen or a splitter
changes. Building blocks:

- `Desktop` (the patterned background), `MenuBar`, `StatusBar`
- `Window`: movable, resizable from any edge or the corner, zoom button,
  close button, window number (Alt+1..9), cascade/tile
- `Split`: horizontal/vertical, draggable divider, nests
- `EditorView`, `TerminalView`, `TreeView`, `ListView`, `Table`,
  `MarkdownView`, `HexView`, `ImageView`
- Dialog controls: `Button`, `InputLine`, `CheckBox`, `RadioGroup`,
  `ComboBox`, `ScrollBar`, `Label`, `History`

**Control styles** decide how each control is drawn (Turbo Vision brackets
and shadows, QBasic flat, Win 3.1 bevels drawn with half blocks, VB-DOS),
separately from **colour schemes** (Turbo blue, QBasic, Borland C++
"cyan", MS-DOS Edit, monochrome amber/green, high contrast, light). Both are
data files any extension can add.

### Text

`turbine-text`:

- Rope buffer (`ropey`), with undo tree, multi-cursor, and marks that
  survive edits.
- **Encodings.** Files are decoded on open and re-encoded on save in the
  same encoding and line endings. Detection order: BOM, then valid UTF-8,
  then `chardetng` (web encodings, CJK, Cyrillic, ...), then DOS code pages
  (CP437, 850, 852, 866, ...) using a box-drawing heuristic, since DOS-era
  text is the reason this editor exists. The user can reopen with any
  encoding from the status bar.
- **Bidi.** Lines are laid out with the Unicode Bidirectional Algorithm
  (`unicode-bidi`): visual order for display, logical order for editing,
  cursor movement that follows what the eye sees. A window can have RTL
  base direction, and the whole interface can mirror (menus on the right).
- Grapheme-aware cursor movement and deletion everywhere.

### Input and keymaps

Input is normalised to `KeyEvent { key, mods, kind: press/repeat/release }`
and `MouseEvent`. In a terminal, the kitty keyboard protocol is used when
the terminal supports it (probe with `CSI ? u`, push disambiguate + event
types + alternate keys + all-keys-as-escapes), so Ctrl+Shift+letter,
Shift+Enter, key release and lone modifier taps all work. Others fall back
to legacy decoding.

Keymaps are layered: **CUA/DOS** (the default: F10 menu, Alt+letter,
Shift+arrows, Ctrl+Ins/Shift+Ins), **VS Code**, **Emacs**, **Vim**.

**Vim** is a full modal layer, starting from tessh's vi engine
(`tessera/tessh/src/editor/{vi,ex,text}.rs`: operators, motions, text
objects, surround, registers, marks, dot repeat, visual block, and a real
ex command line with ranges, `:s`, `:g`, `:norm`, `:sort`), extended with
macros, windows (`:sp`, `Ctrl-W`), buffers, `:set` options and a vimrc
subset. It lives in `turbine-vi` behind a small text trait so the same
engine runs on the rope.

### Embedded terminal

`turbine-term` wraps a VT engine and draws its cell grid into a
`TerminalView`, so the terminal works in TUI mode too (a terminal inside a
terminal, like tmux). Engine: **libghostty-vt**, as in Tessera. Its
`RenderState` / row / cell iterators give exactly a cell grid with
graphemes, colours, styles and wide-cell flags, which maps straight onto
`Surface`. It needs Zig 0.15 at build time, so it sits behind a cargo
feature (`ghostty`, on in the Nix build) with `alacritty_terminal` as the
fallback engine for builds without Zig and for retro targets. PTYs come
from `portable-pty` (ConPTY on Windows).

### Extensions (WASI)

Extensions are WebAssembly components (WASI preview 2, component model)
described by `wit/turbine.wit`, hosted with `wasmtime`. An extension can:

- contribute commands, menu items, keybindings, status bar items, settings
- add languages (grammar, LSP/DAP launch config, formatter, snippets)
- add themes and control styles
- add views (it draws into a cell canvas the host gives it) and panels
- add file viewers (`can-view(path, bytes) -> priority`, `render`)
- add AI providers and agent tools
- read and edit documents through the host's text API

Capabilities are explicit (filesystem roots, network, process spawn) and
shown when an extension is installed. In the web build, components run on
the browser's own WebAssembly engine (transpiled with `jco`).

Built-in features use the same interface compiled natively, so moving a
feature into or out of the core never changes its API.

### Language services

- **Syntax:** tree-sitter, with grammars downloaded on demand (Helix
  runtime grammars, as Tessera's `grammars.rs` does) and highlight queries.
- **LSP:** client over stdio/TCP, any server; the language extension says
  which server to start, and Turbine offers to install missing ones.
  Diagnostics, hover, completion, signature help, go to definition,
  references, rename, code actions, formatting, inlay hints, semantic
  tokens.
- **DAP:** debugger client; breakpoints in the gutter, a Watch window, Call
  Stack, Locals, step keys F7/F8 like the old IDEs.
- **Help:** Markdown help everywhere, shown in a QBasic-style help window
  with hyperlinks (`<Contents> <Index> <Back>`). Language guides are
  downloaded and cached automatically, reusing Tessera's guide table
  (`guides.rs`, ~230 languages) and its HTML→Markdown pipeline.
- **File viewers:** text, hex, images (kitty/sixel in TUI, real pixels in
  app), Markdown, CSV/TSV tables, JSON/YAML/TOML trees, archives, PDF text,
  following tessh's viewer kinds.

### AI

- **Providers:** Anthropic (Claude), OpenAI (Codex/GPT), Google (Gemini),
  OpenRouter, and local servers (Ollama, llama.cpp, LM Studio, vLLM),
  after Tessera's `ai.rs`. Keys in the system keyring.
- **Tab completion:** Cursor-style next-edit prediction shown as dim ghost
  text, accepted with Tab, word by word with Ctrl+Right. Uses a fast local
  model by default (FIM), cloud model when chosen.
- **Assistant:** a chat window that can read the workspace and propose
  edits as reviewable diffs, or work as an agent.
- **Agent:** runs tools (read/edit files, run commands in the terminal,
  search, LSP queries) with ask/allow/deny per tool. Every action goes
  through the automation layer below, so you see it happen.
- Agents from outside (Claude Code, Codex, Gemini CLI) can drive Turbine
  through its MCP server.

### hearth AI hub

On a hearth desktop, Turbine works with hearth's AI hub (`hearth-ai`, the
JSON-lines helper behind Settings > AI, Spotlight's Ask and the voice
assistant) instead of running a second, competing model manager:

- **Models from the hub.** A `hearth` provider spawns `hearth-ai` and reads
  its `state`: local servers (Ollama, llama.cpp router, LM Studio, units
  like inkling), what's loaded, what's installed, and per-GPU VRAM. Turbine
  shows those in its model picker and asks the hub to `load` a model
  rather than starting servers itself, so hearth's VRAM arbiter (idle
  unload, eviction, `--make-room`) stays in charge of the graphics card.
  Requests then go straight to that server's OpenAI-compatible endpoint.
- **Same rules.** The hub's `allowCloud` setting and its pinned models are
  respected; cloud CLIs (Claude Code, Codex, Gemini) found by the hub are
  offered as agent back ends.
- **hearth's assistant can drive Turbine.** Turbine's automation server
  speaks MCP, the same way the hub already talks to the computer-use
  server, so "open main.rs in Turbine and fix the warning" from the voice
  assistant or Spotlight works through Turbine's own widget tree (exact,
  paced and visible) instead of screenshots and synthetic clicks.
  Approvals use hearth's approval card when Turbine runs on hearth.
- **Global menu** through hearth-appmenu (dbusmenu, below).

Off hearth, the same features use Turbine's own provider list.

### Automation: the AI drives the UI like a person

`turbine-ui` keeps an **automation tree**: every widget reports its id,
role, name, rectangle, state (focused, enabled, checked, selected) and
value. The automation API (JSON-RPC over a socket, also exposed as an MCP
server) offers:

- `snapshot` → the tree (and optionally the screen as text)
- `click(target)`, `drag(from, to)`, `scroll`
- `key(chord)`, `type(text)`
- `focus(target)`, `menu(path)` ("File/Save As...")
- `wait(condition)`

Targets are widget ids, roles+names, or cell coordinates. **Pacing** is the
point: actions are not teleported. The host animates a visible pointer
along a path to the target, opens menus item by item, and types at a set
rate (default ~15 chars/s, adjustable, or instant). A banner in the status
bar shows which agent is driving; any key or mouse movement from the human
pauses it. Each action is logged in an Agent Log window you can replay.

Human input and agent input go through the same event queue, so there is
no feature an agent can use that a person can't, and vice versa.

### Global menu (hearth, macOS, Windows)

The menu bar is a data model (`MenuModel`), not just drawing. The app front
end exports it over D-Bus as `com.canonical.dbusmenu` and registers the
window with `com.canonical.AppMenu.Registrar`, so hearth/arbor show it in
the global menu bar; on macOS it becomes the native NSMenu and on Windows
the in-window menu stays (via `muda` for the non-Linux cases). The in-window
DOS menu bar can be hidden when a global menu is present.

### Configuration

`~/.config/turbine/settings.toml` plus per-workspace `.turbine/`. Settings,
keymaps, themes and styles are all data; a Settings dialog edits them with
help text for every option.

## Retro platforms

Honest tiers:

| Platform | Plan |
|---|---|
| Linux, macOS, Windows 10+ | Full: TUI, app, web, extensions, all services |
| Windows 7/8 | TUI and app via Rust's `win7` targets |
| Windows 95/98/ME, NT4 | TUI build with the rust9x toolchain (patched std for old Win32); no wasmtime (extensions off or interpreted), no GPU app |
| Windows 3.1 | Thin client: a small 16-bit program speaking the remote surface protocol over serial/TCP to a Turbine host |
| MS-DOS | Thin client (Open Watcom C or Rust `no_std` + DJGPP) that draws the streamed cell grid in text mode and sends keys back over serial, null modem or packet driver TCP. Also: any DOS terminal program (Telix, ANSI.SYS) works with `turbine --ansi-cp437` over a serial line |

So the full IDE runs where Rust's std runs, and DOS/Win3.1 machines become
real Turbine screens connected to a host.

## Repository layout

```
crates/
  turbine-text   rope, encodings, unicode width/graphemes/bidi
  turbine-ui     surface, themes, widgets, layout, automation tree
  turbine-tui    terminal front end (crossterm, kitty keyboard)
  turbine        the binary
wit/turbine.wit  extension interface
docs/            design and status
```

The binary also holds the workbench (`workbench.rs`), the agent driver
(`agent.rs`) and the automation socket (`server.rs`) for now.

Planned crates: `turbine-vi`, `turbine-term`, `turbine-ext`, `turbine-lsp`,
`turbine-dap`, `turbine-ai`, `turbine-app`, `turbine-web`.
