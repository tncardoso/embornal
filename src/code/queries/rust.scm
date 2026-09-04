; Rust definitions. Nesting is not asked for here: a definition that sits
; inside the span of another is its child, and the tree does that itself.

(mod_item name: (identifier) @name) @definition.module

(struct_item name: (type_identifier) @name) @definition.class
(enum_item name: (type_identifier) @name) @definition.class
(union_item name: (type_identifier) @name) @definition.class
(trait_item name: (type_identifier) @name) @definition.class
(type_item name: (type_identifier) @name) @definition.class

; Two patterns reach one node. The second adds the trait, so that `impl Memory`
; and `impl Display for Memory` do not answer to the same name.
(impl_item type: (_) @name) @definition.impl
(impl_item trait: (_) @context type: (_) @name) @definition.impl

; A body is what a summary describes, so `function_signature_item` — a method
; that a trait declares and does not define — is absent. The trait itself is
; indexed, and its summary covers the contract.
(function_item name: (identifier) @name) @definition.function
(macro_definition name: (identifier) @name) @definition.function
