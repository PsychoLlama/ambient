//! The entry points a native bundle may call: the one list of them.
//!
//! What a value handed over is owned by is said of each. A value a
//! bundle owns it gives away or releases; one it borrows it retains
//! before it keeps it past its own return.

/// Hands every entry point, as a function's signature and what it
/// does, to the macro named.
///
/// A signature names only `Word`, `Call`, `Make`, `Data`, integers,
/// and pointers to them, which whoever expands this has in scope.
#[macro_export]
macro_rules! entries {
    ($with:ident) => {
        $with! {
            /// What a value is: one of the `kind` numbers.
            fn amb_kind(value: Word) -> u64;
            /// Holds a value once more.
            fn amb_retain(value: Word);
            /// Lets go of a value held: nothing may read it through
            /// that hold again.
            fn amb_release(value: Word);

            /// The integer an integer value is.
            fn amb_int_value(value: Word) -> i64;
            /// An integer as a value, which the caller owns.
            fn amb_int_new(value: i64) -> Word;
            /// The bits of the float a float value is.
            fn amb_float_bits(value: Word) -> u64;
            /// A float, given as its bits, as a value the caller owns.
            fn amb_float_new(bits: u64) -> Word;

            /// How many members a tuple or list has, fields a record
            /// has, or bytes a string, a tag, or a bytes value has.
            fn amb_length(value: Word) -> u64;
            /// Where the bytes of a string, a tag, or a bytes value
            /// are, for as long as the value is held. A tag's are its
            /// package's ID, a zero byte, and its name.
            fn amb_string_bytes(value: Word) -> *const u8;
            /// A string of `length` bytes of UTF-8, copied, which the
            /// caller owns.
            fn amb_string_new(bytes: *const u8, length: u64) -> Word;
            /// A bytes value of `length` bytes, copied, which the
            /// caller owns.
            fn amb_bytes_new(bytes: *const u8, length: u64) -> Word;

            /// The word at `index` of a value, borrowed: a member of a
            /// tuple or list, or, of a record, a key at an even index
            /// and its value at the next. Of a handle, its brand at
            /// zero, a tag, and its number at one, which is no value.
            fn amb_word_at(value: Word, index: u64) -> Word;
            /// A tuple, or a list if `list` is not zero, of `length`
            /// nulls, which the caller owns. Fill it with
            /// `amb_sequence_set` before anything else sees it.
            fn amb_sequence_new(list: u64, length: u64) -> Word;
            /// Puts a value at `index` of a tuple or list being made,
            /// which holds it for itself: the caller owns it still.
            fn amb_sequence_set(sequence: Word, index: u64, value: Word);

            /// The one field name with these bytes, never released.
            /// Two names are the same exactly when their words are.
            fn amb_intern(name: *const u8, length: u64) -> Word;
            /// A record with room for `length` fields, which the
            /// caller owns. Fill it with `amb_record_set` before
            /// anything else sees it.
            fn amb_record_new(length: u64) -> Word;
            /// Adds a field to a record being made, under a name from
            /// `amb_intern`. The record holds the value for itself.
            fn amb_record_set(record: Word, key: Word, value: Word);

            /// The one tag with this text, never released: its
            /// package's ID, a zero byte, and its name. Two tags are
            /// the same exactly when their words are.
            fn amb_tag(text: *const u8, length: u64) -> Word;
            /// A handle minted under `brand`, a tag, that its bundle
            /// knows by `number`. The caller owns it.
            fn amb_handle_new(brand: Word, number: u64) -> Word;

            /// Says the call failed, with a message of `length` bytes:
            /// the task that made it faults. The function then gives
            /// null.
            fn amb_bundle_fail(call: *mut Call, message: *const u8, length: u64);
            /// Says the call answers later, and gives the ticket its
            /// answer comes by. The function then gives null. Zero,
            /// with the call failed, if the extern is not declared
            /// `suspending`.
            fn amb_bundle_later(call: *mut Call) -> u64;
            /// Answers the wait of a ticket, from any thread: the one
            /// entry beside `amb_bundle_refuse` a thread the bundle
            /// started may call. `make` is called once with `data`, on
            /// the program's thread, and gives the answer. Zero, with
            /// `make` never called, if the task has left the wait.
            fn amb_bundle_answer(ticket: u64, make: Make, data: *mut Data) -> u64;
            /// Fails the wait of a ticket, from any thread, with a
            /// message of `length` bytes. Zero if the task has left
            /// the wait.
            fn amb_bundle_refuse(ticket: u64, message: *const u8, length: u64) -> u64;
            /// Whether the task of a ticket waits still, from any
            /// thread: zero once it has left the wait or was answered.
            /// A thread asks before it takes more of the world for an
            /// answer, since what it takes for a task that has left is
            /// the bundle's to keep for the next.
            fn amb_bundle_waiting(ticket: u64) -> u64;
            /// Has `dropped` called with the number of each handle of
            /// `brand`, a tag, that the program holds no longer: on
            /// the program's thread, before native code next runs,
            /// when no task can run, or when a call is over. It calls
            /// nothing of the runtime. Said again of a brand, the
            /// later function is the one called.
            fn amb_bundle_dropping(call: *mut Call, brand: Word, dropped: Dropped);
            /// The handler bound for an ability of the extern's row,
            /// named by its package's ID and its module path and name,
            /// borrowed for the call. Zero if the row has none such.
            fn amb_bundle_handler(
                call: *mut Call,
                package: *const u8,
                package_length: u64,
                path: *const u8,
                path_length: u64
            ) -> Word;
            /// Calls a function value with `count` arguments, and gives
            /// what it gave, which the caller owns. Zero if the call
            /// unwound or could not be made: the function that asked
            /// gives null at once.
            fn amb_bundle_call(call: *mut Call, function: Word, args: *const Word, count: u64) -> Word;
        }
    };
}
