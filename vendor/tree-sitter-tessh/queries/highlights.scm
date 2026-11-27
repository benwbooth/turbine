(comment) @comment
(string) @string
(number) @number
(variable) @variable.parameter
(status) @variable.builtin
(operator) @operator
(infix_function) @operator
(symbol) @constant
(unquote) @embedded
(interpolation "$(" @punctuation.special ")" @punctuation.special)
(interpolation "${" @punctuation.special "}" @punctuation.special)
(quasi ":(" @punctuation.special)
(process_substitution (procsub_open) @punctuation.special ")" @punctuation.special)

; Redirections (2>f, 3>&1, <<<, &>), a heredoc's start (<<END) and its body.
(redirection) @operator
(heredoc_start) @operator
(heredoc_body) @string

["(" ")" "[" "]" "{" "}"] @punctuation.bracket
["," ";"] @punctuation.delimiter

; Keywords: special forms and word operators.
((bareword) @keyword
  (#match? @keyword "^(def|class|if|elif|else|then|while|for|in|try|except|except\\*|finally|with|as|match|case|return|yield|await|async|raise|assert|del|pass|break|continue|global|nonlocal|import|from|do|fn|lambda|quote|quasi|defmacro|defop|and|or|not|is)$"))

((bareword) @constant.builtin
  (#match? @constant.builtin "^(True|False|None)$"))

; name=value first on a line: an assignment, as name = value (before a
; program, its environment). Ahead of the next: the first match wins.
((line . (bareword) @variable)
  (#match? @variable "^[A-Za-z_][A-Za-z0-9_]*=([^=]|$)"))

; A line's first word: the command or function it runs.
(line . (bareword) @function)

; Flags and keyword arguments.
((bareword) @attribute
  (#match? @attribute "^--?[A-Za-z]"))
((bareword) @property
  (#match? @property "^[A-Za-z_][A-Za-z0-9_]*="))

; \cmd: a program, not a function.
((bareword) @function.builtin
  (#match? @function.builtin "^\\\\"))
