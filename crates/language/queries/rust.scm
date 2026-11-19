; Upstream groups literals as constants. Preserve their language roles so each
; theme can distinguish numbers, boolean keywords, and named constants.
(integer_literal) @number
(float_literal) @number
(boolean_literal) @keyword
(primitive_type) @keyword
(self) @keyword
