; TypeScript definitions. TSX reads this file as well.
;
; A declaration with no body is absent on purpose. `function_signature` in an
; ambient declaration, like a method of an `interface`, holds nothing that a
; summary could say that its name does not already say. The type that holds it
; is indexed, and its summary covers the contract.

(class_declaration name: (type_identifier) @name) @definition.class
(abstract_class_declaration name: (type_identifier) @name) @definition.class
(interface_declaration name: (type_identifier) @name) @definition.class
(type_alias_declaration name: (type_identifier) @name) @definition.class
(enum_declaration name: (identifier) @name) @definition.class

(internal_module name: (identifier) @name) @definition.module

(function_declaration name: (identifier) @name) @definition.function
(generator_function_declaration name: (identifier) @name) @definition.function
(method_definition name: (property_identifier) @name) @definition.function

(variable_declarator
  name: (identifier) @name
  value: [(arrow_function) (function_expression)]) @definition.function
