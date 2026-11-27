//! Guides for the editor's languages: each language's own documentation
//! (its tutorial or reference, from its official site), fetched the first
//! time its guide is opened, turned into Markdown and kept in the state
//! folder (`turbine/guides/LANG.md`). From Tessera's guides.rs. Nothing is bundled: the documents keep their own
//! licenses, recorded at the top of each one and in docs/GUIDES.md.

use std::path::PathBuf;

/// Where a guide comes from.
pub struct Guide {
    /// The editor's language name, and other names for the same guide.
    pub langs: &'static [&'static str],
    pub title: &'static str,
    /// Its license, and where the license is stated.
    pub license: &'static str,
    pub license_url: &'static str,
    /// Pages, in order: HTML pages, or Markdown (a `.md` URL).
    pub pages: &'static [&'static str],
    /// Then the pages an index page links to: (index URL, links' CSS selector).
    pub index: Option<(&'static str, &'static str)>,
    /// The part of an HTML page that's the document (CSS selector; "" the body).
    pub select: &'static str,
    /// Parts to leave out (CSS selector, besides scripts, buttons and the like).
    pub drop: &'static str,
    /// The language of code blocks that don't say theirs.
    pub code: &'static str,
    /// Elements that are code drawn some other way (tables of tokens, lines
    /// with <br>): made code blocks of their text (CSS selector).
    pub code_sel: &'static str,
    /// Every part `select` matches, in order, not just the first (chapters
    /// of a one-page manual).
    pub all: bool,
    /// Its own name, for a language with more than one guide (its kept
    /// files' name; "" for the language's).
    pub id: &'static str,
    /// The index's chapter titles (CSS selector): each chapter a part, its
    /// pages under it; and the chapter where the index stops being read.
    pub chapters: &'static str,
    pub until: &'static str,
    /// Elements to write as others: (CSS selector, "h3" or "p>strong"...).
    pub renames: &'static [(&'static str, &'static str)],
}

const fn g(langs: &'static [&'static str], title: &'static str, license: &'static str, license_url: &'static str, pages: &'static [&'static str], select: &'static str, code: &'static str) -> Guide {
    Guide { langs, title, license, license_url, pages, index: None, select, drop: "", code, code_sel: "", all: false, id: "", chapters: "", until: "", renames: &[] }
}

const fn gi(langs: &'static [&'static str], title: &'static str, license: &'static str, license_url: &'static str, index: (&'static str, &'static str), select: &'static str, code: &'static str) -> Guide {
    Guide { langs, title, license, license_url, pages: &[], index: Some(index), select, drop: "", code, code_sel: "", all: false, id: "", chapters: "", until: "", renames: &[] }
}

impl Guide {
    const fn pages(mut self, p: &'static [&'static str]) -> Self {
        self.pages = p;
        self
    }
    const fn renames(mut self, r: &'static [(&'static str, &'static str)]) -> Self {
        self.renames = r;
        self
    }
    const fn id(mut self, id: &'static str) -> Self {
        self.id = id;
        self
    }
    const fn chapters(mut self, titles: &'static str, until: &'static str) -> Self {
        self.chapters = titles;
        self.until = until;
        self
    }
    /// The name its kept files and progress go by.
    pub fn key(&self) -> &'static str {
        if self.id.is_empty() { self.langs[0] } else { self.id }
    }
    const fn all(mut self) -> Self {
        self.all = true;
        self
    }
    const fn code_sel(mut self, c: &'static str) -> Self {
        self.code_sel = c;
        self
    }
    const fn drop(mut self, d: &'static str) -> Self {
        self.drop = d;
        self
    }
}

const GNU_DROP: &str = ".contents, .shortcontents, .contents-heading, .shortcontents-heading, a.copiable-link, .nav-panel, .header, .navigation, hr, .chapter-toc, .section-toc, div.printindex, table[class$='-printindex'], table[class^='index-']";

