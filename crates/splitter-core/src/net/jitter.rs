use crate::net::packet::Packet;
use crate::settings::JitterMode;
use std::collections::{BTreeMap, VecDeque};
use std::time::Instant;

pub const PACKET_INTERVAL_MS: u32 = 20;
pub const MAX_DEPTH_MS_HARD_CAP: u32 = 200;

// queue/arrival_order only shrink when pop_ready runs; a stalled consumer would
// otherwise grow them unbounded. At 20ms/packet and a 200ms hard cap an in-spec buffer
// holds ~10 packets, so 512 is far above any legitimate depth while still bounding memory.
pub const MAX_QUEUED_PACKETS: usize = 512;

#[derive(Debug, Clone)]
pub enum JitterOutput {
    Packet(Packet),
    Lost { seq: u32 },
}

#[derive(Debug)]
pub struct JitterBuffer {
    mode: JitterMode,
    max_depth_ms: u32,
    target_depth: usize,
    next_expected_seq: Option<u32>,
    queue: BTreeMap<u32, (Packet, Instant)>,
    arrival_order: VecDeque<u32>,
    arrival_intervals_ms: VecDeque<u32>,
    last_arrival: Option<Instant>,
    pops_since_resize: u32,
}

impl JitterBuffer {
    pub fn new(mode: JitterMode, max_depth_ms: u32) -> Self {
        let max_depth_ms = max_depth_ms.min(MAX_DEPTH_MS_HARD_CAP);
        let initial_target = match mode {
            JitterMode::Min => 1,
            JitterMode::Fixed(ms) => (ms / PACKET_INTERVAL_MS).max(1) as usize,
            JitterMode::Auto => 2,
        };
        Self {
            mode,
            max_depth_ms,
            target_depth: initial_target,
            next_expected_seq: None,
            queue: BTreeMap::new(),
            arrival_order: VecDeque::new(),
            arrival_intervals_ms: VecDeque::with_capacity(256),
            last_arrival: None,
            pops_since_resize: 0,
        }
    }

    pub fn target_depth_packets(&self) -> usize {
        self.target_depth
    }

    pub fn p99_jitter_ms(&self) -> u32 {
        if self.arrival_intervals_ms.len() < 10 {
            return 0;
        }
        let mut deltas: Vec<u32> = self
            .arrival_intervals_ms
            .iter()
            .map(|iv| iv.abs_diff(PACKET_INTERVAL_MS))
            .collect();
        deltas.sort_unstable();
        let idx = ((deltas.len() as f32) * 0.99) as usize;
        deltas[idx.min(deltas.len() - 1)]
    }

    pub fn push(&mut self, packet: Packet, arrival: Instant) {
        if let Some(prev) = self.last_arrival {
            let delta = arrival.duration_since(prev).as_millis() as u32;
            if self.arrival_intervals_ms.len() >= 256 {
                self.arrival_intervals_ms.pop_front();
            }
            self.arrival_intervals_ms.push_back(delta);
        }
        self.last_arrival = Some(arrival);
        let seq = packet.seq;
        let is_new = !self.queue.contains_key(&seq);
        self.queue.insert(seq, (packet, arrival));
        if is_new {
            self.arrival_order.push_back(seq);
        }
        // Initialise next_expected_seq to the smallest seq we have buffered so
        // that out-of-order early arrivals don't skew the starting point.
        match self.next_expected_seq {
            None => self.next_expected_seq = Some(seq),
            Some(cur) if seq < cur => self.next_expected_seq = Some(seq),
            _ => {}
        }
        while self.arrival_order.len() > MAX_QUEUED_PACKETS {
            if let Some(dropped) = self.arrival_order.pop_front() {
                self.queue.remove(&dropped);
            } else {
                break;
            }
        }
    }

    pub fn pop_ready(&mut self, now: Instant) -> Option<JitterOutput> {
        let want = self.next_expected_seq?;
        if self.queue.contains_key(&want) {
            let (pkt, _) = self.queue.remove(&want)?;
            // arrival_order is arrival-ordered, not seq-ordered: the popped seq is the
            // front only in the in-order steady state. Fast-path that; fall back to a
            // scan when a reordered arrival put `want` elsewhere.
            if self.arrival_order.front() == Some(&want) {
                self.arrival_order.pop_front();
            } else {
                self.arrival_order.retain(|&s| s != want);
            }
            self.next_expected_seq = Some(want.wrapping_add(1));
            self.bump_pops();
            return Some(JitterOutput::Packet(pkt));
        }
        if self.queue.is_empty() {
            return None;
        }
        let oldest_seq = *self.arrival_order.front()?;
        let oldest_arrival = self.queue.get(&oldest_seq).map(|(_, t)| *t)?;
        let age_ms = now.duration_since(oldest_arrival).as_millis() as u32;
        if age_ms >= self.max_depth_ms {
            let lost = JitterOutput::Lost { seq: want };
            self.next_expected_seq = Some(want.wrapping_add(1));
            self.bump_pops();
            return Some(lost);
        }
        None
    }

