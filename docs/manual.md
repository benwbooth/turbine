# Turbine

Turbine is a code editor and IDE that looks like the DOS IDEs (Turbo
Vision, QBasic and MS-DOS Edit, VB-DOS) and works like Cursor: side bars,
a panel, tabs, a command palette, language servers, a debugger, build
tasks and an AI that can drive the editor where you can watch it.

Press F10 (or Alt and a menu's letter) for the menus. Every command is
also in the Command Palette (Ctrl+Shift+P). This manual is Help >
Contents (F1); Help > Index (Shift+F1) lists its topics, and the
[Contents](#contents) below goes to each. The [keys](#keys) are at the end.

## Contents

- [Getting around](#getting-around)
- [Files](#files)
- [Editing](#editing)
- [Languages](#languages)
- [Problems](#problems)
- [Building](#building)
- [Debugging](#debugging)
- [Tools](#tools)
- [Terminals](#terminals)
- [Source control](#source-control)
- [AI](#ai)
- [Settings and looks](#settings-and-looks)
- [Extensions](#extensions)
- [Keys](#keys)

## Getting around

- **Windows.** Each file is a window on the desktop. F6 goes to the next
  window, Shift+F6 to the one before, Alt+1..9 to a numbered one, Alt+0
  lists them all. F5 zooms the window; Alt+F3 or Ctrl+W closes it. Drag
  a window's title bar to move it and its corner to resize it.
- **Editor tabs and splits.** View > Editor Tabs shows a tab for each
  file. Ctrl+\ splits the editors in two; a file chosen next opens on the
  other side.
- **Side bars and the panel.** Ctrl+B shows or hides the primary side
  bar (Explorer, Outline, Search, Source Control, Run and Debug,
  Extensions). Ctrl+J shows the panel (Problems, Output, terminals,
  Immediate). Ctrl+Alt+B shows the secondary side bar (the AI chat).
- **Quick open.** Ctrl+P finds a file in the project. In the same box,
  `>` runs a command, `:` goes to a line, `@` to a symbol in the file and
  `#` to a symbol anywhere in the project (Ctrl+T).
- **Breadcrumbs and minimap.** View > Breadcrumbs shows the file's
  folders and the definitions the cursor is in; View > Minimap shows a
  miniature of the file to click or drag.

## Files

- F3 or Ctrl+O opens a file, Ctrl+N makes a new one, F2 or Ctrl+S saves,
  File > Save As saves under another name.
- File > Open Folder makes a folder the project: the Explorer, Search,
  Source Control and tasks work in it.
- In the Explorer, Enter opens, F2 renames, Delete moves to the trash,
  and the context menu makes files and folders.
- Files keep their encoding and line endings. The encoding is detected
  (Unicode, the DOS code pages, Windows and ISO pages, KOI8, Mac,
  Japanese, Chinese, Korean, EBCDIC and more); File > Reopen with
  Encoding reads a file again in another, and Save with Encoding writes
  it in another. Right to Left (View) lays out right-to-left text.
- Pictures, PDFs, archives, media and office documents open in the
  desktop's app for them; another binary file asks: its app, a hex view,
  or as text.
- File > Open Recent lists the files opened lately. Alt+Left and
  Alt+Right go back and forward through the places you jumped from.
- A new, untitled file guesses its language from what you type; Edit >
  Change Language Mode sets it by hand.
- View > Markdown Preview (Ctrl+Shift+V) shows a Markdown file rendered
  beside it, following your edits. Enter or a click on a link follows it.

## Editing

- Shift with the arrows selects; Ctrl+X, Ctrl+C and Ctrl+V cut, copy and
  paste; Ctrl+Z undoes and Ctrl+Y redoes.
- Enter keeps the line's indentation and adds a level after an opening
  bracket (or a `:` in Python and tessh). A closing bracket typed on an
  empty line goes back a level. Tab and Shift+Tab indent and outdent the
  selected lines.
- Ctrl+/ comments the lines out, or back in.
- Alt+Up and Alt+Down move the lines; Shift+Alt+Up and Shift+Alt+Down
  copy them.
- Shift+Alt+Right grows the selection to the next larger piece of
  syntax (a word, an expression, a statement, a block, a function);
  Shift+Alt+Left shrinks it back.
- Ctrl+Shift+\ goes to the matching bracket. Alt+PgDn and Alt+PgUp go
  to the next and previous definition.
- **Folding.** Ctrl+Shift+[ folds the block the cursor is in and
  Ctrl+Shift+] opens it; View > Fold All and Unfold All do every block.
  With line numbers on, the gutter's ▾ and ▸ fold and open with a click.
  A folded line ends in ⋯.
- **Find and replace.** Ctrl+F finds in the file and Ctrl+H replaces.
  Ctrl+Shift+F searches every file in the project.
- **More cursors.** Ctrl+Alt+Up and Ctrl+Alt+Down add a cursor above or
  below; Ctrl+D selects the word, then each next match with a cursor of
  its own. Selection > Column Selection Mode makes Shift+Up and Down
  select a column. Typing happens at every cursor; Esc goes back to one.
- **Word wrap.** View > Word Wrap (Alt+Z) wraps long lines at the
  window's edge.
- **Vim.** Options > Keymap (or Settings) switches to Vim keys; Help >
  Vim Keys lists them.

## Languages

Turbine knows a language from the file's name, its extension or its
first line (`#!/usr/bin/env python3`). It colours code with tree-sitter:
grammars for the common languages are built in, and Helix's grammars for
about 250 more are used when Helix is installed (or downloaded once).

For each language, Turbine uses what is installed:

- **A language server** for completion (Ctrl+Space), hover (Ctrl+F1), Go
  to Definition (F12), references (Shift+F12), rename (Shift+F2), Format
  Document (Ctrl+Shift+I), quick fixes (Ctrl+.), parameter hints (they
  appear as you type `(` and `,`, or Ctrl+Shift+Space), inlay hints,
  semantic colouring and symbols in the project. When a language has no
  server, using one of these offers to install one (Edit > Install
  Language Server), in a terminal so you can see what runs.
- **A checker.** Files of languages with no server are checked with the
  language's own tool (`bash -n`, `python3 -m py_compile`, `tessh
  --check`, `node --check`, ...) when they are opened and saved. Their
  problems are marked in the text and listed in [Problems](#problems).
- **A runner.** Run > Run Without Debugging (Ctrl+F5) runs the file, or
  its project, in a [terminal](#terminals); [Building](#building) has
  compiling and tasks.
- **A debugger.** Run > Start Debugging uses the language's debug adapter
  (lldb-dap or gdb for native languages, debugpy for Python, dlv for Go,
  tessh's own for [tessh](#tessh), and others); see [Debugging](#debugging).
- **A guide.** Help > Language Guide opens the guide for the file's
  language; Help > Language Guides lists them all.

### tessh

tessh is a shell that is Python underneath. Turbine colours tessh
(`.tsh` files, and `#!/usr/bin/env tessh` scripts), including what its
strings interpolate (`$x`, `$(...)`), outlines its `def`, `class`,
`defmacro` and `defop` definitions, folds its blocks by indentation,
checks files with `tessh --check`, runs them with `tessh`, and debugs
them with tessh's own debugger (`tessh --dap`): breakpoints with
conditions, stepping, variables, and an Immediate window that takes
tessh or Python and can change the running script. Its two
guides, the tessh guide and the Advanced tessh Scripting Guide, come
from the installed tessh.

## Problems

The Problems view (in the panel) lists what language servers, checkers
and builds report, file by file. Enter goes to one. F8 and Shift+F8 go
to the next and previous problem in the file; the status line says what
it is.

## Building

- Run > Make (F9), or Terminal > Run Build Task (Ctrl+Shift+B), runs the
  project's build. Run > Compile (Alt+F9) checks the file in front.
  Run > Build All rebuilds everything. Run > Stop Build stops it.
- The output goes to the Output window; the compiler's errors and
  warnings go to [Problems](#problems).
- Terminal > Run Task lists every task. Tasks come from
  `.turbine/tasks.json` or `.vscode/tasks.json`, else from the project's
  files: Cargo, make, CMake, Meson, Zig, Go, npm (pnpm, yarn, bun),
  Python, Nix, .NET, Gradle, Maven, Mix, Stack or Cabal, Dune, Swift,
  Dart.

A tasks file looks like VS Code's:

```json
{
  "tasks": [
    { "label": "build", "type": "shell", "command": "make", "args": ["-j8"],
      "group": { "kind": "build", "isDefault": true } }
  ]
}
```

## Debugging

- Ctrl+F8 sets or clears a breakpoint on the cursor's line;
  Ctrl+Shift+F8 gives it a condition (`x > 3`), a hit count or a log
  message (`x is {x}`), which logs instead of stopping.
- Ctrl+F9 runs (or goes on); F7 traces into; F8 steps over; Step Out
  and Pause are in the Run menu; F4 runs to the cursor; Ctrl+F2 stops.
- When the program stops, the line is marked, the Debug window and the
  Run and Debug view show the call stack and the variables.
- Ctrl+F7 adds a watch: an expression evaluated at every stop, listed in
  Run and Debug (Delete removes it). Watches are kept between runs.
- Ctrl+F4 is Evaluate and Modify: an expression's value, and a new value
  for it.
- Ctrl+Shift+Y opens the Immediate window. Type an expression or a
  statement and press Enter: it runs in the stopped program and the
  answer appears under it.
- **Launch configurations.** Run > Debug Configurations opens the
  project's `.turbine/launch.json` (made from a template if there is
  none; `.vscode/launch.json` works too). When there are several,
  starting asks which; the choice is remembered. `${workspaceFolder}`,
  `${file}` and the other VS Code variables work, and `"request":
  "attach"` attaches to a running program.
- Run > Arguments sets the program and its arguments when there is no
  configuration.

## Tools

The Tools menu (also the ≡ menu at the left of the menu bar) holds the
programs that come with Turbine.

Tools > File Manager (**Ctrl+Alt+F**) is XTree Gold's screen: the folder
tree on the left, the files of the highlighted folder beside it, and the
disk's numbers at the right. Its keys are on the bottom line, as XTree
showed them.

Tools > Show Clipboard shows what's on it. Tools > Calculator works out
an expression (`+ - * / % ^`, hex `0x1F` and binary `0b101`, `sqrt`,
`sin`, `ln`, `pi` ...) and shows whole numbers in hex and binary too.
File > New Window starts another Turbine.

## Terminals

Terminal > New Terminal (Ctrl+Shift+`) opens your shell in the panel.
Ctrl+Shift+C and Ctrl+Shift+V copy and paste; every other key goes to
the program. Drag the scroll bar, or use the wheel, to read back.

## Source control

The Source Control view shows what changed in the project's git
repository. Enter shows a file's changes (o opens the file), Space
stages or unstages it, Tab goes to the message field, and Ctrl+Enter (or
Enter in the field) commits.

The Git menu shows a file's changes, all changes, the staged changes,
the history (Enter on a commit shows it), a file's history and who
changed the cursor's line; it switches branches, and pulls, pushes,
fetches and stashes (their output in the Output window).

Changes show as delta shows them: each file under a rule, each hunk
under its line and the function it's in, removed lines on red and added
ones on green with the words that changed picked out, the code coloured
by its language. In a diff, s switches between one after another and
side by side, n and p go to the next and previous change, and Enter
opens the file at that line. A `.patch` or `.diff` file shows the same
way with Ctrl+Shift+V.

## AI

- Ctrl+L opens the chat. Ctrl+. in the chat field switches it between
  asking and Agent mode, where the AI works the editor itself, visibly:
  it opens menus, types and runs commands at a speed you can watch
  (Settings > Agent > Typing speed). It asks before running anything in
  a terminal.
- Ctrl+K edits the selection (or writes at the cursor) from what you ask.
- With AI tab completion on, grey suggestions appear as you type; Tab
  takes one.
- Models come from hearth's AI hub, or a local Ollama or llama.cpp.

## Settings and looks

Options > Settings (Ctrl+,) lists every setting with what it does. Options
> Control Style chooses how windows and controls are drawn (Turbo Vision,
QBasic and MS-DOS Edit, VB-DOS, plain ASCII), each with its colours;
the colour scheme can be chosen apart from the style. The layout, the
look and the settings are kept between runs.

## Extensions

Extensions are WebAssembly components (`wit/turbine.wit`). Each lives in
`~/.local/share/turbine/extensions/<name>/extension.wasm` and can read
and edit documents, add menu items and key bindings, run commands and
read settings. The Extensions view lists them. `extensions/hello` is an
example; `extensions/build.sh` builds one.

## Keys

| Key | Does |
|---|---|
| F1 | This manual |
| Shift+F1 | Its index |
| F2 / Ctrl+S | Save |
| F3 / Ctrl+O | Open |
| F5 | Zoom the window |
| F6 / Shift+F6 | Next / previous window |
| F7 / F8 | Trace into / step over (debugging) |
| F8 / Shift+F8 | Next / previous problem |
| F9 / Alt+F9 | Make / compile |
| F10 | Menus |
| F12 / Shift+F12 | Definition / references |
| Ctrl+F1 | What the language server says about the word |
| Ctrl+F2 | Stop debugging |
| Ctrl+F4 | Evaluate and Modify |
| Ctrl+F5 | Run without debugging |
| Ctrl+F7 | Add a watch |
| Ctrl+F8 / Ctrl+Shift+F8 | Breakpoint / its condition |
| Ctrl+F9 | Run |
| Ctrl+P / Ctrl+Shift+P | Quick open / command palette |
| Ctrl+T | Symbol in the project |
| Ctrl+Shift+O | Symbol in the file |
| Ctrl+G | Go to line |
| Ctrl+F / Ctrl+H | Find / replace |
| Ctrl+Shift+F | Search the project |
| Ctrl+Space | Completion |
| Ctrl+. | Quick fix |
| Ctrl+Shift+I | Format the document |
| Shift+F2 | Rename the symbol |
| Ctrl+/ | Comment lines |
| Ctrl+Shift+[ / ] | Fold / unfold |
| Shift+Alt+Right / Left | Grow / shrink the selection |
| Alt+Up / Down | Move lines |
| Alt+PgDn / PgUp | Next / previous definition |
| Ctrl+B / Ctrl+J / Ctrl+Alt+B | Side bar / panel / chat |
| Ctrl+\ | Split the editor |
| Ctrl+L / Ctrl+K | AI chat / edit with AI |
| Ctrl+Shift+B | Run the build task |
| Ctrl+Shift+Y | Immediate window |
| Ctrl+Shift+` | New terminal |
| Alt+Left / Alt+Right | Back / forward |
| Alt+Z | Word wrap |
| Ctrl+Alt+Up / Down | Add a cursor |
| Ctrl+D | Select the next match too |
| Ctrl+Shift+V | Markdown or patch preview |
| Alt+X | Quit |
