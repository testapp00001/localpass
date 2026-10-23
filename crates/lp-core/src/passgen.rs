//! Random password generator (uniform sampling, at least one char per class).

use serde::{Deserialize, Serialize};
use zeroize::Zeroizing;

use crate::crypto::random_below;
use crate::{CoreError, Result};

const LOWER: &str = "abcdefghijklmnopqrstuvwxyz";
const UPPER: &str = "ABCDEFGHIJKLMNOPQRSTUVWXYZ";
const DIGITS: &str = "0123456789";
const SYMBOLS: &str = "!@#$%^&*()-_=+[]{};:,.<>/?~";
const AMBIGUOUS: &str = "Il1O0o|`'\"";

pub const MIN_LENGTH: usize = 4;
pub const MAX_LENGTH: usize = 256;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct PasswordOptions {
    pub length: usize,
    pub lowercase: bool,
    pub uppercase: bool,
    pub digits: bool,
    pub symbols: bool,
    pub avoid_ambiguous: bool,
}

impl Default for PasswordOptions {
    fn default() -> Self {
        Self {
            length: 20,
            lowercase: true,
            uppercase: true,
            digits: true,
            symbols: true,
            avoid_ambiguous: false,
        }
    }
}

impl PasswordOptions {
    fn classes(&self) -> Vec<Vec<char>> {
        let filter = |set: &str| -> Vec<char> {
            set.chars()
                .filter(|c| !self.avoid_ambiguous || !AMBIGUOUS.contains(*c))
                .collect()
        };
        [
            (self.lowercase, LOWER),
            (self.uppercase, UPPER),
            (self.digits, DIGITS),
            (self.symbols, SYMBOLS),
        ]
        .into_iter()
        .filter(|(on, _)| *on)
        .map(|(_, set)| filter(set))
        .collect()
    }

    /// Entropy in bits of a password generated with these options.
    pub fn entropy_bits(&self) -> f64 {
        let alphabet: usize = self.classes().iter().map(Vec::len).sum();
        if alphabet == 0 {
            return 0.0;
        }
        self.length as f64 * (alphabet as f64).log2()
    }
}

pub fn generate_password(opts: &PasswordOptions) -> Result<Zeroizing<String>> {
    let classes = opts.classes();
    if classes.is_empty() {
        return Err(CoreError::Invalid(
            "select at least one character set".into(),
        ));
    }
    if !(MIN_LENGTH.max(classes.len())..=MAX_LENGTH).contains(&opts.length) {
        return Err(CoreError::Invalid(format!(
            "length must be between {} and {MAX_LENGTH}",
            MIN_LENGTH.max(classes.len())
        )));
    }
    let all: Vec<char> = classes.iter().flatten().copied().collect();
    let pick = |set: &[char]| -> Result<char> { Ok(set[random_below(set.len() as u32)? as usize]) };

    let mut chars: Zeroizing<Vec<char>> = Zeroizing::new(Vec::with_capacity(opts.length));
    for class in &classes {
        chars.push(pick(class)?);
    }
    while chars.len() < opts.length {
        chars.push(pick(&all)?);
    }
    // Fisher–Yates so the guaranteed characters aren't always first.
    for i in (1..chars.len()).rev() {
        let j = random_below(i as u32 + 1)? as usize;
        chars.swap(i, j);
    }
    Ok(Zeroizing::new(chars.iter().collect()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn respects_length_and_classes() {
        let opts = PasswordOptions::default();
        for _ in 0..200 {
            let p = generate_password(&opts).unwrap();
            assert_eq!(p.chars().count(), 20);
            assert!(p.chars().any(|c| c.is_ascii_lowercase()));
            assert!(p.chars().any(|c| c.is_ascii_uppercase()));
            assert!(p.chars().any(|c| c.is_ascii_digit()));
            assert!(p.chars().any(|c| SYMBOLS.contains(c)));
        }
    }

    #[test]
    fn avoids_ambiguous_characters() {
        let opts = PasswordOptions {
            avoid_ambiguous: true,
            length: 200,
            ..Default::default()
        };
        let p = generate_password(&opts).unwrap();
        assert!(!p.chars().any(|c| AMBIGUOUS.contains(c)));
    }

    #[test]
    fn rejects_bad_options() {
        let none = PasswordOptions {
            lowercase: false,
            uppercase: false,
            digits: false,
            symbols: false,
            ..Default::default()
        };
        assert!(generate_password(&none).is_err());
        assert!(
            generate_password(&PasswordOptions {
                length: 3,
                ..Default::default()
            })
            .is_err()
        );
        assert!(
            generate_password(&PasswordOptions {
                length: 999,
                ..Default::default()
            })
            .is_err()
        );
    }

    #[test]
    fn digits_only_pin() {
        let opts = PasswordOptions {
            length: 6,
            lowercase: false,
            uppercase: false,
            symbols: false,
            ..Default::default()
        };
        let p = generate_password(&opts).unwrap();
        assert!(p.chars().all(|c| c.is_ascii_digit()));
        assert!((opts.entropy_bits() - 6.0 * 10f64.log2()).abs() < 1e-9);
    }
}
