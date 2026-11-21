# Time

The standard library reads a moment as a date and writes it as text. It is written in the language and takes no ability: what time it is now is the world's to say.

- A moment is an integer, milliseconds since the start of 1970, which is what a clock gives.
- A length of time is an integer of milliseconds too, so moments and lengths add and compare as integers do.
- A length made as one is [branded](types.md) a duration, and what waits takes only a duration: a bare number is not mistaken for a length, in whatever unit its writer had in mind.
- A date is a record of a year, a month, a day, an hour, a minute, a second, and a millisecond, by the Gregorian calendar in UTC.
- Every moment has one date. A date has one moment, or none if there is no such date, as with the thirtieth of February.
- A day is 86,400 seconds: leap seconds are not counted, as the clock does not count them.
- The day of the week is a number from 1, Monday, to 7.
- A moment is written as RFC 3339 text in UTC, to the millisecond.
- Reading takes RFC 3339 text at any offset, a date alone as its first moment, and text with no offset as UTC. It gives a moment or null; nothing is read halfway.
- A date somewhere else is the date of the moment with that place's offset added: an offset is a length of time, ahead of UTC or behind it.
- The library knows no place's offset. How far ahead the machine's own zone is at a moment is the [world's](targets.md#the-system-package) to say, and is asked of each moment, since daylight saving changes it.
- TODO: there are no named time zones. A program that wants the date in a place other than its machine's is told that place's offset.
- TODO: nothing writes a date in words or by a pattern.
