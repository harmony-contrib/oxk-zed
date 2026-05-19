; ArkTS-specific rules from harmony-contrib/tree-sitter-arkts, plus common
; JavaScript/TypeScript nodes that the ArkTS grammar inherits.

(hash_bang_line) @comment
(comment) @comment
(html_comment) @comment

[
  (string)
  (template_string)
] @string

(escape_sequence) @string.escape
(regex_pattern) @string.regex
(regex_flags) @string.special

(number) @number

[
  (true)
  (false)
] @boolean

[
  (null)
  (undefined)
] @constant.builtin

(type_identifier) @type
(predefined_type) @type.builtin

((identifier) @type
 (#match? @type "^[A-Z]"))

[
  "abstract"
  "accessor"
  "as"
  "assert"
  "asserts"
  "async"
  "await"
  "break"
  "case"
  "catch"
  "class"
  "const"
  "continue"
  "debugger"
  "declare"
  "default"
  "delete"
  "do"
  "else"
  "enum"
  "export"
  "extends"
  "finally"
  "for"
  "from"
  "function"
  "get"
  "global"
  "if"
  "implements"
  "import"
  "in"
  "infer"
  "instanceof"
  "interface"
  "is"
  "keyof"
  "lazy"
  "let"
  "module"
  "namespace"
  "new"
  "of"
  "override"
  "private"
  "protected"
  "public"
  "readonly"
  "require"
  "return"
  "satisfies"
  "set"
  "static"
  "struct"
  "switch"
  "target"
  "throw"
  "try"
  "type"
  "typeof"
  "using"
  "var"
  "void"
  "while"
  "with"
  "yield"
] @keyword

[
  "any"
  "boolean"
  "never"
  "number"
  "object"
  "string"
  "symbol"
  "unknown"
  "unique symbol"
] @type.builtin

[
  "="
  "+="
  "-="
  "*="
  "/="
  "%="
  "&="
  "|="
  "^="
  "<<="
  ">>="
  ">>>="
  "&&="
  "||="
  "??="
  "+"
  "-"
  "*"
  "/"
  "%"
  "++"
  "--"
  "**"
  "&"
  "|"
  "^"
  "~"
  "<<"
  ">>"
  ">>>"
  "&&"
  "||"
  "!"
  "=="
  "!="
  "==="
  "!=="
  "<"
  ">"
  "<="
  ">="
  "?"
  "?:"
  "+?:"
  "-?:"
  ":"
  "??"
  "?."
  "..."
  "=>"
] @operator

[
  ";"
  ","
  "."
] @punctuation.delimiter

[
  "("
  ")"
  "["
  "]"
  "{"
  "}"
  "{|"
  "|}"
] @punctuation.bracket

(decorator "@" @attribute)

(struct_declaration
  name: (type_identifier) @type)

(annotation_declaration
  name: (type_identifier) @type)

(class_declaration
  name: (type_identifier) @type)

(abstract_class_declaration
  name: (type_identifier) @type)

(interface_declaration
  name: (type_identifier) @type)

(type_alias_declaration
  name: (type_identifier) @type)

(enum_declaration
  name: (identifier) @type)

(function_declaration
  name: (identifier) @function)

(function_signature
  name: (identifier) @function)

(method_definition
  name: [
    (property_identifier)
    (private_property_identifier)
  ] @function.method)

(method_signature
  name: [
    (property_identifier)
    (private_property_identifier)
  ] @function.method)

(abstract_method_signature
  name: [
    (property_identifier)
    (private_property_identifier)
  ] @function.method)

(call_expression
  function: (identifier) @function)

(call_expression
  function: (member_expression
    property: (property_identifier) @function.method))

(arkui_component_expression
  function: (identifier) @constructor)

(leading_dot_expression
  "." @punctuation.delimiter)

(member_expression
  property: [
    (property_identifier)
    (private_property_identifier)
  ] @property)

(public_field_definition
  name: [
    (property_identifier)
    (private_property_identifier)
  ] @property)

(property_signature
  name: [
    (property_identifier)
    (private_property_identifier)
  ] @property)

(pair
  key: (property_identifier) @property)

(required_parameter (identifier) @variable.parameter)
(optional_parameter (identifier) @variable.parameter)

[
  (this)
  (super)
] @variable.special

(shorthand_property_identifier) @variable
(shorthand_property_identifier_pattern) @variable
(statement_identifier) @label

(ERROR) @error