pub static GUIDES: &[Guide] = &[
    // tessh's own two, from the installed tessh (`tessh --guide`).
    g(&["tessh"], "tessh guide", "MIT", "https://github.com/benwbooth/tessera/blob/main/LICENSE", &["tessh:"], "", "tessh").id("tessh"),
    g(&["tessh"], "Advanced tessh Scripting Guide", "Public domain (the ABS Guide, ported)", "https://github.com/benwbooth/tessera/tree/main/tessh/doc/advanced",
        &["tessh:advanced"], "", "tessh").id("tessh-advanced"),
    gi(&["python", "python3"], "The Python Tutorial", "PSF License 2.0", "https://docs.python.org/3/license.html",
        ("https://docs.python.org/3/tutorial/index.html", "div.toctree-wrapper li.toctree-l1 > a"), "div.body", "python"),
    g(&["rust"], "The Rust Programming Language", "MIT or Apache-2.0", "https://github.com/rust-lang/book/blob/main/LICENSE-MIT",
        &["https://doc.rust-lang.org/book/print.html"], "main", "rust").drop(".boring, .buttons"),
    g(&["bash", "sh"], "Bash Reference Manual", "GNU FDL 1.3", "https://www.gnu.org/licenses/fdl-1.3.html",
        &["https://www.gnu.org/software/bash/manual/bash.html | https://tiswww.case.edu/php/chet/bash/bashref.html"], "body", "bash").drop(GNU_DROP),
    gi(&["zsh"], "A User's Guide to the Z-Shell", "Freely distributable (© Peter Stephenson)", "https://zsh.sourceforge.io/Guide/zshguide.html",
        ("https://zsh.sourceforge.io/Guide/zshguide.html", "a[href^='zshguide0']"), "body", "bash"),
    g(&["fish"], "fish Tutorial", "GPL-2.0", "https://fishshell.com/docs/current/license.html",
        &["https://fishshell.com/docs/current/tutorial.html"], "div.body, div[role=main]", "fish"),
    g(&["lua"], "Lua 5.4 Reference Manual", "MIT (Lua license)", "https://www.lua.org/license.html",
        &["https://www.lua.org/manual/5.4/manual.html"], "body", "lua"),
    g(&["go", "golang"], "Effective Go and the Go Spec", "CC BY 4.0 (code BSD-3-Clause)", "https://go.dev/copyright",
        &["https://go.dev/doc/effective_go", "https://go.dev/ref/spec"], "#main-content article, article, main", "go"),
    g(&["javascript", "node", "js"], "MDN JavaScript Guide", "CC BY-SA 2.5", "https://developer.mozilla.org/en-US/docs/MDN/Writing_guidelines/Attrib_copyright_license", &[
        "https://raw.githubusercontent.com/mdn/content/main/files/en-us/web/javascript/guide/introduction/index.md",
        "https://raw.githubusercontent.com/mdn/content/main/files/en-us/web/javascript/guide/grammar_and_types/index.md",
        "https://raw.githubusercontent.com/mdn/content/main/files/en-us/web/javascript/guide/control_flow_and_error_handling/index.md",
        "https://raw.githubusercontent.com/mdn/content/main/files/en-us/web/javascript/guide/loops_and_iteration/index.md",
        "https://raw.githubusercontent.com/mdn/content/main/files/en-us/web/javascript/guide/functions/index.md",
        "https://raw.githubusercontent.com/mdn/content/main/files/en-us/web/javascript/guide/expressions_and_operators/index.md",
        "https://raw.githubusercontent.com/mdn/content/main/files/en-us/web/javascript/guide/numbers_and_strings/index.md",
        "https://raw.githubusercontent.com/mdn/content/main/files/en-us/web/javascript/guide/regular_expressions/index.md",
        "https://raw.githubusercontent.com/mdn/content/main/files/en-us/web/javascript/guide/indexed_collections/index.md",
        "https://raw.githubusercontent.com/mdn/content/main/files/en-us/web/javascript/guide/keyed_collections/index.md",
        "https://raw.githubusercontent.com/mdn/content/main/files/en-us/web/javascript/guide/working_with_objects/index.md",
        "https://raw.githubusercontent.com/mdn/content/main/files/en-us/web/javascript/guide/using_classes/index.md",
        "https://raw.githubusercontent.com/mdn/content/main/files/en-us/web/javascript/guide/using_promises/index.md",
        "https://raw.githubusercontent.com/mdn/content/main/files/en-us/web/javascript/guide/iterators_and_generators/index.md",
        "https://raw.githubusercontent.com/mdn/content/main/files/en-us/web/javascript/guide/modules/index.md",
    ], "", "javascript"),
    g(&["typescript", "tsx", "ts"], "The TypeScript Handbook", "CC BY 4.0", "https://github.com/microsoft/TypeScript-Website/blob/v2/LICENSE", &[
        "https://www.typescriptlang.org/docs/handbook/intro.html",
        "https://www.typescriptlang.org/docs/handbook/2/basic-types.html",
        "https://www.typescriptlang.org/docs/handbook/2/everyday-types.html",
        "https://www.typescriptlang.org/docs/handbook/2/narrowing.html",
        "https://www.typescriptlang.org/docs/handbook/2/functions.html",
        "https://www.typescriptlang.org/docs/handbook/2/objects.html",
        "https://www.typescriptlang.org/docs/handbook/2/generics.html",
        "https://www.typescriptlang.org/docs/handbook/2/keyof-types.html",
        "https://www.typescriptlang.org/docs/handbook/2/typeof-types.html",
        "https://www.typescriptlang.org/docs/handbook/2/conditional-types.html",
        "https://www.typescriptlang.org/docs/handbook/2/mapped-types.html",
        "https://www.typescriptlang.org/docs/handbook/2/classes.html",
        "https://www.typescriptlang.org/docs/handbook/2/modules.html",
    ], "article", "typescript"),
    g(&["c"], "GNU C Language Intro and Reference Manual", "GNU FDL 1.3", "https://www.gnu.org/licenses/fdl-1.3.html",
        &["https://www.gnu.org/software/c-intro-and-ref/manual/c-intro-and-ref.html"], "body", "c").drop(GNU_DROP),
    gi(&["cpp", "c++", "cxx"], "LearnCpp.com", "© LearnCpp.com; read from the site, not redistributed", "https://www.learncpp.com/about/",
        ("https://www.learncpp.com/", ".lessontable-row-title a"), "div.entry-content, article", "cpp")
        .id("learncpp").chapters(".lessontable-header-title", "C++ Updates")
        .renames(&[("p.cpp-section", "h2"), ("p.cpp-note-title", "p>strong"), ("div.cpp-note", "blockquote")])
        .drop(".prevnext, .wpd-wrapper, #comments, .comments-area, .cpp-ad, .header-image, .sharedaddy, ins, .adsbygoogle"),
    g(&["cpp", "c++", "cxx"], "C++ language (cppreference.com)", "CC BY-SA 3.0", "https://en.cppreference.com/w/Cppreference:Copyright/CC-BY-SA", &[
        "https://en.cppreference.com/w/cpp/language/basic_concepts",
        "https://en.cppreference.com/w/cpp/language/types",
        "https://en.cppreference.com/w/cpp/language/declarations",
        "https://en.cppreference.com/w/cpp/language/initialization",
        "https://en.cppreference.com/w/cpp/language/expressions",
        "https://en.cppreference.com/w/cpp/language/statements",
        "https://en.cppreference.com/w/cpp/language/functions",
        "https://en.cppreference.com/w/cpp/language/lambda",
        "https://en.cppreference.com/w/cpp/language/classes",
        "https://en.cppreference.com/w/cpp/language/templates",
        "https://en.cppreference.com/w/cpp/language/exceptions",
    ], "#mw-content-text", "cpp").drop(".t-navbar, .editsection, #toc, .printfooter").id("cppreference"),
    gi(&["java"], "Learn Java: Language Basics (dev.java)", "© Oracle; read from dev.java, not redistributed", "https://www.oracle.com/legal/copyright.html",
        ("https://dev.java/learn/language/constructs/basics/", "main a[href^='/learn/language/constructs/basics/']"), "main", "java").code_sel("readonly-code-editor").drop(".learn-lesson-navigation, .learn-breadcrumb")
        .pages(&["https://dev.java/learn/language/constructs/basics/"]),
    gi(&["ruby"], "Ruby Syntax (docs.ruby-lang.org)", "Ruby License or BSD-2-Clause", "https://www.ruby-lang.org/en/about/license.txt",
        ("https://docs.ruby-lang.org/en/master/syntax_rdoc.html", "main a[href^='syntax/']"), "main", "ruby").drop("#nav-mobile-panel, header, nav, footer")
        .pages(&["https://www.ruby-lang.org/en/documentation/quickstart/", "https://www.ruby-lang.org/en/documentation/quickstart/2/", "https://www.ruby-lang.org/en/documentation/quickstart/3/", "https://www.ruby-lang.org/en/documentation/quickstart/4/"]),
    g(&["perl"], "Perl documentation (perlintro and friends)", "Artistic License or GPL", "https://perldoc.perl.org/perlartistic", &[
        "https://perldoc.perl.org/perlintro", "https://perldoc.perl.org/perldata", "https://perldoc.perl.org/perlsyn",
        "https://perldoc.perl.org/perlop", "https://perldoc.perl.org/perlsub", "https://perldoc.perl.org/perlreftut",
        "https://perldoc.perl.org/perlretut", "https://perldoc.perl.org/perlootut",
    ], "#perldocdiv, main", "perl").drop("a.permalink, h1#toc, h1#toc + ul, #more, #breadcrumbs"),
    g(&["php", "php-only"], "PHP Manual: Getting Started and Language Reference", "CC BY 3.0", "https://www.php.net/manual/en/copyright.php", &[
        "https://www.php.net/manual/en/introduction.php", "https://www.php.net/manual/en/tutorial.firstpage.php",
        "https://www.php.net/manual/en/tutorial.useful.php", "https://www.php.net/manual/en/tutorial.forms.php",
        "https://www.php.net/manual/en/language.basic-syntax.php", "https://www.php.net/manual/en/language.types.intro.php",
        "https://www.php.net/manual/en/language.variables.basics.php", "https://www.php.net/manual/en/language.operators.php",
        "https://www.php.net/manual/en/language.control-structures.php", "https://www.php.net/manual/en/functions.user-defined.php",
        "https://www.php.net/manual/en/language.oop5.basic.php",
    ], "#layout-content", "php"),
    g(&["r"], "An Introduction to R", "Verbatim copying permitted (R Core Team)", "https://cran.r-project.org/doc/manuals/r-release/R-intro.html",
        &["https://cran.r-project.org/doc/manuals/r-release/R-intro.html"], "body", "r").drop(GNU_DROP),
    g(&["julia"], "The Julia Manual", "MIT", "https://github.com/JuliaLang/julia/blob/master/LICENSE.md", &[
        "https://docs.julialang.org/en/v1/manual/getting-started/", "https://docs.julialang.org/en/v1/manual/variables/",
        "https://docs.julialang.org/en/v1/manual/integers-and-floating-point-numbers/", "https://docs.julialang.org/en/v1/manual/mathematical-operations/",
        "https://docs.julialang.org/en/v1/manual/strings/", "https://docs.julialang.org/en/v1/manual/functions/",
        "https://docs.julialang.org/en/v1/manual/control-flow/", "https://docs.julialang.org/en/v1/manual/variables-and-scoping/",
        "https://docs.julialang.org/en/v1/manual/types/", "https://docs.julialang.org/en/v1/manual/methods/",
        "https://docs.julialang.org/en/v1/manual/constructors/", "https://docs.julialang.org/en/v1/manual/modules/",
        "https://docs.julialang.org/en/v1/manual/arrays/",
    ], "#documenter-page, article", "julia"),
    g(&["elixir", "iex"], "Elixir Getting Started", "Apache-2.0", "https://github.com/elixir-lang/elixir/blob/main/LICENSE", &[
        "https://raw.githubusercontent.com/elixir-lang/elixir/main/lib/elixir/pages/getting-started/introduction.md",
        "https://raw.githubusercontent.com/elixir-lang/elixir/main/lib/elixir/pages/getting-started/basic-types.md",
        "https://raw.githubusercontent.com/elixir-lang/elixir/main/lib/elixir/pages/getting-started/lists-and-tuples.md",
        "https://raw.githubusercontent.com/elixir-lang/elixir/main/lib/elixir/pages/getting-started/pattern-matching.md",
        "https://raw.githubusercontent.com/elixir-lang/elixir/main/lib/elixir/pages/getting-started/case-cond-and-if.md",
        "https://raw.githubusercontent.com/elixir-lang/elixir/main/lib/elixir/pages/getting-started/anonymous-functions.md",
        "https://raw.githubusercontent.com/elixir-lang/elixir/main/lib/elixir/pages/getting-started/binaries-strings-and-charlists.md",
        "https://raw.githubusercontent.com/elixir-lang/elixir/main/lib/elixir/pages/getting-started/keywords-and-maps.md",
        "https://raw.githubusercontent.com/elixir-lang/elixir/main/lib/elixir/pages/getting-started/modules-and-functions.md",
        "https://raw.githubusercontent.com/elixir-lang/elixir/main/lib/elixir/pages/getting-started/recursion.md",
        "https://raw.githubusercontent.com/elixir-lang/elixir/main/lib/elixir/pages/getting-started/enumerable-and-streams.md",
        "https://raw.githubusercontent.com/elixir-lang/elixir/main/lib/elixir/pages/getting-started/processes.md",
        "https://raw.githubusercontent.com/elixir-lang/elixir/main/lib/elixir/pages/getting-started/structs.md",
        "https://raw.githubusercontent.com/elixir-lang/elixir/main/lib/elixir/pages/getting-started/protocols.md",
        "https://raw.githubusercontent.com/elixir-lang/elixir/main/lib/elixir/pages/getting-started/comprehensions.md",
    ], "", "elixir"),
    g(&["erlang"], "Getting Started with Erlang", "Apache-2.0", "https://www.erlang.org/doc/system/license.html", &[
        "https://www.erlang.org/doc/system/getting_started.html", "https://www.erlang.org/doc/system/seq_prog.html",
        "https://www.erlang.org/doc/system/conc_prog.html", "https://www.erlang.org/doc/system/robustness.html",
        "https://www.erlang.org/doc/system/records_macros.html",
    ], "#content, main", "erlang"),
    g(&["groovy"], "Groovy Language Documentation", "Apache-2.0", "https://groovy-lang.org/", &[
        "https://groovy-lang.org/syntax.html", "https://groovy-lang.org/operators.html", "https://groovy-lang.org/objectorientation.html",
        "https://groovy-lang.org/closures.html", "https://groovy-lang.org/semantics.html",
    ], "#content", "groovy").drop("#toc"),
    g(&["powershell", "pwsh"], "PowerShell 101", "CC BY 4.0", "https://github.com/MicrosoftDocs/PowerShell-Docs/blob/main/LICENSE", &[
        "https://raw.githubusercontent.com/MicrosoftDocs/PowerShell-Docs/main/reference/docs-conceptual/learn/ps101/01-getting-started.md",
        "https://raw.githubusercontent.com/MicrosoftDocs/PowerShell-Docs/main/reference/docs-conceptual/learn/ps101/02-help-system.md",
        "https://raw.githubusercontent.com/MicrosoftDocs/PowerShell-Docs/main/reference/docs-conceptual/learn/ps101/03-discovering-objects.md",
        "https://raw.githubusercontent.com/MicrosoftDocs/PowerShell-Docs/main/reference/docs-conceptual/learn/ps101/04-pipelines.md",
        "https://raw.githubusercontent.com/MicrosoftDocs/PowerShell-Docs/main/reference/docs-conceptual/learn/ps101/05-formatting-aliases-providers-comparison.md",
        "https://raw.githubusercontent.com/MicrosoftDocs/PowerShell-Docs/main/reference/docs-conceptual/learn/ps101/06-flow-control.md",
        "https://raw.githubusercontent.com/MicrosoftDocs/PowerShell-Docs/main/reference/docs-conceptual/learn/ps101/09-functions.md",
        "https://raw.githubusercontent.com/MicrosoftDocs/PowerShell-Docs/main/reference/docs-conceptual/learn/ps101/10-script-modules.md",
    ], "", "powershell"),
    g(&["nu", "nushell"], "The Nushell Book", "MIT", "https://github.com/nushell/nushell.github.io/blob/main/LICENSE", &[
        "https://raw.githubusercontent.com/nushell/nushell.github.io/main/book/quick_tour.md",
        "https://raw.githubusercontent.com/nushell/nushell.github.io/main/book/moving_around.md",
        "https://raw.githubusercontent.com/nushell/nushell.github.io/main/book/thinking_in_nu.md",
        "https://raw.githubusercontent.com/nushell/nushell.github.io/main/book/types_of_data.md",
        "https://raw.githubusercontent.com/nushell/nushell.github.io/main/book/pipelines.md",
        "https://raw.githubusercontent.com/nushell/nushell.github.io/main/book/working_with_strings.md",
        "https://raw.githubusercontent.com/nushell/nushell.github.io/main/book/working_with_lists.md",
        "https://raw.githubusercontent.com/nushell/nushell.github.io/main/book/working_with_tables.md",
        "https://raw.githubusercontent.com/nushell/nushell.github.io/main/book/custom_commands.md",
        "https://raw.githubusercontent.com/nushell/nushell.github.io/main/book/variables.md",
        "https://raw.githubusercontent.com/nushell/nushell.github.io/main/book/control_flow.md",
        "https://raw.githubusercontent.com/nushell/nushell.github.io/main/book/scripts.md",
    ], "", "nu"),
    gi(&["tcl"], "Tcl Tutorial", "Tcl/Tk license (BSD-style)", "https://www.tcl-lang.org/software/tcltk/license.html",
        ("https://www.tcl-lang.org/man/tcl8.5/tutorial/tcltutorial.html", "a[href^='Tcl']"), "body", "tcl"),
    g(&["awk"], "GAWK: Effective AWK Programming", "GNU FDL 1.3", "https://www.gnu.org/licenses/fdl-1.3.html",
        &["https://www.gnu.org/software/gawk/manual/gawk.html"], "body", "awk").drop(GNU_DROP),
    g(&["jq"], "jq Manual", "CC BY 3.0 (code MIT)", "https://github.com/jqlang/jq/blob/master/COPYING",
        &["https://jqlang.org/manual/"], "main", "jq"),
    g(&["nix"], "Nix language basics (nix.dev)", "CC BY-SA 4.0", "https://github.com/NixOS/nix.dev/blob/master/LICENSE",
        &["https://nix.dev/tutorials/nix-language"], "article, div[role=main]", "nix"),
    gi(&["octave", "matlab"], "GNU Octave: Getting Started", "Verbatim copying permitted (GNU Octave manual)", "https://docs.octave.org/latest/",
        ("https://docs.octave.org/latest/Getting-Started.html", "ul.mini-toc a, ul.menu a"), "body", "matlab").drop(GNU_DROP),
    g(&["sql", "sqlite", "duckdb"], "SQL As Understood By SQLite", "Public domain", "https://www.sqlite.org/copyright.html", &[
        "https://www.sqlite.org/lang_select.html", "https://www.sqlite.org/lang_insert.html", "https://www.sqlite.org/lang_update.html",
        "https://www.sqlite.org/lang_delete.html", "https://www.sqlite.org/lang_createtable.html", "https://www.sqlite.org/lang_expr.html",
        "https://www.sqlite.org/lang_with.html", "https://www.sqlite.org/lang_aggfunc.html", "https://www.sqlite.org/lang_corefunc.html",
        "https://www.sqlite.org/windowfunctions.html", "https://www.sqlite.org/lang_transaction.html",
    ], "div.fancy, body", "sql").drop(".nosearch, .menu, .tooltip, svg"),
    gi(&["haskell", "ghc"], "A Gentle Introduction to Haskell 98", "Free to copy (Hudak, Peterson, Fasel)", "https://www.haskell.org/tutorial/",
        ("https://www.haskell.org/tutorial/", "a[href$='.html']"), "body", "haskell").code_sel("tt:has(br)"),
    g(&["ocaml", "ocaml_interface"], "The OCaml Manual: An Introduction to OCaml", "CC BY-SA 4.0", "https://ocaml.org/manual/latest/", &[
        "https://ocaml.org/manual/latest/coreexamples.html", "https://ocaml.org/manual/latest/moduleexamples.html",
        "https://ocaml.org/manual/latest/objectexamples.html", "https://ocaml.org/manual/latest/lablexamples.html",
        "https://ocaml.org/manual/latest/polymorphism.html",
    ], "div.content, main, body", "ocaml").drop("#part-menu, .navbar, header, footer"),
    gi(&["racket"], "The Racket Guide: Racket Essentials", "MIT or Apache-2.0", "https://download.racket-lang.org/license.html",
        ("https://docs.racket-lang.org/guide/to-scheme.html", "table.toctable a, .toclink"), "div.maincolumn", "scheme").drop(".tocset, .navsettop, .navsetbottom, .tocview, .tocsub, .RForeground, .button-group").code_sel(".SCodeFlow, .SInteraction").pages(&["https://docs.racket-lang.org/guide/to-scheme.html"]),
    // (The one-page manual, its two introductory chapters: one request, so
    // the Internet Archive's copy works too when gnu.org doesn't answer.)
    g(&["scheme", "guile"], "Guile: Hello Guile! and Hello Scheme!", "GNU FDL 1.3", "https://www.gnu.org/licenses/fdl-1.3.html",
        &["https://www.gnu.org/software/guile/manual/guile.html"], "div#Hello-Guile_0021, div#Hello-Scheme_0021", "scheme").all().drop(GNU_DROP),
    g(&["common-lisp", "lisp", "commonlisp"], "The Common Lisp Cookbook", "See the Cookbook's license", "https://github.com/LispCookbook/cl-cookbook", &[
        "https://raw.githubusercontent.com/LispCookbook/cl-cookbook/master/getting-started.md",
        "https://raw.githubusercontent.com/LispCookbook/cl-cookbook/master/data-structures.md",
        "https://raw.githubusercontent.com/LispCookbook/cl-cookbook/master/strings.md",
        "https://raw.githubusercontent.com/LispCookbook/cl-cookbook/master/functions.md",
        "https://raw.githubusercontent.com/LispCookbook/cl-cookbook/master/iteration.md",
        "https://raw.githubusercontent.com/LispCookbook/cl-cookbook/master/clos.md",
        "https://raw.githubusercontent.com/LispCookbook/cl-cookbook/master/error_handling.md",
    ], "", "common-lisp"),
    g(&["dart"], "Dart Language Tour", "CC BY 4.0 (code BSD-3-Clause)", "https://dart.dev/terms", &[
        "https://dart.dev/language", "https://dart.dev/language/variables", "https://dart.dev/language/operators",
        "https://dart.dev/language/built-in-types", "https://dart.dev/language/records", "https://dart.dev/language/collections",
        "https://dart.dev/language/functions", "https://dart.dev/language/loops", "https://dart.dev/language/branches",
        "https://dart.dev/language/classes", "https://dart.dev/language/async",
    ], "main article, main", "dart"),
    g(&["swift"], "The Swift Programming Language", "Apache-2.0 with Runtime Library Exception", "https://github.com/swiftlang/swift-book/blob/main/LICENSE.txt", &[
        "https://raw.githubusercontent.com/swiftlang/swift-book/main/TSPL.docc/GuidedTour/GuidedTour.md",
        "https://raw.githubusercontent.com/swiftlang/swift-book/main/TSPL.docc/LanguageGuide/TheBasics.md",
        "https://raw.githubusercontent.com/swiftlang/swift-book/main/TSPL.docc/LanguageGuide/BasicOperators.md",
        "https://raw.githubusercontent.com/swiftlang/swift-book/main/TSPL.docc/LanguageGuide/StringsAndCharacters.md",
        "https://raw.githubusercontent.com/swiftlang/swift-book/main/TSPL.docc/LanguageGuide/CollectionTypes.md",
        "https://raw.githubusercontent.com/swiftlang/swift-book/main/TSPL.docc/LanguageGuide/ControlFlow.md",
        "https://raw.githubusercontent.com/swiftlang/swift-book/main/TSPL.docc/LanguageGuide/Functions.md",
        "https://raw.githubusercontent.com/swiftlang/swift-book/main/TSPL.docc/LanguageGuide/Closures.md",
        "https://raw.githubusercontent.com/swiftlang/swift-book/main/TSPL.docc/LanguageGuide/Enumerations.md",
        "https://raw.githubusercontent.com/swiftlang/swift-book/main/TSPL.docc/LanguageGuide/ClassesAndStructures.md",
    ], "", "swift"),
    g(&["zig"], "Zig Language Reference", "MIT", "https://github.com/ziglang/zig/blob/master/LICENSE",
        &["https://ziglang.org/documentation/master/"], "#contents, main", "zig").drop("#toc, #navigation, a.hdr"),
    g(&["nim"], "Nim Tutorial", "MIT", "https://github.com/nim-lang/Nim/blob/devel/copying.txt", &[
        "https://nim-lang.org/docs/tut1.html", "https://nim-lang.org/docs/tut2.html", "https://nim-lang.org/docs/tut3.html",
    ], "#content, .container", "nim").drop("#global-links, .twelve-columns.theme-select-wrapper, #searchInputDiv, .three.columns"),
    g(&["crystal"], "Crystal Tutorials: Language Introduction", "Apache-2.0", "https://github.com/crystal-lang/crystal-book/blob/master/LICENSE", &[
        "https://raw.githubusercontent.com/crystal-lang/crystal-book/master/docs/tutorials/basics/10_hello_world.md",
        "https://raw.githubusercontent.com/crystal-lang/crystal-book/master/docs/tutorials/basics/20_variables.md",
        "https://raw.githubusercontent.com/crystal-lang/crystal-book/master/docs/tutorials/basics/30_math.md",
        "https://raw.githubusercontent.com/crystal-lang/crystal-book/master/docs/tutorials/basics/40_strings.md",
        "https://raw.githubusercontent.com/crystal-lang/crystal-book/master/docs/tutorials/basics/50_control_flow.md",
        "https://raw.githubusercontent.com/crystal-lang/crystal-book/master/docs/tutorials/basics/60_methods.md",
    ], "", "crystal"),
    g(&["v"], "V Documentation", "MIT", "https://github.com/vlang/v/blob/master/LICENSE",
        &["https://raw.githubusercontent.com/vlang/v/master/doc/docs.md"], "", "v"),
    g(&["fsharp", "f#"], "F#: a tour and the language reference", "CC BY 4.0", "https://github.com/dotnet/docs/blob/main/LICENSE", &[
        "https://raw.githubusercontent.com/dotnet/docs/main/docs/fsharp/tour.md",
        "https://raw.githubusercontent.com/dotnet/docs/main/docs/fsharp/language-reference/values/index.md",
        "https://raw.githubusercontent.com/dotnet/docs/main/docs/fsharp/language-reference/functions/index.md",
        "https://raw.githubusercontent.com/dotnet/docs/main/docs/fsharp/language-reference/basic-types.md",
        "https://raw.githubusercontent.com/dotnet/docs/main/docs/fsharp/language-reference/literals.md",
        "https://raw.githubusercontent.com/dotnet/docs/main/docs/fsharp/language-reference/interpolated-strings.md",
        "https://raw.githubusercontent.com/dotnet/docs/main/docs/fsharp/language-reference/tuples.md",
        "https://raw.githubusercontent.com/dotnet/docs/main/docs/fsharp/language-reference/lists.md",
        "https://raw.githubusercontent.com/dotnet/docs/main/docs/fsharp/language-reference/arrays.md",
        "https://raw.githubusercontent.com/dotnet/docs/main/docs/fsharp/language-reference/options.md",
        "https://raw.githubusercontent.com/dotnet/docs/main/docs/fsharp/language-reference/records.md",
        "https://raw.githubusercontent.com/dotnet/docs/main/docs/fsharp/language-reference/discriminated-unions.md",
        "https://raw.githubusercontent.com/dotnet/docs/main/docs/fsharp/language-reference/pattern-matching.md",
        "https://raw.githubusercontent.com/dotnet/docs/main/docs/fsharp/language-reference/match-expressions.md",
        "https://raw.githubusercontent.com/dotnet/docs/main/docs/fsharp/language-reference/conditional-expressions-if-then-else.md",
        "https://raw.githubusercontent.com/dotnet/docs/main/docs/fsharp/language-reference/loops-for-in-expression.md",
        "https://raw.githubusercontent.com/dotnet/docs/main/docs/fsharp/language-reference/classes.md",
        "https://raw.githubusercontent.com/dotnet/docs/main/docs/fsharp/language-reference/interfaces.md",
        "https://raw.githubusercontent.com/dotnet/docs/main/docs/fsharp/language-reference/computation-expressions.md",
        "https://raw.githubusercontent.com/dotnet/docs/main/docs/fsharp/language-reference/async-expressions.md",
    ], "", "fsharp"),
    g(&["c-sharp", "csharp", "c#", "cs"], "C#: a tour and the fundamentals", "CC BY 4.0", "https://github.com/dotnet/docs/blob/main/LICENSE", &[
        "https://raw.githubusercontent.com/dotnet/docs/main/docs/csharp/tour-of-csharp/overview.md",
        "https://raw.githubusercontent.com/dotnet/docs/main/docs/csharp/tour-of-csharp/strategy.md",
        "https://raw.githubusercontent.com/dotnet/docs/main/docs/csharp/fundamentals/program-structure/index.md",
        "https://raw.githubusercontent.com/dotnet/docs/main/docs/csharp/fundamentals/program-structure/top-level-statements.md",
        "https://raw.githubusercontent.com/dotnet/docs/main/docs/csharp/fundamentals/program-structure/namespaces.md",
        "https://raw.githubusercontent.com/dotnet/docs/main/docs/csharp/fundamentals/types/index.md",
        "https://raw.githubusercontent.com/dotnet/docs/main/docs/csharp/fundamentals/types/built-in-types.md",
        "https://raw.githubusercontent.com/dotnet/docs/main/docs/csharp/fundamentals/types/classes.md",
        "https://raw.githubusercontent.com/dotnet/docs/main/docs/csharp/fundamentals/types/structs.md",
        "https://raw.githubusercontent.com/dotnet/docs/main/docs/csharp/fundamentals/types/records.md",
        "https://raw.githubusercontent.com/dotnet/docs/main/docs/csharp/fundamentals/types/interfaces.md",
        "https://raw.githubusercontent.com/dotnet/docs/main/docs/csharp/fundamentals/types/enums.md",
        "https://raw.githubusercontent.com/dotnet/docs/main/docs/csharp/fundamentals/types/tuples.md",
        "https://raw.githubusercontent.com/dotnet/docs/main/docs/csharp/fundamentals/types/generics.md",
        "https://raw.githubusercontent.com/dotnet/docs/main/docs/csharp/fundamentals/types/delegates-lambdas.md",
        "https://raw.githubusercontent.com/dotnet/docs/main/docs/csharp/fundamentals/types/conversions.md",
        "https://raw.githubusercontent.com/dotnet/docs/main/docs/csharp/fundamentals/object-oriented/index.md",
        "https://raw.githubusercontent.com/dotnet/docs/main/docs/csharp/fundamentals/object-oriented/objects.md",
        "https://raw.githubusercontent.com/dotnet/docs/main/docs/csharp/fundamentals/object-oriented/inheritance.md",
        "https://raw.githubusercontent.com/dotnet/docs/main/docs/csharp/fundamentals/object-oriented/polymorphism.md",
    ], "", "c-sharp"),
    g(&["kotlin"], "Kotlin Tour", "Apache-2.0", "https://github.com/JetBrains/kotlin-web-site/blob/master/LICENSE", &[
        "https://raw.githubusercontent.com/JetBrains/kotlin-web-site/master/docs/topics/tour/kotlin-tour-hello-world.md",
        "https://raw.githubusercontent.com/JetBrains/kotlin-web-site/master/docs/topics/tour/kotlin-tour-basic-types.md",
        "https://raw.githubusercontent.com/JetBrains/kotlin-web-site/master/docs/topics/tour/kotlin-tour-collections.md",
        "https://raw.githubusercontent.com/JetBrains/kotlin-web-site/master/docs/topics/tour/kotlin-tour-control-flow.md",
        "https://raw.githubusercontent.com/JetBrains/kotlin-web-site/master/docs/topics/tour/kotlin-tour-functions.md",
        "https://raw.githubusercontent.com/JetBrains/kotlin-web-site/master/docs/topics/tour/kotlin-tour-classes.md",
        "https://raw.githubusercontent.com/JetBrains/kotlin-web-site/master/docs/topics/tour/kotlin-tour-null-safety.md",
    ], "", "kotlin"),
    g(&["scala"], "Scala 3 Book", "CC BY-SA 4.0", "https://github.com/scala/docs.scala-lang/blob/main/LICENSE.md", &[
        "https://docs.scala-lang.org/scala3/book/introduction.html", "https://docs.scala-lang.org/scala3/book/taste-intro.html",
        "https://docs.scala-lang.org/scala3/book/taste-hello-world.html", "https://docs.scala-lang.org/scala3/book/taste-vars-data-types.html",
        "https://docs.scala-lang.org/scala3/book/taste-control-structures.html", "https://docs.scala-lang.org/scala3/book/taste-modeling.html",
        "https://docs.scala-lang.org/scala3/book/taste-methods.html", "https://docs.scala-lang.org/scala3/book/taste-functions.html",
        "https://docs.scala-lang.org/scala3/book/taste-objects.html", "https://docs.scala-lang.org/scala3/book/taste-collections.html",
    ], ".content-primary, main", "scala").drop(".tabsection > .nav-tab, .tabs-header"),
    g(&["clojure"], "Learn Clojure (clojure.org)", "CC BY-SA 3.0", "https://clojure.org/community/license", &[
        "https://clojure.org/guides/learn/clojure", "https://clojure.org/guides/learn/syntax", "https://clojure.org/guides/learn/functions",
        "https://clojure.org/guides/learn/sequential_colls", "https://clojure.org/guides/learn/hashed_colls", "https://clojure.org/guides/learn/flow",
        "https://clojure.org/guides/learn/namespaces",
    ], ".clj-content-container, main", "clojure").drop("#toc, .clj-prev-next-container"),
    g(&["prolog"], "SWI-Prolog Manual: Getting started", "BSD-2-Clause", "https://www.swi-prolog.org/license.html", &[
        "https://www.swi-prolog.org/pldoc/man?section=quickstart", "https://www.swi-prolog.org/pldoc/man?section=syntax",
    ], ".manual, #contents, body", "prolog").drop(".navhdr, #header, #footer, .menubar"),
    g(&["fortran"], "Fortran Quickstart Tutorial (fortran-lang.org)", "MIT", "https://github.com/fortran-lang/webpage/blob/main/LICENSE", &[
        "https://raw.githubusercontent.com/fortran-lang/webpage/main/source/learn/quickstart/hello_world.md",
        "https://raw.githubusercontent.com/fortran-lang/webpage/main/source/learn/quickstart/variables.md",
        "https://raw.githubusercontent.com/fortran-lang/webpage/main/source/learn/quickstart/arrays_strings.md",
        "https://raw.githubusercontent.com/fortran-lang/webpage/main/source/learn/quickstart/operators_control_flow.md",
        "https://raw.githubusercontent.com/fortran-lang/webpage/main/source/learn/quickstart/organising_code.md",
        "https://raw.githubusercontent.com/fortran-lang/webpage/main/source/learn/quickstart/derived_types.md",
    ], "", "fortran"),
    g(&["elvish"], "Tour of Elvish", "BSD-2-Clause", "https://github.com/elves/elvish/blob/master/LICENSE",
        &["https://elv.sh/learn/tour.html"], "article, main", "elvish"),
    g(&["fennel"], "Fennel Tutorial", "MIT", "https://git.sr.ht/~technomancy/fennel/tree/main/item/LICENSE",
        &["https://fennel-lang.org/tutorial"], "main, body", "fennel"),
    gi(&["forth"], "Gforth Tutorial", "GNU FDL 1.3", "https://www.gnu.org/licenses/fdl-1.3.html",
        ("https://www.complang.tuwien.ac.at/forth/gforth/Docs-html/Tutorial.html", "ul.mini-toc a, ul.menu a"), "body", "forth").drop(GNU_DROP),
    g(&["hare"], "Hare: An introduction", "CC BY-ND 4.0", "https://harelang.org/", &["https://harelang.org/tutorials/introduction"], "main", "hare"),
    gi(&["janet", "janet-simple"], "Janet Documentation", "MIT", "https://github.com/janet-lang/janet/blob/master/LICENSE",
        ("https://janet-lang.org/docs/index.html", "nav a[href^='/docs/'], .toc a[href^='/docs/']"), "main, .main", "janet-simple").pages(&["https://janet-lang.org/docs/index.html"]),
    g(&["koka"], "The Koka Programming Language", "Apache-2.0", "https://github.com/koka-lang/koka/blob/master/LICENSE",
        &["https://koka-lang.github.io/koka/doc/book.html"], "body", "koka").drop(".toc, .sidepanel, #sidetoc"),
    gi(&["lean"], "Functional Programming in Lean", "CC BY 4.0", "https://lean-lang.org/functional_programming_in_lean/",
        ("https://lean-lang.org/functional_programming_in_lean/", "a[href*='/#']"), "main", "lean"),
    g(&["luau"], "Luau: Syntax and Types", "MIT", "https://github.com/luau-lang/site/blob/master/LICENSE", &[
        "https://raw.githubusercontent.com/luau-lang/site/master/src/content/docs/getting-started/intro.md",
        "https://raw.githubusercontent.com/luau-lang/site/master/src/content/docs/getting-started/syntax.md",
        "https://raw.githubusercontent.com/luau-lang/site/master/src/content/docs/types/overview.md",
        "https://raw.githubusercontent.com/luau-lang/site/master/src/content/docs/types/basic-types.md",
        "https://raw.githubusercontent.com/luau-lang/site/master/src/content/docs/types/tables.md",
        "https://raw.githubusercontent.com/luau-lang/site/master/src/content/docs/types/generics.md",
        "https://raw.githubusercontent.com/luau-lang/site/master/src/content/docs/types/unions-and-intersections.md",
        "https://raw.githubusercontent.com/luau-lang/site/master/src/content/docs/types/refinements.md",
        "https://raw.githubusercontent.com/luau-lang/site/master/src/content/docs/types/object-oriented-programs.md",
    ], "", "luau"),
    g(&["odin"], "Odin Overview", "BSD-3-Clause", "https://github.com/odin-lang/Odin/blob/master/LICENSE",
        &["https://odin-lang.org/docs/overview/"], "main, article", "odin").drop("#TableOfContents, nav"),
    gi(&["pascal"], "Free Pascal Reference Guide", "Free Pascal documentation license (free to copy)", "https://www.freepascal.org/docs.html",
        ("https://www.freepascal.org/docs-html/ref/ref.html", "a[href^='refch'], a[href^='refse']"), "body", "pascal").code_sel("div.verbatim"),
    gi(&["ada"], "Introduction to Ada (learn.adacore.com)", "CC BY-SA 4.0", "https://github.com/AdaCore/learn/blob/main/LICENSE",
        ("https://learn.adacore.com/courses/intro-to-ada/index.html", "div.toctree-wrapper li.toctree-l1 > a"), "article[role=main], div[role=main], div.body", "ada").drop(".skip-to-content, .related-pages, .back-to-top, .content-icon-container"),
    g(&["gleam"], "The Gleam Language Tour", "Apache-2.0", "https://github.com/gleam-lang/language-tour/blob/main/LICENCE",
        &["https://tour.gleam.run/everything/"], "main, body", "gleam"),
    g(&["koto"], "The Koto Language Guide", "MIT", "https://github.com/koto-lang/koto/blob/main/LICENSE",
        &["https://koto.dev/docs/next/language/"], "main, article", "koto"),
    gi(&["inko"], "The Inko Manual", "MPL-2.0", "https://github.com/inko-lang/inko/blob/main/LICENSE",
        ("https://docs.inko-lang.org/manual/latest/", "a[href^='getting-started/'], a[href^='references/syntax'], a[href^='references/']"), "main, article", "inko"),
    gi(&["amber"], "Amber Documentation", "GPL-3.0", "https://github.com/amber-lang/amber/blob/master/LICENSE",
        ("https://docs.amber-lang.com/", "a[href^='/getting_started'], a[href^='/basic_syntax'], a[href^='/advanced_syntax']"), "main, article", "amber").drop("[class*='SideBar-module']"),
    g(&["uxntal"], "Uxntal (XXIIVV wiki)", "CC BY-NC-SA 4.0; read from the wiki, not redistributed", "https://wiki.xxiivv.com/site/about.html",
        &[
        "https://wiki.xxiivv.com/site/uxntal.html", "https://wiki.xxiivv.com/site/uxntal_notation.html", "https://wiki.xxiivv.com/site/uxntal_numbers.html",
        "https://wiki.xxiivv.com/site/uxntal_labels.html", "https://wiki.xxiivv.com/site/uxntal_stacks.html", "https://wiki.xxiivv.com/site/uxntal_memory.html",
        "https://wiki.xxiivv.com/site/uxntal_opcodes.html", "https://wiki.xxiivv.com/site/uxntal_macros.html", "https://wiki.xxiivv.com/site/uxntal_devices.html",
        "https://wiki.xxiivv.com/site/uxntal_reference.html",
    ], "main", "uxntal"),
    g(&["nasm"], "NASM Manual: The NASM Language", "BSD-2-Clause", "https://www.nasm.us/doc/nasmdoc1.html",
        &["https://www.nasm.us/doc/nasmdoc2.html", "https://www.nasm.us/doc/nasmdoc3.html"], "body", "nasm").drop(".navbar, #navbar, .nav, ul.nav"),
    g(&["gas"], "Using as: Syntax", "GNU FDL 1.3", "https://www.gnu.org/licenses/fdl-1.3.html", &[
        "https://sourceware.org/binutils/docs/as/Syntax.html", "https://sourceware.org/binutils/docs/as/Preprocessing.html",
        "https://sourceware.org/binutils/docs/as/Whitespace.html", "https://sourceware.org/binutils/docs/as/Comments.html",
        "https://sourceware.org/binutils/docs/as/Symbol-Intro.html", "https://sourceware.org/binutils/docs/as/Statements.html",
        "https://sourceware.org/binutils/docs/as/Constants.html", "https://sourceware.org/binutils/docs/as/Sections.html",
        "https://sourceware.org/binutils/docs/as/Symbols.html", "https://sourceware.org/binutils/docs/as/Expressions.html",
    ], "body", "gas").drop(GNU_DROP),
    g(&["llvm"], "LLVM Language Reference Manual", "Apache-2.0 with LLVM Exception", "https://llvm.org/LICENSE.txt",
        &["https://llvm.org/docs/LangRef.html"], "div[role=main], div.body", "llvm").drop("#contents, .contents"),
    g(&["elisp"], "An Introduction to Programming in Emacs Lisp", "GNU FDL 1.3", "https://www.gnu.org/licenses/fdl-1.3.html",
        &["https://www.gnu.org/software/emacs/manual/html_mono/eintr.html"], "body", "elisp").drop(GNU_DROP),
    g(&["jsonnet"], "Jsonnet Tutorial", "Apache-2.0", "https://github.com/google/jsonnet/blob/master/LICENSE",
        &["https://jsonnet.org/learning/tutorial.html"], "body", "jsonnet").drop(".navbar, #navbar, .menu, .tab-header, .bigarrow, .tab-window-output, nav, footer, #header").code_sel(".tab-window-input"),
    g(&["nickel"], "The Nickel User Manual", "MIT", "https://github.com/tweag/nickel/blob/master/LICENSE", &[
        "https://raw.githubusercontent.com/tweag/nickel/master/doc/manual/introduction.md",
        "https://raw.githubusercontent.com/tweag/nickel/master/doc/manual/tutorial.md",
        "https://raw.githubusercontent.com/tweag/nickel/master/doc/manual/syntax.md",
        "https://raw.githubusercontent.com/tweag/nickel/master/doc/manual/merging.md",
        "https://raw.githubusercontent.com/tweag/nickel/master/doc/manual/contracts.md",
    ], "", "nickel"),
    g(&["dhall"], "Dhall Language Tour", "BSD-3-Clause", "https://github.com/dhall-lang/dhall-lang/blob/master/LICENSE",
        &["https://raw.githubusercontent.com/dhall-lang/dhall-lang/master/docs/tutorials/Language-Tour.md"], "", "dhall"),
    g(&["cue"], "A Tour of CUE", "Apache-2.0", "https://github.com/cue-lang/cuelang.org/blob/master/LICENSE", &[
        "https://cuelang.org/docs/tour/basics/json-superset/",
        "https://cuelang.org/docs/tour/basics/types-are-values/",
        "https://cuelang.org/docs/tour/basics/unification/",
        "https://cuelang.org/docs/tour/basics/constraints/",
        "https://cuelang.org/docs/tour/basics/definitions/",
        "https://cuelang.org/docs/tour/basics/validation/",
        "https://cuelang.org/docs/tour/basics/order-irrelevance/",
        "https://cuelang.org/docs/tour/basics/folding-structs/",
        "https://cuelang.org/docs/tour/types/types/",
        "https://cuelang.org/docs/tour/types/bottom/",
        "https://cuelang.org/docs/tour/types/top/",
        "https://cuelang.org/docs/tour/types/numbers/",
        "https://cuelang.org/docs/tour/types/stringlit/",
        "https://cuelang.org/docs/tour/types/stringraw/",
        "https://cuelang.org/docs/tour/types/bytes/",
        "https://cuelang.org/docs/tour/types/structs/",
        "https://cuelang.org/docs/tour/types/closed/",
        "https://cuelang.org/docs/tour/types/definitions/",
        "https://cuelang.org/docs/tour/types/disjunctions/",
        "https://cuelang.org/docs/tour/types/defaults/",
        "https://cuelang.org/docs/tour/types/sumstruct/",
        "https://cuelang.org/docs/tour/types/bounds/",
        "https://cuelang.org/docs/tour/types/bounddef/",
        "https://cuelang.org/docs/tour/types/lists/",
        "https://cuelang.org/docs/tour/types/templates/",
        "https://cuelang.org/docs/tour/references/scopes/",
        "https://cuelang.org/docs/tour/references/selectors/",
        "https://cuelang.org/docs/tour/references/aliases/",
        "https://cuelang.org/docs/tour/references/emit/",
        "https://cuelang.org/docs/tour/references/cycle/",
        "https://cuelang.org/docs/tour/references/cycleref/",
        "https://cuelang.org/docs/tour/references/hidden/",
        "https://cuelang.org/docs/tour/expressions/operators/",
        "https://cuelang.org/docs/tour/expressions/interpolation/",
        "https://cuelang.org/docs/tour/expressions/dynamic-fields/",
        "https://cuelang.org/docs/tour/expressions/conditional/",
        "https://cuelang.org/docs/tour/expressions/listcomp/",
        "https://cuelang.org/docs/tour/expressions/fieldcomp/",
        "https://cuelang.org/docs/tour/expressions/query-projection/",
        "https://cuelang.org/docs/tour/expressions/regexp/",
        "https://cuelang.org/docs/tour/expressions/coalesce/",
        "https://cuelang.org/docs/tour/packages/packages/",
        "https://cuelang.org/docs/tour/packages/imports/",
        "https://cuelang.org/docs/tour/packages/standard-library/",
    ], "main article, main", "cue"),
    gi(&["pkl"], "Pkl Language Tutorial", "Apache-2.0", "https://github.com/apple/pkl/blob/main/LICENSE.txt",
        ("https://pkl-lang.org/main/current/language-tutorial/index.html", "article a[href$='.html']"), "article.doc, article", "pkl"),
    g(&["rego"], "Rego: Policy Language", "Apache-2.0", "https://github.com/open-policy-agent/opa/blob/main/LICENSE",
        &["https://www.openpolicyagent.org/docs/policy-language"], "article, main", "rego"),
    g(&["d"], "The D Language Tour: D's Basics", "Boost-1.0 (D Language Tour)", "https://github.com/dlang-tour/core/blob/master/LICENSE.txt", &[
        "https://raw.githubusercontent.com/dlang-tour/english/master/basics/imports-and-modules.md",
        "https://raw.githubusercontent.com/dlang-tour/english/master/basics/basic-types.md",
        "https://raw.githubusercontent.com/dlang-tour/english/master/basics/memory.md",
        "https://raw.githubusercontent.com/dlang-tour/english/master/basics/type-qualifiers.md",
        "https://raw.githubusercontent.com/dlang-tour/english/master/basics/controlling-flow.md",
        "https://raw.githubusercontent.com/dlang-tour/english/master/basics/functions.md",
        "https://raw.githubusercontent.com/dlang-tour/english/master/basics/structs.md",
        "https://raw.githubusercontent.com/dlang-tour/english/master/basics/arrays.md",
        "https://raw.githubusercontent.com/dlang-tour/english/master/basics/slices.md",
        "https://raw.githubusercontent.com/dlang-tour/english/master/basics/alias-strings.md",
        "https://raw.githubusercontent.com/dlang-tour/english/master/basics/loops.md",
        "https://raw.githubusercontent.com/dlang-tour/english/master/basics/foreach.md",
        "https://raw.githubusercontent.com/dlang-tour/english/master/basics/ranges.md",
        "https://raw.githubusercontent.com/dlang-tour/english/master/basics/associative-arrays.md",
        "https://raw.githubusercontent.com/dlang-tour/english/master/basics/classes.md",
        "https://raw.githubusercontent.com/dlang-tour/english/master/basics/interfaces.md",
        "https://raw.githubusercontent.com/dlang-tour/english/master/basics/templates.md",
        "https://raw.githubusercontent.com/dlang-tour/english/master/basics/delegates.md",
        "https://raw.githubusercontent.com/dlang-tour/english/master/basics/exceptions.md",
    ], "", "d"),
    g(&["sml"], "SML Help (Standard ML)", "© the SML Help authors; read from the site, not redistributed", "https://github.com/smlhelp/book", &[
        "https://smlhelp.github.io/book/docs/start/install/", "https://smlhelp.github.io/book/docs/types/",
        "https://smlhelp.github.io/book/docs/concepts/basic/", "https://smlhelp.github.io/book/docs/examples/basics/",
        "https://smlhelp.github.io/book/docs/debugging/errors/", "https://smlhelp.github.io/book/docs/libs/basis/",
    ], "article, main", "sml"),
    g(&["prql"], "The PRQL Language Book", "Apache-2.0", "https://github.com/PRQL/prql/blob/main/LICENSE",
        &["https://prql-lang.org/book/print.html"], "main", "prql").drop(".buttons"),
    g(&["scss", "sass"], "Sass: basics and documentation", "MIT", "https://github.com/sass/sass-site/blob/main/LICENSE", &[
        "https://sass-lang.com/guide/",
        "https://sass-lang.com/documentation/syntax/",
        "https://sass-lang.com/documentation/style-rules/",
        "https://sass-lang.com/documentation/style-rules/parent-selector/",
        "https://sass-lang.com/documentation/variables/",
        "https://sass-lang.com/documentation/interpolation/",
        "https://sass-lang.com/documentation/at-rules/use/",
        "https://sass-lang.com/documentation/at-rules/mixin/",
        "https://sass-lang.com/documentation/at-rules/function/",
        "https://sass-lang.com/documentation/at-rules/extend/",
        "https://sass-lang.com/documentation/at-rules/control/if/",
        "https://sass-lang.com/documentation/at-rules/control/each/",
        "https://sass-lang.com/documentation/at-rules/control/for/",
        "https://sass-lang.com/documentation/values/maps/",
        "https://sass-lang.com/documentation/operators/",
        "https://sass-lang.com/documentation/modules/",
    ], "main", "scss").drop(".sl-c-callout--impl-status, nav, .sl-l-medium-holy-grail__navigation, .sl-c-toc, .sl-r-banner, footer"),
    g(&["make"], "GNU make Manual", "GNU FDL 1.3", "https://www.gnu.org/licenses/fdl-1.3.html",
        &["https://www.gnu.org/software/make/manual/make.html"], "body", "make").drop(GNU_DROP),
    gi(&["cmake"], "CMake Tutorial", "BSD-3-Clause", "https://cmake.org/licensing/",
        ("https://cmake.org/cmake/help/latest/guide/tutorial/index.html", "div.toctree-wrapper a"), "div[role=main], div.body", "cmake"),
    g(&["typst"], "Typst Tutorial", "Apache-2.0", "https://github.com/typst/typst/blob/main/LICENSE", &[
        "https://typst.app/docs/tutorial/", "https://typst.app/docs/tutorial/writing-in-typst/", "https://typst.app/docs/tutorial/formatting/",
        "https://typst.app/docs/tutorial/advanced-styling/", "https://typst.app/docs/tutorial/making-a-template/",
    ], "main", "typst").drop("nav, .page-nav, .previews"),
    g(&["dot", "graphviz"], "The DOT Language (Graphviz)", "EPL-2.0", "https://graphviz.org/license/", &[
        "https://graphviz.org/doc/info/lang.html", "https://graphviz.org/doc/info/attrs.html", "https://graphviz.org/doc/info/shapes.html",
    ], "main .td-content, main", "dot").drop(".td-page-meta, .td-toc, nav"),
    g(&["latex", "tex"], "Learn LaTeX", "CC BY-SA 4.0", "https://github.com/learnlatex/learnlatex.github.io/blob/main/LICENSE.md", &[
        "https://raw.githubusercontent.com/learnlatex/learnlatex.github.io/main/en/lesson-01.md",
        "https://raw.githubusercontent.com/learnlatex/learnlatex.github.io/main/en/lesson-02.md",
        "https://raw.githubusercontent.com/learnlatex/learnlatex.github.io/main/en/lesson-03.md",
        "https://raw.githubusercontent.com/learnlatex/learnlatex.github.io/main/en/lesson-04.md",
        "https://raw.githubusercontent.com/learnlatex/learnlatex.github.io/main/en/lesson-05.md",
        "https://raw.githubusercontent.com/learnlatex/learnlatex.github.io/main/en/lesson-06.md",
        "https://raw.githubusercontent.com/learnlatex/learnlatex.github.io/main/en/lesson-07.md",
        "https://raw.githubusercontent.com/learnlatex/learnlatex.github.io/main/en/lesson-08.md",
        "https://raw.githubusercontent.com/learnlatex/learnlatex.github.io/main/en/lesson-09.md",
        "https://raw.githubusercontent.com/learnlatex/learnlatex.github.io/main/en/lesson-10.md",
        "https://raw.githubusercontent.com/learnlatex/learnlatex.github.io/main/en/lesson-11.md",
        "https://raw.githubusercontent.com/learnlatex/learnlatex.github.io/main/en/lesson-12.md",
        "https://raw.githubusercontent.com/learnlatex/learnlatex.github.io/main/en/lesson-13.md",
        "https://raw.githubusercontent.com/learnlatex/learnlatex.github.io/main/en/lesson-14.md",
        "https://raw.githubusercontent.com/learnlatex/learnlatex.github.io/main/en/lesson-15.md",
        "https://raw.githubusercontent.com/learnlatex/learnlatex.github.io/main/en/lesson-16.md",
    ], "", "latex"),
    g(&["openscad"], "The OpenSCAD Language (OpenSCAD User Manual)", "CC BY-SA 4.0 (Wikibooks)", "https://en.wikibooks.org/wiki/OpenSCAD_User_Manual",
        &["https://en.wikibooks.org/wiki/OpenSCAD_User_Manual/The_OpenSCAD_Language"], "#mw-content-text", "openscad")
        .drop(".mw-editsection, .toc, #toc, .navbox, .noprint, .mw-references-wrap"),
    g(&["dockerfile"], "Dockerfile reference", "Apache-2.0", "https://github.com/docker/docs/blob/main/LICENSE",
        &["https://docs.docker.com/reference/dockerfile/"], "article, main", "dockerfile").drop("nav, aside, .toc, #TableOfContents"),
    g(&["hcl"], "HCL native syntax and OpenTofu configuration", "MPL-2.0", "https://github.com/hashicorp/hcl/blob/main/LICENSE", &[
        "https://raw.githubusercontent.com/hashicorp/hcl/main/hclsyntax/spec.md",
        "https://opentofu.org/docs/language/syntax/configuration/",
    ], "article, main", "hcl").drop("nav, aside, .theme-doc-toc-desktop, .theme-doc-breadcrumbs, .pagination-nav"),
    g(&["graphql"], "Learn GraphQL (graphql.org)", "MIT", "https://github.com/graphql/graphql.github.io/blob/source/LICENSE", &[
        "https://graphql.org/learn/introduction",
        "https://graphql.org/learn/schema",
        "https://graphql.org/learn/queries",
        "https://graphql.org/learn/mutations",
        "https://graphql.org/learn/subscriptions",
        "https://graphql.org/learn/validation",
        "https://graphql.org/learn/execution",
        "https://graphql.org/learn/response",
        "https://graphql.org/learn/introspection",
        "https://graphql.org/learn/thinking-in-graphs",
        "https://graphql.org/learn/serving-over-http",
        "https://graphql.org/learn/pagination",
        "https://graphql.org/learn/schema-design",
    ], "article, main", "graphql").drop("nav, aside"),
    g(&["proto"], "Protocol Buffers: Language Guide (proto 3)", "CC BY 4.0", "https://github.com/protocolbuffers/protocolbuffers.github.io/blob/main/LICENSE", &[
        "https://raw.githubusercontent.com/protocolbuffers/protocolbuffers.github.io/main/content/programming-guides/proto3.md",
        "https://raw.githubusercontent.com/protocolbuffers/protocolbuffers.github.io/main/content/programming-guides/style.md",
    ], "", "proto"),
    g(&["nginx"], "nginx: Beginner's Guide and How nginx processes a request", "BSD-2-Clause", "https://nginx.org/LICENSE", &[
        "https://nginx.org/en/docs/beginners_guide.html", "https://nginx.org/en/docs/http/request_processing.html",
        "https://nginx.org/en/docs/http/server_names.html", "https://nginx.org/en/docs/http/configuring_https_servers.html",
    ], "#content", "nginx"),
    g(&["just"], "The just manual", "CC0-1.0", "https://github.com/casey/just/blob/master/LICENSE",
        &["https://raw.githubusercontent.com/casey/just/master/README.md"], "", "just"),
    g(&["regex"], "MDN: Regular expressions", "CC BY-SA 2.5", "https://developer.mozilla.org/en-US/docs/MDN/Writing_guidelines/Attrib_copyright_license", &[
        "https://raw.githubusercontent.com/mdn/content/main/files/en-us/web/javascript/guide/regular_expressions/index.md",
        "https://raw.githubusercontent.com/mdn/content/main/files/en-us/web/javascript/guide/regular_expressions/cheatsheet/index.md",
        "https://raw.githubusercontent.com/mdn/content/main/files/en-us/web/javascript/reference/regular_expressions/index.md",
    ], "", "regex"),
    g(&["xml"], "MDN: XML introduction", "CC BY-SA 2.5", "https://developer.mozilla.org/en-US/docs/MDN/Writing_guidelines/Attrib_copyright_license", &[
        "https://raw.githubusercontent.com/mdn/content/main/files/en-us/web/xml/guides/xml_introduction/index.md",
    ], "", "xml"),
    g(&["wat", "wast"], "MDN: Understanding WebAssembly text format", "CC BY-SA 2.5", "https://developer.mozilla.org/en-US/docs/MDN/Writing_guidelines/Attrib_copyright_license", &[
        "https://raw.githubusercontent.com/mdn/content/main/files/en-us/webassembly/guides/understanding_the_text_format/index.md",
    ], "", "wat"),
    g(&["elm"], "An Introduction to Elm", "BSD-3-Clause", "https://github.com/evancz/guide.elm-lang.org/blob/master/LICENSE", &[
        "https://raw.githubusercontent.com/evancz/guide.elm-lang.org/master/book/core_language.md",
        "https://raw.githubusercontent.com/evancz/guide.elm-lang.org/master/book/architecture/README.md",
        "https://raw.githubusercontent.com/evancz/guide.elm-lang.org/master/book/architecture/buttons.md",
        "https://raw.githubusercontent.com/evancz/guide.elm-lang.org/master/book/architecture/text_fields.md",
        "https://raw.githubusercontent.com/evancz/guide.elm-lang.org/master/book/architecture/forms.md",
        "https://raw.githubusercontent.com/evancz/guide.elm-lang.org/master/book/types/README.md",
        "https://raw.githubusercontent.com/evancz/guide.elm-lang.org/master/book/types/reading_types.md",
        "https://raw.githubusercontent.com/evancz/guide.elm-lang.org/master/book/types/type_aliases.md",
        "https://raw.githubusercontent.com/evancz/guide.elm-lang.org/master/book/types/custom_types.md",
        "https://raw.githubusercontent.com/evancz/guide.elm-lang.org/master/book/types/pattern_matching.md",
        "https://raw.githubusercontent.com/evancz/guide.elm-lang.org/master/book/error_handling/README.md",
        "https://raw.githubusercontent.com/evancz/guide.elm-lang.org/master/book/error_handling/maybe.md",
        "https://raw.githubusercontent.com/evancz/guide.elm-lang.org/master/book/error_handling/result.md",
    ], "", "elm"),
    g(&["purescript"], "PureScript language reference", "CC BY-NC-SA 3.0; read from the repository, not redistributed", "https://github.com/purescript/documentation/blob/master/LICENSE.md", &[
        "https://raw.githubusercontent.com/purescript/documentation/master/language/Syntax.md",
        "https://raw.githubusercontent.com/purescript/documentation/master/language/Types.md",
        "https://raw.githubusercontent.com/purescript/documentation/master/language/Records.md",
        "https://raw.githubusercontent.com/purescript/documentation/master/language/Pattern-Matching.md",
        "https://raw.githubusercontent.com/purescript/documentation/master/language/Type-Classes.md",
        "https://raw.githubusercontent.com/purescript/documentation/master/language/Modules.md",
        "https://raw.githubusercontent.com/purescript/documentation/master/language/Differences-from-Haskell.md",
    ], "", "purescript"),
    g(&["solidity"], "Solidity documentation", "GPL-3.0", "https://github.com/ethereum/solidity/blob/develop/LICENSE.txt", &[
        "https://docs.soliditylang.org/en/latest/introduction-to-smart-contracts.html",
        "https://docs.soliditylang.org/en/latest/layout-of-source-files.html",
        "https://docs.soliditylang.org/en/latest/structure-of-a-contract.html",
        "https://docs.soliditylang.org/en/latest/types.html",
        "https://docs.soliditylang.org/en/latest/units-and-global-variables.html",
        "https://docs.soliditylang.org/en/latest/control-structures.html",
        "https://docs.soliditylang.org/en/latest/contracts.html",
        "https://docs.soliditylang.org/en/latest/cheatsheet.html",
    ], "div[role=main], div.body", "solidity"),
    g(&["glsl"], "The OpenGL Shading Language 4.60", "© Khronos Group; read from the registry, not redistributed", "https://registry.khronos.org/OpenGL/specs/gl/GLSLangSpec.4.60.html",
        &["https://registry.khronos.org/OpenGL/specs/gl/GLSLangSpec.4.60.html"], "#content, body", "glsl").drop("#toc, #header"),
    g(&["wgsl", "wesl"], "Tour of WGSL", "Apache-2.0", "https://github.com/google/tour-of-wgsl/blob/main/LICENSE", &[
        "https://google.github.io/tour-of-wgsl/",
        "https://google.github.io/tour-of-wgsl/functions/",
        "https://google.github.io/tour-of-wgsl/types/",
        "https://google.github.io/tour-of-wgsl/expressions/",
        "https://google.github.io/tour-of-wgsl/variables/",
        "https://google.github.io/tour-of-wgsl/control-flow/",
        "https://google.github.io/tour-of-wgsl/binding-points/",
        "https://google.github.io/tour-of-wgsl/uniformity-analysis/",
    ], "main, article, body", "wgsl").drop("nav, header, footer"),
    g(&["verilog"], "Programmable Logic: Verilog (Wikibooks)", "CC BY-SA 4.0", "https://en.wikibooks.org/wiki/Programmable_Logic/Verilog", &[
        "https://en.wikibooks.org/wiki/Programmable_Logic/Verilog_Module_Structure",
        "https://en.wikibooks.org/wiki/Programmable_Logic/Verilog_Data_Types",
        "https://en.wikibooks.org/wiki/Programmable_Logic/Verilog_Operators",
        "https://en.wikibooks.org/wiki/Programmable_Logic/Verilog_Always_and_Initial",
        "https://en.wikibooks.org/wiki/Programmable_Logic/Verilog_Control_Structures",
        "https://en.wikibooks.org/wiki/Programmable_Logic/Verilog_Tasks",
        "https://en.wikibooks.org/wiki/Programmable_Logic/Verilog_for_Software_Programmers",
        "https://en.wikibooks.org/wiki/Programmable_Logic/Verilog_RTL_Coding_Guidelines",
    ], "#mw-content-text", "verilog").drop(".mw-editsection, .toc, #toc, .navbox, .noprint, .wikitable.metadata"),
    g(&["vhdl"], "Programmable Logic: VHDL, and VHDL for FPGA Design (Wikibooks)", "CC BY-SA 4.0", "https://en.wikibooks.org/wiki/Programmable_Logic/VHDL", &[
        "https://en.wikibooks.org/wiki/Programmable_Logic/VHDL_General_Syntax",
        "https://en.wikibooks.org/wiki/Programmable_Logic/VHDL_Data_Types",
        "https://en.wikibooks.org/wiki/Programmable_Logic/VHDL_Operators",
        "https://en.wikibooks.org/wiki/Programmable_Logic/VHDL_Module_Structure",
        "https://en.wikibooks.org/wiki/Programmable_Logic/VHDL_Processes",
        "https://en.wikibooks.org/wiki/Programmable_Logic/VHDL_Sequential_Statement",
        "https://en.wikibooks.org/wiki/Programmable_Logic/VHDL_Procedures",
        "https://en.wikibooks.org/wiki/Programmable_Logic/VHDL_Coding_and_Project_Style",
        "https://en.wikibooks.org/wiki/VHDL_for_FPGA_Design/Decoder",
        "https://en.wikibooks.org/wiki/VHDL_for_FPGA_Design/Multiplexer",
        "https://en.wikibooks.org/wiki/VHDL_for_FPGA_Design/4-Bit_Adder",
        "https://en.wikibooks.org/wiki/VHDL_for_FPGA_Design/D_Flip_Flop",
        "https://en.wikibooks.org/wiki/VHDL_for_FPGA_Design/4-Bit_Shift_Register",
        "https://en.wikibooks.org/wiki/VHDL_for_FPGA_Design/State-Machine_Design_Example_Serial_Parity_Generator",
    ], "#mw-content-text", "vhdl").drop(".mw-editsection, .toc, #toc, .navbox, .noprint"),
    g(&["vue"], "Vue.js Guide: Essentials", "MIT", "https://github.com/vuejs/docs/blob/main/LICENSE", &[
        "https://raw.githubusercontent.com/vuejs/docs/main/src/guide/essentials/application.md",
        "https://raw.githubusercontent.com/vuejs/docs/main/src/guide/essentials/template-syntax.md",
        "https://raw.githubusercontent.com/vuejs/docs/main/src/guide/essentials/reactivity-fundamentals.md",
        "https://raw.githubusercontent.com/vuejs/docs/main/src/guide/essentials/computed.md",
        "https://raw.githubusercontent.com/vuejs/docs/main/src/guide/essentials/class-and-style.md",
        "https://raw.githubusercontent.com/vuejs/docs/main/src/guide/essentials/conditional.md",
        "https://raw.githubusercontent.com/vuejs/docs/main/src/guide/essentials/list.md",
        "https://raw.githubusercontent.com/vuejs/docs/main/src/guide/essentials/event-handling.md",
        "https://raw.githubusercontent.com/vuejs/docs/main/src/guide/essentials/forms.md",
        "https://raw.githubusercontent.com/vuejs/docs/main/src/guide/essentials/lifecycle.md",
        "https://raw.githubusercontent.com/vuejs/docs/main/src/guide/essentials/watchers.md",
        "https://raw.githubusercontent.com/vuejs/docs/main/src/guide/essentials/template-refs.md",
        "https://raw.githubusercontent.com/vuejs/docs/main/src/guide/essentials/component-basics.md",
    ], "", "vue"),
    g(&["svelte"], "Svelte documentation", "MIT", "https://github.com/sveltejs/svelte/blob/main/LICENSE.md", &[
        "https://svelte.dev/docs/svelte/overview",
        "https://svelte.dev/docs/svelte/getting-started",
        "https://svelte.dev/docs/svelte/svelte-files",
        "https://svelte.dev/docs/svelte/what-are-runes",
        "https://svelte.dev/docs/svelte/$state",
        "https://svelte.dev/docs/svelte/$derived",
        "https://svelte.dev/docs/svelte/$effect",
        "https://svelte.dev/docs/svelte/$props",
        "https://svelte.dev/docs/svelte/$bindable",
        "https://svelte.dev/docs/svelte/basic-markup",
        "https://svelte.dev/docs/svelte/if",
        "https://svelte.dev/docs/svelte/each",
        "https://svelte.dev/docs/svelte/key",
        "https://svelte.dev/docs/svelte/await",
        "https://svelte.dev/docs/svelte/snippet",
        "https://svelte.dev/docs/svelte/bind",
        "https://svelte.dev/docs/svelte/transition",
        "https://svelte.dev/docs/svelte/style",
        "https://svelte.dev/docs/svelte/class",
        "https://svelte.dev/docs/svelte/scoped-styles",
    ], "main article, main", "svelte").drop("nav, aside, .ts-toggle, .controls"),
    g(&["astro"], "Astro basics", "MIT", "https://github.com/withastro/docs/blob/main/LICENSE", &[
        "https://raw.githubusercontent.com/withastro/docs/main/src/content/docs/en/basics/project-structure.mdx",
        "https://raw.githubusercontent.com/withastro/docs/main/src/content/docs/en/basics/astro-components.mdx",
        "https://raw.githubusercontent.com/withastro/docs/main/src/content/docs/en/basics/astro-pages.mdx",
        "https://raw.githubusercontent.com/withastro/docs/main/src/content/docs/en/basics/layouts.mdx",
    ], "", "astro"),
    g(&["kdl"], "The KDL Document Language, version 2", "CC BY-SA 4.0", "https://github.com/kdl-org/kdl/blob/main/LICENSE.md",
        &["https://raw.githubusercontent.com/kdl-org/kdl/main/draft-marchan-kdl2.md"], "", "kdl"),
    g(&["gdscript"], "GDScript reference (Godot)", "CC BY 3.0", "https://docs.godotengine.org/en/stable/about/license.html", &[
        "https://docs.godotengine.org/en/stable/tutorials/scripting/gdscript/gdscript_basics.html",
        "https://docs.godotengine.org/en/stable/tutorials/scripting/gdscript/static_typing.html",
        "https://docs.godotengine.org/en/stable/tutorials/scripting/gdscript/gdscript_styleguide.html",
    ], "div[role=main], div.body", "gdscript"),
    g(&["gitignore"], "Git documentation: gitignore", "GPL-2.0", "https://git-scm.com/about/free-and-open-source",
        &["https://git-scm.com/docs/gitignore"], "#main, main", "gitignore"),
    g(&["gitattributes"], "Git documentation: gitattributes", "GPL-2.0", "https://git-scm.com/about/free-and-open-source",
        &["https://git-scm.com/docs/gitattributes"], "#main, main", "gitattributes"),
    g(&["git-config"], "Git documentation: git config", "GPL-2.0", "https://git-scm.com/about/free-and-open-source",
        &["https://git-scm.com/docs/git-config"], "#main, main", "git-config"),
    g(&["gitcommit"], "Git documentation: git commit", "GPL-2.0", "https://git-scm.com/about/free-and-open-source",
        &["https://git-scm.com/docs/git-commit"], "#main, main", "gitcommit"),
    g(&["git-rebase"], "Git documentation: git rebase", "GPL-2.0", "https://git-scm.com/about/free-and-open-source",
        &["https://git-scm.com/docs/git-rebase"], "#main, main", "git-rebase"),
    g(&["mermaid"], "Mermaid diagram syntax", "MIT", "https://github.com/mermaid-js/mermaid/blob/develop/LICENSE", &[
        "https://raw.githubusercontent.com/mermaid-js/mermaid/develop/packages/mermaid/src/docs/syntax/flowchart.md",
        "https://raw.githubusercontent.com/mermaid-js/mermaid/develop/packages/mermaid/src/docs/syntax/sequenceDiagram.md",
        "https://raw.githubusercontent.com/mermaid-js/mermaid/develop/packages/mermaid/src/docs/syntax/classDiagram.md",
        "https://raw.githubusercontent.com/mermaid-js/mermaid/develop/packages/mermaid/src/docs/syntax/stateDiagram.md",
        "https://raw.githubusercontent.com/mermaid-js/mermaid/develop/packages/mermaid/src/docs/syntax/entityRelationshipDiagram.md",
        "https://raw.githubusercontent.com/mermaid-js/mermaid/develop/packages/mermaid/src/docs/syntax/gantt.md",
        "https://raw.githubusercontent.com/mermaid-js/mermaid/develop/packages/mermaid/src/docs/syntax/pie.md",
        "https://raw.githubusercontent.com/mermaid-js/mermaid/develop/packages/mermaid/src/docs/syntax/mindmap.md",
    ], "", "mermaid"),
    g(&["meson"], "The Meson Build system: Tutorial and Syntax", "Apache-2.0", "https://github.com/mesonbuild/meson/blob/master/COPYING", &[
        "https://raw.githubusercontent.com/mesonbuild/meson/master/docs/markdown/Tutorial.md",
        "https://raw.githubusercontent.com/mesonbuild/meson/master/docs/markdown/Syntax.md",
        "https://raw.githubusercontent.com/mesonbuild/meson/master/docs/markdown/Build-targets.md",
        "https://raw.githubusercontent.com/mesonbuild/meson/master/docs/markdown/Dependencies.md",
        "https://raw.githubusercontent.com/mesonbuild/meson/master/docs/markdown/Configuration.md",
    ], "", "meson"),
    g(&["qmljs", "qml"], "QML Reference (Qt 6)", "GNU FDL 1.3", "https://doc.qt.io/qt-6/licensing.html", &[
        "https://doc.qt.io/qt-6/qtqml-syntax-basics.html",
        "https://doc.qt.io/qt-6/qtqml-syntax-objectattributes.html",
        "https://doc.qt.io/qt-6/qtqml-syntax-propertybinding.html",
        "https://doc.qt.io/qt-6/qtqml-syntax-signals.html",
        "https://doc.qt.io/qt-6/qtqml-javascript-expressions.html",
        "https://doc.qt.io/qt-6/qtqml-typesystem-topic.html",
        "https://doc.qt.io/qt-6/qtqml-documents-topic.html",
    ], ".mainContent, article, main", "qmljs").drop(".naviNextPrevious, #qds-toc-menu, nav"),
    g(&["jinja2", "jinja"], "Jinja Template Designer Documentation", "BSD-3-Clause", "https://github.com/pallets/jinja/blob/main/LICENSE.txt",
        &["https://jinja.palletsprojects.com/en/stable/templates/"], "div[role=main], div.body", "jinja2"),
    g(&["twig"], "Twig for Template Designers", "BSD-3-Clause", "https://github.com/twigphp/Twig/blob/3.x/LICENSE",
        &["https://twig.symfony.com/doc/3.x/templates.html"], "#content, main, article", "twig").drop("nav, .toc"),
    g(&["cairo"], "The Cairo Programming Language", "MIT or Apache-2.0", "https://github.com/cairo-book/cairo-book/blob/main/LICENSE",
        &["https://book.cairo-lang.org/print.html"], "main", "cairo").drop(".buttons"),
    g(&["hurl"], "Hurl documentation", "Apache-2.0", "https://github.com/Orange-OpenSource/hurl/blob/master/LICENSE", &[
        "https://hurl.dev/docs/tutorial/your-first-hurl-file.html",
        "https://hurl.dev/docs/tutorial/adding-asserts.html",
        "https://hurl.dev/docs/tutorial/chaining-requests.html",
        "https://hurl.dev/docs/tutorial/captures.html",
        "https://hurl.dev/docs/hurl-file.html",
        "https://hurl.dev/docs/entry.html",
        "https://hurl.dev/docs/request.html",
        "https://hurl.dev/docs/response.html",
        "https://hurl.dev/docs/capturing-response.html",
        "https://hurl.dev/docs/asserting-response.html",
        "https://hurl.dev/docs/filters.html",
        "https://hurl.dev/docs/templates.html",
    ], "main article, main, article", "hurl").drop("nav, aside, header, footer, .top-nav, .top-nav-toc"),
    g(&["caddyfile"], "The Caddyfile", "Apache-2.0", "https://github.com/caddyserver/website/blob/master/LICENSE", &[
        "https://raw.githubusercontent.com/caddyserver/website/master/src/docs/markdown/caddyfile-tutorial.md",
        "https://raw.githubusercontent.com/caddyserver/website/master/src/docs/markdown/caddyfile/concepts.md",
        "https://raw.githubusercontent.com/caddyserver/website/master/src/docs/markdown/caddyfile/directives.md",
        "https://raw.githubusercontent.com/caddyserver/website/master/src/docs/markdown/caddyfile/matchers.md",
    ], "", "caddyfile"),
    g(&["rst"], "reStructuredText: Primer and Quick Start", "Public domain (Docutils) and BSD-2-Clause (Sphinx)", "https://docutils.sourceforge.io/COPYING.html", &[
        "https://docutils.sourceforge.io/docs/user/rst/quickstart.html",
        "https://www.sphinx-doc.org/en/master/usage/restructuredtext/basics.html",
    ], "div[role=main], div.document, body", "rst"),
    g(&["json5"], "JSON5", "MIT", "https://github.com/json5/json5/blob/main/LICENSE.md",
        &["https://raw.githubusercontent.com/json5/json5/main/README.md"], "", "json5"),
    g(&["gherkin"], "Gherkin reference (Cucumber)", "MIT", "https://github.com/cucumber/docs/blob/main/LICENSE",
        &["https://raw.githubusercontent.com/cucumber/docs/main/content/docs/gherkin/reference.md"], "", "gherkin"),
    g(&["agda"], "Agda: A Taste of Agda and the language", "MIT", "https://github.com/agda/agda/blob/master/LICENSE", &[
        "https://agda.readthedocs.io/en/latest/getting-started/a-taste-of-agda.html",
        "https://agda.readthedocs.io/en/latest/language/data-types.html",
        "https://agda.readthedocs.io/en/latest/language/function-definitions.html",
        "https://agda.readthedocs.io/en/latest/language/record-types.html",
        "https://agda.readthedocs.io/en/latest/language/module-system.html",
        "https://agda.readthedocs.io/en/latest/language/implicit-arguments.html",
        "https://agda.readthedocs.io/en/latest/language/mixfix-operators.html",
    ], "div[role=main], div.body", "agda"),
    g(&["gotmpl"], "Go text/template", "BSD-3-Clause", "https://go.dev/LICENSE",
        &["https://pkg.go.dev/text/template"], "section.Documentation-overview, .Documentation-overview, #main", "gotmpl"),
    g(&["gomod", "gowork"], "Go Modules Reference", "CC BY 4.0", "https://go.dev/copyright",
        &["https://go.dev/ref/mod"], "#main-content article, article, main", "gomod"),
    g(&["cel"], "CEL language definition", "Apache-2.0", "https://github.com/google/cel-spec/blob/master/LICENSE",
        &["https://raw.githubusercontent.com/google/cel-spec/master/doc/langdef.md"], "", "cel"),
    gi(&["org"], "The compact Org guide", "GNU FDL 1.3", "https://www.gnu.org/licenses/fdl-1.3.html",
        ("https://orgmode.org/guide/", "a[href$='.html']"), "body", "org").drop(GNU_DROP),
    g(&["jsdoc"], "Use JSDoc", "CC BY-SA 3.0", "https://github.com/jsdoc/jsdoc.github.io/blob/master/LICENSE", &[
        "https://jsdoc.app/about-getting-started",
        "https://jsdoc.app/about-namepaths",
        "https://jsdoc.app/about-block-inline-tags",
        "https://jsdoc.app/howto-es2015-classes",
        "https://jsdoc.app/howto-es2015-modules",
        "https://jsdoc.app/tags-param",
        "https://jsdoc.app/tags-returns",
        "https://jsdoc.app/tags-type",
        "https://jsdoc.app/tags-typedef",
        "https://jsdoc.app/tags-callback",
        "https://jsdoc.app/tags-example",
    ], "article, main", "jsdoc"),
    g(&["thrift"], "Thrift interface description language", "Apache-2.0", "https://www.apache.org/licenses/LICENSE-2.0",
        &["https://thrift.apache.org/docs/idl"], "main, .content, article, body", "thrift").drop("nav, header, footer"),
    g(&["capnp"], "Cap'n Proto: Schema Language", "MIT", "https://github.com/capnproto/capnproto/blob/v2/LICENSE",
        &["https://capnproto.org/language.html"], "#main-content, main, body", "capnp").drop("nav, #sidebar, header, footer"),
    g(&["bicep"], "Bicep file structure and syntax", "CC BY 4.0", "https://github.com/MicrosoftDocs/azure-docs/blob/main/LICENSE", &[
        "https://learn.microsoft.com/en-us/azure/azure-resource-manager/bicep/file",
        "https://learn.microsoft.com/en-us/azure/azure-resource-manager/bicep/data-types",
        "https://learn.microsoft.com/en-us/azure/azure-resource-manager/bicep/parameters",
        "https://learn.microsoft.com/en-us/azure/azure-resource-manager/bicep/variables",
        "https://learn.microsoft.com/en-us/azure/azure-resource-manager/bicep/outputs",
        "https://learn.microsoft.com/en-us/azure/azure-resource-manager/bicep/loops",
        "https://learn.microsoft.com/en-us/azure/azure-resource-manager/bicep/conditional-resource-deployment",
        "https://learn.microsoft.com/en-us/azure/azure-resource-manager/bicep/modules",
    ], "main", "bicep").drop("nav, aside, #ms--additional-resources, .feedback-section, .page-metadata"),
    g(&["smithy"], "Smithy IDL", "Apache-2.0", "https://github.com/smithy-lang/smithy/blob/main/LICENSE",
        &["https://smithy.io/2.0/spec/idl.html"], "article, div[role=main], main", "smithy"),
    g(&["tlaplus"], "Learn TLA+", "© Hillel Wayne; read from the site, not redistributed", "https://learntla.com/", &[
        "https://learntla.com/core/index.html",
        "https://learntla.com/core/setup.html",
        "https://learntla.com/core/operators.html",
        "https://learntla.com/core/pluscal.html",
        "https://learntla.com/core/invariants.html",
        "https://learntla.com/core/functions.html",
        "https://learntla.com/core/action-properties.html",
        "https://learntla.com/core/concurrency.html",
    ], "div[role=main], div.body, main", "tlaplus"),
    g(&["ron"], "RON: Rusty Object Notation", "MIT or Apache-2.0", "https://github.com/ron-rs/ron",
        &["https://raw.githubusercontent.com/ron-rs/ron/master/README.md", "https://raw.githubusercontent.com/ron-rs/ron/master/docs/grammar.md"], "", "ron"),
    g(&["rescript"], "ReScript Language Manual", "MIT", "https://github.com/rescript-lang/rescript-lang.org/blob/master/LICENSE", &[
        "https://rescript-lang.org/docs/manual/introduction",
        "https://rescript-lang.org/docs/manual/overview",
        "https://rescript-lang.org/docs/manual/let-binding",
        "https://rescript-lang.org/docs/manual/type",
        "https://rescript-lang.org/docs/manual/primitive-types",
        "https://rescript-lang.org/docs/manual/tuple",
        "https://rescript-lang.org/docs/manual/record",
        "https://rescript-lang.org/docs/manual/object",
        "https://rescript-lang.org/docs/manual/variant",
        "https://rescript-lang.org/docs/manual/null-undefined-option",
        "https://rescript-lang.org/docs/manual/array-and-list",
        "https://rescript-lang.org/docs/manual/function",
        "https://rescript-lang.org/docs/manual/control-flow",
        "https://rescript-lang.org/docs/manual/pipe",
        "https://rescript-lang.org/docs/manual/pattern-matching-destructuring",
        "https://rescript-lang.org/docs/manual/mutation",
        "https://rescript-lang.org/docs/manual/jsx",
        "https://rescript-lang.org/docs/manual/exception",
        "https://rescript-lang.org/docs/manual/promise",
        "https://rescript-lang.org/docs/manual/async-await",
        "https://rescript-lang.org/docs/manual/module",
        "https://rescript-lang.org/docs/manual/import-export",
    ], "main article, main", "rescript").drop("nav, aside"),
    g(&["pug"], "Pug language reference", "MIT", "https://github.com/pugjs/pug/blob/master/packages/pug/LICENSE", &[
        "https://pugjs.org/language/attributes.html",
        "https://pugjs.org/language/case.html",
        "https://pugjs.org/language/code.html",
        "https://pugjs.org/language/comments.html",
        "https://pugjs.org/language/conditionals.html",
        "https://pugjs.org/language/doctype.html",
        "https://pugjs.org/language/filters.html",
        "https://pugjs.org/language/includes.html",
        "https://pugjs.org/language/inheritance.html",
        "https://pugjs.org/language/interpolation.html",
        "https://pugjs.org/language/iteration.html",
        "https://pugjs.org/language/mixins.html",
        "https://pugjs.org/language/plain-text.html",
        "https://pugjs.org/language/tags.html",
    ], ".content, main, article", "pug").drop("nav, .sidebar, header, footer"),
    g(&["yara"], "Writing YARA rules", "BSD-3-Clause", "https://github.com/VirusTotal/yara/blob/master/COPYING",
        &["https://yara.readthedocs.io/en/stable/writingrules.html"], "div[role=main], div.body", "yara"),
    g(&["ledger"], "Ledger 3 manual", "BSD-3-Clause (Ledger)", "https://github.com/ledger/ledger/blob/master/LICENSE.md",
        &["https://www.ledger-cli.org/3.0/doc/ledger3.html"], "body", "ledger").drop(GNU_DROP),
    g(&["pest"], "pest: Grammar syntax", "MIT or Apache-2.0", "https://github.com/pest-parser/book",
        &["https://raw.githubusercontent.com/pest-parser/book/master/src/grammars/syntax.md", "https://raw.githubusercontent.com/pest-parser/book/master/src/grammars/peg.md"], "", "pest"),
    g(&["mojo"], "Mojo Manual", "© Modular; read from the site, not redistributed", "https://docs.modular.com/", &[
        "https://docs.modular.com/mojo/manual/basics",
        "https://docs.modular.com/mojo/manual/variables",
        "https://docs.modular.com/mojo/manual/types",
        "https://docs.modular.com/mojo/manual/functions",
        "https://docs.modular.com/mojo/manual/structs",
        "https://docs.modular.com/mojo/manual/traits",
        "https://docs.modular.com/mojo/manual/errors",
        "https://docs.modular.com/mojo/manual/control-flow",
    ], "article, main", "mojo").drop("nav, aside, .theme-doc-toc-desktop, .pagination-nav, .theme-doc-breadcrumbs"),
    gi(&["move"], "The Move Book", "CC BY 4.0", "https://github.com/MystenLabs/move-book", ("https://move-book.com/", ".VPSidebar a[href^='/'], nav a[href^='/']"), "main .vp-doc, main", "move").drop("nav, aside, .VPSidebar, .VPDocFooter, .VPLocalNav, header, footer"),
    g(&["prisma"], "Prisma schema", "Apache-2.0", "https://github.com/prisma/docs/blob/main/LICENSE", &[
        "https://www.prisma.io/docs/orm/prisma-schema/overview",
        "https://www.prisma.io/docs/orm/prisma-schema/data-model/models",
        "https://www.prisma.io/docs/orm/prisma-schema/data-model/relations",
        "https://www.prisma.io/docs/orm/prisma-schema/data-model/indexes",
        "https://www.prisma.io/docs/orm/prisma-schema/data-model/database-mapping",
    ], "article, main", "prisma").drop("nav, aside, header, footer"),
    g(&["robot"], "Robot Framework User Guide", "CC BY 3.0", "https://robotframework.org/robotframework/latest/RobotFrameworkUserGuide.html", &["https://robotframework.org/robotframework/latest/RobotFrameworkUserGuide.html"], "body", "robot").drop("#toc, .contents, #header, #footer"),
    g(&["vala"], "Vala programming language tutorial", "CC BY-SA 4.0", "https://github.com/vala-lang/vala-docs", &[
        "https://raw.githubusercontent.com/vala-lang/vala-docs/main/docs/tutorials/main/01-00-first-program.md",
        "https://raw.githubusercontent.com/vala-lang/vala-docs/main/docs/tutorials/main/02-00-basics.md",
        "https://raw.githubusercontent.com/vala-lang/vala-docs/main/docs/tutorials/main/02-00-basics/02-01-source-files-and-compilation.md",
        "https://raw.githubusercontent.com/vala-lang/vala-docs/main/docs/tutorials/main/02-00-basics/02-02-syntax-overview.md",
        "https://raw.githubusercontent.com/vala-lang/vala-docs/main/docs/tutorials/main/02-00-basics/02-03-comments.md",
        "https://raw.githubusercontent.com/vala-lang/vala-docs/main/docs/tutorials/main/02-00-basics/02-04-data-types.md",
        "https://raw.githubusercontent.com/vala-lang/vala-docs/main/docs/tutorials/main/02-00-basics/02-05-operators.md",
        "https://raw.githubusercontent.com/vala-lang/vala-docs/main/docs/tutorials/main/02-00-basics/02-06-control-structures.md",
        "https://raw.githubusercontent.com/vala-lang/vala-docs/main/docs/tutorials/main/02-00-basics/02-07-language-elements.md",
        "https://raw.githubusercontent.com/vala-lang/vala-docs/main/docs/tutorials/main/02-00-basics/02-08-code-attributes.md",
        "https://raw.githubusercontent.com/vala-lang/vala-docs/main/docs/tutorials/main/02-00-basics/02-09-input-output.md",
        "https://raw.githubusercontent.com/vala-lang/vala-docs/main/docs/tutorials/main/02-00-basics/02-10-format-strings.md",
        "https://raw.githubusercontent.com/vala-lang/vala-docs/main/docs/tutorials/main/03-00-object-oriented-programming.md",
        "https://raw.githubusercontent.com/vala-lang/vala-docs/main/docs/tutorials/main/03-00-object-oriented-programming/03-01-basics.md",
        "https://raw.githubusercontent.com/vala-lang/vala-docs/main/docs/tutorials/main/03-00-object-oriented-programming/03-02-construction.md",
        "https://raw.githubusercontent.com/vala-lang/vala-docs/main/docs/tutorials/main/03-00-object-oriented-programming/03-03-destruction.md",
        "https://raw.githubusercontent.com/vala-lang/vala-docs/main/docs/tutorials/main/03-00-object-oriented-programming/03-04-signals.md",
        "https://raw.githubusercontent.com/vala-lang/vala-docs/main/docs/tutorials/main/03-00-object-oriented-programming/03-05-properties.md",
        "https://raw.githubusercontent.com/vala-lang/vala-docs/main/docs/tutorials/main/03-00-object-oriented-programming/03-06-inheritance.md",
        "https://raw.githubusercontent.com/vala-lang/vala-docs/main/docs/tutorials/main/03-00-object-oriented-programming/03-07-abstract-classes.md",
        "https://raw.githubusercontent.com/vala-lang/vala-docs/main/docs/tutorials/main/03-00-object-oriented-programming/03-08-interfaces.md",
        "https://raw.githubusercontent.com/vala-lang/vala-docs/main/docs/tutorials/main/03-00-object-oriented-programming/03-09-polymorphism.md",
        "https://raw.githubusercontent.com/vala-lang/vala-docs/main/docs/tutorials/main/03-00-object-oriented-programming/03-13-generics.md",
        "https://raw.githubusercontent.com/vala-lang/vala-docs/main/docs/tutorials/main/04-00-advanced-features.md",
        "https://raw.githubusercontent.com/vala-lang/vala-docs/main/docs/tutorials/main/04-00-advanced-features/04-02-error-handling.md",
        "https://raw.githubusercontent.com/vala-lang/vala-docs/main/docs/tutorials/main/04-00-advanced-features/04-03-parameter-directions.md",
        "https://raw.githubusercontent.com/vala-lang/vala-docs/main/docs/tutorials/main/04-00-advanced-features/04-04-collections.md",
        "https://raw.githubusercontent.com/vala-lang/vala-docs/main/docs/tutorials/main/04-00-advanced-features/04-08-asynchronous-methods.md",
        "https://raw.githubusercontent.com/vala-lang/vala-docs/main/docs/tutorials/main/04-00-advanced-features/04-10-ownership.md",
    ], "", "vala"),
    g(&["teal"], "The Teal language guide", "MIT", "https://github.com/teal-language/tl/blob/master/LICENSE", &[
        "https://raw.githubusercontent.com/teal-language/tl/master/docs/src/tutorial.md",
        "https://raw.githubusercontent.com/teal-language/tl/master/docs/src/types_in_teal.md",
        "https://raw.githubusercontent.com/teal-language/tl/master/docs/src/local_variables.md",
        "https://raw.githubusercontent.com/teal-language/tl/master/docs/src/functions.md",
        "https://raw.githubusercontent.com/teal-language/tl/master/docs/src/arrays.md",
        "https://raw.githubusercontent.com/teal-language/tl/master/docs/src/tuples.md",
        "https://raw.githubusercontent.com/teal-language/tl/master/docs/src/maps.md",
        "https://raw.githubusercontent.com/teal-language/tl/master/docs/src/records.md",
        "https://raw.githubusercontent.com/teal-language/tl/master/docs/src/interfaces.md",
        "https://raw.githubusercontent.com/teal-language/tl/master/docs/src/generics.md",
        "https://raw.githubusercontent.com/teal-language/tl/master/docs/src/enums.md",
        "https://raw.githubusercontent.com/teal-language/tl/master/docs/src/union_types.md",
        "https://raw.githubusercontent.com/teal-language/tl/master/docs/src/the_type_any.md",
        "https://raw.githubusercontent.com/teal-language/tl/master/docs/src/variable_attributes.md",
        "https://raw.githubusercontent.com/teal-language/tl/master/docs/src/using_tl_with_lua.md",
    ], "", "teal"),
    g(&["typespec"], "TypeSpec language basics", "MIT", "https://github.com/microsoft/typespec/blob/main/LICENSE", &[
        "https://typespec.io/docs/language-basics/overview/",
        "https://typespec.io/docs/language-basics/built-in-types/",
        "https://typespec.io/docs/language-basics/identifiers/",
        "https://typespec.io/docs/language-basics/imports/",
        "https://typespec.io/docs/language-basics/namespaces/",
        "https://typespec.io/docs/language-basics/decorators/",
        "https://typespec.io/docs/language-basics/models/",
        "https://typespec.io/docs/language-basics/operations/",
        "https://typespec.io/docs/language-basics/interfaces/",
        "https://typespec.io/docs/language-basics/templates/",
        "https://typespec.io/docs/language-basics/enums/",
        "https://typespec.io/docs/language-basics/unions/",
        "https://typespec.io/docs/language-basics/scalars/",
        "https://typespec.io/docs/language-basics/aliases/",
        "https://typespec.io/docs/language-basics/values/",
    ], "main article, main", "typespec").drop("nav, aside, header, footer"),
    gi(&["slint"], "Slint language reference", "© SixtyFPS; read from the site, not redistributed", "https://slint.dev/", ("https://docs.slint.dev/latest/docs/slint/", "a[href*='/docs/slint/guide/language/'], a[href*='/docs/slint/reference/']"), "main article, main", "slint").drop("nav, aside, header, footer"),
    g(&["unison"], "A tour of Unison", "MIT", "https://github.com/unisonweb/website", &["https://www.unison-lang.org/docs/tour/"], "main article, main", "unison").drop("nav, aside, header, footer"),
    g(&["ponylang"], "The Pony Tutorial", "BSD-2-Clause", "https://github.com/ponylang/pony-tutorial/blob/main/LICENSE", &[
        "https://raw.githubusercontent.com/ponylang/pony-tutorial/main/docs/getting-started/what-you-need.md",
        "https://raw.githubusercontent.com/ponylang/pony-tutorial/main/docs/getting-started/hello-world.md",
        "https://raw.githubusercontent.com/ponylang/pony-tutorial/main/docs/getting-started/how-it-works.md",
        "https://raw.githubusercontent.com/ponylang/pony-tutorial/main/docs/types/at-a-glance.md",
        "https://raw.githubusercontent.com/ponylang/pony-tutorial/main/docs/types/classes.md",
        "https://raw.githubusercontent.com/ponylang/pony-tutorial/main/docs/types/primitives.md",
        "https://raw.githubusercontent.com/ponylang/pony-tutorial/main/docs/types/actors.md",
        "https://raw.githubusercontent.com/ponylang/pony-tutorial/main/docs/types/traits-and-interfaces.md",
        "https://raw.githubusercontent.com/ponylang/pony-tutorial/main/docs/types/structs.md",
        "https://raw.githubusercontent.com/ponylang/pony-tutorial/main/docs/types/type-aliases.md",
        "https://raw.githubusercontent.com/ponylang/pony-tutorial/main/docs/types/type-expressions.md",
        "https://raw.githubusercontent.com/ponylang/pony-tutorial/main/docs/expressions/literals.md",
        "https://raw.githubusercontent.com/ponylang/pony-tutorial/main/docs/expressions/variables.md",
        "https://raw.githubusercontent.com/ponylang/pony-tutorial/main/docs/expressions/ops.md",
        "https://raw.githubusercontent.com/ponylang/pony-tutorial/main/docs/expressions/arithmetic.md",
        "https://raw.githubusercontent.com/ponylang/pony-tutorial/main/docs/expressions/control-structures.md",
        "https://raw.githubusercontent.com/ponylang/pony-tutorial/main/docs/expressions/methods.md",
        "https://raw.githubusercontent.com/ponylang/pony-tutorial/main/docs/expressions/errors.md",
        "https://raw.githubusercontent.com/ponylang/pony-tutorial/main/docs/expressions/equality.md",
        "https://raw.githubusercontent.com/ponylang/pony-tutorial/main/docs/expressions/sugar.md",
        "https://raw.githubusercontent.com/ponylang/pony-tutorial/main/docs/expressions/object-literals.md",
        "https://raw.githubusercontent.com/ponylang/pony-tutorial/main/docs/expressions/partial-application.md",
        "https://raw.githubusercontent.com/ponylang/pony-tutorial/main/docs/expressions/match.md",
        "https://raw.githubusercontent.com/ponylang/pony-tutorial/main/docs/expressions/as.md",
        "https://raw.githubusercontent.com/ponylang/pony-tutorial/main/docs/reference-capabilities/reference-capabilities.md",
        "https://raw.githubusercontent.com/ponylang/pony-tutorial/main/docs/reference-capabilities/guarantees.md",
        "https://raw.githubusercontent.com/ponylang/pony-tutorial/main/docs/reference-capabilities/consume-and-destructive-read.md",
        "https://raw.githubusercontent.com/ponylang/pony-tutorial/main/docs/reference-capabilities/recovering-capabilities.md",
        "https://raw.githubusercontent.com/ponylang/pony-tutorial/main/docs/reference-capabilities/aliasing.md",
        "https://raw.githubusercontent.com/ponylang/pony-tutorial/main/docs/reference-capabilities/passing-and-sharing.md",
        "https://raw.githubusercontent.com/ponylang/pony-tutorial/main/docs/generics/index.md",
        "https://raw.githubusercontent.com/ponylang/pony-tutorial/main/docs/generics/using-generics.md",
        "https://raw.githubusercontent.com/ponylang/pony-tutorial/main/docs/modules/use-statement.md",
    ], "", "ponylang"),
    g(&["wren"], "Wren language guide", "MIT", "https://github.com/wren-lang/wren/blob/main/LICENSE", &[
        "https://wren.io/syntax.html",
        "https://wren.io/values.html",
        "https://wren.io/lists.html",
        "https://wren.io/maps.html",
        "https://wren.io/method-calls.html",
        "https://wren.io/control-flow.html",
        "https://wren.io/variables.html",
        "https://wren.io/functions.html",
        "https://wren.io/classes.html",
        "https://wren.io/concurrency.html",
        "https://wren.io/error-handling.html",
    ], "main", "wren").drop("nav, header, footer"),
    gi(&["gren"], "The Gren Book", "BSD-3-Clause", "https://github.com/gren-lang/gren-lang.org", ("https://gren-lang.org/book/", "a[href^='/book/']"), "main, article", "gren").drop("nav, header, footer"),
    g(&["hoon"], "Hoon (Urbit docs)", "MIT", "https://github.com/urbit/docs.urbit.org", &[
        "https://raw.githubusercontent.com/urbit/docs.urbit.org/master/content/hoon/basic.md",
        "https://raw.githubusercontent.com/urbit/docs.urbit.org/master/content/hoon/cheat-sheet.md",
        "https://raw.githubusercontent.com/urbit/docs.urbit.org/master/content/hoon/advanced.md",
        "https://raw.githubusercontent.com/urbit/docs.urbit.org/master/content/hoon/irregular.md",
        "https://raw.githubusercontent.com/urbit/docs.urbit.org/master/content/hoon/auras.md",
        "https://raw.githubusercontent.com/urbit/docs.urbit.org/master/content/hoon/aura-guide.md",
        "https://raw.githubusercontent.com/urbit/docs.urbit.org/master/content/hoon/generators.md",
        "https://raw.githubusercontent.com/urbit/docs.urbit.org/master/content/hoon/hoon-errors.md",
    ], "", "hoon"),
    gi(&["sway"], "The Sway language (Fuel)", "Apache-2.0", "https://github.com/FuelLabs/sway/blob/master/LICENSE", ("https://docs.fuel.network/docs/sway/", "a[href^='/docs/sway/basics/'], a[href^='/docs/sway/introduction/']"), "main article, main", "sway").drop("nav, aside, header, footer"),
    g(&["circom"], "The circom language", "GPL-3.0", "https://github.com/iden3/circom/blob/master/COPYING", &[
        "https://raw.githubusercontent.com/iden3/circom/master/mkdocs/docs/circom-language/signals.md",
        "https://raw.githubusercontent.com/iden3/circom/master/mkdocs/docs/circom-language/variables-and-mutability.md",
        "https://raw.githubusercontent.com/iden3/circom/master/mkdocs/docs/circom-language/constraint-generation.md",
        "https://raw.githubusercontent.com/iden3/circom/master/mkdocs/docs/circom-language/templates-and-components.md",
        "https://raw.githubusercontent.com/iden3/circom/master/mkdocs/docs/circom-language/control-flow.md",
        "https://raw.githubusercontent.com/iden3/circom/master/mkdocs/docs/circom-language/data-types.md",
        "https://raw.githubusercontent.com/iden3/circom/master/mkdocs/docs/circom-language/basic-operators.md",
        "https://raw.githubusercontent.com/iden3/circom/master/mkdocs/docs/circom-language/the-main-component.md",
        "https://raw.githubusercontent.com/iden3/circom/master/mkdocs/docs/circom-language/include.md",
    ], "", "circom"),
    g(&["clarity"], "Clarity functions reference (Stacks)", "© Stacks; read from the site, not redistributed", "https://docs.stacks.co/", &["https://docs.stacks.co/reference/functions"], "main, article", "clarity").drop("nav, aside, header, footer"),
    gi(&["tact"], "The Tact Book", "CC BY 4.0", "https://github.com/tact-lang/tact-docs", ("https://docs.tact-lang.org/book/", "a[href^='/book/']"), "main article, main", "tact").drop("nav, aside, header, footer"),
    g(&["quint"], "The Quint language", "Apache-2.0", "https://github.com/informalsystems/quint/blob/main/LICENSE", &["https://quint-lang.org/docs/lang"], "main article, main", "quint").drop("nav, aside, header, footer"),
    g(&["alloy"], "Alloy documentation: the language", "MIT", "https://github.com/AlloyTools/alloy", &[
        "https://alloy.readthedocs.io/en/latest/intro.html",
        "https://alloy.readthedocs.io/en/latest/language/signatures.html",
        "https://alloy.readthedocs.io/en/latest/language/sets-and-relations.html",
        "https://alloy.readthedocs.io/en/latest/language/expressions-and-constraints.html",
        "https://alloy.readthedocs.io/en/latest/language/predicates-and-functions.html",
        "https://alloy.readthedocs.io/en/latest/language/commands.html",
        "https://alloy.readthedocs.io/en/latest/language/time.html",
        "https://alloy.readthedocs.io/en/latest/language/modules.html",
    ], "div[role=main], div.body", "alloy"),
    g(&["spade"], "The Spade language", "EUPL-1.2", "https://gitlab.com/spade-lang/spade-docs", &["https://docs.spade-lang.org/print.html"], "main", "spade").drop(".buttons"),
    g(&["slang"], "Slang User Guide", "MIT or Apache-2.0", "https://github.com/shader-slang/slang/blob/master/LICENSE", &[
        "https://raw.githubusercontent.com/shader-slang/slang/master/docs/user-guide/00-introduction.md",
        "https://raw.githubusercontent.com/shader-slang/slang/master/docs/user-guide/01-get-started.md",
        "https://raw.githubusercontent.com/shader-slang/slang/master/docs/user-guide/02-conventional-features.md",
        "https://raw.githubusercontent.com/shader-slang/slang/master/docs/user-guide/03-convenience-features.md",
        "https://raw.githubusercontent.com/shader-slang/slang/master/docs/user-guide/04-modules-and-access-control.md",
        "https://raw.githubusercontent.com/shader-slang/slang/master/docs/user-guide/05-capabilities.md",
        "https://raw.githubusercontent.com/shader-slang/slang/master/docs/user-guide/06-interfaces-generics.md",
        "https://raw.githubusercontent.com/shader-slang/slang/master/docs/user-guide/07-autodiff.md",
        "https://raw.githubusercontent.com/shader-slang/slang/master/docs/user-guide/08-compiling.md",
        "https://raw.githubusercontent.com/shader-slang/slang/master/docs/user-guide/09-reflection.md",
        "https://raw.githubusercontent.com/shader-slang/slang/master/docs/user-guide/09-targets.md",
    ], "", "slang"),
    g(&["sourcepawn"], "Introduction to SourcePawn", "© AlliedModders; read from the site, not redistributed", "https://wiki.alliedmods.net/", &["https://wiki.alliedmods.net/Introduction_to_SourcePawn_1.7"], "#mw-content-text", "sourcepawn").drop(".mw-editsection, .toc, #toc"),
    gi(&["supercollider"], "Getting Started With SuperCollider", "CC BY-SA 4.0", "https://github.com/supercollider/supercollider", ("https://doc.sccode.org/Tutorials/Getting-Started/00-Getting-Started-With-SC.html", "a[href*='Getting-Started/']"), "body", "supercollider").drop("#menubar, #toc, #toctitle, #related, .header, .footer").code_sel(".codeMirrorContainer"),
    g(&["opencl"], "The OpenCL C Specification", "© Khronos Group; read from the site, not redistributed", "https://registry.khronos.org/OpenCL/", &["https://registry.khronos.org/OpenCL/specs/3.0-unified/html/OpenCL_C.html"], "#content, body", "opencl").drop("#toc, #header"),
    g(&["ink"], "Writing with Ink", "MIT", "https://github.com/inkle/ink/blob/master/LICENSE.txt", &["https://raw.githubusercontent.com/inkle/ink/master/Documentation/WritingWithInk.md"], "", "ink"),
    g(&["bitbake"], "BitBake User Manual: syntax and operators", "CC BY 2.5", "https://docs.yoctoproject.org/bitbake/", &["https://docs.yoctoproject.org/bitbake/bitbake-user-manual/bitbake-user-manual-metadata.html"], "div[role=main], div.body", "bitbake"),
    g(&["snakemake"], "Snakemake: Snakefiles and rules", "MIT", "https://github.com/snakemake/snakemake/blob/main/LICENSE.md", &[
        "https://snakemake.readthedocs.io/en/stable/snakefiles/rules.html",
        "https://snakemake.readthedocs.io/en/stable/snakefiles/configuration.html",
        "https://snakemake.readthedocs.io/en/stable/snakefiles/deployment.html",
    ], "div[role=main], div.body", "snakemake"),
    g(&["earthfile"], "Earthfile reference", "MPL-2.0", "https://github.com/earthly/earthly/blob/main/LICENSE", &[
        "https://raw.githubusercontent.com/earthly/earthly/main/docs/earthfile/earthfile.md",
        "https://raw.githubusercontent.com/earthly/earthly/main/docs/earthfile/builtin-args.md",
    ], "main, article", "earthfile").drop("nav, aside, header, footer"),
    g(&["gn"], "GN Language and Operation", "BSD-3-Clause", "https://gn.googlesource.com/gn/+/main/LICENSE", &["https://gn.googlesource.com/gn/+/main/docs/language.md"], "div.doc, .Site-content, main", "gn"),
    gi(&["ld"], "GNU ld: Linker Scripts", "GNU FDL 1.3", "https://www.gnu.org/licenses/fdl-1.3.html", ("https://sourceware.org/binutils/docs/ld/Scripts.html", "ul.mini-toc a, ul.menu a"), "body", "ld").drop(GNU_DROP),
    g(&["devicetree"], "Devicetree Specification: source format", "Apache-2.0", "https://github.com/devicetree-org/devicetree-specification", &["https://devicetree-specification.readthedocs.io/en/latest/chapter6-source-language.html"], "div[role=main], div.body", "devicetree"),
    g(&["hocon"], "HOCON (Human-Optimized Config Object Notation)", "Apache-2.0", "https://github.com/lightbend/config/blob/main/LICENSE-2.0.txt", &["https://raw.githubusercontent.com/lightbend/config/main/HOCON.md"], "", "hocon"),
    g(&["dbml"], "DBML: Database Markup Language", "Apache-2.0", "https://github.com/holistics/dbml/blob/master/LICENSE", &["https://dbml.dbdiagram.io/docs/"], "article, main", "!dbml").drop("nav, aside, header, footer"),
    g(&["wit"], "The WIT format (Component Model)", "Apache-2.0", "https://github.com/bytecodealliance/component-docs", &["https://component-model.bytecodealliance.org/design/wit.html"], "main", "wit").drop(".buttons"),
    g(&["tablegen"], "TableGen Programmer's Reference", "Apache-2.0 with LLVM Exception", "https://llvm.org/LICENSE.txt", &["https://llvm.org/docs/TableGen/ProgRef.html"], "div[role=main], div.body", "tablegen").drop("#contents, .contents"),
    g(&["heex", "eex"], "EEx and HEEx templates", "Apache-2.0 (Elixir) and MIT (Phoenix LiveView)", "https://github.com/phoenixframework/phoenix_live_view/blob/main/LICENSE.md", &[
        "https://hexdocs.pm/eex/EEx.html",
        "https://hexdocs.pm/phoenix_live_view/assigns-eex.html",
    ], "#content, main", "heex").drop("#sidebar, .sidebar, footer"),
    g(&["htmldjango"], "The Django template language", "BSD-3-Clause", "https://github.com/django/django/blob/main/LICENSE", &[
        "https://docs.djangoproject.com/en/stable/ref/templates/language/",
        "https://docs.djangoproject.com/en/stable/ref/templates/builtins/",
    ], "#docs-content, article, main", "htmldjango").drop("nav, aside, header, footer"),
    g(&["tera"], "Tera templates", "MIT", "https://github.com/Keats/tera/blob/master/LICENSE", &["https://keats.github.io/tera/"], "main, article, body", "tera"),
    g(&["templ"], "templ syntax and usage", "MIT", "https://github.com/a-h/templ/blob/main/LICENSE", &[
        "https://templ.guide/syntax-and-usage/basic-syntax",
        "https://templ.guide/syntax-and-usage/elements",
        "https://templ.guide/syntax-and-usage/attributes",
        "https://templ.guide/syntax-and-usage/expressions",
        "https://templ.guide/syntax-and-usage/statements",
        "https://templ.guide/syntax-and-usage/if-else",
        "https://templ.guide/syntax-and-usage/switch",
        "https://templ.guide/syntax-and-usage/raw-go",
        "https://templ.guide/syntax-and-usage/template-composition",
        "https://templ.guide/syntax-and-usage/css-style-management",
        "https://templ.guide/syntax-and-usage/script-templates",
    ], "article, main", "templ").drop("nav, aside, header, footer"),
    g(&["blade"], "Laravel Blade templates", "MIT", "https://github.com/laravel/docs", &["https://laravel.com/docs/12.x/blade"], "#main-content, .docs_main, main, article", "blade").drop("nav, aside, header, footer, #table-of-contents"),
    g(&["glimmer"], "Ember component templates", "MIT", "https://github.com/ember-learn/guides-source/blob/master/LICENSE", &[
        "https://guides.emberjs.com/release/components/",
        "https://guides.emberjs.com/release/components/introducing-components",
        "https://guides.emberjs.com/release/components/component-arguments-and-html-attributes",
        "https://guides.emberjs.com/release/components/conditional-content",
        "https://guides.emberjs.com/release/components/block-content",
        "https://guides.emberjs.com/release/components/helper-functions",
        "https://guides.emberjs.com/release/components/looping-through-lists",
    ], "main article, main, article", "glimmer").drop("nav, aside, header, footer"),
    g(&["markdoc"], "Markdoc syntax", "MIT", "https://github.com/markdoc/markdoc/blob/main/LICENSE", &[
        "https://markdoc.dev/docs/syntax",
        "https://markdoc.dev/docs/nodes",
        "https://markdoc.dev/docs/tags",
        "https://markdoc.dev/docs/attributes",
        "https://markdoc.dev/docs/variables",
        "https://markdoc.dev/docs/functions",
    ], "main article, main, article", "markdoc").drop("nav, aside, header, footer"),
    g(&["djot"], "Djot syntax reference", "MIT", "https://github.com/jgm/djot/blob/main/LICENSE", &["https://raw.githubusercontent.com/jgm/djot/main/doc/syntax.md"], "", "djot"),
    g(&["bibtex"], "BibTeX format explained", "© bibtex.com; read from the site, not redistributed", "https://www.bibtex.com/", &[
        "https://www.bibtex.com/g/bibtex-format/",
        "https://www.bibtex.com/e/entry-types/",
        "https://www.bibtex.com/format/",
    ], "article, main", "bibtex").drop("nav, aside, header, footer"),
    g(&["query"], "Tree-sitter queries", "MIT", "https://github.com/tree-sitter/tree-sitter/blob/master/LICENSE", &[
        "https://tree-sitter.github.io/tree-sitter/using-parsers/queries/1-syntax.html",
        "https://tree-sitter.github.io/tree-sitter/using-parsers/queries/2-operators.html",
        "https://tree-sitter.github.io/tree-sitter/using-parsers/queries/3-predicates-and-directives.html",
        "https://tree-sitter.github.io/tree-sitter/using-parsers/queries/4-api.html",
    ], "main", "query").drop(".buttons"),
    g(&["rust-format-args"], "Rust std::fmt (format strings)", "MIT or Apache-2.0", "https://www.rust-lang.org/policies/licenses", &["https://doc.rust-lang.org/std/fmt/index.html"], "#main-content", "rust"),
    g(&["embedded-template"], "ERB templates (Ruby)", "Ruby License or BSD-2-Clause", "https://www.ruby-lang.org/en/about/license.txt", &["https://docs.ruby-lang.org/en/master/ERB.html"], "main", "embedded-template"),
    g(&["csv"], "CSV (Python's csv module)", "PSF License 2.0", "https://docs.python.org/3/license.html", &["https://docs.python.org/3/library/csv.html"], "div.body", "csv"),
    g(&["ini"], "INI files (Python's configparser)", "PSF License 2.0", "https://docs.python.org/3/license.html", &["https://docs.python.org/3/library/configparser.html"], "div.body", "ini"),
    g(&["properties"], "Java .properties files", "© Oracle; read from the site, not redistributed", "https://www.oracle.com/legal/copyright.html", &["https://docs.oracle.com/en/java/javase/21/docs/api/java.base/java/util/Properties.html"], "main, .block, body", "properties").drop("nav, header, footer, .sub-nav"),
    g(&["po"], "GNU gettext: The Format of PO Files", "GNU FDL 1.3", "https://www.gnu.org/licenses/fdl-1.3.html", &["https://www.gnu.org/software/gettext/manual/gettext.html"], "div#PO-Files", "po").drop(GNU_DROP),
    g(&["diff"], "GNU diffutils: unified and context diffs", "GNU FDL 1.3", "https://www.gnu.org/licenses/fdl-1.3.html", &[
        "https://www.gnu.org/software/diffutils/manual/html_node/Unified-Format.html",
        "https://www.gnu.org/software/diffutils/manual/html_node/Detailed-Unified.html",
        "https://www.gnu.org/software/diffutils/manual/html_node/Context-Format.html",
        "https://www.gnu.org/software/diffutils/manual/html_node/Detailed-Context.html",
        "https://www.gnu.org/software/diffutils/manual/html_node/Hunks.html",
    ], "body", "diff").drop(GNU_DROP),
    g(&["todotxt"], "The todo.txt format", "GPL-3.0", "https://github.com/todotxt/todo.txt", &["https://raw.githubusercontent.com/todotxt/todo.txt/master/README.md"], "", "todotxt"),
    g(&["hosts"], "Linux manual page: hosts(5)", "See the page's colophon (man-pages project and upstream)", "https://man7.org/linux/man-pages/", &["https://man7.org/linux/man-pages/man5/hosts.5.html"], "body", "hosts").drop("table.top, table.sec-table, .nav-bar, .footer, #top-nav"),
    g(&["passwd"], "Linux manual page: passwd(5)", "See the page's colophon (man-pages project and upstream)", "https://man7.org/linux/man-pages/", &["https://man7.org/linux/man-pages/man5/passwd.5.html"], "body", "passwd").drop("table.top, table.sec-table, .nav-bar, .footer, #top-nav"),
    g(&["strace"], "Linux manual page: strace(1)", "See the page's colophon (man-pages project and upstream)", "https://man7.org/linux/man-pages/", &["https://man7.org/linux/man-pages/man1/strace.1.html"], "body", "strace").drop("table.top, table.sec-table, .nav-bar, .footer, #top-nav"),
    g(&["sshclientconfig"], "Linux manual page: ssh_config(5)", "See the page's colophon (man-pages project and upstream)", "https://man7.org/linux/man-pages/", &["https://man7.org/linux/man-pages/man5/ssh_config.5.html"], "body", "sshclientconfig").drop("table.top, table.sec-table, .nav-bar, .footer, #top-nav"),
    g(&["debian"], "Debian Policy: control files and their fields", "GPL-2.0-or-later", "https://www.debian.org/doc/debian-policy/", &[
        "https://www.debian.org/doc/debian-policy/ch-controlfields.html",
        "https://www.debian.org/doc/debian-policy/ch-relationships.html",
    ], "div[role=main], div.body", "debian"),
    g(&["dunstrc"], "dunst configuration", "BSD-3-Clause", "https://github.com/dunst-project/dunst/blob/master/LICENSE", &["https://dunst-project.org/documentation/"], "main, article, body", "dunstrc").drop("nav, header, footer"),
    gi(&["hyprlang"], "Hyprland wiki: Configuring", "BSD-3-Clause", "https://github.com/hyprwm/hyprland-wiki", ("https://wiki.hypr.land/configuring/", "a[href^='/configuring/']"), "main#content, main article, main, article", "hyprlang").drop("nav, aside, header, footer, main#content > div:first-child, main#content > div:last-child"),
    g(&["ghostty"], "Ghostty configuration", "MIT", "https://github.com/ghostty-org/ghostty/blob/main/LICENSE", &[
        "https://ghostty.org/docs/config",
        "https://ghostty.org/docs/config/reference",
    ], "main article, main, article", "ghostty").drop("nav, aside, header, footer"),
    g(&["jjdescription"], "Jujutsu tutorial", "Apache-2.0", "https://github.com/jj-vcs/jj/blob/main/LICENSE", &["https://raw.githubusercontent.com/jj-vcs/jj/main/docs/tutorial.md | https://raw.githubusercontent.com/martinvonz/jj/main/docs/tutorial.md"], "", "jjdescription"),
    g(&["gemini"], "Gemtext", "CC BY 4.0", "https://geminiprotocol.net/", &["https://geminiprotocol.net/docs/gemtext.gmi"], "", "gemini"),
    g(&["vhs"], "VHS tapes", "MIT", "https://github.com/charmbracelet/vhs/blob/main/LICENSE", &["https://raw.githubusercontent.com/charmbracelet/vhs/main/README.md"], "", "vhs"),
    gi(&["task"], "Task (Taskfile) guide", "MIT", "https://github.com/go-task/task/blob/main/LICENSE", ("https://taskfile.dev/docs/guide", "a[href^='/docs/guide']"), "main article, main, article", "task").drop("nav, aside, header, footer"),
    g(&["xit"], "[x]it! specification", "CC0-1.0", "https://github.com/jotaen/xit", &["https://raw.githubusercontent.com/jotaen/xit/main/Specification.md"], "", "xit"),
    g(&["textproto"], "Protocol Buffers text format", "CC BY 4.0", "https://github.com/protocolbuffers/protocolbuffers.github.io/blob/main/LICENSE", &["https://raw.githubusercontent.com/protocolbuffers/protocolbuffers.github.io/main/content/reference/protobuf/textformat-spec.md | https://protobuf.dev/reference/protobuf/textformat-spec/"], "main, article", "textproto"),
    g(&["gpr"], "GNAT Project Manager (GPR files)", "GNU FDL 1.3", "https://docs.adacore.com/", &["https://docs.adacore.com/gprbuild-docs/html/gprbuild_ug/gnat_project_manager.html"], "div[role=main], div.body", "gpr"),
    g(&["ungrammar"], "Ungrammar", "MIT or Apache-2.0", "https://github.com/rust-analyzer/ungrammar", &[
        "https://raw.githubusercontent.com/rust-analyzer/ungrammar/master/README.md",
        "https://rust-analyzer.github.io/blog/2020/10/24/introducing-ungrammar.html",
    ], "article, main, body", "ungrammar"),
    g(&["ohm"], "Ohm syntax reference", "MIT", "https://github.com/ohmjs/ohm/blob/main/LICENSE", &["https://ohmjs.org/docs/syntax-reference"], "main article, main", "!ohm").drop("nav, aside, header, footer"),
    g(&["fidl"], "FIDL language specification", "CC BY 4.0", "https://fuchsia.dev/", &["https://fuchsia.dev/fuchsia-src/reference/fidl/language/language"], "devsite-content article, article, main", "fidl").drop("devsite-toc, nav, aside, header, footer, devsite-feedback"),
    g(&["spicedb"], "SpiceDB schema language", "Apache-2.0", "https://github.com/authzed/docs", &["https://authzed.com/docs/spicedb/concepts/schema"], "main article, main", "spicedb").drop("nav, aside, header, footer"),
    g(&["fga"], "OpenFGA configuration language", "Apache-2.0", "https://github.com/openfga/openfga.dev", &[
        "https://openfga.dev/docs/configuration-language",
        "https://openfga.dev/docs/modeling/getting-started",
    ], "main article, main", "fga").drop("nav, aside, header, footer"),
    g(&["smali"], "smali wiki", "BSD-3-Clause", "https://github.com/JesusFreke/smali", &[
        "https://github.com/JesusFreke/smali/wiki/Registers",
        "https://github.com/JesusFreke/smali/wiki/TypesMethodsAndFields",
        "https://github.com/JesusFreke/smali/wiki",
    ], "#wiki-body, .markdown-body", "smali"),
    g(&["llvm-mir"], "Machine IR (MIR) Format Reference", "Apache-2.0 with LLVM Exception", "https://llvm.org/LICENSE.txt", &["https://llvm.org/docs/MIRLangRef.html"], "div[role=main], div.body", "llvm-mir").drop("#contents, .contents"),
    g(&["cylc"], "Cylc: workflow configuration", "GPL-3.0", "https://github.com/cylc/cylc-doc", &["https://cylc.github.io/cylc-doc/stable/html/user-guide/writing-workflows/configuration.html"], "div[role=main], div.body, article", "cylc"),
    g(&["edoc"], "EDoc user's guide", "Apache-2.0", "https://www.erlang.org/", &["https://www.erlang.org/doc/apps/edoc/chapter.html"], "#content, main", "edoc"),
    g(&["ldif"], "LDIF: the LDAP Data Interchange Format", "© Ping Identity (ldap.com); read from the site, not redistributed", "https://ldap.com/", &["https://ldap.com/ldif-the-ldap-data-interchange-format/"], "article, main", "ldif").drop("nav, aside, header, footer"),
    g(&["adl"], "ADL: the Algebraic Data Language", "BSD-3-Clause", "https://github.com/adl-lang/adl/blob/master/LICENSE", &[
        "https://raw.githubusercontent.com/adl-lang/adl/master/docs/introduction.md",
        "https://raw.githubusercontent.com/adl-lang/adl/master/docs/language.md",
    ], "", "adl"),
    g(&["bass"], "Bass", "MIT", "https://github.com/vito/bass/blob/main/LICENSE", &["https://bass-lang.org/"], "main, body", "bass").drop("nav, header, footer"),
    g(&["ql"], "QL language reference (CodeQL)", "CC BY 4.0", "https://github.com/github/codeql/blob/main/LICENSE", &[
        "https://codeql.github.com/docs/ql-language-reference/about-the-ql-language/",
        "https://codeql.github.com/docs/ql-language-reference/predicates/",
        "https://codeql.github.com/docs/ql-language-reference/queries/",
        "https://codeql.github.com/docs/ql-language-reference/types/",
        "https://codeql.github.com/docs/ql-language-reference/modules/",
        "https://codeql.github.com/docs/ql-language-reference/expressions/",
        "https://codeql.github.com/docs/ql-language-reference/formulas/",
        "https://codeql.github.com/docs/ql-language-reference/aliases/",
        "https://codeql.github.com/docs/ql-language-reference/variables/",
    ], "div[role=main], div.body, main", "ql"),
    g(&["werk"], "The Werk book", "MIT or Apache-2.0", "https://github.com/simonask/werk", &[
        "https://raw.githubusercontent.com/simonask/werk/main/book/src/introduction.md",
        "https://raw.githubusercontent.com/simonask/werk/main/book/src/getting_started.md",
        "https://raw.githubusercontent.com/simonask/werk/main/book/src/first_example.md",
        "https://raw.githubusercontent.com/simonask/werk/main/book/src/language.md",
        "https://raw.githubusercontent.com/simonask/werk/main/book/src/language/variables.md",
        "https://raw.githubusercontent.com/simonask/werk/main/book/src/language/strings.md",
        "https://raw.githubusercontent.com/simonask/werk/main/book/src/language/arrays.md",
        "https://raw.githubusercontent.com/simonask/werk/main/book/src/language/expressions.md",
        "https://raw.githubusercontent.com/simonask/werk/main/book/src/language/patterns.md",
        "https://raw.githubusercontent.com/simonask/werk/main/book/src/language/recipe_commands.md",
        "https://raw.githubusercontent.com/simonask/werk/main/book/src/language/builtins.md",
        "https://raw.githubusercontent.com/simonask/werk/main/book/src/language/include.md",
        "https://raw.githubusercontent.com/simonask/werk/main/book/src/language/config.md",
        "https://raw.githubusercontent.com/simonask/werk/main/book/src/build_recipes.md",
        "https://raw.githubusercontent.com/simonask/werk/main/book/src/task_recipes.md",
        "https://raw.githubusercontent.com/simonask/werk/main/book/src/paths.md",
        "https://raw.githubusercontent.com/simonask/werk/main/book/src/examples/c.md",
        "https://raw.githubusercontent.com/simonask/werk/main/book/src/examples/cargo.md",
    ], "", "werk"),
    g(&["esdl"], "Gel (EdgeDB) schema language", "Apache-2.0", "https://github.com/geldata/gel/blob/master/LICENSE", &["https://docs.geldata.com/reference/datamodel"], "main article, main", "esdl").drop("nav, aside, header, footer"),
    g(&["dtd"], "XML 1.0: document type declarations", "W3C Document License", "https://www.w3.org/copyright/document-license-2023/", &["https://www.w3.org/TR/xml/"], "body", "dtd").drop(".head, .toc, #toc"),
    g(&["pod"], "perlpod: the Plain Old Documentation format", "Artistic License or GPL", "https://perldoc.perl.org/perlartistic", &[
        "https://perldoc.perl.org/perlpod",
        "https://perldoc.perl.org/perlpodspec",
    ], "#perldocdiv, main", "pod").drop("a.permalink, h1#toc, h1#toc + ul, #more, #breadcrumbs"),
    g(&["blueprint"], "Blueprint syntax reference", "LGPL-3.0", "https://gitlab.gnome.org/GNOME/blueprint-compiler", &[
        "https://gitlab.gnome.org/GNOME/blueprint-compiler/-/raw/main/docs/reference/index.rst",
        "https://gitlab.gnome.org/GNOME/blueprint-compiler/-/raw/main/docs/reference/document_root.rst",
        "https://gitlab.gnome.org/GNOME/blueprint-compiler/-/raw/main/docs/reference/objects.rst",
        "https://gitlab.gnome.org/GNOME/blueprint-compiler/-/raw/main/docs/reference/templates.rst",
        "https://gitlab.gnome.org/GNOME/blueprint-compiler/-/raw/main/docs/reference/values.rst",
        "https://gitlab.gnome.org/GNOME/blueprint-compiler/-/raw/main/docs/reference/expressions.rst",
        "https://gitlab.gnome.org/GNOME/blueprint-compiler/-/raw/main/docs/reference/menus.rst",
        "https://gitlab.gnome.org/GNOME/blueprint-compiler/-/raw/main/docs/reference/extensions.rst",
        "https://gitlab.gnome.org/GNOME/blueprint-compiler/-/raw/main/docs/reference/diagnostics.rst",
    ], "", "blueprint"),
    g(&["haskell-persistent"], "Persistent (Yesod book)", "CC BY 4.0", "https://www.yesodweb.com/book", &["https://www.yesodweb.com/book/persistent"], "article, main, #content", "haskell"),
    g(&["beancount"], "Beancount language syntax", "© Martin Blais; read from the site, not redistributed", "https://beancount.github.io/", &["https://beancount.github.io/docs/beancount_language_syntax/"], "main, article", "!beancount").drop("nav, aside, header, footer"),
    g(&["yuck"], "eww configuration (yuck)", "MIT", "https://github.com/elkowar/eww/blob/master/LICENSE", &[
        "https://elkowar.github.io/eww/configuration.html",
        "https://elkowar.github.io/eww/expression_language.html",
        "https://elkowar.github.io/eww/widgets.html",
    ], "main", "yuck").drop(".buttons"),
    g(&["vento"], "Vento templates", "MIT", "https://github.com/ventojs/vento/blob/main/LICENSE", &[
        "https://vento.js.org/syntax/",
        "https://vento.js.org/syntax/print/",
        "https://vento.js.org/syntax/for/",
        "https://vento.js.org/syntax/if/",
        "https://vento.js.org/syntax/include/",
        "https://vento.js.org/syntax/layout/",
        "https://vento.js.org/syntax/set/",
        "https://vento.js.org/syntax/function/",
        "https://vento.js.org/syntax/pipes/",
    ], "main, article", "vento").drop("nav, aside, header, footer"),
    g(&["godot_resource"], "Godot's TSCN scene file format", "CC BY 3.0", "https://docs.godotengine.org/en/stable/about/license.html", &["https://docs.godotengine.org/en/stable/engine_details/file_formats/tscn.html | https://docs.godotengine.org/en/stable/contributing/development/file_formats/tscn.html"], "div[role=main], div.body", "godot_resource"),
    g(&["json"], "MDN: Working with JSON", "CC BY-SA 2.5", "https://developer.mozilla.org/en-US/docs/MDN/Writing_guidelines/Attrib_copyright_license",
        &["https://raw.githubusercontent.com/mdn/content/main/files/en-us/learn_web_development/core/scripting/json/index.md"], "", "json"),
    g(&["yaml", "yml"], "YAML 1.2.2 Specification", "Free to copy (the YAML spec's own terms)", "https://yaml.org/spec/1.2.2/",
        &["https://yaml.org/spec/1.2.2/"], "main, article, body", "yaml").drop("#toc, #markdown-toc, nav"),
    g(&["toml"], "TOML v1.0.0", "MIT", "https://github.com/toml-lang/toml/blob/main/LICENSE",
        &["https://raw.githubusercontent.com/toml-lang/toml/1.0.0/toml.md"], "", "toml"),
    g(&["html"], "MDN: Structuring content with HTML", "CC BY-SA 2.5", "https://developer.mozilla.org/en-US/docs/MDN/Writing_guidelines/Attrib_copyright_license", &[
        "https://raw.githubusercontent.com/mdn/content/main/files/en-us/learn_web_development/core/structuring_content/basic_html_syntax/index.md",
        "https://raw.githubusercontent.com/mdn/content/main/files/en-us/learn_web_development/core/structuring_content/webpage_metadata/index.md",
        "https://raw.githubusercontent.com/mdn/content/main/files/en-us/learn_web_development/core/structuring_content/headings_and_paragraphs/index.md",
        "https://raw.githubusercontent.com/mdn/content/main/files/en-us/learn_web_development/core/structuring_content/lists/index.md",
        "https://raw.githubusercontent.com/mdn/content/main/files/en-us/learn_web_development/core/structuring_content/structuring_documents/index.md",
        "https://raw.githubusercontent.com/mdn/content/main/files/en-us/learn_web_development/core/structuring_content/creating_links/index.md",
        "https://raw.githubusercontent.com/mdn/content/main/files/en-us/learn_web_development/core/structuring_content/html_images/index.md",
        "https://raw.githubusercontent.com/mdn/content/main/files/en-us/learn_web_development/core/structuring_content/html_forms/index.md",
    ], "", "html"),
    g(&["css"], "MDN: CSS styling basics", "CC BY-SA 2.5", "https://developer.mozilla.org/en-US/docs/MDN/Writing_guidelines/Attrib_copyright_license", &[
        "https://raw.githubusercontent.com/mdn/content/main/files/en-us/learn_web_development/core/styling_basics/what_is_css/index.md",
        "https://raw.githubusercontent.com/mdn/content/main/files/en-us/learn_web_development/core/styling_basics/getting_started/index.md",
        "https://raw.githubusercontent.com/mdn/content/main/files/en-us/learn_web_development/core/styling_basics/basic_selectors/index.md",
        "https://raw.githubusercontent.com/mdn/content/main/files/en-us/learn_web_development/core/styling_basics/box_model/index.md",
        "https://raw.githubusercontent.com/mdn/content/main/files/en-us/learn_web_development/core/styling_basics/handling_conflicts/index.md",
        "https://raw.githubusercontent.com/mdn/content/main/files/en-us/learn_web_development/core/styling_basics/values_and_units/index.md",
    ], "", "css"),
    g(&["markdown", "markdown_inline"], "GitHub Docs: Basic writing and formatting syntax", "CC BY 4.0", "https://github.com/github/docs/blob/main/LICENSE", &[
        "https://raw.githubusercontent.com/github/docs/main/content/get-started/writing-on-github/getting-started-with-writing-and-formatting-on-github/basic-writing-and-formatting-syntax.md",
        "https://raw.githubusercontent.com/github/docs/main/content/get-started/writing-on-github/working-with-advanced-formatting/creating-and-highlighting-code-blocks.md",
        "https://raw.githubusercontent.com/github/docs/main/content/get-started/writing-on-github/working-with-advanced-formatting/organizing-information-with-tables.md",
    ], "", "markdown"),
];

