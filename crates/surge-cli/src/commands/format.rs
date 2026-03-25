/// Format a number with thousands separators (e.g., 1_234_567 → "1,234,567").
pub fn format_number(n: u64) -> String {
    let s = n.to_string();
    let mut result = String::new();
    for (count, c) in s.chars().rev().enumerate() {
        if count > 0 && count % 3 == 0 {
            result.push(',');
        }
        result.push(c);
    }
    result.chars().rev().collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_format_number() {
        assert_eq!(format_number(0), "0");
        assert_eq!(format_number(999), "999");
        assert_eq!(format_number(1_000), "1,000");
        assert_eq!(format_number(1_234_567), "1,234,567");
        assert_eq!(format_number(1_000_000_000), "1,000,000,000");
    }
}
