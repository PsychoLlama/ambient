# Operations

An operation is something the language itself does with values: arithmetic, comparison, and reading and building strings and lists. Operations are the only code that is not a definition.

## Form

- An operation is a node of a [tree](trees.md): an operator and its operands. It is not a function, has no hash, and cannot be redefined.
- Which operation an operator means is decided by the static types of its operands; operands no operation takes are a type error.
- An operation performs no [ability](abilities.md) and never suspends.
- An operation that can miss yields `null` for the miss, and its type says so.
- A library wraps an operation in a function where a function value is wanted.

## Numbers

- Integers add, subtract, multiply, divide, and take remainders; floats add, subtract, multiply, and divide.
- Integer division truncates toward zero, and a remainder has the sign of what was divided.
- Dividing an integer by zero is a [fault](values.md), as leaving the range of `int` is.
- Less-than orders two integers, two floats, or two strings; a string is ordered by its bytes.

## Conversions

- Nothing converts unless a conversion is written.
- An integer converts to the float nearest it.
- A float converts to the integer left when its fraction is dropped, and misses if it is not a number or that integer is out of range.
- A number converts to text: an integer in decimal, and a float as the shortest decimal that reads back as the same float.
- Text converts to an integer or to a float if the whole of it is one, and misses if not; what a number converts to always converts back.

## Strings

- A position in a string is a count of the bytes of its UTF-8 encoding, and a string's length is its size in bytes.
- Joining two strings gives the string of one followed by the other.
- A slice takes a start and an end and gives the string between them.
- A slice whose positions are out of range, out of order, or inside a character misses, so every string is valid UTF-8.
- A character is one Unicode scalar value. There is no shape for one: a character is a string that holds it alone.
- A character converts to its scalar value as an integer, and an integer to the character it is the value of; a string that is not one character and an integer that is no scalar value miss.

## Bytes

- A bytes value is a run of bytes, each an integer from 0 to 255. It is a shape of its own and no kind of string: nothing in it need be text.
- Its length is how many bytes it holds, and getting one by position gives it as an integer, or misses.
- Joining and slicing are a list's: a slice misses only when its positions are out of range or out of order.
- A string converts to the bytes of its UTF-8 encoding, which never misses.
- Bytes convert to the string they encode, and miss if they encode none.
- A list of integers converts to bytes, and misses if a member is not from 0 to 255.
- Two bytes values are equal when they hold the same bytes.
- TODO: bytes cannot be written out; a program makes them from a string or a list.

## Lists

- A list's length is how many members it has, and a position is a count of members before it.
- Getting a member by position gives the member, or misses.
- Pushing a value gives a list with that value after the others; its type is a list of the union of what was there and what was pushed.
- Joining two lists gives the members of one followed by those of the other.
- A slice takes a start and an end and gives the members between them; one whose positions are out of range or out of order misses.
- No list operation takes a [tuple](types.md): its members are reached by matching.

## Iterators

- A function that steps makes an [iterator](iteration.md), a list makes one over its members, a string makes one over its characters, and two integers make one over the integers from the first up to the second.
- Stepping an iterator is not an operation.