/// A guide by its own name, else the (first) guide for a language (its
/// name in the editor).
pub fn find(key: &str) -> Option<&'static Guide> {
    GUIDES.iter().find(|g| !g.id.is_empty() && g.id == key).or_else(|| GUIDES.iter().find(|g| g.langs.contains(&key)))
}


/// Guides there are (JSON {lang: title of its first guide}), tessh's own included.
pub fn list_json() -> String {
    let mut m = serde_json::Map::new();
    for g in GUIDES {
        for l in g.langs {
            m.entry(l.to_string()).or_insert_with(|| g.title.into());
        }
    }
    serde_json::Value::Object(m).to_string()
}

/// A language's guides, when it has more than one (JSON [[key, title]]).
pub fn choices_json(lang: &str) -> String {
    // (The language the name stands for: "c++" is C++, so is "learncpp".)
    let lang = find(lang).map(|g| g.langs[0]).unwrap_or(lang);
    let list: Vec<[&str; 2]> = GUIDES.iter().filter(|g| g.langs.contains(&lang)).map(|g| [g.key(), g.title]).collect();
    serde_json::to_string(&list).unwrap_or_default()
}

fn cache_dir() -> PathBuf {
    std::env::var_os("XDG_STATE_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/state")))
        .unwrap_or_else(std::env::temp_dir)
        .join("turbine/guides")
}

