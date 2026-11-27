//! Colour schemes and control styles.
//!
//! A colour scheme says which colours each role gets; a control style says
//! how controls are drawn (frame characters, shadows, button brackets).
//! They are independent, so any scheme works with any style.

use serde::Serialize;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct Color(pub u8, pub u8, pub u8);

/// The 16 VGA text-mode colours, in attribute order.
pub mod vga {
    use super::Color;
    pub const BLACK: Color = Color(0x00, 0x00, 0x00);
    pub const BLUE: Color = Color(0x00, 0x00, 0xAA);
    pub const GREEN: Color = Color(0x00, 0xAA, 0x00);
    pub const CYAN: Color = Color(0x00, 0xAA, 0xAA);
    pub const RED: Color = Color(0xAA, 0x00, 0x00);
    pub const MAGENTA: Color = Color(0xAA, 0x00, 0xAA);
    pub const BROWN: Color = Color(0xAA, 0x55, 0x00);
    pub const LIGHT_GRAY: Color = Color(0xAA, 0xAA, 0xAA);
    pub const DARK_GRAY: Color = Color(0x55, 0x55, 0x55);
    pub const LIGHT_BLUE: Color = Color(0x55, 0x55, 0xFF);
    pub const LIGHT_GREEN: Color = Color(0x55, 0xFF, 0x55);
    pub const LIGHT_CYAN: Color = Color(0x55, 0xFF, 0xFF);
    pub const LIGHT_RED: Color = Color(0xFF, 0x55, 0x55);
    pub const LIGHT_MAGENTA: Color = Color(0xFF, 0x55, 0xFF);
    pub const YELLOW: Color = Color(0xFF, 0xFF, 0x55);
    pub const WHITE: Color = Color(0xFF, 0xFF, 0xFF);
    pub const ALL: [Color; 16] = [
        BLACK, BLUE, GREEN, CYAN, RED, MAGENTA, BROWN, LIGHT_GRAY, DARK_GRAY, LIGHT_BLUE, LIGHT_GREEN,
        LIGHT_CYAN, LIGHT_RED, LIGHT_MAGENTA, YELLOW, WHITE,
    ];
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
pub struct Attrs {
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
    pub reverse: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct Style {
    pub fg: Color,
    pub bg: Color,
    pub attrs: Attrs,
}

impl Style {
    pub const fn new(fg: Color, bg: Color) -> Self {
        Style { fg, bg, attrs: Attrs { bold: false, italic: false, underline: false, reverse: false } }
    }
    pub fn fg(mut self, c: Color) -> Self {
        self.fg = c;
        self
    }
    pub fn bg(mut self, c: Color) -> Self {
        self.bg = c;
        self
    }
}

/// Every colour role the UI uses.
#[derive(Clone, Debug)]
pub struct Scheme {
    pub name: &'static str,
    pub desktop: Style,
    pub desktop_char: &'static str,
    pub menu: Style,
    pub menu_hotkey: Style,
    pub menu_selected: Style,
    pub menu_selected_hotkey: Style,
    pub menu_disabled: Style,
    pub frame: Style,
    pub frame_active: Style,
    pub frame_icons: Style,
    pub editor: Style,
    pub editor_selection: Style,
    pub editor_gutter: Style,
    pub editor_ghost: Style,
    pub scrollbar: Style,
    pub dialog: Style,
    pub dialog_frame: Style,
    pub button: Style,
    pub button_focused: Style,
    pub button_hotkey: Style,
    pub input: Style,
    pub status: Style,
    pub status_key: Style,
    pub shadow: Style,
    pub agent_pointer: Style,
    /// The built-in terminal's default colours.
    pub terminal: Style,
    /// Text colour per highlight kind, indexed like `SYNTAX_KINDS`
    /// (None: the editor's own colour).
    pub syntax: [Option<Color>; SYNTAX_KINDS.len()],
    /// Draw keywords bold (for monochrome schemes).
    pub bold_keywords: bool,
    /// Text colour of errors and warnings (also underlined).
    pub diag_error: Color,
    pub diag_warning: Color,
    /// A line with a breakpoint, and the line the debugger stopped at.
    pub breakpoint: Style,
    pub exec_bar: Style,
    /// Filled title bars (VB-DOS): the active window's, others', dialogs'.
    pub title_active: Style,
    pub title_inactive: Style,
    pub dialog_title: Style,
}

/// Highlight kinds, in index order (the same list as turbine-syntax's).
pub const SYNTAX_KINDS: [&str; 19] = [
    "keyword", "string", "comment", "number", "constant", "function", "operator", "property", "variable", "embedded",
    "type", "constructor", "punctuation", "escape", "attribute", "tag", "label", "namespace", "special",
];

/// Build a syntax colour table from (kind, colour) pairs.
pub fn syntax_table(pairs: &[(&str, Color)]) -> [Option<Color>; 19] {
    let mut t = [None; 19];
    for (k, c) in pairs {
        if let Some(i) = SYNTAX_KINDS.iter().position(|x| x == k) {
            t[i] = Some(*c);
        }
    }
    t
}

/// Borland C++'s editor colours on blue.
fn borland_blue_syntax() -> [Option<Color>; 19] {
    use vga::*;
    syntax_table(&[
        ("keyword", WHITE),
        ("comment", LIGHT_GRAY),
        ("string", LIGHT_CYAN),
        ("number", LIGHT_GREEN),
        ("constant", LIGHT_GREEN),
        ("attribute", LIGHT_GREEN),
        ("label", LIGHT_GREEN),
        ("type", LIGHT_MAGENTA),
        ("constructor", LIGHT_MAGENTA),
        ("namespace", LIGHT_MAGENTA),
        ("escape", LIGHT_RED),
        ("special", LIGHT_RED),
        ("tag", WHITE),
    ])
}

macro_rules! s {
    ($fg:ident on $bg:ident) => {
        Style::new(vga::$fg, vga::$bg)
    };
}

impl Scheme {
    /// Turbo Vision / Borland blue.
    pub fn turbo() -> Scheme {
        Scheme {
            name: "Turbo",
            desktop: s!(BLUE on LIGHT_GRAY),
            desktop_char: "░",
            menu: s!(BLACK on LIGHT_GRAY),
            menu_hotkey: s!(RED on LIGHT_GRAY),
            menu_selected: s!(BLACK on GREEN),
            menu_selected_hotkey: s!(RED on GREEN),
            menu_disabled: s!(DARK_GRAY on LIGHT_GRAY),
            frame: s!(LIGHT_GRAY on BLUE),
            frame_active: s!(WHITE on BLUE),
            frame_icons: s!(LIGHT_GREEN on BLUE),
            editor: s!(YELLOW on BLUE),
            editor_selection: s!(BLUE on LIGHT_GRAY),
            editor_gutter: s!(LIGHT_CYAN on BLUE),
            editor_ghost: s!(LIGHT_BLUE on BLUE),
            scrollbar: s!(BLUE on CYAN),
            dialog: s!(BLACK on LIGHT_GRAY),
            dialog_frame: s!(WHITE on LIGHT_GRAY),
            button: s!(BLACK on GREEN),
            button_focused: s!(WHITE on GREEN),
            button_hotkey: s!(YELLOW on GREEN),
            input: s!(WHITE on BLUE),
            status: s!(BLACK on LIGHT_GRAY),
            status_key: s!(RED on LIGHT_GRAY),
            shadow: s!(DARK_GRAY on BLACK),
            agent_pointer: s!(BLACK on YELLOW),
            terminal: s!(LIGHT_GRAY on BLACK),
            syntax: borland_blue_syntax(),
            bold_keywords: false,
            diag_error: vga::LIGHT_RED,
            diag_warning: vga::LIGHT_MAGENTA,
            breakpoint: s!(WHITE on RED),
            exec_bar: s!(BLACK on CYAN),
            title_active: s!(WHITE on BLUE),
            title_inactive: s!(LIGHT_GRAY on BLUE),
            dialog_title: s!(WHITE on LIGHT_GRAY),
        }
    }

