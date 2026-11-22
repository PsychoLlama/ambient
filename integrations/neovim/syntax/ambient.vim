" Ambient source (`.ab`), as `docs/spec/syntax.md` describes it.
"
" Matched by pattern, not parsed. The reserved words are highlighted
" wherever they stand; a word that only means something in one place
" (`tag`, `target`, `in`, `as`) is highlighted only there, since anywhere
" else it is a name like any other.

if exists('b:current_syntax')
  finish
endif

syn case match

" Reserved words.
syn keyword ambientDefine fn type
syn keyword ambientStorage pub comptime
syn keyword ambientKeyword let return with using
syn keyword ambientConditional if else match
syn keyword ambientRepeat for
syn keyword ambientScope provide restricted
syn keyword ambientBoolean true false
syn keyword ambientConstant null
syn keyword ambientBuiltinType never unknown

" Types the language has without a definition. None is reserved.
syn keyword ambientBuiltinType int float string bool bytes

" What a method is called on.
syn keyword ambientSelf self

" A capitalized word is a type, tag, or ability by convention.
syn match ambientType '\<\u\w*\>'

" A call, and a function where it is defined.
syn match ambientCall '\<\h\w*\ze\%(::<[^()]*>\)\=('
syn match ambientFunction '\%(\<fn\s\+\)\@<=\h\w*'

" An ability performed: `log!(...)`.
syn match ambientPerform '\<\h\w*!\ze('

" Words that begin a definition, which are names anywhere else.
syn match ambientDefine '\%(^\s*\%(pub\s\+\)\=\)\@<=\%(ability\|const\|contract\|tag\|target\|mod\)\>\ze\s\+\h'
syn match ambientDefine '\%(^\s*\%(pub\s\+\)\=\)\@<=extern\>\ze\s\+\%(fn\|handler\)\>'
syn match ambientDefine '\%(\<extern\s\+\)\@<=handler\>'
syn match ambientInclude '\%(^\s*\%(pub\s\+\)\=\)\@<=use\>\ze\s\+\h'

" Words that mean something only after another: `for x in xs`, `use a as
" b`, `target t extends u built by tool`, `type(nominal)`, `extern handler
" a for b in t`.
syn match ambientRepeat '\%(^\s*for\>.*\s\)\@<=in\>\ze\s'
syn match ambientInclude '\%(\w\s\+\)\@<=as\>\ze\s\+\h\w*\s*[,;}]'
syn match ambientKeyword '\%(^\s*\%(pub\s\+\)\=target\>.*\s\)\@<=\%(extends\|built\s\+by\)\>'
syn match ambientKeyword '\%(^\s*\%(pub\s\+\)\=extern\s\+handler\>.*\s\)\@<=\%(for\|in\)\>\ze\s'
syn match ambientKeyword '\%(\<type(\)\@<=nominal\>\ze)'
syn match ambientKeyword '\%(\<handler(\)\@<=suspending\>\ze)'

" An annotation: `#[binding("document.title")]`, or `#[test.case]`. Its
" key is a path, and one with no value closes where its key ends.
syn match ambientAnnotation '#\[\s*\h\w*\%(\.\h\w*\)*\%(\s*\]\)\=' contains=ambientAnnotationMark
syn match ambientAnnotationMark '#\[\|\]' contained

" Numbers: `1_000`, `1.5`, `1e3`, `2.5E-4`. A dot is a fraction only
" before a digit, so `1.to_text()` is a call on `1`.
syn match ambientNumber '\<\d[0-9_]*\>'
syn match ambientFloat '\<\d[0-9_]*\.\d[0-9_]*\%([eE][+-]\=\d[0-9_]*\)\=\>'
syn match ambientFloat '\<\d[0-9_]*[eE][+-]\=\d[0-9_]*\>'

" Bytes written out: `0x[89 50 4e 47]`.
syn region ambientBytes start='\<0x\[' end='\]'

" A string stays on its line, and has values written into it by `${...}`.
syn region ambientString start='"' skip='\\.' end='"' end='$' oneline contains=ambientEscape,ambientInterpolation
syn match ambientEscape '\\[nrt0"\\$]' contained
syn match ambientEscape '\\u{\x\+}' contained
syn region ambientInterpolation matchgroup=ambientInterpolationDelim start='\${' end='}' contained contains=TOP,ambientComment,ambientDoc

" Operators, before comments so that `//` is not two of them.
syn match ambientOperator '\.\.\.\|=>\|==\|!=\|<=\|>=\|&&\|||\|??\|::\|\*\*\|[-+*/%|&?=<>!]'

" `///` is documentation; `//` and `////` are comments.
syn match ambientComment '//.*$' contains=ambientTodo,@Spell
syn match ambientDoc '///\%(/\)\@!.*$' contains=ambientTodo,ambientDocCode,@Spell
syn match ambientDocCode '`[^`]*`' contained
syn keyword ambientTodo TODO FIXME XXX NOTE contained

hi def link ambientDefine Keyword
hi def link ambientInclude Include
hi def link ambientStorage StorageClass
hi def link ambientKeyword Keyword
hi def link ambientConditional Conditional
hi def link ambientRepeat Repeat
hi def link ambientScope Statement
hi def link ambientBoolean Boolean
hi def link ambientConstant Constant
hi def link ambientBuiltinType Type
hi def link ambientType Type
hi def link ambientSelf Identifier
hi def link ambientFunction Function
hi def link ambientCall Function
hi def link ambientPerform Special
hi def link ambientAnnotation PreProc
hi def link ambientAnnotationMark PreProc
hi def link ambientNumber Number
hi def link ambientFloat Float
hi def link ambientBytes Number
hi def link ambientString String
hi def link ambientEscape SpecialChar
hi def link ambientInterpolationDelim Special
hi def link ambientComment Comment
hi def link ambientDoc SpecialComment
hi def link ambientDocCode String
hi def link ambientTodo Todo
hi def link ambientOperator Operator

let b:current_syntax = 'ambient'
