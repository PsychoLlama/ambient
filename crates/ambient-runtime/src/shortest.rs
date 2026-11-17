//! A float as the shortest decimal that reads back as the same float.
//! See "Conversions" in `spec/operations.md`.
//!
//! Written out here because the standard library's formatting, which
//! would do it, is twenty-six thousand bytes of a WebAssembly module
//! that only turns a float into text. This is its exact method and no
//! other (Steele and White's, as Burger and Dybvig give it), which is
//! short to write and slow only for a float far from one: it works in
//! whole numbers as large as the float's exponent makes them.
//!
//! What it writes is what the standard library does for `{:?}`, which
//! the tests hold it to.

use core::cmp::Ordering;

/// The words of the largest number the method meets, which is under
/// two to the 1140th: the smallest float's mantissa, scaled to a whole
/// number of its last place.
const LIMBS: usize = 36;

/// A whole number of up to [`LIMBS`] words, least first.
#[derive(Clone, Copy)]
struct Big {
    limbs: [u32; LIMBS],
    /// How many of them are in use: the rest are zero.
    used: usize,
}

#[allow(clippy::cast_possible_truncation)]
impl Big {
    fn of(value: u64) -> Self {
        let mut limbs = [0; LIMBS];
        (limbs[0], limbs[1]) = (value as u32, (value >> 32) as u32);
        let mut big = Self { limbs, used: 2 };
        big.trim();
        big
    }

    fn trim(&mut self) {
        while self.used > 0 && self.limbs[self.used - 1] == 0 {
            self.used -= 1;
        }
    }

    fn times(&mut self, by: u32) {
        let mut carry = 0;
        for limb in &mut self.limbs[..self.used] {
            let wide = u64::from(*limb) * u64::from(by) + carry;
            (*limb, carry) = (wide as u32, wide >> 32);
        }
        if carry != 0 {
            self.limbs[self.used] = carry as u32;
            self.used += 1;
        }
    }

    /// Multiplies by ten to a power.
    fn tens(&mut self, power: u32) {
        for _ in 0..power / 9 {
            self.times(1_000_000_000);
        }
        self.times(10_u32.pow(power % 9));
    }

    /// Multiplies by two to a power.
    fn doubled(&mut self, power: u32) {
        let (whole, part) = ((power / 32) as usize, power % 32);
        for at in (0..self.used).rev() {
            self.limbs[at + whole] = self.limbs[at];
        }
        self.limbs[..whole].fill(0);
        self.used += whole;
        let mut carry = 0;
        for limb in &mut self.limbs[whole..self.used] {
            let wide = (u64::from(*limb) << part) | carry;
            (*limb, carry) = (wide as u32, wide >> 32);
        }
        if carry != 0 {
            self.limbs[self.used] = carry as u32;
            self.used += 1;
        }
        self.trim();
    }

    fn plus(mut self, other: &Self) -> Self {
        let used = self.used.max(other.used);
        let mut carry = 0;
        for at in 0..used {
            let wide = u64::from(self.limbs[at]) + u64::from(other.limbs[at]) + carry;
            (self.limbs[at], carry) = (wide as u32, wide >> 32);
        }
        self.used = used;
        if carry != 0 {
            self.limbs[used] = 1;
            self.used += 1;
        }
        self
    }

    /// Takes away `other`, which is no larger.
    fn less(&mut self, other: &Self) {
        let mut borrow = false;
        for at in 0..self.used {
            let (first, under) = self.limbs[at].overflowing_sub(other.limbs[at]);
            let (second, again) = first.overflowing_sub(u32::from(borrow));
            (self.limbs[at], borrow) = (second, under || again);
        }
        self.trim();
    }

    fn cmp(&self, other: &Self) -> Ordering {
        let used = self.used.max(other.used);
        let (ours, theirs) = (self.limbs[..used].iter(), other.limbs[..used].iter());
        ours.rev().cmp(theirs.rev())
    }
}

/// The most digits the shortest decimal of a float has, and one more
/// for a round up that carries through all of them.
const DIGITS: usize = 18;

