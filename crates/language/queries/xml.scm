; Tag punctuation shares the element name's color, including closing/empty tags.
(STag ["<" ">"] @tag.delimiter)
(ETag ["</" ">"] @tag.delimiter)
(EmptyElemTag ["<" "/>"] @tag.delimiter)

; XML attributes are distinct from programming-language properties and strings.
(Attribute (Name) @tag.attribute)
(Attribute "=" @tag.attribute)
(Attribute (AttValue) @string.attribute)

; The XML declaration uses the same tag, attribute and quoted-value colors.
(XMLDecl ["<?" "xml" "?>"] @tag)
(XMLDecl ["version" "encoding" "standalone" "="] @tag.attribute)
(XMLDecl [(VersionNum) (EncName) "yes" "no" "\"" "'"] @string.attribute)