fn cache_path(g: &Guide) -> PathBuf {
    cache_dir().join(format!("{}.md", g.key()))
}

/// Marks the kept Markdown: a guide converted by an older Turbine is
/// fetched and converted again.
const CONVERT_VERSION: &str = "<!-- turbine-guide-convert 1 -->";

/// Is this guide kept already (readable offline)?
pub fn is_kept(lang: &str) -> bool {
    find(lang).is_some_and(|g| std::fs::read_to_string(cache_path(g)).is_ok_and(|t| t.starts_with(CONVERT_VERSION)))
}

/// A guide as Markdown, fetched and converted the first time (or with
/// `refresh`), then kept in the state folder. Slow the first time: call it
/// off the UI thread.
pub fn guide_markdown(lang: &str, refresh: bool) -> Result<String, String> {
    let g = find(lang).ok_or_else(|| format!("No guide for {lang}."))?;
    // (tessh's come from the tessh installed, so they match it: kept only
    // for when it isn't there.)
    if let Some(topic) = g.pages.first().and_then(|p| p.strip_prefix("tessh:")) {
        let path = cache_path(g);
        return match tessh_guide(topic) {
            Ok(md) => {
                let _ = std::fs::create_dir_all(cache_dir());
                let _ = std::fs::write(&path, format!("{CONVERT_VERSION}\n{md}"));
                Ok(md)
            }
            Err(e) => std::fs::read_to_string(&path).map(|t| t.strip_prefix(CONVERT_VERSION).unwrap_or(&t).trim_start().to_string()).map_err(|_| e),
        };
    }
    let path = cache_path(g);
    let kept = std::fs::read_to_string(&path).ok();
    tidy_once();
    if !refresh {
        if let (Some(md), Ok(age)) = (&kept, std::fs::metadata(&path).and_then(|m| m.modified()).map(|t| t.elapsed().unwrap_or_default())) {
            let stale = age > std::time::Duration::from_secs(30 * 86400) || (md.contains("Internet Archive's copy") && age > std::time::Duration::from_secs(86400));
            if stale && md.starts_with(CONVERT_VERSION) {
                refresh_in_background(g);
            }
        }
    }
    let md = match kept {
        Some(md) if !refresh && md.starts_with(CONVERT_VERSION) => md,
        _ => {
            set_progress(g.key(), "Fetching…");
            let md = match fetch_markdown(g) {
                Ok(md) => format!("{CONVERT_VERSION}\n{md}"),
                Err(e) => match kept {
                    Some(old) if !refresh => old,
                    _ => {
                        set_progress(g.key(), "");
                        return Err(e);
                    }
                },
            };
            let _ = std::fs::create_dir_all(cache_dir());
            let tmp = path.with_extension("md.part");
            if std::fs::write(&tmp, &md).is_ok() {
                let _ = std::fs::rename(&tmp, &path);
            }
            set_progress(g.key(), "");
            md
        }
    };
    Ok(md.strip_prefix(CONVERT_VERSION).map(|m| m.trim_start_matches('\n').to_string()).unwrap_or(md))
}

