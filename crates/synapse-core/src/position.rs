//! Fractional indexing for sibling order: keys are strings over `[0-9a-z]` with no trailing
//! `0`, so a key strictly between any two keys always exists and moves touch one row.

const DIGITS: &[u8] = b"0123456789abcdefghijklmnopqrstuvwxyz";
const BASE: usize = DIGITS.len();

fn digit(c: u8) -> usize {
    DIGITS
        .iter()
        .position(|&d| d == c)
        .expect("invalid position digit")
}

/// Returns a key strictly between `lo` and `hi` (`None` = open bound).
pub fn between(lo: Option<&str>, hi: Option<&str>) -> String {
    let lo = lo.unwrap_or("");
    if let Some(h) = hi {
        assert!(lo < h, "position bounds out of order: {lo:?} >= {h:?}");
    }
    midpoint(lo.as_bytes(), hi.map(str::as_bytes))
}

fn midpoint(a: &[u8], b: Option<&[u8]>) -> String {
    if let Some(b) = b {
        let mut n = 0;
        while n < b.len() && a.get(n).copied().unwrap_or(b'0') == b[n] {
            n += 1;
        }
        if n > 0 {
            let rest = midpoint(a.get(n..).unwrap_or(&[]), Some(&b[n..]));
            return String::from_utf8(b[..n].to_vec()).unwrap() + &rest;
        }
    }
    let da = a.first().map(|&c| digit(c)).unwrap_or(0);
    let db = b.and_then(|b| b.first()).map(|&c| digit(c)).unwrap_or(BASE);
    if db - da > 1 {
        return (DIGITS[(da + db) / 2] as char).to_string();
    }
    match b {
        Some(b) if b.len() > 1 => (b[0] as char).to_string(),
        _ => (DIGITS[da] as char).to_string() + &midpoint(a.get(1..).unwrap_or(&[]), None),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ordering_holds_under_repeated_inserts() {
        let mut keys = vec![between(None, None)];
        for i in 0..200 {
            let k = match i % 3 {
                0 => between(None, Some(&keys[0])),
                1 => between(Some(keys.last().unwrap()), None),
                _ => between(Some(&keys[0]), Some(&keys[1])),
            };
            keys.push(k);
            keys.sort();
            keys.dedup();
            assert_eq!(keys.len(), i + 2);
        }
        assert!(keys.iter().all(|k| !k.ends_with('0')));
    }
}
