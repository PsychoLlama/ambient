# Standard Library Plan

## Overview

Build a functional, immutable standard library for all language built-ins. Functions should follow functional programming idioms with lambdas typically appearing last for nice line wrapping.

## Design Principles

1. **Functional style** - Higher-order functions, composition, no mutation
2. **Immutable** - All operations return new values
3. **Lambda last** - When a function takes a lambda, it should be the last parameter for better line wrapping
4. **No side effects** - Pure functions only (side effects via abilities)
5. **Consistent naming** - Follow established functional programming conventions

## Built-in Types to Support

### Primitives
- `Number` - Mathematical operations, comparisons, conversions
- `String` - Text manipulation, searching, transformation
- `Bool` - Logical operations

### Collections
- `List<T>` - Ordered, homogeneous collection
- `Map<K, V>` - Key-value store (K = String)
- `Set<T>` - Unique elements

### Sum Types
- `Option<T>` - Optional values (None | Some(T))
- `Result<T, E>` - Error handling (Ok(T) | Err(E))

## Implementation Approach

The VM already has bytecode operations for these types. The stdlib will be written in Ambient itself, using:
1. Direct language constructs where supported (pattern matching, etc.)
2. Built-in operations exposed via the type checker/compiler

### Current VM Operations

**List**: MakeList, ListGet, ListLength, ListConcat, ListAppend, ListHead, ListTail
**String**: StringLength, StringSplit, StringJoin, StringTrim, StringContains, StringConcat
**Map**: MakeEmptyMap, MapGet, MapInsert, MapRemove, MapContains, MapLength, MapKeys, MapValues
**Set**: MakeEmptySet, MakeSet, SetInsert, SetRemove, SetContains, SetLength, SetUnion, SetIntersection, SetDifference, SetToList
**Enum**: MakeEnum, EnumIs, EnumPayload, EnumTag
**Option**: OptionUnwrapOr (others not yet implemented in VM)

## Standard Library Functions

### List Module

```ambient
// Construction
fn empty(): List<a>
fn singleton(value: a): List<a>
fn repeat(value: a, count: number): List<a>
fn range(start: number, end: number): List<number>

// Access
fn head(list: List<a>): Option<a>
fn tail(list: List<a>): List<a>
fn last(list: List<a>): Option<a>
fn init(list: List<a>): List<a>
fn get(list: List<a>, index: number): Option<a>
fn length(list: List<a>): number

// Transform (lambda last)
fn map(list: List<a>, f: (a) -> b): List<b>
fn filter(list: List<a>, pred: (a) -> bool): List<a>
fn filter_map(list: List<a>, f: (a) -> Option<b>): List<b>

// Fold (lambda last)
fn fold(list: List<a>, init: b, f: (b, a) -> b): b
fn fold_right(list: List<a>, init: b, f: (a, b) -> b): b
fn reduce(list: List<a>, f: (a, a) -> a): Option<a>

// Search
fn find(list: List<a>, pred: (a) -> bool): Option<a>
fn find_index(list: List<a>, pred: (a) -> bool): Option<number>
fn any(list: List<a>, pred: (a) -> bool): bool
fn all(list: List<a>, pred: (a) -> bool): bool
fn contains(list: List<a>, value: a): bool

// Combine
fn concat(a: List<a>, b: List<a>): List<a>
fn flatten(lists: List<List<a>>): List<a>
fn flat_map(list: List<a>, f: (a) -> List<b>): List<b>
fn zip(a: List<a>, b: List<b>): List<(a, b)>
fn zip_with(a: List<a>, b: List<b>, f: (a, b) -> c): List<c>

// Slice
fn take(list: List<a>, count: number): List<a>
fn drop(list: List<a>, count: number): List<a>
fn take_while(list: List<a>, pred: (a) -> bool): List<a>
fn drop_while(list: List<a>, pred: (a) -> bool): List<a>
fn slice(list: List<a>, start: number, end: number): List<a>

// Transform
fn reverse(list: List<a>): List<a>
fn sort(list: List<number>): List<number>
fn sort_by(list: List<a>, compare: (a, a) -> number): List<a>
fn unique(list: List<a>): List<a>
fn intersperse(list: List<a>, sep: a): List<a>

// Partition
fn partition(list: List<a>, pred: (a) -> bool): (List<a>, List<a>)
fn split_at(list: List<a>, index: number): (List<a>, List<a>)
fn group_by(list: List<a>, key: (a) -> k): Map<k, List<a>>
```

### String Module