    fn bump_pops(&mut self) {
        self.pops_since_resize = self.pops_since_resize.saturating_add(1);
        if self.pops_since_resize >= 100 {
            self.pops_since_resize = 0;
            self.recompute_target();
        }
    }

    fn recompute_target(&mut self) {
        let new_target = match self.mode {
            JitterMode::Min => 1,
            JitterMode::Fixed(ms) => (ms / PACKET_INTERVAL_MS).max(1) as usize,
            JitterMode::Auto => {
                let p99 = self.p99_jitter_ms().max(PACKET_INTERVAL_MS);
                let depth_ms = p99.min(self.max_depth_ms);
                (depth_ms / PACKET_INTERVAL_MS).max(1) as usize
            }
        };
        if new_target != self.target_depth {
            tracing::debug!(
                old = self.target_depth,
                new = new_target,
                "jitter target depth changed"
            );
            self.target_depth = new_target;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::net::packet::Packet;
    use bytes::Bytes;

    fn pkt(seq: u32) -> Packet {
        Packet {
            stream_id: 0,
            seq,
            timestamp_ms: 0,
            payload: Bytes::from_static(b"x"),
        }
    }

    #[test]
    fn pops_in_seq_order_when_ordered() {
        let mut jb = JitterBuffer::new(JitterMode::Min, 100);
        let t = Instant::now();
        jb.push(pkt(0), t);
        jb.push(pkt(1), t);
        jb.push(pkt(2), t);
        for i in 0..3 {
            match jb.pop_ready(t).unwrap() {
                JitterOutput::Packet(p) => assert_eq!(p.seq, i),
                JitterOutput::Lost { .. } => panic!(),
            }
        }
        assert!(jb.pop_ready(t).is_none());
    }

    #[test]
    fn reorders_out_of_order_arrival() {
        let mut jb = JitterBuffer::new(JitterMode::Min, 100);
        let t = Instant::now();
        jb.push(pkt(2), t);
        jb.push(pkt(0), t);
        jb.push(pkt(1), t);
        for i in 0..3 {
            match jb.pop_ready(t).unwrap() {
                JitterOutput::Packet(p) => assert_eq!(p.seq, i),
                _ => panic!(),
            }
        }
    }

    #[test]
    fn missing_packet_marked_lost_after_max_depth() {
        use std::time::Duration;
        let mut jb = JitterBuffer::new(JitterMode::Min, 100);
        let t0 = Instant::now();
        jb.push(pkt(0), t0);
        jb.push(pkt(2), t0);
        let _ = jb.pop_ready(t0).unwrap();
        let later = t0 + Duration::from_millis(150);
        match jb.pop_ready(later).unwrap() {
            JitterOutput::Lost { seq } => assert_eq!(seq, 1),
            _ => panic!("expected Lost"),
        }
        match jb.pop_ready(later).unwrap() {
            JitterOutput::Packet(p) => assert_eq!(p.seq, 2),
            _ => panic!(),
        }
    }

    #[test]
    fn missing_packet_waits_until_max_depth_reached() {
        use std::time::Duration;
        let mut jb = JitterBuffer::new(JitterMode::Min, 100);
        let t0 = Instant::now();
        jb.push(pkt(0), t0);
        jb.push(pkt(2), t0);
        let _ = jb.pop_ready(t0).unwrap();
        assert!(jb.pop_ready(t0 + Duration::from_millis(30)).is_none());
    }

    #[test]
    fn auto_mode_grows_target_with_p99_jitter() {
        let mut jb = JitterBuffer::new(JitterMode::Auto, 200);
        let t0 = Instant::now();
        for i in 0..150 {
            let arrival =
                t0 + std::time::Duration::from_millis(if i % 3 == 0 { 60 } else { 20 } * i as u64);
            jb.push(pkt(i as u32), arrival);
            let _ = jb.pop_ready(arrival);
        }
        assert!(jb.target_depth_packets() >= 2);
    }

    #[test]
    fn fixed_mode_target_equals_configured_ms() {
        let jb = JitterBuffer::new(JitterMode::Fixed(60), 200);
        assert_eq!(jb.target_depth_packets(), 3);
    }

    #[test]
    fn max_depth_clamped_to_hard_cap() {
        let jb = JitterBuffer::new(JitterMode::Auto, 10_000);
        assert!(jb.max_depth_ms <= MAX_DEPTH_MS_HARD_CAP);
    }

    #[test]
    fn age_gating_uses_oldest_arrival_not_lowest_seq() {
        use std::time::Duration;

        // Scenario: packets arrive out-of-order relative to their seq numbers.
        // seq=10 arrives first (t0), seq=7 arrives next (t0+20ms), seq=4 arrives last (t0+40ms).
        // After popping seq=4, the buffer holds seq=7 and seq=10 with seq=5 missing.
        // The OLDEST ARRIVAL is seq=10 (arrived at t0).
        // The LOWEST SEQ (buggy path) is seq=7 (arrived at t0+20ms).
        //
        // At now=t0+60ms with max_depth_ms=50ms:
        //   Correct (arrival order): age = 60ms >= 50ms → emit Lost{seq=5}
        //   Buggy  (seq order):      age = 40ms <  50ms → return None  (wrong)
        let mut jb = JitterBuffer::new(JitterMode::Min, 50);
        let t0 = Instant::now();

        jb.push(pkt(10), t0);
        jb.push(pkt(7), t0 + Duration::from_millis(20));
        jb.push(pkt(4), t0 + Duration::from_millis(40));

        let popped = jb.pop_ready(t0 + Duration::from_millis(40)).unwrap();
        match popped {
            JitterOutput::Packet(p) => assert_eq!(p.seq, 4),
            _ => panic!("expected Packet(seq=4)"),
        }

        let now = t0 + Duration::from_millis(60);
        match jb.pop_ready(now) {
            Some(JitterOutput::Lost { seq }) => assert_eq!(seq, 5),
            Some(JitterOutput::Packet(p)) => {
                panic!("expected Lost{{seq=5}}, got Packet(seq={})", p.seq)
            }
            None => {
                panic!("expected Lost{{seq=5}} — oldest arrival is seq=10 at t0, age=60ms >= 50ms")
            }
        }
    }

    #[test]
    fn in_order_pop_removes_only_matching_head() {
        let mut jb = JitterBuffer::new(JitterMode::Min, 100);
        let t = Instant::now();
        jb.push(pkt(0), t);
        jb.push(pkt(1), t);
        jb.push(pkt(2), t);

        match jb.pop_ready(t).unwrap() {
            JitterOutput::Packet(p) => assert_eq!(p.seq, 0),
            _ => panic!("expected Packet(seq=0)"),
        }
        match jb.pop_ready(t).unwrap() {
            JitterOutput::Packet(p) => assert_eq!(p.seq, 1),
            _ => panic!("expected Packet(seq=1)"),
        }
        match jb.pop_ready(t).unwrap() {
            JitterOutput::Packet(p) => assert_eq!(p.seq, 2),
            _ => panic!("expected Packet(seq=2)"),
        }
    }

    #[test]
    fn out_of_order_pop_preserves_arrival_order_for_age_gating() {
        use std::time::Duration;
        let mut jb = JitterBuffer::new(JitterMode::Min, 50);
        let t0 = Instant::now();

        jb.push(pkt(10), t0);
        jb.push(pkt(7), t0 + Duration::from_millis(20));
        jb.push(pkt(4), t0 + Duration::from_millis(40));

        match jb.pop_ready(t0 + Duration::from_millis(40)).unwrap() {
            JitterOutput::Packet(p) => assert_eq!(p.seq, 4),
            _ => panic!("expected Packet(seq=4)"),
        }
        assert_eq!(
            jb.arrival_order.front(),
            Some(&10),
            "fallback pop must leave the oldest arrival at the front"
        );

        match jb.pop_ready(t0 + Duration::from_millis(60)) {
            Some(JitterOutput::Lost { seq }) => assert_eq!(seq, 5),
            other => panic!("expected Lost{{seq=5}}, got {other:?}"),
        }
    }

    #[test]
    fn push_beyond_cap_drops_oldest() {
        let mut jb = JitterBuffer::new(JitterMode::Min, 100);
        let t = Instant::now();
        for seq in 0..(MAX_QUEUED_PACKETS as u32 + 5) {
            jb.push(pkt(seq), t);
        }
        assert_eq!(jb.arrival_order.len(), MAX_QUEUED_PACKETS);
        for seq in 0..5u32 {
            assert!(
                !jb.queue.contains_key(&seq),
                "oldest-arrived seq {seq} must be evicted from queue"
            );
        }
    }

    #[test]
    fn cap_does_not_break_in_order_playback_within_cap() {
        let mut jb = JitterBuffer::new(JitterMode::Min, 100);
        let t = Instant::now();
        for seq in 0..(MAX_QUEUED_PACKETS as u32) {
            jb.push(pkt(seq), t);
        }
        for expected in 0..(MAX_QUEUED_PACKETS as u32) {
            match jb.pop_ready(t).unwrap() {
                JitterOutput::Packet(p) => assert_eq!(p.seq, expected),
                _ => panic!("expected Packet(seq={expected}) — nothing should drop at/under cap"),
            }
        }
    }
}
