//! Fractional ordering keys: between any two keys there is always a third.
//! Keys are ASCII strings over `0-9A-Za-z` and never end in `0`.

const DIGITS: &[u8] = b"0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz";
const BASE: usize = DIGITS.len();

fn index(b: u8) -> usize {
    DIGITS.iter().position(|d| *d == b).unwrap_or(0)
}

/// Returns a key strictly between `lo` and `hi`. `None` means "no bound".
/// If the bounds are equal or reversed, the result sorts after `lo`.
pub fn between(lo: Option<&str>, hi: Option<&str>) -> String {
    let lo = lo.unwrap_or("").as_bytes();
    let hi = match hi {
        Some(h) if h.as_bytes() > lo => h.as_bytes(),
        _ => b"",
    };
    let mut out = Vec::with_capacity(lo.len() + 1);
    let mut hi_open = hi.is_empty();
    let mut i = 0;
    loop {
        let a = lo.get(i).map_or(0, |b| index(*b));
        let b = if hi_open {
            BASE
        } else {
            hi.get(i).map_or(0, |b| index(*b))
        };
        if b > a + 1 {
            out.push(DIGITS[(a + b) / 2]);
            break;
        }
        out.push(DIGITS[a]);
        if b == a + 1 {
            hi_open = true;
        }
        i += 1;
    }
    String::from_utf8(out).expect("ascii")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn first_key_and_appends_grow_slowly() {
        let mut key = between(None, None);
        for _ in 0..1000 {
            let next = between(Some(&key), None);
            assert!(next > key);
            key = next;
        }
        assert!(key.len() < 400);
    }

    #[test]
    fn prepends_stay_ordered() {
        let mut key = between(None, None);
        for _ in 0..1000 {
            let next = between(None, Some(&key));
            assert!(next < key, "{next} < {key}");
            assert!(!next.ends_with('0'));
            key = next;
        }
    }

    #[test]
    fn repeated_insertion_between_neighbours() {
        let lo = between(None, None);
        let mut hi = between(Some(&lo), None);
        for _ in 0..500 {
            let mid = between(Some(&lo), Some(&hi));
            assert!(lo < mid && mid < hi, "{lo} < {mid} < {hi}");
            hi = mid;
        }
    }

    #[test]
    fn equal_bounds_do_not_loop() {
        let k = between(Some("V"), Some("V"));
        assert!(k.as_str() > "V");
    }
}
