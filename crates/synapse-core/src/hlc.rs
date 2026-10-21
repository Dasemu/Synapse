//! Hybrid logical clock. Encoded as a lexicographically sortable string
//! `{millis:013}-{counter:05}-{actor}` so ordering is plain string comparison.

use std::sync::Mutex;

#[derive(Debug)]
pub struct HlcClock {
    actor: String,
    state: Mutex<(i64, u32)>,
}

impl HlcClock {
    pub fn new(actor: impl Into<String>) -> Self {
        Self {
            actor: actor.into(),
            state: Mutex::new((0, 0)),
        }
    }

    pub fn actor(&self) -> &str {
        &self.actor
    }

    /// Next timestamp for a local event at wall-clock `now_ms`.
    pub fn tick(&self, now_ms: i64) -> String {
        let mut s = self.state.lock().unwrap();
        if now_ms > s.0 {
            *s = (now_ms, 0);
        } else {
            s.1 += 1;
        }
        encode(s.0, s.1, &self.actor)
    }

    /// Merge a remote timestamp so later local ticks sort after it.
    pub fn observe(&self, remote: &str) {
        if let Some((ms, c)) = decode(remote) {
            let mut s = self.state.lock().unwrap();
            if (ms, c) > *s {
                *s = (ms, c);
            }
        }
    }
}

pub fn encode(ms: i64, counter: u32, actor: &str) -> String {
    format!("{ms:013}-{counter:05}-{actor}")
}

pub fn decode(h: &str) -> Option<(i64, u32)> {
    let mut it = h.splitn(3, '-');
    Some((it.next()?.parse().ok()?, it.next()?.parse().ok()?))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn monotonic_and_sortable() {
        let c = HlcClock::new("a");
        let t1 = c.tick(1000);
        let t2 = c.tick(1000);
        let t3 = c.tick(999);
        assert!(t1 < t2 && t2 < t3);
        c.observe(&encode(5000, 7, "b"));
        assert!(c.tick(10) > encode(5000, 7, "b"));
    }
}
