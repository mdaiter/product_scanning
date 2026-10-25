//! GTIN (UPC/EAN) validation. A barcode is only trusted if its check digit is right.

/// Check digit for the digits *before* the check digit (GS1 mod-10, weights 3,1,3,1,... from the right).
pub fn check_digit(body: &str) -> Option<u8> {
    if body.is_empty() || !body.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let sum: u32 = body
        .bytes()
        .rev()
        .enumerate()
        .map(|(i, b)| (b - b'0') as u32 * if i % 2 == 0 { 3 } else { 1 })
        .sum();
    Some(((10 - sum % 10) % 10) as u8)
}

/// Returns the digits-only GTIN if it is GTIN-8/12/13/14 with a valid check digit.
pub fn normalize(raw: &str) -> Option<String> {
    let digits: String = raw.chars().filter(|c| !c.is_whitespace() && *c != '-').collect();
    if !matches!(digits.len(), 8 | 12 | 13 | 14) || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let (body, check) = digits.split_at(digits.len() - 1);
    (check_digit(body)? == check.as_bytes()[0] - b'0').then_some(digits)
}

pub fn is_valid(raw: &str) -> bool {
    normalize(raw).is_some()
}

/// Canonical key for comparing the same product across UPC-A / EAN-13 / GTIN-14 spellings.
pub fn canonical(raw: &str) -> Option<String> {
    let g = normalize(raw)?;
    Some(format!("{:0>14}", g))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_good_codes() {
        assert_eq!(normalize("4006381333931").as_deref(), Some("4006381333931")); // EAN-13
        assert_eq!(normalize("036000291452").as_deref(), Some("036000291452")); // UPC-A
        assert_eq!(normalize("96385074").as_deref(), Some("96385074")); // EAN-8
    }

    #[test]
    fn rejects_bad_check_digit_length_and_junk() {
        assert!(normalize("4006381333932").is_none());
        assert!(normalize("12345").is_none());
        assert!(normalize("40063813339a1").is_none());
        assert!(normalize("").is_none());
    }

    #[test]
    fn every_generated_code_validates_and_single_digit_errors_fail() {
        for body in ["400638133393", "03600029145", "9638507", "1234567890123"] {
            let cd = check_digit(body).unwrap();
            let full = format!("{body}{cd}");
            if matches!(full.len(), 8 | 12 | 13 | 14) {
                assert!(is_valid(&full), "{full}");
                let mut bytes = full.clone().into_bytes();
                let last = bytes.len() - 1;
                bytes[last] = b'0' + ((bytes[last] - b'0' + 1) % 10);
                assert!(!is_valid(&String::from_utf8(bytes).unwrap()));
            }
        }
    }

    #[test]
    fn upc_a_and_ean13_with_leading_zero_are_the_same_product() {
        assert_eq!(canonical("036000291452"), canonical("0036000291452"));
        assert_ne!(canonical("036000291452"), canonical("4006381333931"));
    }

    #[test]
    fn tolerates_spaces_and_dashes() {
        assert!(is_valid("4006381 333931"));
        assert!(is_valid("4006381-333931"));
    }
}
