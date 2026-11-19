# Encodings

The standard library writes [bytes](operations.md#bytes) as text and reads them back. It is written in the language and takes no ability.

- Hexadecimal writes each byte as two digits, in lower case, and reads either case.
- Base64 writes three bytes as four characters, and pads what it writes with `=` to a multiple of four.
- The URL form of Base64 writes `-` and `_` where the standard form writes `+` and `/`, and pads nothing.
- Reading Base64 takes either form, padded or not, so a reader need not know which a sender chose.
- Reading gives bytes or null; nothing is read halfway, and text with a character of neither form is none.
- What is written and read back is the same bytes.
- Text whose last character holds bits past the last whole byte is none too, so each run of bytes is read from one text of each form and no other.
