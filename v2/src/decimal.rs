//! Exact decimal arithmetic for std.decimal — "no floating-point rounding
//! errors" so `0.1 + 0.2 == 0.3`. A decimal is `mantissa * 10^-scale` with a
//! BigInt mantissa, giving arbitrary precision.

use crate::bigint::BigInt;

/// Default number of fractional digits produced by division.
const DIV_PRECISION: u32 = 28;

#[derive(Clone, Debug)]
pub struct Decimal {
    mantissa: BigInt,
    scale: u32,
}

impl Decimal {
    pub fn from_i64(n: i64) -> Self {
        Decimal { mantissa: BigInt::from_i64(n), scale: 0 }
    }

    /// Parse a decimal string like "19.99", "-0.1", "42", "1e3" (no exponent).
    pub fn from_str(s: &str) -> Option<Self> {
        let s = s.trim();
        if s.is_empty() {
            return None;
        }
        // Optional scientific notation: split on 'e'/'E'.
        let (base, exp) = match s.split_once(['e', 'E']) {
            Some((b, e)) => (b, e.parse::<i64>().ok()?),
            None => (s, 0),
        };
        let (int_part, frac_part) = match base.split_once('.') {
            Some((i, f)) => (i, f),
            None => (base, ""),
        };
        let digits = format!("{}{}", int_part, frac_part);
        // Validate: optional sign then digits.
        let check = digits.strip_prefix('-').unwrap_or(&digits);
        let check = check.strip_prefix('+').unwrap_or(check);
        if check.is_empty() || !check.bytes().all(|b| b.is_ascii_digit()) {
            return None;
        }
        let mantissa = BigInt::from_str(&digits)?;
        let mut scale = frac_part.len() as i64;
        scale -= exp; // positive exponent reduces scale
        let mut d = Decimal { mantissa, scale: 0 };
        if scale >= 0 {
            d.scale = scale as u32;
        } else {
            // Negative scale: multiply mantissa by 10^(-scale).
            d.mantissa = d.mantissa.mul(&pow10((-scale) as u32));
            d.scale = 0;
        }
        Some(d)
    }

    pub fn from_f64(f: f64) -> Self {
        // Route through the shortest decimal string representation.
        Decimal::from_str(&format!("{}", f)).unwrap_or_else(|| Decimal::from_i64(0))
    }

    fn is_negative(&self) -> bool {
        self.mantissa.is_negative()
    }

    /// Rescale the mantissa to a target scale (>= current scale).
    fn scaled_mantissa(&self, target: u32) -> BigInt {
        if target > self.scale {
            self.mantissa.mul(&pow10(target - self.scale))
        } else {
            self.mantissa.clone()
        }
    }

    pub fn add(&self, other: &Decimal) -> Decimal {
        let scale = self.scale.max(other.scale);
        Decimal { mantissa: self.scaled_mantissa(scale).add(&other.scaled_mantissa(scale)), scale }
    }

    pub fn sub(&self, other: &Decimal) -> Decimal {
        let scale = self.scale.max(other.scale);
        Decimal { mantissa: self.scaled_mantissa(scale).sub(&other.scaled_mantissa(scale)), scale }
    }

    pub fn mul(&self, other: &Decimal) -> Decimal {
        Decimal { mantissa: self.mantissa.mul(&other.mantissa), scale: self.scale + other.scale }
    }

    /// Division to DIV_PRECISION fractional digits (truncated toward zero).
    pub fn div(&self, other: &Decimal) -> Option<Decimal> {
        if other.mantissa.is_zero() {
            return None;
        }
        // a/b = (ma * 10^(sb+prec)) / (mb * 10^sa), scale = prec
        let prec = DIV_PRECISION;
        let num = self.mantissa.mul(&pow10(other.scale + prec));
        let den = other.mantissa.mul(&pow10(self.scale));
        let (q, _) = num.div_rem(&den)?;
        Some(Decimal { mantissa: q, scale: prec }.trim_zeros())
    }

    pub fn neg(&self) -> Decimal {
        Decimal { mantissa: self.mantissa.neg(), scale: self.scale }
    }

    pub fn abs(&self) -> Decimal {
        if self.is_negative() {
            self.neg()
        } else {
            self.clone()
        }
    }

    pub fn cmp(&self, other: &Decimal) -> std::cmp::Ordering {
        let scale = self.scale.max(other.scale);
        self.scaled_mantissa(scale).cmp(&other.scaled_mantissa(scale))
    }

    /// Round to `places` fractional digits, half-up.
    pub fn round(&self, places: u32) -> Decimal {
        self.round_with_mode(places, "HALF_UP")
    }

