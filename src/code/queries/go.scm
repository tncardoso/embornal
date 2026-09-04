; Go definitions.

(type_spec name: (type_identifier) @name) @definition.class

(function_declaration name: (identifier) @name) @definition.function

; The receiver joins the name, because two types of one package can both hold
; a method called `String`. A pointer receiver names the same type as a value
; receiver, so the star does not reach the name.
(method_declaration
  receiver: (parameter_list (parameter_declaration type: (type_identifier) @context))
  name: (field_identifier) @name) @definition.function

(method_declaration
  receiver: (parameter_list
    (parameter_declaration type: (pointer_type (type_identifier) @context)))
  name: (field_identifier) @name) @definition.function
