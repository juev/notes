//! Hybrid logical clock. A stamp is a fixed-width string
//! `<12 hex ms>-<4 hex counter>-<device>`, so stamps compare as plain strings.

pub const DEVICE_ID_LEN: usize = 12;

#[derive(Debug, Clone)]
pub struct Clock {
    ms: u64,
    counter: u32,
    device: String,
}

impl Clock {
    pub fn new(device: &str, last_stamp: Option<&str>) -> Self {
        let mut clock = Clock {
            ms: 0,
            counter: 0,
            device: device.to_string(),
        };
        if let Some(stamp) = last_stamp {
            clock.observe(stamp);
        }
        clock
    }

    pub fn device(&self) -> &str {
        &self.device
    }

    /// Next local stamp, strictly greater than every stamp issued or observed so far.
    pub fn tick(&mut self, now_ms: u64) -> String {
        if now_ms > self.ms {
            self.ms = now_ms;
            self.counter = 0;
        } else if self.counter == 0xffff {
            self.ms += 1;
            self.counter = 0;
        } else {
            self.counter += 1;
        }
        format(self.ms, self.counter, &self.device)
    }

    /// Moves the clock past a stamp received from another device.
    pub fn observe(&mut self, stamp: &str) {
        let Some((ms, counter)) = parse(stamp) else {
            return;
        };
        if (ms, counter) > (self.ms, self.counter) {
            self.ms = ms;
            self.counter = counter;
        }
    }
}

fn format(ms: u64, counter: u32, device: &str) -> String {
    format!("{ms:012x}-{counter:04x}-{device}")
}

fn parse(stamp: &str) -> Option<(u64, u32)> {
    let mut parts = stamp.splitn(3, '-');
    let ms = u64::from_str_radix(parts.next()?, 16).ok()?;
    let counter = u32::from_str_radix(parts.next()?, 16).ok()?;
    Some((ms, counter))
}

/// A stamp is accepted from the network only in its canonical form; anything
/// else would break string comparison.
pub fn is_valid(stamp: &str) -> bool {
    let bytes = stamp.as_bytes();
    let hex = |b: &u8| b.is_ascii_digit() || (b'a'..=b'f').contains(b);
    bytes.len() == 12 + 1 + 4 + 1 + DEVICE_ID_LEN
        && bytes[..12].iter().all(hex)
        && bytes[12] == b'-'
        && bytes[13..17].iter().all(hex)
        && bytes[17] == b'-'
        && bytes[18..].iter().all(hex)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ticks_are_strictly_increasing_even_when_time_stands_still() {
        let mut c = Clock::new("aaaaaaaaaaaa", None);
        let a = c.tick(1000);
        let b = c.tick(1000);
        let d = c.tick(999);
        assert!(a < b && b < d);
    }

    #[test]
    fn local_stamp_after_observing_future_stamp_is_greater() {
        let mut far = Clock::new("bbbbbbbbbbbb", None);
        let future = far.tick(9_000_000);
        let mut c = Clock::new("aaaaaaaaaaaa", None);
        c.observe(&future);
        assert!(c.tick(1000) > future);
    }

    #[test]
    fn counter_overflow_keeps_order() {
        let mut c = Clock::new("aaaaaaaaaaaa", None);
        let mut prev = c.tick(5);
        for _ in 0..70_000 {
            let next = c.tick(5);
            assert!(next > prev);
            prev = next;
        }
    }

    #[test]
    fn validity() {
        let mut c = Clock::new("0123456789ab", None);
        assert!(is_valid(&c.tick(1)));
        assert!(!is_valid("zz"));
        assert!(!is_valid("00000000000Z-0000-0123456789ab"));
    }
}
