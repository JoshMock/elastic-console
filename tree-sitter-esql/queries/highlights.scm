; Commands and clause keywords
["FROM" "TS" "METADATA" "ROW" "SHOW" "INFO" "WHERE" "EVAL" "STATS" "INLINESTATS" "BY" "KEEP" "DROP" "RENAME" "AS" "SORT" "ASC" "DESC" "NULLS" "FIRST" "LAST" "LIMIT" "DISSECT" "GROK" "ENRICH" "ON" "WITH" "MV_EXPAND" "LOOKUP" "JOIN" "SAMPLE" "CHANGE_POINT"] @keyword

; Word operators
["AND" "OR" "NOT" "IS" "IN" "LIKE" "RLIKE"] @keyword.operator

["==" "!=" "<" "<=" ">" ">=" "=~" "+" "-" "*" "/" "%" "=" "::" ":"] @operator

["|" "," "."] @punctuation.delimiter
["(" ")" "[" "]"] @punctuation.bracket

(string) @string
(integer) @number
(decimal) @number
(qualified_integer (unit) @type)
(boolean) @boolean
(null) @constant.builtin
(parameter) @variable.parameter
(variable) @variable.parameter
(comment) @comment

(function_name) @function
(type) @type
(index_pattern) @module
(policy_name) @module
(field name: (identifier) @property)
(identifier) @variable
