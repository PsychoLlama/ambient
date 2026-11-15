# Time

The standard library reads a moment as a date and writes it as text. It is written in the language and takes no ability: what time it is now is the world's to say.

- A moment is an integer, milliseconds since the start of 1970, which is what a clock gives.
- A length of time is an integer of milliseconds too, so moments and lengths add and compare as integers do.
- A date is a record of a year, a month, a day, an hour, a minute, a second, and a millisecond, by the Gregorian calendar in UTC.
- Every moment has one date. A date has one moment, or none if there is no such date, as with the thirtieth of February.
- A day is 86,400 seconds: leap seconds are not counted, as the clock does not count them.
- The day of the week is a number from 1, Monday, to 7.
- A moment is written as RFC 3339 text in UTC, to the millisecond.
- Reading takes RFC 3339 text at any offset, a date alone as its first moment, and text with no offset as UTC. It gives a moment or null; nothing is read halfway.
- TODO: there are no time zones. A date is in UTC, and a program that wants another offset adds it.
- TODO: nothing writes a date in words or by a pattern.
