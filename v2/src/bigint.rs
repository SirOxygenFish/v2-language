//! Arbitrary-precision integers for V2's unsized `int` type, which the docs
//! specify as "arbitrary-precision and never overflows". Sign-magnitude with
//! base-1e9 little-endian limbs (each limb 0..1_000_000_000) so decimal I/O is
//! trivial and limb products fit in u64.

const BASE: u64 = 1_000_000_000;
const BASE_DIGITS: usize = 9;

#[derive(Clone, Debug)]
pub struct BigInt {
    /// true if negative. Zero is always non-negative.
    negative: bool,
    /// Little-endian base-1e9 limbs, no trailing zeros (empty == zero).
    limbs: Vec<u32>,
}

impl BigInt {
    pub fn zero() -> Self {
        BigInt { negative: false, limbs: vec![] }
    }

    pub fn from_i64(mut n: i64) -> Self {
        if n == 0 {
            return Self::zero();
        }
        let negative = n < 0;
        // Use i128 to safely negate i64::MIN.
        let mut m = (n as i128).unsigned_abs();
        n = 0;
        let _ = n;
        let mut limbs = Vec::new();
        while m > 0 {
            limbs.push((m % BASE as u128) as u32);
            m /= BASE as u128;
        }
        BigInt { negative, limbs }
    }

    pub fn from_i128(n: i128) -> Self {
        if n == 0 {
            return Self::zero();
        }
        let negative = n < 0;
        let mut m = n.unsigned_abs();
        let mut limbs = Vec::new();
        while m > 0 {
            limbs.push((m % BASE as u128) as u32);
            m /= BASE as u128;
        }
        BigInt { negative, limbs }
    }