    /// QBasic / MS-DOS Edit: grey text on blue, plain desktop.
    pub fn qbasic() -> Scheme {
        Scheme {
            name: "QBasic",
            desktop: s!(LIGHT_GRAY on BLUE),
            desktop_char: " ",
            menu: s!(BLACK on LIGHT_GRAY),
            menu_hotkey: s!(WHITE on LIGHT_GRAY),
            menu_selected: s!(LIGHT_GRAY on BLACK),
            menu_selected_hotkey: s!(WHITE on BLACK),
            menu_disabled: s!(DARK_GRAY on LIGHT_GRAY),
            frame: s!(LIGHT_GRAY on BLUE),
            frame_active: s!(LIGHT_GRAY on BLUE),
            // The title and the ↑ box: reverse video.
            frame_icons: s!(BLUE on LIGHT_GRAY),
            editor: s!(LIGHT_GRAY on BLUE),
            editor_selection: s!(BLUE on LIGHT_GRAY),
            editor_gutter: s!(DARK_GRAY on BLUE),
            editor_ghost: s!(DARK_GRAY on BLUE),
            scrollbar: s!(BLACK on LIGHT_GRAY),
            dialog: s!(BLACK on LIGHT_GRAY),
            dialog_frame: s!(BLACK on LIGHT_GRAY),
            button: s!(BLACK on LIGHT_GRAY),
            // The focused button's < > brackets are bright.
            button_focused: s!(WHITE on LIGHT_GRAY),
            button_hotkey: s!(WHITE on LIGHT_GRAY),
            input: s!(BLACK on LIGHT_GRAY),
            status: s!(WHITE on CYAN),
            status_key: s!(WHITE on CYAN),
            shadow: s!(DARK_GRAY on BLACK),
            agent_pointer: s!(BLACK on YELLOW),
            terminal: s!(LIGHT_GRAY on BLACK),
            // QBasic and Edit never coloured code.
            syntax: [None; 19],
            bold_keywords: false,
            diag_error: vga::LIGHT_RED,
            diag_warning: vga::LIGHT_MAGENTA,
            breakpoint: s!(WHITE on RED),
            exec_bar: s!(BLACK on CYAN),
            title_active: s!(WHITE on BLUE),
            title_inactive: s!(LIGHT_GRAY on BLUE),
            dialog_title: s!(WHITE on LIGHT_GRAY),
        }
    }

