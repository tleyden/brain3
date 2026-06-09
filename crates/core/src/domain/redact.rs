pub fn elide_secret(s: &str) -> String {
    let len = s.len();
    if len < 6 {
        return format!("***({len} chars)");
    }
    let show = (len + 9) / 10; // ceil(10%)
    let show = show.max(1);
    format!(
        "{}***{}({len} chars)",
        &s[..show],
        &s[len - show..]
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn short_secret() {
        assert_eq!(elide_secret("abc"), "***(3 chars)");
    }

    #[test]
    fn medium_secret() {
        assert_eq!(elide_secret("0123456789"), "0***9(10 chars)");
    }

    #[test]
    fn long_hex_secret() {
        let s = "abcdef0123456789abcdef0123456789abcdef0123456789abcdef0123456789";
        let e = elide_secret(s);
        assert!(e.starts_with("abcdef0"));
        assert!(e.contains("***"));
        assert!(e.contains("3456789(64 chars)"));
    }
}
