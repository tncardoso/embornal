; JavaScript definitions. The same file serves JSX.

(class_declaration name: (identifier) @name) @definition.class

(function_declaration name: (identifier) @name) @definition.function
(generator_function_declaration name: (identifier) @name) @definition.function
(method_definition name: (property_identifier) @name) @definition.function

; A function that a constant holds is a definition like any other, and in this
; language it is the common one.
(variable_declarator
  name: (identifier) @name
  value: [(arrow_function) (function_expression)]) @definition.function