    /// Visual Basic for MS-DOS: QBasic's sibling in text-mode Windows 2
    /// clothes: a dark grey dithered desktop, magenta title bars, blue code
    /// windows, grey menus and dialogs, a cyan status line.
    pub fn vbdos() -> Scheme {
        let mut s = Scheme::qbasic();
        s.name = "VB-DOS";
        s.desktop = s!(LIGHT_GRAY on BLACK);
        s.desktop_char = "▒";
        s.menu_hotkey = s!(WHITE on LIGHT_GRAY);
        s.menu_selected = s!(WHITE on BLACK);
        s.menu_selected_hotkey = s!(WHITE on BLACK);
        s.frame = s!(BLACK on BLACK);
        s.frame_active = s!(BLACK on BLACK);
        s.frame_icons = s!(BLACK on LIGHT_GRAY);
        s.editor = s!(WHITE on BLUE);
        s.editor_selection = s!(BLUE on LIGHT_GRAY);
        s.scrollbar = s!(BLACK on LIGHT_GRAY);
        s.dialog_frame = s!(BLACK on LIGHT_GRAY);
        s.button_focused = s!(WHITE on LIGHT_GRAY);
        s.input = s!(BLACK on LIGHT_GRAY);
        s.status = s!(BLACK on CYAN);
        s.status_key = s!(BLACK on CYAN);
        s.shadow = s!(BLACK on BLACK);
        s.title_active = s!(WHITE on MAGENTA);
        s.title_inactive = s!(BLACK on LIGHT_GRAY);
        s.dialog_title = s!(WHITE on MAGENTA);
        s
    }

    /// Borland C++ 3.1 style: cyan editor.
    pub fn borland_cyan() -> Scheme {
        let mut s = Scheme::turbo();
        s.name = "Borland Cyan";
        s.frame = s!(LIGHT_GRAY on CYAN);
        s.frame_active = s!(WHITE on CYAN);
        s.frame_icons = s!(LIGHT_GREEN on CYAN);
        s.editor = s!(BLACK on CYAN);
        s.editor_selection = s!(WHITE on BLUE);
        s.editor_gutter = s!(BLUE on CYAN);
        s.editor_ghost = s!(DARK_GRAY on CYAN);
        s.syntax = {
            use vga::*;
            syntax_table(&[
                ("keyword", WHITE),
                ("comment", BLUE),
                ("string", RED),
                ("number", MAGENTA),
                ("constant", MAGENTA),
                ("attribute", GREEN),
                ("type", BLUE),
                ("escape", LIGHT_RED),
            ])
        };
        s.diag_error = vga::RED;
        s.diag_warning = vga::MAGENTA;
        s.exec_bar = s!(WHITE on BLUE);
        s
    }