/// The digits of the shortest decimal that reads back as a float that
/// is neither zero nor infinite nor not a number, written into `room`:
/// how many there are, and the power of ten the number is as a
/// fraction of, so that `[1, 5]` and 1 are 1.5.
fn digits(bits: u64, room: &mut [u8; DIGITS]) -> (usize, i32) {
    // The float is `mant` times two to `exp`.
    let fraction = bits & ((1 << 52) - 1);
    #[allow(clippy::cast_possible_truncation)]
    let field = ((bits >> 52) & 0x7ff) as i32;
    let (mant, exp) = match field {
        0 => (fraction << 1, -1075),
        _ => (fraction | (1 << 52), field - 1075),
    };
    // Whatever is within `minus` below and `plus` above reads back as
    // the float, in units of half its last place: a power of two is
    // nearer the float below it. The ends do if the float is even.
    let inclusive = mant & 1 == 0;
    let (mant, plus, exp) = match (field, fraction) {
        (0, _) => (mant, 1, exp),
        (_, 0) => (mant << 2, 2, exp - 2),
        _ => (mant << 1, 1, exp - 1),
    };
    let within = |order: Ordering| order == Ordering::Less || (inclusive && order.is_eq());

    // An estimate of the power of ten, which is right or one too low.
    let bits = i64::from(64 - (mant + plus - 1).leading_zeros());
    #[allow(clippy::cast_possible_truncation)]
    let mut power = (((bits + i64::from(exp)) * 1_292_913_986) >> 32) as i32;

    // The float is `mant / scale`, times ten to `power`.
    let (mut mant, mut minus, mut plus) = (Big::of(mant), Big::of(1), Big::of(plus));
    let mut scale = Big::of(1);
    if exp < 0 {
        scale.doubled(exp.unsigned_abs());
    } else {
        for scaled in [&mut mant, &mut minus, &mut plus] {
            scaled.doubled(exp.unsigned_abs());
        }
    }
    if power >= 0 {
        scale.tens(power.unsigned_abs());
    } else {
        for scaled in [&mut mant, &mut minus, &mut plus] {
            scaled.tens(power.unsigned_abs());
        }
    }
    if within(scale.cmp(&mant.plus(&plus))) {
        power += 1;
    } else {
        for scaled in [&mut mant, &mut minus, &mut plus] {
            scaled.times(10);
        }
    }

    // A digit at a time, until what is left is within either bound.
    let mut count = 0;
    let (down, up) = loop {
        let mut digit = 0;
        while mant.cmp(&scale).is_ge() {
            mant.less(&scale);
            digit += 1;
        }
        room[count] = b'0' + digit;
        count += 1;
        let down = within(mant.cmp(&minus));
        let up = within(scale.cmp(&mant.plus(&plus)));
        if down || up {
            break (down, up);
        }
        for scaled in [&mut mant, &mut minus, &mut plus] {
            scaled.times(10);
        }
    };
    // Up if only that is within, or if what is left is half or more.
    if up && (!down || mant.plus(&mant).cmp(&scale).is_ge()) {
        let nines = room[..count]
            .iter()
            .rev()
            .take_while(|digit| **digit == b'9');
        let nines = nines.count();
        room[count - nines..count].fill(b'0');
        if nines == count {
            // It carried through: one more digit, and a power more.
            room[0] = b'1';
            room[count] = b'0';
            count += 1;
            power += 1;
        } else {
            room[count - nines - 1] += 1;
        }
    }
    (count, power)
}

/// The most bytes a float takes as text.
pub(crate) const ROOM: usize = 32;

/// What is written of a float so far.
struct Written<'a> {
    room: &'a mut [u8; ROOM],
    length: usize,
}

impl Written<'_> {
    fn push(&mut self, bytes: &[u8]) {
        self.room[self.length..self.length + bytes.len()].copy_from_slice(bytes);
        self.length += bytes.len();
    }

    fn zeros(&mut self, count: usize) {
        self.room[self.length..self.length + count].fill(b'0');
        self.length += count;
    }
}

/// `value` as text, written at the start of `room`: with a point and
/// at least one digit after it, or from a ten-thousandth down and ten
/// to the sixteenth up, as one digit, the rest, and a power of ten.
pub(crate) fn shortest(value: f64, room: &mut [u8; ROOM]) -> &[u8] {
    let mut written = Written { room, length: 0 };
    if value.is_nan() {
        written.push(b"NaN");
        return &written.room[..written.length];
    }
    if value.is_sign_negative() {
        written.push(b"-");
    }
    let size = value.abs();
    if size.is_infinite() {
        written.push(b"inf");
    } else if size == 0.0 {
        written.push(b"0.0");
    } else {
        let mut found = [0; DIGITS];
        let (count, power) = digits(size.to_bits(), &mut found);
        let found = &found[..count];
        if (1e-4..1e16).contains(&size) {
            plain(&mut written, found, power);
        } else {
            powered(&mut written, found, power);
        }
    }
    &written.room[..written.length]
}