```ambient
// Access
fn length(s: string): number
fn char_at(s: string, index: number): Option<string>
fn chars(s: string): List<string>

// Transform
fn to_upper(s: string): string
fn to_lower(s: string): string
fn trim(s: string): string
fn trim_start(s: string): string
fn trim_end(s: string): string
fn reverse(s: string): string

// Search
fn contains(s: string, substr: string): bool
fn starts_with(s: string, prefix: string): bool
fn ends_with(s: string, suffix: string): bool
fn index_of(s: string, substr: string): Option<number>

// Split/Join
fn split(s: string, delimiter: string): List<string>
fn join(parts: List<string>, delimiter: string): string
fn lines(s: string): List<string>
fn words(s: string): List<string>

// Modify
fn replace(s: string, old: string, new: string): string
fn replace_all(s: string, old: string, new: string): string
fn concat(a: string, b: string): string
fn repeat(s: string, count: number): string
fn pad_left(s: string, width: number, char: string): string
fn pad_right(s: string, width: number, char: string): string
fn slice(s: string, start: number, end: number): string

// Conversion
fn parse_number(s: string): Option<number>
fn parse_bool(s: string): Option<bool>
```

### Number Module

```ambient
// Math
fn abs(n: number): number
fn neg(n: number): number
fn sign(n: number): number
fn min(a: number, b: number): number
fn max(a: number, b: number): number
fn clamp(n: number, low: number, high: number): number

// Rounding
fn floor(n: number): number
fn ceil(n: number): number
fn round(n: number): number
fn trunc(n: number): number

// Comparison
fn is_nan(n: number): bool
fn is_finite(n: number): bool
fn is_positive(n: number): bool
fn is_negative(n: number): bool
fn is_zero(n: number): bool

// Conversion
fn to_string(n: number): string

// Constants (via functions since no const values)
fn pi(): number
fn e(): number
fn infinity(): number
fn nan(): number
```

### Option Module

```ambient
// Construction
fn none(): Option<a>
fn some(value: a): Option<a>

// Query
fn is_some(opt: Option<a>): bool
fn is_none(opt: Option<a>): bool

// Access
fn unwrap(opt: Option<a>): a  // Panics on None - use sparingly
fn unwrap_or(opt: Option<a>, default: a): a
fn unwrap_or_else(opt: Option<a>, f: () -> a): a

// Transform (lambda last)
fn map(opt: Option<a>, f: (a) -> b): Option<b>
fn and_then(opt: Option<a>, f: (a) -> Option<b>): Option<b>
fn or_else(opt: Option<a>, f: () -> Option<a>): Option<a>
fn filter(opt: Option<a>, pred: (a) -> bool): Option<a>

// Combine
fn or(opt: Option<a>, other: Option<a>): Option<a>
fn and(opt: Option<a>, other: Option<b>): Option<b>
fn zip(a: Option<a>, b: Option<b>): Option<(a, b)>

// Convert
fn to_list(opt: Option<a>): List<a>
fn ok_or(opt: Option<a>, err: e): Result<a, e>
```

### Result Module

```ambient
// Construction
fn ok(value: a): Result<a, e>
fn err(error: e): Result<a, e>

// Query
fn is_ok(res: Result<a, e>): bool
fn is_err(res: Result<a, e>): bool

// Access
fn unwrap(res: Result<a, e>): a  // Panics on Err
fn unwrap_err(res: Result<a, e>): e  // Panics on Ok
fn unwrap_or(res: Result<a, e>, default: a): a
fn unwrap_or_else(res: Result<a, e>, f: (e) -> a): a

// Transform (lambda last)
fn map(res: Result<a, e>, f: (a) -> b): Result<b, e>
fn map_err(res: Result<a, e>, f: (e) -> f): Result<a, f>
fn and_then(res: Result<a, e>, f: (a) -> Result<b, e>): Result<b, e>
fn or_else(res: Result<a, e>, f: (e) -> Result<a, f>): Result<a, f>

// Combine
fn or(res: Result<a, e>, other: Result<a, e>): Result<a, e>
fn and(res: Result<a, e>, other: Result<b, e>): Result<b, e>

// Convert
fn ok(res: Result<a, e>): Option<a>
fn err(res: Result<a, e>): Option<e>
```

### Map Module

```ambient
// Construction
fn empty(): Map<k, v>
fn singleton(key: string, value: v): Map<string, v>
fn from_list(pairs: List<(string, v)>): Map<string, v>

// Access
fn get(map: Map<k, v>, key: k): Option<v>
fn get_or(map: Map<k, v>, key: k, default: v): v
fn contains_key(map: Map<k, v>, key: k): bool
fn length(map: Map<k, v>): number
fn is_empty(map: Map<k, v>): bool

// Modify
fn insert(map: Map<k, v>, key: k, value: v): Map<k, v>
fn remove(map: Map<k, v>, key: k): Map<k, v>
fn update(map: Map<k, v>, key: k, f: (Option<v>) -> Option<v>): Map<k, v>

// Transform (lambda last)
fn map_values(map: Map<k, v>, f: (v) -> w): Map<k, w>
fn filter(map: Map<k, v>, pred: (k, v) -> bool): Map<k, v>
fn fold(map: Map<k, v>, init: a, f: (a, k, v) -> a): a

// Convert
fn keys(map: Map<k, v>): List<k>
fn values(map: Map<k, v>): List<v>
fn entries(map: Map<k, v>): List<(k, v)>
fn to_list(map: Map<k, v>): List<(k, v)>

// Combine
fn merge(a: Map<k, v>, b: Map<k, v>): Map<k, v>
fn merge_with(a: Map<k, v>, b: Map<k, v>, f: (v, v) -> v): Map<k, v>
```