    /// Amber monochrome monitor.
    pub fn amber() -> Scheme {
        let a = Color(0xFF, 0xB0, 0x00);
        let dim = Color(0x80, 0x58, 0x00);
        let k = vga::BLACK;
        let n = Style::new(a, k);
        let r = Style::new(k, a);
        Scheme {
            name: "Amber",
            desktop: Style::new(dim, k),
            desktop_char: "░",
            menu: r,
            menu_hotkey: r,
            menu_selected: n,
            menu_selected_hotkey: n,
            menu_disabled: Style::new(dim, a),
            frame: Style::new(dim, k),
            frame_active: n,
            frame_icons: n,
            editor: n,
            editor_selection: r,
            editor_gutter: Style::new(dim, k),
            editor_ghost: Style::new(dim, k),
            scrollbar: Style::new(dim, k),
            dialog: r,
            dialog_frame: r,
            button: n,
            button_focused: n,
            button_hotkey: n,
            input: n,
            status: r,
            status_key: r,
            shadow: Style::new(dim, k),
            agent_pointer: r,
            terminal: n,
            syntax: syntax_table(&[("comment", dim)]),
            bold_keywords: true,
            diag_error: a,
            diag_warning: a,
            breakpoint: r,
            exec_bar: Style { attrs: Attrs { bold: true, ..Attrs::default() }, ..r },
            title_active: r,
            title_inactive: n,
            dialog_title: r,
        }
    }