/// A tessh guide from `tessh --guide [topic]`, else (the main one) from
/// GitHub.
fn tessh_guide(topic: &str) -> Result<String, String> {
    let mut cmd = std::process::Command::new("tessh");
    cmd.arg("--guide");
    if !topic.is_empty() {
        cmd.arg(topic);
    }
    let out = cmd.env("NO_COLOR", "1").stdin(std::process::Stdio::null()).output();
    let clean = |t: &str| t.lines().filter(|l| !l.starts_with("<!-- help:")).collect::<Vec<_>>().join("\n");
    match out {
        Ok(o) if o.status.success() && o.stdout.len() > 100 => Ok(clean(&String::from_utf8_lossy(&o.stdout))),
        _ if topic.is_empty() => {
            let url = "https://raw.githubusercontent.com/benwbooth/tessera/main/tessh/doc/guide.md";
            ureq::get(url).call().map_err(|e| format!("tessh isn't installed, and {url}: {e}"))?.body_mut().read_to_string().map(|t| clean(&t)).map_err(|e| e.to_string())
        }
        _ => Err("Install tessh to read this guide (nix run github:benwbooth/tessera#tessh)".into()),
    }
}

/// What a guide being fetched is doing now ("" when nothing).
pub fn progress(lang: &str) -> String {
    let key = find(lang).map(|g| g.key()).unwrap_or(lang);
    progress_map().lock().unwrap().get(key).cloned().unwrap_or_default()
}