### Set Module

```ambient
// Construction
fn empty(): Set<a>
fn singleton(value: a): Set<a>
fn from_list(list: List<a>): Set<a>

// Access
fn contains(set: Set<a>, value: a): bool
fn length(set: Set<a>): number
fn is_empty(set: Set<a>): bool

// Modify
fn insert(set: Set<a>, value: a): Set<a>
fn remove(set: Set<a>, value: a): Set<a>

// Set operations
fn union(a: Set<a>, b: Set<a>): Set<a>
fn intersection(a: Set<a>, b: Set<a>): Set<a>
fn difference(a: Set<a>, b: Set<a>): Set<a>
fn symmetric_difference(a: Set<a>, b: Set<a>): Set<a>
fn is_subset(a: Set<a>, b: Set<a>): bool
fn is_superset(a: Set<a>, b: Set<a>): bool
fn is_disjoint(a: Set<a>, b: Set<a>): bool

// Transform (lambda last)
fn map(set: Set<a>, f: (a) -> b): Set<b>
fn filter(set: Set<a>, pred: (a) -> bool): Set<a>
fn fold(set: Set<a>, init: b, f: (b, a) -> b): b

// Convert
fn to_list(set: Set<a>): List<a>
```

### Bool Module

```ambient
fn not(b: bool): bool
fn and(a: bool, b: bool): bool
fn or(a: bool, b: bool): bool
fn xor(a: bool, b: bool): bool
fn to_string(b: bool): string
```

### Tuple Module

```ambient
fn pair(a: a, b: b): (a, b)
fn triple(a: a, b: b, c: c): (a, b, c)
fn first(t: (a, b)): a
fn second(t: (a, b)): b
fn swap(t: (a, b)): (b, a)
fn map_first(t: (a, b), f: (a) -> c): (c, b)
fn map_second(t: (a, b), f: (b) -> c): (a, c)
fn map_both(t: (a, b), f: (a) -> c, g: (b) -> d): (c, d)
```

### Function Utilities

```ambient
fn identity(x: a): a
fn constant(x: a): (b) -> a
fn compose(f: (b) -> c, g: (a) -> b): (a) -> c
fn pipe(x: a, f: (a) -> b): b
fn flip(f: (a, b) -> c): (b, a) -> c
fn curry(f: (a, b) -> c): (a) -> (b) -> c
fn uncurry(f: (a) -> (b) -> c): (a, b) -> c
```

## Implementation Order

### Phase 1: Core Functions
Focus on the most commonly needed operations.

1. **List basics**: map, filter, fold, head, tail, length, get, concat
2. **Option basics**: is_some, is_none, unwrap_or, map, and_then
3. **Result basics**: is_ok, is_err, unwrap_or, map, map_err
4. **String basics**: length, split, join, contains, trim
5. **Number basics**: abs, min, max, floor, ceil, round

### Phase 2: Extended Operations
Build on Phase 1 for richer functionality.

1. **List advanced**: find, any, all, take, drop, reverse, zip, flatten
2. **Map/Set**: from_list, to_list, merge, filter
3. **Function utilities**: identity, compose, pipe

### Phase 3: Complete Coverage
Fill in remaining functions.

1. Sorting, grouping, partitioning
2. String manipulation (pad, replace)
3. Edge cases and optimizations

## File Structure

```
stdlib/
  list.ab      - List operations
  option.ab    - Option operations
  result.ab    - Result operations
  string.ab    - String operations
  number.ab    - Number operations
  map.ab       - Map operations
  set.ab       - Set operations
  bool.ab      - Boolean operations
  tuple.ab     - Tuple operations
  function.ab  - Function utilities
  prelude.ab   - Re-exports common functions
```

## Open Questions

1. **Module system**: How are modules imported? Need to check if there's an `import` syntax.
2. **Primitive operations**: Are bytecode ops like ListGet exposed as language constructs or do we need compiler support?
3. **Type annotations**: Can we use `forall` for generic type signatures?
4. **Recursion for loops**: Since there are no loops, recursion is needed for iteration. Need tail-call optimization.