/// Digits that are a fraction of ten to `power`, with a point.
fn plain(written: &mut Written, digits: &[u8], power: i32) {
    let whole = usize::try_from(power).unwrap_or(0);
    if power <= 0 {
        written.push(b"0.");
        written.zeros(power.unsigned_abs() as usize);
        written.push(digits);
    } else if whole < digits.len() {
        written.push(&digits[..whole]);
        written.push(b".");
        written.push(&digits[whole..]);
    } else {
        written.push(digits);
        written.zeros(whole - digits.len());
        written.push(b".0");
    }
}

/// Digits that are a fraction of ten to `power`, as the first, the
/// rest, and the power of ten the first is of.
fn powered(written: &mut Written, digits: &[u8], power: i32) {
    written.push(&digits[..1]);
    if digits.len() > 1 {
        written.push(b".");
        written.push(&digits[1..]);
    }
    written.push(b"e");
    let power = power - 1;
    if power < 0 {
        written.push(b"-");
    }
    // At most three digits, written from the last.
    let mut left = power.unsigned_abs();
    let mut places = [0; 3];
    let mut at = places.len();
    loop {
        at -= 1;
        #[allow(clippy::cast_possible_truncation)]
        let digit = (left % 10) as u8;
        places[at] = b'0' + digit;
        left /= 10;
        if left == 0 {
            break;
        }
    }
    written.push(&places[at..]);
}

#[cfg(test)]
mod tests {
    use alloc::format;
    use alloc::string::String;

    use proptest::prelude::*;

    use super::*;

    fn text(value: f64) -> String {
        let mut room = [0; ROOM];
        String::from_utf8_lossy(shortest(value, &mut room)).into_owned()
    }

    /// A float one place either side of another, or the float itself.
    fn beside(value: f64, places: i64) -> f64 {
        f64::from_bits(value.to_bits().saturating_add_signed(places))
    }

    #[test]
    fn a_float_is_written_with_a_point_or_a_power_of_ten() {
        let written = [0.0, -0.0, 1.0, 1.5, -2.25, 0.1, 1e-4, 1e15, 1e16, 1.5e-7].map(text);
        let expected = [
            "0.0",
            "-0.0",
            "1.0",
            "1.5",
            "-2.25",
            "0.1",
            "0.0001",
            "1000000000000000.0",
            "1e16",
            "1.5e-7",
        ];
        assert_eq!(written, expected);
    }

    #[test]
    fn what_is_no_number_and_what_has_no_end_are_written_by_name() {
        let written = [f64::NAN, f64::INFINITY, f64::NEG_INFINITY].map(text);
        assert_eq!(written, ["NaN", "inf", "-inf"]);
    }

    #[test]
    fn the_ends_of_the_range_are_written() {
        for value in [f64::MAX, f64::MIN_POSITIVE, f64::from_bits(1), f64::EPSILON] {
            assert_eq!(text(value), format!("{value:?}"));
        }
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(4096))]

        #[test]
        fn any_float_is_written_as_the_standard_library_writes_it(bits: u64) {
            let value = f64::from_bits(bits);
            prop_assert_eq!(text(value), format!("{value:?}"));
        }

        #[test]
        fn a_short_decimal_and_what_is_beside_it_are_written_so(
            digits in 1..100_000_u64,
            power in -330..310_i32,
            places in -2..=2_i64,
        ) {
            let value: f64 = format!("{digits}e{power}").parse().unwrap_or(0.0);
            let value = beside(value, places);
            prop_assert_eq!(text(value), format!("{value:?}"));
        }

        #[test]
        fn a_power_of_two_and_what_is_beside_it_are_written_so(
            power in -1074..1024_i32,
            places in -2..=2_i64,
        ) {
            let value = beside(2f64.powi(power), places);
            prop_assert_eq!(text(value), format!("{value:?}"));
        }

        #[test]
        fn a_whole_number_is_written_so(whole: i64) {
            #[allow(clippy::cast_precision_loss)]
            let value = whole as f64;
            prop_assert_eq!(text(value), format!("{value:?}"));
        }
    }
}