fn progress_map() -> &'static std::sync::Mutex<std::collections::HashMap<String, String>> {
    static P: std::sync::OnceLock<std::sync::Mutex<std::collections::HashMap<String, String>>> = std::sync::OnceLock::new();
    P.get_or_init(Default::default)
}

fn set_progress(key: &str, what: &str) {
    let mut m = progress_map().lock().unwrap();
    if what.is_empty() {
        m.remove(key);
    } else {
        m.insert(key.to_string(), what.to_string());
    }
}

/// A guide fetched again without anyone waiting; kept only when whole.
fn refresh_in_background(g: &'static Guide) {
    static BUSY: std::sync::Mutex<Vec<&str>> = std::sync::Mutex::new(Vec::new());
    {
        let mut b = BUSY.lock().unwrap();
        if b.contains(&g.key()) {
            return;
        }
        b.push(g.key());
    }
    std::thread::spawn(move || {
        if let Ok(md) = fetch_markdown(g) {
            if !md.contains("couldn't be fetched") {
                let path = cache_path(g);
                let tmp = path.with_extension("md.part");
                if std::fs::write(&tmp, format!("{CONVERT_VERSION}\n{md}")).is_ok() {
                    let _ = std::fs::rename(&tmp, &path);
                }
            }
        }
        BUSY.lock().unwrap().retain(|k| *k != g.key());
    });
}

/// Kept files no guide uses any more, removed: once a run.
fn tidy_once() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        let names: std::collections::HashSet<&str> = GUIDES.iter().map(|g| g.key()).collect();
        let Ok(rd) = std::fs::read_dir(cache_dir()) else { return };
        for e in rd.flatten() {
            let name = e.file_name().to_string_lossy().to_string();
            let stem = name.split('.').next().unwrap_or("");
            if (name.ends_with(".md") || name.ends_with(".md.part")) && !names.contains(stem) {
                let _ = std::fs::remove_file(e.path());
            }
        }
    });
}

/// Every guide fetched now, for reading offline (kept ones skipped).
/// `progress(done, total, failed)`. Returns (fetched, failed names).
pub fn download_all(progress: impl Fn(usize, usize, usize) + Sync) -> (usize, Vec<String>) {
    let todo: Vec<&'static Guide> = GUIDES.iter().filter(|g| !is_kept(g.key())).collect();
    let total = todo.len();
    let done = std::sync::atomic::AtomicUsize::new(0);
    let failed = std::sync::Mutex::new(Vec::new());
    std::thread::scope(|s| {
        for chunk in todo.chunks(total.div_ceil(4).max(1)) {
            let (done, failed, progress) = (&done, &failed, &progress);
            s.spawn(move || {
                for g in chunk {
                    if guide_markdown(g.key(), false).is_err() {
                        failed.lock().unwrap().push(g.title.to_string());
                    }
                    let n = done.fetch_add(1, std::sync::atomic::Ordering::SeqCst) + 1;
                    progress(n, total, failed.lock().unwrap().len());
                }
            });
        }
    });
    let failed = failed.into_inner().unwrap();
    (total - failed.len(), failed)
}

/// Markdown with its headings a level down (not lines in code blocks).
fn demote(md: &str) -> String {
    let mut fence: Option<&str> = None;
    let mut out = String::with_capacity(md.len() + 256);
    for line in md.split_inclusive('\n') {
        let t = line.trim_start();
        let mark = if t.starts_with("```") { Some("```") } else if t.starts_with("~~~") { Some("~~~") } else { None };
        match (fence, mark) {
            (None, Some(m)) => fence = Some(m),
            (Some(f), Some(m)) if f == m => fence = None,
            _ => {}
        }
        if fence.is_none() && mark.is_none() && line.starts_with('#') && line.trim_start_matches('#').starts_with(' ') && line.find(' ').unwrap_or(0) < 6 {
            out.push('#');
        }
        out.push_str(line);
    }
    out
}

/// Today's date (UTC), for the note at the top of a guide.
fn today() -> String {
    let days = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() / 86400).unwrap_or(0) as i64;
    // (Civil from days, Howard Hinnant's way.)
    let z = days + 719468;
    let era = z.div_euclid(146097);
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + if m <= 2 { 1 } else { 0 };
    format!("{y:04}-{m:02}-{d:02}")
}

/// A page from whichever of its addresses answers first ("A | B": B is
/// another copy of the same page; both are asked at once, so a site that
/// doesn't answer costs no time).
fn get_any(urls: &str) -> Result<String, String> {
    let urls: Vec<String> = urls.split(" | ").map(|u| u.trim().to_string()).collect();
    if urls.len() == 1 {
        return get(&urls[0]);
    }
    let (tx, rx) = std::sync::mpsc::channel();
    for u in urls.clone() {
        let tx = tx.clone();
        std::thread::spawn(move || {
            let _ = tx.send(get(&u));
        });
    }
    drop(tx);
    let mut errs = vec![];
    for r in rx {
        match r {
            Ok(t) => return Ok(t),
            Err(e) => errs.push(e),
        }
    }
    Err(errs.join("; "))
}

/// The Wayback Machine's latest copy of a page, as the page itself (`id_`:
/// without the archive's toolbar or rewritten links).
fn archive_url(url: &str) -> String {
    format!("https://web.archive.org/web/2id_/{url}")
}

/// A page from its site; if the site fails, or hasn't answered in 5 s, the
/// Internet Archive's copy too, whichever comes first. Some(date) when it's
/// the archive's (the day it was saved).
fn get_or_archived(urls: &str) -> Result<(String, Option<String>), String> {
    let first = urls.split(" | ").next().unwrap_or(urls).trim().to_string();
    let (tx, rx) = std::sync::mpsc::channel::<(bool, Result<(String, Option<String>), String>)>();
    {
        let (tx, urls) = (tx.clone(), urls.to_string());
        std::thread::spawn(move || {
            let _ = tx.send((false, get_any(&urls).map(|t| (t, None))));
        });
    }
    let ask_archive = |tx: std::sync::mpsc::Sender<_>| {
        let first = first.clone();
        std::thread::spawn(move || {
            let _ = tx.send((true, get_archived(&first).map(|(t, d)| (t, Some(d)))));
        });
    };
    let mut archive_asked = false;
    let mut errs: Vec<String> = vec![];
    let mut pending = 1;
    loop {
        let got = if archive_asked { rx.recv().ok() } else { rx.recv_timeout(std::time::Duration::from_secs(5)).ok() };
        match got {
            Some((_, Ok(r))) => return Ok(r),
            Some((from_archive, Err(e))) => {
                pending -= 1;
                errs.push(if from_archive { format!("Internet Archive: {e}") } else { e });
                if !archive_asked {
                    archive_asked = true;
                    pending += 1;
                    ask_archive(tx.clone());
                } else if pending == 0 {
                    return Err(errs.join("; "));
                }
            }
            // (No answer yet in 5 s: the archive too.)
            None if !archive_asked => {
                archive_asked = true;
                pending += 1;
                ask_archive(tx.clone());
            }
            None => return Err(errs.join("; ")),
        }
    }
}