    pub fn from_str(s: &str) -> Option<Self> {
        let s = s.trim();
        if s.is_empty() {
            return None;
        }
        let (negative, digits) = match s.strip_prefix('-') {
            Some(rest) => (true, rest),
            None => (false, s.strip_prefix('+').unwrap_or(s)),
        };
        if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
            return None;
        }
        let bytes = digits.as_bytes();
        let mut limbs = Vec::new();
        let mut i = bytes.len();
        while i > 0 {
            let start = i.saturating_sub(BASE_DIGITS);
            let chunk = std::str::from_utf8(&bytes[start..i]).ok()?;
            limbs.push(chunk.parse::<u32>().ok()?);
            i = start;
        }
        let mut b = BigInt { negative, limbs };
        b.normalize();
        Some(b)
    }

    fn normalize(&mut self) {
        while self.limbs.last() == Some(&0) {
            self.limbs.pop();
        }
        if self.limbs.is_empty() {
            self.negative = false;
        }
    }

    pub fn is_zero(&self) -> bool {
        self.limbs.is_empty()
    }

    pub fn is_negative(&self) -> bool {
        self.negative
    }

    /// Return an i64 if the value fits, else None (used to demote results).
    pub fn to_i64(&self) -> Option<i64> {
        let mut acc: i128 = 0;
        for &limb in self.limbs.iter().rev() {
            acc = acc.checked_mul(BASE as i128)?.checked_add(limb as i128)?;
            if acc > (i64::MAX as i128) + 1 {
                return None;
            }
        }
        if self.negative {
            acc = -acc;
        }
        if acc >= i64::MIN as i128 && acc <= i64::MAX as i128 {
            Some(acc as i64)
        } else {
            None
        }
    }

    pub fn to_f64(&self) -> f64 {
        let mut acc = 0.0f64;
        for &limb in self.limbs.iter().rev() {
            acc = acc * BASE as f64 + limb as f64;
        }
        if self.negative {
            -acc
        } else {
            acc
        }
    }

    pub fn to_string(&self) -> String {
        if self.limbs.is_empty() {
            return "0".to_string();
        }
        let mut s = String::new();
        if self.negative {
            s.push('-');
        }
        // Most significant limb without padding, rest zero-padded to 9 digits.
        let mut it = self.limbs.iter().rev();
        if let Some(first) = it.next() {
            s.push_str(&first.to_string());
        }
        for limb in it {
            s.push_str(&format!("{:09}", limb));
        }
        s
    }

    // ── magnitude helpers (ignore sign) ──

    fn cmp_mag(a: &[u32], b: &[u32]) -> std::cmp::Ordering {
        use std::cmp::Ordering;
        if a.len() != b.len() {
            return a.len().cmp(&b.len());
        }
        for i in (0..a.len()).rev() {
            if a[i] != b[i] {
                return a[i].cmp(&b[i]);
            }
        }
        Ordering::Equal
    }

    fn add_mag(a: &[u32], b: &[u32]) -> Vec<u32> {
        let mut out = Vec::with_capacity(a.len().max(b.len()) + 1);
        let mut carry = 0u64;
        for i in 0..a.len().max(b.len()) {
            let x = *a.get(i).unwrap_or(&0) as u64;
            let y = *b.get(i).unwrap_or(&0) as u64;
            let sum = x + y + carry;
            out.push((sum % BASE) as u32);
            carry = sum / BASE;
        }
        if carry > 0 {
            out.push(carry as u32);
        }
        out
    }

    /// a - b, requires a >= b (magnitudes).
    fn sub_mag(a: &[u32], b: &[u32]) -> Vec<u32> {
        let mut out = Vec::with_capacity(a.len());
        let mut borrow = 0i64;
        for i in 0..a.len() {
            let x = a[i] as i64;
            let y = *b.get(i).unwrap_or(&0) as i64;
            let mut diff = x - y - borrow;
            if diff < 0 {
                diff += BASE as i64;
                borrow = 1;
            } else {
                borrow = 0;
            }
            out.push(diff as u32);
        }
        while out.last() == Some(&0) {
            out.pop();
        }
        out
    }

    fn mul_mag(a: &[u32], b: &[u32]) -> Vec<u32> {
        if a.is_empty() || b.is_empty() {
            return vec![];
        }
        let mut out = vec![0u64; a.len() + b.len()];
        for (i, &ai) in a.iter().enumerate() {
            let mut carry = 0u64;
            for (j, &bj) in b.iter().enumerate() {
                let cur = out[i + j] + ai as u64 * bj as u64 + carry;
                out[i + j] = cur % BASE;
                carry = cur / BASE;
            }
            out[i + b.len()] += carry;
        }
        let mut limbs: Vec<u32> = out.iter().map(|&x| x as u32).collect();
        while limbs.last() == Some(&0) {
            limbs.pop();
        }
        limbs
    }

    pub fn add(&self, other: &BigInt) -> BigInt {
        let mut r = if self.negative == other.negative {
            BigInt { negative: self.negative, limbs: Self::add_mag(&self.limbs, &other.limbs) }
        } else {
            match Self::cmp_mag(&self.limbs, &other.limbs) {
                std::cmp::Ordering::Equal => BigInt::zero(),
                std::cmp::Ordering::Greater => {
                    BigInt { negative: self.negative, limbs: Self::sub_mag(&self.limbs, &other.limbs) }
                }
                std::cmp::Ordering::Less => {
                    BigInt { negative: other.negative, limbs: Self::sub_mag(&other.limbs, &self.limbs) }
                }
            }
        };
        r.normalize();
        r
    }

    pub fn neg(&self) -> BigInt {
        if self.is_zero() {
            BigInt::zero()
        } else {
            BigInt { negative: !self.negative, limbs: self.limbs.clone() }
        }
    }

    pub fn abs(&self) -> BigInt {
        BigInt { negative: false, limbs: self.limbs.clone() }
    }

    pub fn sub(&self, other: &BigInt) -> BigInt {
        self.add(&other.neg())
    }

    pub fn mul(&self, other: &BigInt) -> BigInt {
        let mut r = BigInt {
            negative: self.negative != other.negative,
            limbs: Self::mul_mag(&self.limbs, &other.limbs),
        };
        r.normalize();
        r
    }

    pub fn cmp(&self, other: &BigInt) -> std::cmp::Ordering {
        use std::cmp::Ordering;
        match (self.negative, other.negative) {
            (false, true) => Ordering::Greater,
            (true, false) => Ordering::Less,
            (false, false) => Self::cmp_mag(&self.limbs, &other.limbs),
            (true, true) => Self::cmp_mag(&other.limbs, &self.limbs),
        }
    }

    pub fn pow(&self, mut exp: u64) -> BigInt {
        let mut base = self.clone();
        let mut result = BigInt::from_i64(1);
        while exp > 0 {
            if exp & 1 == 1 {
                result = result.mul(&base);
            }
            exp >>= 1;
            if exp > 0 {
                base = base.mul(&base);
            }
        }
        result
    }

    /// Truncating division returning (quotient, remainder) with the remainder
    /// taking the sign of the dividend. Returns None on divide-by-zero.
    pub fn div_rem(&self, other: &BigInt) -> Option<(BigInt, BigInt)> {
        if other.is_zero() {
            return None;
        }
        // Long division on magnitudes, processing the dividend most-significant
        // limb first and binary-searching each base-1e9 quotient digit.
        let divisor_mag = &other.limbs;
        let mut quotient = vec![0u32; self.limbs.len()];
        let mut rem: Vec<u32> = Vec::new(); // magnitude, little-endian
        for i in (0..self.limbs.len()).rev() {
            // rem = rem * BASE + limb[i]
            rem.insert(0, self.limbs[i]);
            while rem.last() == Some(&0) {
                rem.pop();
            }
            // Binary search largest d in [0, BASE) with divisor*d <= rem.
            let (mut lo, mut hi) = (0u64, BASE - 1);
            let mut best = 0u64;
            while lo <= hi {
                let mid = (lo + hi) / 2;
                let prod = Self::mul_mag(divisor_mag, &[mid as u32]);
                if Self::cmp_mag(&prod, &rem) != std::cmp::Ordering::Greater {
                    best = mid;
                    if mid == BASE - 1 {
                        break;
                    }
                    lo = mid + 1;
                } else {
                    if mid == 0 {
                        break;
                    }
                    hi = mid - 1;
                }
            }
            quotient[i] = best as u32;
            if best > 0 {
                let prod = Self::mul_mag(divisor_mag, &[best as u32]);
                rem = Self::sub_mag(&rem, &prod);
            }
        }
        while quotient.last() == Some(&0) {
            quotient.pop();
        }
        let mut q = BigInt { negative: self.negative != other.negative, limbs: quotient };
        let mut r = BigInt { negative: self.negative, limbs: rem };
        q.normalize();
        r.normalize();
        Some((q, r))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_pow_2_100() {
        let two = BigInt::from_i64(2);
        assert_eq!(two.pow(100).to_string(), "1267650600228229401496703205376");
    }

    #[test]
    fn test_factorial_30() {
        let mut f = BigInt::from_i64(1);
        for i in 2..=30i64 {
            f = f.mul(&BigInt::from_i64(i));
        }
        assert_eq!(f.to_string(), "265252859812191058636308480000000");
    }

    #[test]
    fn test_add_across_i64_boundary() {
        let max = BigInt::from_i64(i64::MAX);
        let one = BigInt::from_i64(1);
        assert_eq!(max.add(&one).to_string(), "9223372036854775808");
    }

    #[test]
    fn test_sub_sign() {
        let a = BigInt::from_i64(5);
        let b = BigInt::from_i64(8);
        assert_eq!(a.sub(&b).to_string(), "-3");
    }

    #[test]
    fn test_div_rem_big() {
        let a = BigInt::from_i64(2).pow(100);
        let b = BigInt::from_i64(7);
        let (q, r) = a.div_rem(&b).unwrap();
        // Verify q*7 + r == a
        assert_eq!(q.mul(&b).add(&r).to_string(), a.to_string());
        assert!(r.cmp(&b) == std::cmp::Ordering::Less);
    }

    #[test]
    fn test_roundtrip_str() {
        let s = "123456789012345678901234567890";
        assert_eq!(BigInt::from_str(s).unwrap().to_string(), s);
        assert_eq!(BigInt::from_str("-42").unwrap().to_string(), "-42");
    }

    #[test]
    fn test_to_i64_demote() {
        assert_eq!(BigInt::from_i64(12345).to_i64(), Some(12345));
        assert_eq!(BigInt::from_i64(2).pow(100).to_i64(), None);
    }
}
