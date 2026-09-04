; Python definitions. The file is the module, so nothing here names one.
; A decorated definition holds the definition itself, which matches on its own.

(class_definition name: (identifier) @name) @definition.class
(function_definition name: (identifier) @name) @definition.function
