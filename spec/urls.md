# URLs

The standard library reads and writes URLs. It is written in the language, over strings, and takes no ability.

- A URL is a record of its parts: a scheme, a host, a port or null, a path, a query, and a fragment or null.
- Reading text gives a URL or null; nothing is read halfway.
- Text with no scheme is read as a path, a query, and a fragment: what a server is asked for is a URL with no host.
- A query is a list of names and values, since a name may come twice and their order is the sender's.
- What is read is decoded and what is written is encoded, so a part held in a URL is the text it means.
- Encoding leaves letters, digits, and `-`, `_`, `.`, `~` as they are, and writes every other character as the bytes of its UTF-8 encoding.
- A `+` in a query is read as a space, as a form sends one; a space is always written as `%20`.
- A URL that is read and written is the same URL, though not always the same text.
- TODO: a name and password before the host are left in the host, and a path is not decoded, since `/` inside a part would be lost.
- TODO: one URL is not resolved against another.