/// The archive's copy, and the day it was saved. (Two at a time, and
/// asked again after a pause when it refuses: it turns away bursts.)
fn get_archived(url: &str) -> Result<(String, String), String> {
    static SLOTS: (std::sync::Mutex<usize>, std::sync::Condvar) = (std::sync::Mutex::new(0), std::sync::Condvar::new());
    {
        let mut n = SLOTS.0.lock().unwrap();
        while *n >= 2 {
            n = SLOTS.1.wait(n).unwrap();
        }
        *n += 1;
    }
    struct Release;
    impl Drop for Release {
        fn drop(&mut self) {
            *SLOTS.0.lock().unwrap() -= 1;
            SLOTS.1.notify_one();
        }
    }
    let _release = Release;
    let mut wait = 2;
    loop {
        match get_archived_once(url) {
            Err(e) if wait <= 8 && (e.contains("refused") || e.contains("429") || e.contains("503")) => {
                std::thread::sleep(std::time::Duration::from_secs(wait));
                wait *= 2;
            }
            r => return r,
        }
    }
}

fn get_archived_once(url: &str) -> Result<(String, String), String> {
    let agent: ureq::Agent = ureq::Agent::config_builder().timeout_connect(Some(std::time::Duration::from_secs(10))).timeout_global(Some(std::time::Duration::from_secs(120))).build().into();
    let mut r = agent.get(&archive_url(url)).header("User-Agent", concat!("Turbine/", env!("CARGO_PKG_VERSION"), " (language guide)")).call().map_err(|e| e.to_string())?;
    let at = {
        use ureq::ResponseExt;
        let u = r.get_uri().to_string();
        u.split("/web/").nth(1).map(|t| t.chars().take_while(|c| c.is_ascii_digit()).collect::<String>()).unwrap_or_default()
    };
    let date = if at.len() >= 8 { format!("{}-{}-{}", &at[..4], &at[4..6], &at[6..8]) } else { "date unknown".into() };
    let bytes = r.body_mut().with_config().limit(32 << 20).read_to_vec().map_err(|e| e.to_string())?;
    Ok((String::from_utf8(bytes).unwrap_or_else(|e| e.into_bytes().iter().map(|&b| b as char).collect()), date))
}

fn get(url: &str) -> Result<String, String> {
    use ureq::config::IpFamily;
    let fetch = |family: IpFamily| -> Result<Vec<u8>, ureq::Error> {
        let agent: ureq::Agent = ureq::Agent::config_builder().ip_family(family).timeout_connect(Some(std::time::Duration::from_secs(8))).timeout_global(Some(std::time::Duration::from_secs(90))).build().into();
        let mut r = agent.get(url).header("User-Agent", concat!("Turbine/", env!("CARGO_PKG_VERSION"), " (language guide)")).call()?;
        r.body_mut().with_config().limit(32 << 20).read_to_vec()
    };
    // (A host with an IPv6 address that can't be reached from here: again
    // over IPv4.)
    let bytes = match fetch(IpFamily::Any) {
        Ok(b) => b,
        Err(ureq::Error::Io(_)) | Err(ureq::Error::ConnectionFailed) | Err(ureq::Error::HostNotFound) | Err(ureq::Error::Timeout(_)) => fetch(IpFamily::Ipv4Only).map_err(|e| format!("{url}: {e}"))?,
        Err(e) => return Err(format!("{url}: {e}")),
    };
    Ok(match String::from_utf8(bytes) {
        Ok(s) => s,
        // (Older pages in Latin-1.)
        Err(e) => e.into_bytes().iter().map(|&b| b as char).collect(),
    })
}

/// An address relative to a page, made whole.
pub fn join(base: &str, href: &str) -> String {
    if href.contains("://") || href.starts_with("mailto:") || href.starts_with("data:") {
        return href.to_string();
    }
    let scheme_end = base.find("://").map(|i| i + 3).unwrap_or(0);
    let origin_end = base[scheme_end..].find('/').map(|i| i + scheme_end).unwrap_or(base.len());
    if let Some(rest) = href.strip_prefix("//") {
        return format!("{}{rest}", &base[..scheme_end]);
    }
    if href.starts_with('/') {
        return format!("{}{href}", &base[..origin_end]);
    }
    let page = base.split(['#', '?']).next().unwrap_or(base);
    if href.starts_with('#') {
        return format!("{page}{href}");
    }
    if href.starts_with('?') {
        return format!("{page}{href}");
    }
    let dir = &page[..page.rfind('/').map(|i| i + 1).unwrap_or(page.len()).max(origin_end.min(page.len()))];
    let mut parts: Vec<&str> = dir[origin_end.min(dir.len())..].split('/').filter(|s| !s.is_empty()).collect();
    let (path, tail) = match href.find(['#', '?']) {
        Some(i) => (&href[..i], &href[i..]),
        None => (href, ""),
    };
    let segs: Vec<&str> = path.split('/').collect();
    for s in &segs {
        match *s {
            "." => {}
            ".." => {
                parts.pop();
            }
            "" => {}
            s => parts.push(s),
        }
    }
    let trailing = if path.ends_with('/') && !path.is_empty() { "/" } else { "" };
    let joined = parts.join("/");
    format!("{}/{}{}{}", &base[..origin_end], joined, if joined.is_empty() { "" } else { trailing }, tail).replace("//#", "/#")
}

fn strip_fragment(u: &str) -> &str {
    u.split('#').next().unwrap_or(u)
}

/// The guide's pages, fetched, as one Markdown document, with a note of
/// where it's from and its license at the top.
pub fn fetch_markdown(g: &Guide) -> Result<String, String> {
    let mut urls: Vec<String> = g.pages.iter().map(|s| s.to_string()).collect();
    // (The pages that start a chapter, and its title.)
    let mut chapter_at: std::collections::HashMap<usize, String> = Default::default();
    if let Some((index, links)) = g.index {
        let (html, _) = get_or_archived(index)?;
        let doc = scraper::Html::parse_document(&html);
        let both = if g.chapters.is_empty() { links.to_string() } else { format!("{links}, {}", g.chapters) };
        let sel = scraper::Selector::parse(&both).map_err(|e| format!("{both}: {e}"))?;
        let title_sel = scraper::Selector::parse(if g.chapters.is_empty() { ":not(*)" } else { g.chapters }).map_err(|e| format!("{}: {e}", g.chapters))?;
        let host = |u: &str| u.split('/').nth(2).unwrap_or("").to_string();
        let mut chapter: Option<String> = None;
        // (Links and chapter titles in the order they come.)
        for a in doc.select(&sel) {
            if title_sel.matches(&a) {
                let t = a.text().collect::<String>().trim().to_string();
                if !g.until.is_empty() && t.starts_with(g.until) {
                    break;
                }
                chapter = Some(t);
                continue;
            }
            if let Some(h) = a.value().attr("href") {
                let u = strip_fragment(&join(index, h)).to_string();
                if host(&u) == host(index) && u != strip_fragment(index) && !urls.contains(&u) {
                    if let Some(c) = chapter.take() {
                        chapter_at.insert(urls.len(), c);
                    }
                    urls.push(u);
                }
            }
        }
    }
    if urls.is_empty() {
        return Err("The guide has no pages.".into());
    }
    let page_ids: Vec<(String, usize)> = urls.iter().enumerate().flat_map(|(i, u)| u.split(" | ").map(move |u| (strip_fragment(u.trim()).to_string(), i))).collect();
    let mut out = String::new();
    let home = g.pages.first().copied().or(g.index.map(|i| i.0)).unwrap_or("").split(" | ").next().unwrap_or("").trim();
    out.push_str(&format!(
        "> **{}**, from [{}]({}), converted to Markdown by Tessera on {}. License: [{}]({}).\n\n",
        g.title,
        if home.starts_with("https://raw.githubusercontent.com/") { "github.com" } else { home.split('/').nth(2).unwrap_or(home) },
        home.replace("raw.githubusercontent.com", "github.com").replacen("/main/", "/blob/main/", 1).replacen("/master/", "/blob/master/", 1),
        today(),
        g.license,
        g.license_url
    ));
    // (Fetched and converted side by side, eight at a time.)
    let key = g.key();
    let n = urls.len();
    let next = std::sync::atomic::AtomicUsize::new(0);
    let done = std::sync::atomic::AtomicUsize::new(0);
    let mut parts: Vec<Option<Result<String, String>>> = (0..n).map(|_| None).collect();
    set_progress(key, &format!("Fetching {n} page{}…", if n == 1 { "" } else { "s" }));
    {
        let slots: Vec<std::sync::Mutex<Option<Result<String, String>>>> = (0..n).map(|_| std::sync::Mutex::new(None)).collect();
        std::thread::scope(|sc| {
            for _ in 0..n.min(8) {
                sc.spawn(|| loop {
                    let i = next.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                    if i >= n {
                        break;
                    }
                    let r = get_or_archived(&urls[i]).and_then(|(text, snapshot)| {
                        let u = urls[i].split(" | ").next().unwrap_or(&urls[i]).trim();
                        let text = if u.ends_with(".gmi") { gemtext_to_markdown(&text) } else if u.ends_with(".rst") { rst_to_markdown(&text) } else { text };
                        let md = if is_markdown(u) { clean_markdown(&expand_includes(&text, u)) } else { html_to_markdown(&text, g)? };
                        let md = relink(&md, u, &page_ids);
                        Ok(match snapshot {
                            Some(date) => format!("> From the [Internet Archive's copy]({}) of [{u}]({u}) (saved {date}): the site didn't answer.\n\n{md}", archive_url(u)),
                            None => md,
                        })
                    });
                    *slots[i].lock().unwrap() = Some(r);
                    let k = done.fetch_add(1, std::sync::atomic::Ordering::SeqCst) + 1;
                    set_progress(key, &format!("Fetched {k} of {n} page{}…", if n == 1 { "" } else { "s" }));
                });
            }
        });
        for (o, s) in parts.iter_mut().zip(slots) {
            *o = s.into_inner().unwrap();
        }
    }
    let mut got = 0;
    let mut failed = vec![];
    for (i, part) in parts.into_iter().enumerate() {
        match part.unwrap_or_else(|| Err("not fetched".into())) {
            Ok(md) => {
                if g.chapters.is_empty() {
                    out.push_str(&format!("<a id=\"page-{i}\"></a>\n\n{}\n\n", md.trim()));
                } else {
                    // (A chapter's title as a part; its pages' headings a level down.)
                    if let Some(c) = chapter_at.get(&i) {
                        out.push_str(&format!("# {c}\n\n"));
                    }
                    out.push_str(&format!("<a id=\"page-{i}\"></a>\n\n{}\n\n", demote(md.trim())));
                }
                got += 1;
            }
            Err(e) if i == 0 => return Err(e),
            Err(e) => failed.push(e),
        }
    }
    if got == 0 {
        return Err("Nothing could be fetched.".into());
    }
    if !failed.is_empty() {
        out.push_str(&format!("\n\n> {} page(s) couldn't be fetched: {}\n", failed.len(), failed.join("; ")));
    }
    Ok(out)
}