    /// Round to `places` fractional digits using the given rounding mode:
    /// HALF_UP, HALF_EVEN (banker's), HALF_DOWN, DOWN/TRUNCATE (toward zero),
    /// UP (away from zero), FLOOR (toward -inf), CEILING (toward +inf).
    pub fn round_with_mode(&self, places: u32, mode: &str) -> Decimal {
        if places >= self.scale {
            return self.clone();
        }
        let drop = self.scale - places;
        let divisor = pow10(drop);
        let (q, r) = self.mantissa.div_rem(&divisor).unwrap();
        if r.is_zero() {
            return Decimal { mantissa: q, scale: places };
        }
        let neg = self.is_negative();
        let step = if neg { BigInt::from_i64(-1) } else { BigInt::from_i64(1) };
        // Compare 2*|r| against the divisor to classify the fractional part.
        let two_r = r.abs().mul(&BigInt::from_i64(2));
        let half = two_r.cmp(&divisor); // Less: <.5, Equal: =.5, Greater: >.5
        let round_away = match mode.to_uppercase().as_str() {
            "DOWN" | "TRUNCATE" => false,
            "UP" => true,
            "FLOOR" => neg, // toward -inf: away from zero only if negative
            "CEILING" | "CEIL" => !neg,
            "HALF_DOWN" => half == std::cmp::Ordering::Greater,
            "HALF_EVEN" | "BANKERS" => match half {
                std::cmp::Ordering::Greater => true,
                std::cmp::Ordering::Less => false,
                std::cmp::Ordering::Equal => {
                    // Round to even: bump only if the kept quotient is odd.
                    let (_, parity) = q.div_rem(&BigInt::from_i64(2)).unwrap();
                    !parity.is_zero()
                }
            },
            // HALF_UP (default)
            _ => half != std::cmp::Ordering::Less,
        };
        let mantissa = if round_away { q.add(&step) } else { q };
        Decimal { mantissa, scale: places }
    }

    /// Drop trailing fractional zeros (keeps value; tidies display).
    fn trim_zeros(&self) -> Decimal {
        let mut m = self.mantissa.clone();
        let mut scale = self.scale;
        let ten = BigInt::from_i64(10);
        while scale > 0 {
            let (q, r) = m.div_rem(&ten).unwrap();
            if !r.is_zero() {
                break;
            }
            m = q;
            scale -= 1;
        }
        Decimal { mantissa: m, scale }
    }

    pub fn to_string(&self) -> String {
        let d = self.trim_zeros();
        if d.scale == 0 {
            return d.mantissa.to_string();
        }
        let neg = d.mantissa.is_negative();
        let digits = d.mantissa.abs().to_string();
        let scale = d.scale as usize;
        let padded = if digits.len() <= scale {
            format!("{}{}", "0".repeat(scale - digits.len() + 1), digits)
        } else {
            digits
        };
        let split = padded.len() - scale;
        let int_part = &padded[..split];
        let frac_part = &padded[split..];
        format!("{}{}.{}", if neg { "-" } else { "" }, int_part, frac_part)
    }

    pub fn to_f64(&self) -> f64 {
        self.to_string().parse().unwrap_or(0.0)
    }

    /// Format with exactly `places` fractional digits (half-up rounded, zero-padded,
    /// no trailing-zero trimming) — for fixed-precision display like currency.
    pub fn to_fixed(&self, places: u32) -> String {
        let r = self.round_with_mode(places, "HALF_UP");
        let neg = r.mantissa.is_negative();
        let mut digits = r.mantissa.abs().to_string();
        let scale = r.scale as usize;
        if digits.len() <= scale {
            digits = format!("{}{}", "0".repeat(scale - digits.len() + 1), digits);
        }
        let split = digits.len() - scale;
        let int_part = digits[..split].to_string();
        let mut frac = digits[split..].to_string();
        let target = places as usize;
        if frac.len() < target {
            frac.push_str(&"0".repeat(target - frac.len()));
        } else if frac.len() > target {
            frac.truncate(target);
        }
        let all_zero = int_part.bytes().all(|b| b == b'0') && frac.bytes().all(|b| b == b'0');
        let sign = if neg && !all_zero { "-" } else { "" };
        if target == 0 {
            format!("{}{}", sign, int_part)
        } else {
            format!("{}{}.{}", sign, int_part, frac)
        }
    }
}

/// 10^n as a BigInt.
fn pow10(n: u32) -> BigInt {
    BigInt::from_i64(10).pow(n as u64)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn d(s: &str) -> Decimal {
        Decimal::from_str(s).unwrap()
    }

    #[test]
    fn test_exact_add() {
        assert_eq!(d("0.1").add(&d("0.2")).to_string(), "0.3");
    }

    #[test]
    fn test_tax_example() {
        // 19.99 * (1 + 0.21) == 24.1879
        let total = d("19.99").mul(&d("1").add(&d("0.21")));
        assert_eq!(total.to_string(), "24.1879");
    }

    #[test]
    fn test_sub_and_neg() {
        assert_eq!(d("0.3").sub(&d("0.1")).to_string(), "0.2");
        assert_eq!(d("1").sub(&d("5")).to_string(), "-4");
    }

    #[test]
    fn test_div() {
        assert_eq!(d("1").div(&d("4")).unwrap().to_string(), "0.25");
        assert_eq!(d("10").div(&d("2")).unwrap().to_string(), "5");
    }

    #[test]
    fn test_cmp_equal_diff_scale() {
        assert_eq!(d("0.30").cmp(&d("0.3")), std::cmp::Ordering::Equal);
        assert_eq!(d("0.1").add(&d("0.2")).cmp(&d("0.3")), std::cmp::Ordering::Equal);
    }

    #[test]
    fn test_round_half_up() {
        assert_eq!(d("2.345").round(2).to_string(), "2.35");
        assert_eq!(d("2.344").round(2).to_string(), "2.34");
        assert_eq!(d("-2.345").round(2).to_string(), "-2.35");
    }

    #[test]
    fn test_big_precision() {
        // 1/3 to default precision, exact digits (truncated)
        let r = d("1").div(&d("3")).unwrap();
        assert!(r.to_string().starts_with("0.3333333333"));
    }
}