    pub fn all() -> Vec<Scheme> {
        vec![Scheme::turbo(), Scheme::qbasic(), Scheme::vbdos(), Scheme::borland_cyan(), Scheme::amber()]
    }
}

/// Box-drawing set for one frame weight.
#[derive(Clone, Copy, Debug)]
pub struct FrameChars {
    pub tl: &'static str,
    pub tr: &'static str,
    pub bl: &'static str,
    pub br: &'static str,
    pub h: &'static str,
    pub v: &'static str,
}

pub const SINGLE: FrameChars = FrameChars { tl: "┌", tr: "┐", bl: "└", br: "┘", h: "─", v: "│" };
pub const DOUBLE: FrameChars = FrameChars { tl: "╔", tr: "╗", bl: "╚", br: "╝", h: "═", v: "║" };
/// No lines: the border is plain cells in the frame's colours (VB-DOS).
pub const BLANK: FrameChars = FrameChars { tl: " ", tr: " ", bl: " ", br: " ", h: " ", v: " " };
pub const ASCII: FrameChars = FrameChars { tl: "+", tr: "+", bl: "+", br: "+", h: "-", v: "|" };

/// How controls are drawn.
#[derive(Clone, Debug)]
pub struct ControlStyle {
    pub name: &'static str,
    pub active_frame: FrameChars,
    pub inactive_frame: FrameChars,
    pub menu_frame: FrameChars,
    pub shadows: bool,
    /// Text around a button label, e.g. ("< ", " >") or ("[ ", " ]").
    pub button: (&'static str, &'static str),
    pub close_icon: &'static str,
    pub zoom_icon: &'static str,
    pub resize_grip: &'static str,
    /// The window title in reverse video (QBasic: " Untitled " black on grey).
    pub title_reverse: bool,
    /// Window numbers in the top border (Turbo Vision's Alt+n).
    pub window_numbers: bool,
    /// Scroll bar characters: up, down, left, right, track, thumb.
    pub scroll: [&'static str; 6],
    /// Help's menu sits alone at the right end of the bar (QBasic, Edit).
    pub help_right: bool,
    /// No ≡ system menu in the bar.
    pub hide_system_menu: bool,
    /// Menu hotkey letters light up only while the bar is active (Alt/F10).
    pub hotkeys_when_active: bool,
    /// Status line key hints as `<F1=Help>` and the position as `00001:001`.
    pub dos_status: bool,
    /// Dialogs: their frame, and a dividing line above the buttons.
    pub dialog_frame: FrameChars,
    pub dialog_rule: bool,
    /// Editors show line numbers (QBasic and Edit never did).
    pub line_numbers: bool,
    /// The whole look comes with this colour scheme (by name).
    pub scheme: Option<&'static str>,
    /// Windows and dialogs get a filled title bar row (VB-DOS) instead of a
    /// title in the top border; ▼▲ buttons at its right.
    pub title_bar: bool,
    /// Status line hints as `F1=Help │ …` (VB-DOS).
    pub status_rules: bool,
}

impl ControlStyle {
    pub fn turbo_vision() -> ControlStyle {
        ControlStyle {
            name: "Turbo Vision",
            active_frame: DOUBLE,
            inactive_frame: SINGLE,
            menu_frame: SINGLE,
            shadows: true,
            button: ("  ", "  "),
            close_icon: "[■]",
            zoom_icon: "[↑]",
            resize_grip: "┘",
            title_reverse: false,
            window_numbers: true,
            scroll: ["▲", "▼", "◄", "►", "▒", "■"],
            help_right: false,
            hide_system_menu: false,
            hotkeys_when_active: false,
            dos_status: false,
            dialog_frame: DOUBLE,
            dialog_rule: false,
            line_numbers: true,
            scheme: None,
            title_bar: false,
            status_rules: false,
        }
    }
    /// QBasic 1.1 and MS-DOS Edit, as they looked: single frames, the title
    /// in reverse video, a ┤↑├ maximize box, arrow scroll bars, Help at the
    /// right of the menu bar, `<F1=Help>` on a cyan status line, no line
    /// numbers, the QBasic colours.
    pub fn qbasic() -> ControlStyle {
        ControlStyle {
            name: "QBasic / MS-DOS Edit",
            active_frame: SINGLE,
            inactive_frame: SINGLE,
            menu_frame: SINGLE,
            shadows: true,
            button: ("< ", " >"),
            close_icon: "",
            zoom_icon: "┤↑├",
            resize_grip: "",
            title_reverse: true,
            window_numbers: false,
            scroll: ["↑", "↓", "←", "→", "▒", "■"],
            help_right: true,
            hide_system_menu: true,
            hotkeys_when_active: true,
            dos_status: true,
            dialog_frame: SINGLE,
            dialog_rule: true,
            line_numbers: false,
            scheme: Some("QBasic"),
            title_bar: false,
            status_rules: false,
        }
    }
    /// Visual Basic for MS-DOS: filled title bars with ▼▲, no frame lines
    /// around code, dithered scroll bars, `F1=Help │` on a cyan line.
    pub fn vbdos() -> ControlStyle {
        ControlStyle {
            name: "VB-DOS",
            active_frame: BLANK,
            inactive_frame: BLANK,
            scroll: ["↑", "↓", "←", "→", "░", "█"],
            zoom_icon: "▼▲",
            title_reverse: false,
            title_bar: true,
            status_rules: true,
            scheme: Some("VB-DOS"),
            dialog_frame: BLANK,
            dialog_rule: false,
            ..ControlStyle::qbasic()
        }
    }
    pub fn ascii() -> ControlStyle {
        ControlStyle {
            name: "ASCII",
            active_frame: ASCII,
            inactive_frame: ASCII,
            menu_frame: ASCII,
            shadows: false,
            button: ("[ ", " ]"),
            close_icon: "[x]",
            zoom_icon: "[^]",
            resize_grip: "+",
            title_reverse: false,
            window_numbers: true,
            scroll: ["^", "v", "<", ">", ":", "#"],
            help_right: false,
            hide_system_menu: false,
            hotkeys_when_active: false,
            dos_status: false,
            dialog_frame: ASCII,
            dialog_rule: false,
            line_numbers: true,
            scheme: None,
            title_bar: false,
            status_rules: false,
        }
    }
    pub fn all() -> Vec<ControlStyle> {
        vec![ControlStyle::turbo_vision(), ControlStyle::qbasic(), ControlStyle::vbdos(), ControlStyle::ascii()]
    }
}

/// The scheme and style in use.
#[derive(Clone, Debug)]
pub struct Theme {
    pub scheme: Scheme,
    pub style: ControlStyle,
}

impl Default for Theme {
    fn default() -> Self {
        Theme { scheme: Scheme::turbo(), style: ControlStyle::turbo_vision() }
    }
}