/// Code a Markdown page pulls in from its repository's files (Microsoft's
/// docs: `[!code-fsharp[Name](~/path#L1-L9)]` and `:::code source="path"
/// id="Region":::`), fetched and put in as code blocks.
fn expand_includes(md: &str, page: &str) -> String {
    use regex::Regex;
    use std::sync::OnceLock;
    static RES: OnceLock<[Regex; 2]> = OnceLock::new();
    let [bang, colons] = RES.get_or_init(|| [
        Regex::new(r"(?m)^[ \t>]*\[!code-([A-Za-z0-9#+-]+)\[[^\]]*\]\(([^)#\s]+)(?:#([^)\s]+))?\)\]\s*$").unwrap(),
        Regex::new(r#"(?m)^[ \t>]*:::code\s+([^\n]*?):::\s*$"#).unwrap(),
    ]);
    let md = &liquid_data(md, page);
    let md = &snippets(md, page);
    if !md.contains("[!code-") && !md.contains(":::code") {
        return md.to_string();
    }
    // (The repository's root, for "~/" paths: raw.githubusercontent.com/OWNER/REPO/BRANCH/.)
    let root: String = page.splitn(7, '/').take(6).collect::<Vec<_>>().join("/") + "/";
    let mut files: std::collections::HashMap<String, Option<String>> = Default::default();
    let mut file = |path: &str| -> Option<String> {
        let url = match path.strip_prefix("~/") {
            Some(p) => format!("{root}{p}"),
            None => join(page, path),
        };
        files.entry(url.clone()).or_insert_with(|| get(&url).ok()).clone()
    };
    let attr = |a: &str, name: &str| Regex::new(&format!(r#"\b{name}="([^"]*)""#)).ok().and_then(|r| r.captures(a).map(|c| c[1].to_string()));
    let mut out = String::with_capacity(md.len());
    for line in md.split_inclusive('\n') {
        let (lang, path, part) = if let Some(c) = bang.captures(line) {
            (c[1].to_string(), c[2].to_string(), c.get(3).map(|m| m.as_str().to_string()).unwrap_or_default())
        } else if let Some(c) = colons.captures(line) {
            let a = &c[1];
            let part = attr(a, "id").or_else(|| attr(a, "range").map(|r| format!("L{}", r.replace('-', "-L")))).unwrap_or_default();
            (attr(a, "language").unwrap_or_default(), attr(a, "source").unwrap_or_default(), part)
        } else {
            out.push_str(line);
            continue;
        };
        let Some(text) = (if path.is_empty() { None } else { file(&path) }) else {
            out.push_str(line);
            continue;
        };
        let code = include_part(&text, &part);
        out.push_str(&format!("\n```{}\n{}\n```\n", lang.to_lowercase(), code.trim_end()));
    }
    out
}

/// MkDocs' `--8<-- "file"` lines (pymdownx.snippets): the file put in, from
/// the repository's code-samples or snippets folder, its root, or beside
/// the page.
fn snippets(md: &str, page: &str) -> String {
    if !md.contains("--8<--") || !page.starts_with("https://raw.githubusercontent.com/") {
        return md.to_string();
    }
    let root: String = page.splitn(7, '/').take(6).collect::<Vec<_>>().join("/") + "/";
    let re = regex::Regex::new(r#"^\s*--8<--\s+"([^"]+)"\s*$"#).unwrap();
    md.split_inclusive('\n')
        .map(|line| match re.captures(line.trim_end()) {
            Some(c) => {
                let name = &c[1];
                ["code-samples/", "snippets/", "docs/snippets/", ""]
                    .iter()
                    .map(|d| format!("{root}{d}{name}"))
                    .chain(std::iter::once(join(page, name)))
                    .find_map(|u| get(&u).ok())
                    .map(|t| format!("{}\n", t.trim_end()))
                    .unwrap_or_else(|| line.to_string())
            }
            None => line.to_string(),
        })
        .collect()
}

/// GitHub Docs' `{% data variables.FILE.KEY %}` (a name, from the
/// repository's data/variables/FILE.yml) and `{% data reusables.A.B %}` (a
/// shared paragraph, data/reusables/A/B.md), filled in as its site does.
fn liquid_data(md: &str, page: &str) -> String {
    use regex::Regex;
    if !md.contains("{% data ") {
        return md.to_string();
    }
    let root: String = page.splitn(7, '/').take(6).collect::<Vec<_>>().join("/") + "/";
    let re = Regex::new(r"\{%-?\s*data\s+(variables|reusables)\.([A-Za-z0-9_.-]+)\s*-?%\}").unwrap();
    let mut vars: std::collections::HashMap<String, std::collections::HashMap<String, String>> = Default::default();
    let mut out = md.to_string();
    // (A shared paragraph can name things and bring in others: filled in
    // again, a few passes at most.)
    for _ in 0..3 {
        if !re.is_match(&out) {
            break;
        }
        out = re
            .replace_all(&out, |c: &regex::Captures| {
                let path = &c[2];
                if &c[1] == "reusables" {
                    return get(&format!("{root}data/reusables/{}.md", path.replace('.', "/"))).map(|t| t.trim().to_string()).unwrap_or_default();
                }
                let Some((file, key)) = path.split_once('.') else { return String::new() };
                let table = vars.entry(file.to_string()).or_insert_with(|| {
                    get(&format!("{root}data/variables/{file}.yml"))
                        .map(|y| y.lines().filter(|l| !l.starts_with([' ', '#'])).filter_map(|l| l.split_once(':')).map(|(k, v)| (k.trim().to_string(), v.trim().trim_matches(['\'', '"']).to_string())).collect())
                        .unwrap_or_default()
                });
                table.get(key).cloned().unwrap_or_default()
            })
            .into_owned();
    }
    out
}

/// Lines `L5-L9`, or the region between `<Name>` and `</Name>` (in any
/// comment), or the whole file.
fn include_part(text: &str, part: &str) -> String {
    let lines: Vec<&str> = text.lines().collect();
    if let Some(r) = part.strip_prefix('L') {
        let (a, b) = r.split_once("-L").or_else(|| r.split_once('-')).unwrap_or((r, r));
        if let (Ok(a), Ok(b)) = (a.parse::<usize>(), b.trim_start_matches('L').parse::<usize>()) {
            let (a, b) = (a.max(1) - 1, b.min(lines.len()));
            if a < b {
                return textwrap_dedent(&lines[a..b].join("\n"));
            }
        }
    }
    if !part.is_empty() {
        let (open, close) = (format!("<{part}>"), format!("</{part}>"));
        if let Some(s) = lines.iter().position(|l| l.contains(&open)) {
            if let Some(e) = lines[s + 1..].iter().position(|l| l.contains(&close)) {
                return textwrap_dedent(&lines[s + 1..s + 1 + e].join("\n"));
            }
        }
    }
    text.to_string()
}

/// reStructuredText as Markdown, the common parts: titles (underlined, or
/// over- and underlined; levels in the order their marks first come), code
/// directives and "::" literal blocks as fenced code, notes as quotes,
/// links, roles and ``literals``; other directives (toctree, labels) left out.
fn rst_to_markdown(t: &str) -> String {
    use regex::Regex;
    let lines: Vec<&str> = t.lines().collect();
    let is_rule = |l: &str| {
        let l = l.trim_end();
        let Some(c) = l.chars().next() else { return false };
        l.len() >= 3 && l.chars().all(|x| x == c) && "=-~^\"'`#*+:._".contains(c)
    };
    let indent = |l: &str| l.len() - l.trim_start().len();
    // (The block indented under a directive or a "::" paragraph: its lines
    // and where it ends.)
    let block = |from: usize| -> (Vec<String>, usize) {
        let mut j = from;
        while j < lines.len() && lines[j].trim().is_empty() {
            j += 1;
        }
        let start_indent = if j < lines.len() { indent(lines[j]) } else { 0 };
        let mut got = vec![];
        while j < lines.len() && (lines[j].trim().is_empty() || (start_indent > 0 && indent(lines[j]) >= start_indent)) {
            got.push(lines[j].get(start_indent..).unwrap_or("").to_string());
            j += 1;
        }
        while got.last().is_some_and(|l| l.trim().is_empty()) {
            got.pop();
        }
        (got, j)
    };
    let literal = Regex::new(r"``([^`]+)``").unwrap();
    let link = Regex::new(r"`([^`<]+?)\s*<([^>]+)>`__?").unwrap();
    let role = Regex::new(r":[a-z:+-]+:`([^`<]+?)(\s*<[^>]+>)?`").unwrap();
    let inline = |l: &str| -> String {
        let l = literal.replace_all(l, "`$1`");
        let l = link.replace_all(&l, "[$1]($2)");
        role.replace_all(&l, "`$1`").into_owned()
    };
    let mut marks: Vec<String> = vec![];
    let mut out = String::new();
    let mut i = 0;
    while i < lines.len() {
        let l = lines[i];
        // Titles.
        let title = if is_rule(l) && i + 2 < lines.len() && !lines[i + 1].trim().is_empty() && is_rule(lines[i + 2]) {
            Some((lines[i + 1].trim(), format!("over{}", &l.trim()[..1]), i + 3))
        } else if !l.trim().is_empty() && indent(l) == 0 && i + 1 < lines.len() && is_rule(lines[i + 1]) && lines[i + 1].trim().len() >= l.trim().chars().count() {
            Some((l.trim(), lines[i + 1].trim()[..1].to_string(), i + 2))
        } else {
            None
        };
        if let Some((text, mark, next)) = title {
            if !marks.contains(&mark) {
                marks.push(mark.clone());
            }
            let level = marks.iter().position(|m| *m == mark).unwrap_or(0) + 1;
            out.push_str(&format!("{} {}\n\n", "#".repeat(level.min(6)), inline(text)));
            i = next;
            continue;
        }
        let tl = l.trim_start();
        if let Some(d) = tl.strip_prefix(".. ") {
            let (body, j) = block(i + 1);
            if let Some(lang) = d.strip_prefix("code-block::").or(d.strip_prefix("code::")).or(d.strip_prefix("sourcecode::")) {
                let code: Vec<String> = body.into_iter().skip_while(|b| b.trim_start().starts_with(':')).collect();
                out.push_str(&format!("```{}\n{}\n```\n\n", lang.trim(), code.join("\n").trim_matches('\n')));
            } else if let Some(kind) = ["note::", "warning::", "tip::", "important::", "caution::", "hint::", "seealso::", "admonition::"].iter().find(|k| d.starts_with(*k)) {
                let first = d[kind.len()..].trim();
                out.push_str(&format!("> **{}**", kind.trim_end_matches("::")));
                if !first.is_empty() {
                    out.push_str(&format!(" {}", inline(first)));
                }
                out.push_str("\n>\n");
                for b in body {
                    out.push_str(&format!("> {}\n", inline(&b)));
                }
                out.push('\n');
            }
            i = j;
            continue;
        }
        // A paragraph ending in "::": the indented block after it is code.
        if tl.ends_with("::") {
            let text = tl.trim_end_matches("::").trim_end();
            if !text.is_empty() {
                out.push_str(&format!("{}{}\n\n", inline(text), if text.ends_with(':') { "" } else { ":" }));
            }
            let (body, j) = block(i + 1);
            if !body.is_empty() {
                out.push_str(&format!("```\n{}\n```\n\n", body.join("\n")));
            }
            i = j;
            continue;
        }
        out.push_str(&inline(l));
        out.push('\n');
        i += 1;
    }
    out
}

/// Gemtext (Gemini's own text) as Markdown: its "=> URL text" lines links,
/// its other lines (headings, lists, quotes, ``` blocks) already Markdown,
/// each line a paragraph.
fn gemtext_to_markdown(t: &str) -> String {
    let mut out = String::new();
    let mut pre = false;
    for l in t.lines() {
        if l.starts_with("```") {
            pre = !pre;
            out.push_str(l);
            out.push('\n');
            continue;
        }
        if pre {
            out.push_str(l);
            out.push('\n');
        } else if let Some(rest) = l.strip_prefix("=>") {
            let rest = rest.trim();
            let (url, text) = rest.split_once(char::is_whitespace).map(|(u, t)| (u, t.trim())).unwrap_or((rest, rest));
            out.push_str(&format!("[{text}]({url})\n\n"));
        } else {
            out.push_str(l);
            out.push_str("\n\n");
        }
    }
    out
}

/// A Markdown file (MDX too: Markdown with components).
fn is_markdown(u: &str) -> bool {
    let path = u.split(['?', '#']).next().unwrap_or(u).to_lowercase();
    path.ends_with(".md") || path.ends_with(".mdx") || path.ends_with(".markdown") || path.ends_with(".gmi") || path.ends_with(".rst")
}

/// Markdown from a repository, cleaned of what only its site understands:
/// front matter (its title kept as the heading), template tags and macros.
fn clean_markdown(text: &str) -> String {
    use regex::Regex;
    let mut text = text.replace("\r\n", "\n");
    let mut title = None;
    if let Some(rest) = text.strip_prefix("---\n") {
        if let Some(end) = rest.find("\n---") {
            for l in rest[..end].lines() {
                if let Some(t) = l.strip_prefix("title:") {
                    title = Some(t.trim().trim_matches(['"', '\'']).to_string());
                }
            }
            text = rest[end + 4..].trim_start_matches(|c| c != '\n').to_string();
        }
    }
    let re = |p: &str| Regex::new(p).unwrap();
    // MDN's macros ({{Glossary("x")}} → x; the rest dropped), Jekyll/Liquid
    // tags, DocC directives, Writerside attribute lines, HTML comments.
    text = re(r#"\{\{\s*(?:Glossary|domxref|jsxref|cssxref|HTMLElement|htmlelement|HTTPHeader|httpheader|CSSxRef|JSxRef|DOMxRef|glossary)\(\s*"([^"]*)"(?:\s*,\s*"([^"]*)")?[^}]*\}\}"#)
        .replace_all(&text, |c: &regex::Captures| c.get(2).filter(|m| !m.as_str().is_empty()).unwrap_or(c.get(1).unwrap()).as_str().to_string())
        .into_owned();
    text = re(r"\{\{[^}]*\}\}").replace_all(&text, "").into_owned();
    text = re(r"\{%[^%]*%\}").replace_all(&text, "").into_owned();
    text = re(r"(?s)<!--.*?-->").replace_all(&text, "").into_owned();
    text = re(r"(?m)^\{[^}\n]*\}\s*$").replace_all(&text, "").into_owned();
    text = re(r"(?s)@Comment\s*\{.*?\n\}").replace_all(&text, "").into_owned();
    text = re(r"(?m)^\s*@(?:Metadata|Options|TechnologyRoot|Image|Row|Column)\b.*$").replace_all(&text, "").into_owned();
    text = re(r"<doc:([A-Za-z0-9_]+)(?:#[^>]*)?>").replace_all(&text, "$1").into_owned();
    // (Microsoft's "> [!NOTE]" callouts read fine as quotes.)
    text = re(r"(?m)^>\s*\[!(\w+)\]\s*$").replace_all(&text, "> **$1**").into_owned();
    // (A "# " line in a code block isn't a heading.)
    let mut fenced = false;
    // (MDX's import and export lines, outside code: the site's, not the text's.)
    let mut fenced = false;
    text = text
        .split_inclusive('\n')
        .filter(|l| {
            let t = l.trim_start();
            if t.starts_with("```") || t.starts_with("~~~") {
                fenced = !fenced;
            }
            fenced || !((t.starts_with("import ") && (t.contains(" from ") || t.contains('\''))) || t.starts_with("export const ") || t.starts_with("export default "))
        })
        .collect();
    let has_h1 = text.lines().any(|l| {
        if l.trim_start().starts_with("```") || l.trim_start().starts_with("~~~") {
            fenced = !fenced;
        }
        !fenced && l.starts_with("# ")
    });
    match title {
        Some(t) if !has_h1 => format!("# {t}\n\n{}", text.trim_start()),
        _ => text,
    }
}

/// An HTML page's document part as Markdown.
fn html_to_markdown(html: &str, g: &Guide) -> Result<String, String> {
    let doc = scraper::Html::parse_document(html);
    let mut part = None;
    for s in g.select.split(',').map(str::trim).filter(|s| !s.is_empty()).chain(["main", "body"]) {
        if let Ok(sel) = scraper::Selector::parse(s) {
            if let Some(e) = doc.select(&sel).next() {
                part = Some(e);
                break;
            }
        }
    }
    if g.all {
        if let Ok(sel) = scraper::Selector::parse(g.select) {
            let mut out = String::new();
            for e in doc.select(&sel) {
                out.push_str(&part_markdown(e, g)?);
                out.push_str("\n\n");
            }
            if !out.trim().is_empty() {
                return Ok(titled(out, &doc));
            }
        }
    }
    let Some(part) = part else { return Ok(String::new()) };
    Ok(titled(part_markdown(part, g)?, &doc))
}

fn titled(md: String, doc: &scraper::Html) -> String {
    // (A page with no top heading of its own (cppreference keeps its
    // title outside the article) gets its title as one, for the contents.)
    if md.lines().any(|l| l.starts_with("# ")) {
        md
    } else {
        // (The page's own heading as it is; else its <title>, less the site's name.)
        let text = |sel: &str| scraper::Selector::parse(sel).ok().and_then(|s| doc.select(&s).next()).map(|t| t.text().collect::<String>().trim().to_string()).filter(|t| !t.is_empty());
        let title = text("h1#firstHeading, h1.entry-title").unwrap_or_else(|| {
            let t = text("title").unwrap_or_default();
            [" | ", " - ", " \u{2013} ", " \u{2014} "].iter().fold(t.clone(), |t, sep| t.rsplit_once(sep).map(|(a, _)| a.to_string()).unwrap_or(t)).trim().to_string()
        });
        if title.is_empty() { md } else { format!("# {title}\n\n{md}") }
    }
}

/// A part of a page written out as HTML again, in one pass: what isn't the
/// document left out, code drawn some other way made code blocks, and the
/// guide's renames (a site's own "heading" paragraphs as headings).
struct Writer {
    drop: Vec<scraper::Selector>,
    code: Vec<scraper::Selector>,
    renames: Vec<(scraper::Selector, &'static str)>,
}

impl Writer {
    fn node(&self, n: ego_tree::NodeRef<scraper::Node>, out: &mut String) {
        let esc = |t: &str| t.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;");
        match n.value() {
            scraper::Node::Text(t) => out.push_str(&esc(t)),
            scraper::Node::Element(e) => {
                let Some(el) = scraper::ElementRef::wrap(n) else { return };
                if self.drop.iter().any(|s| s.matches(&el)) {
                    return;
                }
                // (Code drawn as tables of tokens or lines with <br>: its
                // text, a line per row or break.)
                if self.code.iter().any(|s| s.matches(&el)) {
                    let mut text = String::new();
                    code_text(n, &mut text, el.select(&scraper::Selector::parse("br").unwrap()).next().is_some());
                    let text = textwrap_dedent(&text.replace('\u{a0}', " ").lines().map(str::trim_end).collect::<Vec<_>>().join("\n"));
                    out.push_str(&format!("<pre><code>{}</code></pre>", esc(&text)));
                    return;
                }
                // ("h3", "blockquote", or "p>strong": the element as those, its id kept.)
                let tags: Vec<&str> = match self.renames.iter().find(|(s, _)| s.matches(&el)) {
                    Some((_, to)) => to.split('>').collect(),
                    None => vec![e.name()],
                };
                for (k, t) in tags.iter().enumerate() {
                    out.push('<');
                    out.push_str(t);
                    if k == 0 {
                        for (name, value) in e.attrs() {
                            if tags[0] == e.name() || name == "id" {
                                out.push_str(&format!(" {name}=\"{}\"", value.replace('&', "&amp;").replace('"', "&quot;")));
                            }
                        }
                    }
                    out.push('>');
                }
                const VOID: &[&str] = &["area", "base", "br", "col", "embed", "hr", "img", "input", "link", "meta", "source", "track", "wbr"];
                if tags.len() == 1 && VOID.contains(&tags[0]) {
                    return;
                }
                for c in n.children() {
                    self.node(c, out);
                }
                for t in tags.iter().rev() {
                    out.push_str(&format!("</{t}>"));
                }
            }
            _ => {}
        }
    }
}

/// One part of a page (its document) as Markdown.
fn part_markdown(part: scraper::ElementRef, g: &Guide) -> Result<String, String> {
    // (Leave out what isn't the document.)
    // (Never the document, on any site: breadcrumbs, "last updated", a
    // page's own contents list, "edit this page", mascots.)
    let drop = format!("script, style, noscript, button, template, a.headerlink, a.anchor, .headerlink, .anchorjs-link, .copybutton, .sr-only, .visually-hidden, nav[aria-label*='readcrumb'], .breadcrumb, .breadcrumbs, [class*='Breadcrumb'], [class*='breadcrumb'], [class*='lastUpdated'], [class*='last-updated'], .table-of-contents, nav.toc, .edit-page, .edit-this-page, img[src*='mascot']{}{}", if g.drop.is_empty() { "" } else { ", " }, g.drop);
    let parse = |list: &str| list.split(',').map(str::trim).filter(|s| !s.is_empty()).filter_map(|s| scraper::Selector::parse(s).ok()).collect::<Vec<_>>();
    let w = Writer {
        drop: parse(&drop),
        code: parse(g.code_sel),
        renames: g.renames.iter().filter_map(|(sel, to)| scraper::Selector::parse(sel).ok().map(|s| (s, *to))).collect(),
    };
    let mut inner = String::with_capacity(64 << 10);
    for c in part.children() {
        w.node(c, &mut inner);
    }
    let inner = keep_ids(&code_in_pre(&inner));
    let conv = htmd::HtmlToMarkdown::builder().skip_tags(vec!["script", "style", "svg", "head", "form", "input", "select", "iframe"]).build();
    let md = conv.convert(&inner).map_err(|e| e.to_string())?;
    let md = regex::Regex::new("\u{27e6}#([^\u{27e7}]*)\u{27e7}").unwrap().replace_all(&md, |c: &regex::Captures| format!("<a id=\"{}\"></a>", c[1].replace('\\', ""))).into_owned();
    // (Pilcrows and the like that headings carried as links.)
    Ok(md.replace(" ¶", "").replace("¶", "").replace("\u{a0}", " "))
}

fn code_text(n: ego_tree::NodeRef<scraper::Node>, out: &mut String, brs: bool) {
    for c in n.children() {
        match c.value() {
            scraper::Node::Text(t) if brs => out.push_str(&t.replace('\n', "")),
            scraper::Node::Text(t) => out.push_str(t),
            // (Code kept in a comment for a script to show: dev.java.)
            scraper::Node::Comment(t) => out.push_str(&textwrap_dedent(t)),
            scraper::Node::Element(e) => {
                let name = e.name();
                if name == "br" {
                    out.push('\n');
                    continue;
                }
                code_text(c, out, brs);
                if matches!(name, "tr" | "p" | "div") && !out.ends_with('\n') {
                    out.push('\n');
                }
            }
            _ => {}
        }
    }
}

/// The page's own ids kept (as empty elements Markdown passes along), so
/// its links to its parts still land: ids of headings, sections, terms,
/// and named anchors.
fn keep_ids(html: &str) -> String {
    let re = regex::Regex::new(r#"<(h[1-6]|section|div|dt|p|li|a)\b([^>]*)>"#).unwrap();
    let id_re = regex::Regex::new(r#"\s(?:id|name)\s*=\s*"([^"]+)""#).unwrap();
    re.replace_all(html, |c: &regex::Captures| {
        let whole = &c[0];
        let Some(id) = id_re.captures(&c[2]).map(|m| m[1].to_string()) else { return whole.to_string() };
        let id = id.replace(['"', '<', '>', '&'], "");
        // (A mark in the text, made an anchor after the conversion: htmd
        // keeps no empty elements.)
        let anchor = format!("\u{27e6}#{id}\u{27e7}");
        if &c[1] == "a" { format!("{anchor}{whole}") } else { format!("{whole}{anchor}") }
    })
    .into_owned()
}

/// Lines with their common indent taken off.
fn textwrap_dedent(t: &str) -> String {
    let lines: Vec<&str> = t.trim_matches('\n').lines().collect();
    let pad = lines.iter().filter(|l| !l.trim().is_empty()).map(|l| l.len() - l.trim_start().len()).min().unwrap_or(0);
    lines.iter().map(|l| if l.len() >= pad { &l[pad..] } else { l.trim_start() }).collect::<Vec<_>>().join("\n").trim_end().to_string()
}

/// `<pre>` without a `<code>` inside gets one, so its text stays a code
/// block (Sphinx, texinfo); a language from its class (or its wrapper's)
/// goes along.
fn code_tags_re() -> &'static regex::Regex {
    static RE: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    RE.get_or_init(|| regex::Regex::new(r"(?i)</?code\b[^>]*>").unwrap())
}

fn code_in_pre(html: &str) -> String {
    let lang_re = regex::Regex::new(r#"(?:language|lang|highlight|brush:?)-?\s*([A-Za-z0-9_+#-]+)"#).unwrap();
    let mut out = String::with_capacity(html.len() + 256);
    let mut rest = html;
    let mut last_class = String::new();
    while let Some(i) = rest.find("<pre") {
        let before = &rest[..i];
        // (A wrapper's class, like Sphinx's "highlight-python3".)
        last_class = match before.rfind("class=\"") {
            Some(k) if before.len() - k < 400 => {
                let c = &before[k + 7..];
                c[..c.find('"').unwrap_or(0)].to_string()
            }
            _ => String::new(),
        };
        out.push_str(before);
        let Some(gt) = rest[i..].find('>') else { break };
        let open = &rest[i..i + gt + 1];
        let after = &rest[i + gt + 1..];
        if !(open.starts_with("<pre>") || open.starts_with("<pre ")) {
            out.push_str(open);
            rest = after;
            continue;
        }
        let Some(end) = after.find("</pre>") else {
            out.push_str(open);
            rest = after;
            continue;
        };
        let body = &after[..end];
        // (One code element, the pre's whole text: texinfo puts several in
        // one pre, with text between.)
        let first_code = body.trim_start().starts_with("<code");
        let code_class = if first_code { body.trim_start().split('>').next().and_then(|t| t.split("class=\"").nth(1)).map(|c| c[..c.find('"').unwrap_or(0)].to_string()).unwrap_or_default() } else { String::new() };
        let body = &code_tags_re().replace_all(body, "").into_owned();
        {
            let class = open.split("class=\"").nth(1).map(|c| c[..c.find('"').unwrap_or(0)].to_string()).unwrap_or_default();
            let class = if code_class.starts_with("language-") || code_class.starts_with("lang-") { code_class.clone() } else { class };
            let lang = lang_re.captures(&class).or_else(|| lang_re.captures(&last_class)).map(|c| c[1].to_string()).unwrap_or_default();
            out.push_str("<pre>");
            if lang.is_empty() || lang == "default" || lang == "none" {
                out.push_str("<code>");
            } else {
                out.push_str(&format!("<code class=\"language-{lang}\">"));
            }
            out.push_str(body);
            out.push_str("</code>");
        }
        out.push_str("</pre>");
        rest = &after[end + 6..];
    }
    out.push_str(rest);
    out
}

/// Links made whole; those to the guide's own pages kept in the guide
/// (`#page-N`, or the heading they name).
fn relink(md: &str, page: &str, pages: &[(String, usize)]) -> String {
    let re = regex::Regex::new(r#"\]\(\s*<?([^)\s>]*)>?(\s+"[^"]*")?\)"#).unwrap();
    re.replace_all(md, |c: &regex::Captures| {
        let href = &c[1];
        let title = c.get(2).map(|m| m.as_str()).unwrap_or("");
        if href.is_empty() {
            return c[0].to_string();
        }
        if href.starts_with('#') {
            return format!("]({href}{title})");
        }
        // (A repository's file read raw: its site-wide paths are the
        // repository's (GitHub Docs' /assets/images/...).)
        let full = if href.starts_with('/') && !href.starts_with("//") && page.starts_with("https://raw.githubusercontent.com/") {
            format!("{}{}", page.splitn(7, '/').take(6).collect::<Vec<_>>().join("/"), href)
        } else {
            join(page, href)
        };
        let (base, frag) = match full.split_once('#') {
            Some((b, f)) => (b.to_string(), Some(f.to_string())),
            None => (full.clone(), None),
        };
        if let Some((_, n)) = pages.iter().find(|(p, _)| *p == base || p.trim_end_matches('/') == base.trim_end_matches('/')) {
            return match frag {
                Some(f) if !f.is_empty() => format!("](#{f}{title})"),
                _ => format!("](#page-{n}{title})"),
            };
        }
        format!("]({full}{title})")
    })
    .into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rst_converted() {
        let md = rst_to_markdown("=====\nTitle\n=====\n\nSome ``code`` and `a link <https://x.org>`_.\n\nSection\n-------\n\n.. code-block:: python\n\n   x = 1\n\nA literal::\n\n   raw\n\n.. note:: Mind this.\n\n.. toctree::\n   a\n");
        assert!(md.contains("# Title\n") && md.contains("## Section\n"), "{md}");
        assert!(md.contains("Some `code` and [a link](https://x.org)."), "{md}");
        assert!(md.contains("```python\nx = 1\n```") && md.contains("A literal:\n\n```\nraw\n```"), "{md}");
        assert!(md.contains("> **note** Mind this.") && !md.contains("toctree"), "{md}");
    }

    #[test]
    fn gemtext_converted() {
        let md = gemtext_to_markdown("# Gemtext\n=> https://x.org The link\n```\n=> not a link\n```\n* item\n");
        assert!(md.contains("# Gemtext") && md.contains("[The link](https://x.org)") && md.contains("=> not a link"), "{md}");
    }

    #[test]
    fn cpp_has_two_guides() {
        let c: Vec<[String; 2]> = serde_json::from_str(&choices_json("cpp")).unwrap();
        assert_eq!(c.iter().map(|x| x[0].as_str()).collect::<Vec<_>>(), ["learncpp", "cppreference"]);
        assert_eq!(choices_json("c++"), choices_json("cppreference"));
        assert_eq!(find("cpp").unwrap().key(), "learncpp");
        assert_eq!(find("cppreference").unwrap().key(), "cppreference");
        assert_eq!(serde_json::from_str::<Vec<[String; 2]>>(&choices_json("python")).unwrap().len(), 1);
    }

    #[test]
    fn joins() {
        assert_eq!(join("https://a.org/x/y/z.html", "w.html"), "https://a.org/x/y/w.html");
        assert_eq!(join("https://a.org/x/y/z.html", "../w.html#f"), "https://a.org/x/w.html#f");
        assert_eq!(join("https://a.org/x/y/", "w/"), "https://a.org/x/y/w/");
        assert_eq!(join("https://a.org/x/y/z.html", "/r"), "https://a.org/r");
        assert_eq!(join("https://a.org/x/y/z.html", "//b.org/q"), "https://b.org/q");
        assert_eq!(join("https://a.org/x/y/z.html", "#f"), "https://a.org/x/y/z.html#f");
        assert_eq!(join("https://a.org", "w.html"), "https://a.org/w.html");
    }

    #[test]
    fn pre_gets_code() {
        let h = code_in_pre("<div class=\"highlight-python3\"><pre>x = 1</pre></div><pre><code>y</code></pre>");
        assert_eq!(h, "<div class=\"highlight-python3\"><pre><code class=\"language-python3\">x = 1</code></pre></div><pre><code>y</code></pre>");
    }

    #[test]
    fn relinks() {
        let pages = vec![("https://a.org/t/one.html".to_string(), 0), ("https://a.org/t/two.html".to_string(), 1)];
        let md = relink("[a](two.html) [b](two.html#sec) [c](https://x.org/) [d](../img.png \"t\") [e](#here)", "https://a.org/t/one.html", &pages);
        assert_eq!(md, "[a](#page-1) [b](#sec) [c](https://x.org/) [d](https://a.org/img.png \"t\") [e](#here)");
        let raw = "https://raw.githubusercontent.com/github/docs/main/content/a/b.md";
        assert_eq!(relink("![x](/assets/images/y.png)", raw, &[]), "![x](https://raw.githubusercontent.com/github/docs/main/assets/images/y.png)");
    }

    #[test]
    fn markdown_cleaned() {
        let md = clean_markdown("---\ntitle: Grammar and types\nslug: x\n---\n\n{{jsSidebar}}\nUse {{Glossary(\"Hoisting\", \"hoisted\")}} vars.\n");
        assert!(md.starts_with("# Grammar and types\n"), "{md}");
        assert!(md.contains("Use hoisted vars.") && !md.contains("{{"), "{md}");
    }

    /// Every guide fetched and converted, into $TESSERA_GUIDES_OUT (a
    /// check of the sources: `cargo test fetch_all -- --ignored`).
    #[test]
    #[ignore]
    fn fetch_all() {
        let out = PathBuf::from(std::env::var("TESSERA_GUIDES_OUT").unwrap_or_else(|_| "/tmp/tessera-guides".into()));
        std::fs::create_dir_all(&out).unwrap();
        let only = std::env::var("TESSERA_GUIDES_ONLY").unwrap_or_default();
        let mut bad = vec![];
        for g in GUIDES {
            if !only.is_empty() && !only.split(',').any(|o| g.langs.contains(&o)) {
                continue;
            }
            match fetch_markdown(g) {
                Ok(md) => {
                    let heads = md.lines().filter(|l| l.starts_with('#')).count();
                    let code = md.matches("\n```").count() / 2;
                    eprintln!("{:12} {:>8} bytes {:>4} headings {:>4} code", g.langs[0], md.len(), heads, code);
                    std::fs::write(out.join(format!("{}.md", g.langs[0])), md).unwrap();
                }
                Err(e) => {
                    eprintln!("{:12} FAILED {e}", g.langs[0]);
                    bad.push(g.langs[0]);
                }
            }
        }
        eprintln!("failed: {bad:?}");
    }
}

#[cfg(test)]
mod download {
    /// download_all into $XDG_STATE_HOME, and the tidying of stale files
    /// (`cargo test guides::download -- --ignored --nocapture`).
    #[test]
    #[ignore]
    fn all() {
        let dir = super::cache_dir();
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("cpp.md"), "old").unwrap();
        std::fs::write(dir.join("gone-language.md"), "old").unwrap();
        let t = std::time::Instant::now();
        let (got, failed) = super::download_all(|d, n, bad| if d % 20 == 0 || d == n { eprintln!("{d} of {n} ({bad} failed)") });
        eprintln!("fetched {got}, failed {failed:?}, in {:.0}s", t.elapsed().as_secs_f32());
        assert!(!dir.join("cpp.md").exists() && !dir.join("gone-language.md").exists(), "stale files tidied");
        let (again, _) = super::download_all(|_, _, _| {});
        eprintln!("second run fetched {again}");
        assert_eq!(again, 0, "nothing fetched twice");
    }
}

#[cfg(test)]
mod sweep {
    /// Every guide fetched, converted and drawn as the app does, into
    /// $XDG_STATE_HOME/tessera/guides, four at a time: a line each with its
    /// time and what's in it, and FLAG for one that failed, panicked, took
    /// over a minute, or has no contents or no highlighted code
    /// (`cargo test sweep -- --ignored --nocapture`; $SWEEP_ONLY=a,b).
    #[test]
    #[ignore]
    fn sweep() {
        let only = std::env::var("SWEEP_ONLY").unwrap_or_default();
        let list: Vec<&'static super::Guide> = super::GUIDES.iter().filter(|g| only.is_empty() || only.split(',').any(|o| o == g.key() || g.langs.contains(&o))).collect();
        let next = std::sync::atomic::AtomicUsize::new(0);
        std::thread::scope(|sc| {
            for _ in 0..4 {
                sc.spawn(|| loop {
                    let i = next.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                    let Some(g) = list.get(i) else { break };
                    let key = g.key();
                    let (tx, rx) = std::sync::mpsc::channel();
                    let t = std::time::Instant::now();
                    std::thread::spawn(move || {
                        let r = std::panic::catch_unwind(|| super::guide_markdown(key, true)).unwrap_or_else(|p| Err(format!("PANIC {}", p.downcast_ref::<String>().cloned().or_else(|| p.downcast_ref::<&str>().map(|s| s.to_string())).unwrap_or_default())));
                        let _ = tx.send(r);
                    });
                    let line = match rx.recv_timeout(std::time::Duration::from_secs(240)) {
                        Err(_) => format!("FLAG {key:14} still running after 240 s: {}", super::progress(key)),
                        Ok(Err(e)) => format!("FLAG {key:14} {:>6.1}s failed: {}", t.elapsed().as_secs_f32(), e.chars().take(300).collect::<String>()),
                        Ok(Ok(h)) => {
                            let secs = t.elapsed().as_secs_f32();
                            let parts = h.matches("<a class='part'").count();
                            let topics = h.split("<nav>").nth(1).and_then(|n| n.split("</nav>").next()).map(|n| n.matches("<a href=").count()).unwrap_or(0);
                            let blocks = h.matches("<pre").count();
                            let lit = h.matches("<pre class='code'>").count();
                            let spans = h.matches("<span class=\"t-").count();
                            let md = std::fs::metadata(super::cache_path(g)).map(|m| m.len()).unwrap_or(0);
                            let archived = std::fs::read_to_string(super::cache_path(g)).map(|t| t.matches("Internet Archive's copy").count()).unwrap_or(0);
                            let failed = std::fs::read_to_string(super::cache_path(g)).map(|t| t.contains("couldn't be fetched")).unwrap_or(false);
                            let mut why = vec![];
                            if secs > 60.0 { why.push("slow") }
                            if parts + topics < 2 { why.push("no contents") }
                            if blocks == 0 { why.push("no code") } else if spans == 0 { why.push("no highlighting") }
                            if failed { why.push("pages missing") }
                            format!("{} {key:14} {secs:>6.1}s md {:>5} KB  parts {parts:>3} topics {topics:>4}  code {blocks:>4} ({lit} drawn, {spans} spans){}{}",
                                if why.is_empty() { "ok  " } else { "FLAG" }, md / 1024,
                                if archived > 0 { format!("  archive pages {archived}") } else { String::new() },
                                if why.is_empty() { String::new() } else { format!("  <- {}", why.join(", ")) })
                        }
                    };
                    eprintln!("{line}");
                });
            }
        });
    }
}

#[cfg(test)]
mod stall_probe {
    /// A kept guide drawn ($PROBE_LANG, from $XDG_STATE_HOME/tessera/guides), with what it's doing each second.
    #[test]
    #[ignore]
    fn probe() {
        let lang = std::env::var("PROBE_LANG").unwrap();
        let l2 = lang.clone();
        std::thread::spawn(move || loop {
            std::thread::sleep(std::time::Duration::from_secs(1));
            eprintln!("  … {}", super::progress(&l2));
        });
        let t = std::time::Instant::now();
        let r = super::guide_markdown(&lang, false);
        eprintln!("done in {:.1}s: {}", t.elapsed().as_secs_f32(), r.map(|h| h.len().to_string()).unwrap_or_else(|e| e));
    }
}
