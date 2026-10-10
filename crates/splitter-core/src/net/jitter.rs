use crate::net::packet::Packet;
use crate::settings::JitterMode;
use std::collections::{BTreeMap, VecDeque};
use std::time::{Duration, Instant};

pub const PACKET_INTERVAL_MS: u32 = 20;
pub const MAX_DEPTH_MS_HARD_CAP: u32 = 200;

// queue/arrival_order only shrink when pop_ready runs; a stalled consumer would
// otherwise grow them unbounded. At 20ms/packet and a 200ms hard cap an in-spec buffer
// holds ~10 packets, so 512 is far above any legitimate depth while still bounding memory.
pub const MAX_QUEUED_PACKETS: usize = 512;

const SEQ_MASK: u32 = 0x00FF_FFFF;
const SEQ_HALF_RANGE: u32 = 0x0080_0000;

// Wider gaps are a resync (sender restart, long outage), not loss to conceal slot by slot.
const MAX_CONCEALED_GAP: u32 = 5;

// A sender clocked faster than our device builds a backlog; once it stays above target +
// slack for a second of slots, shed a frame so latency doesn't creep up all session.
const DRIFT_SLACK_PACKETS: usize = 2;
const DRIFT_TRIM_AFTER_SLOTS: u32 = 50;

fn seq_distance(from: u32, to: u32) -> u32 {
    to.wrapping_sub(from) & SEQ_MASK
}

fn seq_is_behind(cursor: u32, seq: u32) -> bool {
    let d = seq_distance(seq, cursor);
    d != 0 && d < SEQ_HALF_RANGE
}

#[derive(Debug, Clone)]
pub enum JitterOutput {
    Packet(Packet),
    Lost { seq: u32 },
}

/// Call `pop_ready` whenever the playback device has room for a frame. Playout starts once
/// `target_depth` packets are buffered (or the oldest waited that long); slot N is then due
/// at start + N * 20ms, and a missing packet is only reported `Lost` once its slot is due.
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
    primed: bool,
    started: bool,
    playout_origin: Option<Instant>,
    slots_played: u32,
    backlog_slots: u32,
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
            primed: false,
            started: false,
            playout_origin: None,
            slots_played: 0,
            backlog_slots: 0,
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
        let seq = packet.seq & SEQ_MASK;
        match self.next_expected_seq {
            None => self.next_expected_seq = Some(seq),
            Some(cur) if seq_is_behind(cur, seq) => {
                if self.started {
                    // Its slot already played; queueing it would rewind playout.
                    return;
                }
                self.next_expected_seq = Some(seq);
            }
            _ => {}
        }
        let is_new = !self.queue.contains_key(&seq);
        self.queue.insert(seq, (packet, arrival));
        if is_new {
            self.arrival_order.push_back(seq);
        }
        while self.arrival_order.len() > MAX_QUEUED_PACKETS {
            if let Some(dropped) = self.arrival_order.pop_front() {
                self.queue.remove(&dropped);
            } else {
                break;
            }
        }
    }

    /// `None`: still prebuffering, the missing packet's slot isn't due yet, or the buffer
    /// ran dry (it then refills to target depth).
    pub fn pop_ready(&mut self, now: Instant) -> Option<JitterOutput> {
        let mut want = self.next_expected_seq?;
        if !self.primed {
            if !self.prebuffered(now) {
                return None;
            }
            self.primed = true;
            self.started = true;
            self.playout_origin = Some(now);
            self.slots_played = 0;
            // Packets lost while we were silent are gone; resume from what we have.
            if !self.queue.contains_key(&want) {
                if let Some(nearest) = self.nearest_queued(want) {
                    want = nearest;
                    self.next_expected_seq = Some(want);
                }
            }
        }
        if let Some((pkt, _)) = self.queue.remove(&want) {
            // arrival_order is arrival-ordered, not seq-ordered: the popped seq is the
            // front only in the in-order steady state. Fast-path that; fall back to a
            // scan when a reordered arrival put `want` elsewhere.
            if self.arrival_order.front() == Some(&want) {
                self.arrival_order.pop_front();
            } else {
                self.arrival_order.retain(|&s| s != want);
            }
            let mut next = want.wrapping_add(1) & SEQ_MASK;
            if self.queue.len() > self.target_depth + DRIFT_SLACK_PACKETS {
                self.backlog_slots += 1;
                if self.backlog_slots >= DRIFT_TRIM_AFTER_SLOTS {
                    self.backlog_slots = 0;
                    if self.queue.remove(&next).is_some() {
                        self.arrival_order.retain(|&s| s != next);
                        next = next.wrapping_add(1) & SEQ_MASK;
                    }
                }
            } else {
                self.backlog_slots = 0;
            }
            self.advance_slot(next);
            return Some(JitterOutput::Packet(pkt));
        }
        if !self.slot_due(now) {
            return None;
        }
        let Some(nearest) = self.nearest_queued(want) else {
            self.primed = false;
            return None;
        };
        let next = if seq_distance(want, nearest) > MAX_CONCEALED_GAP {
            nearest
        } else {
            want.wrapping_add(1) & SEQ_MASK
        };
        self.advance_slot(next);
        Some(JitterOutput::Lost { seq: want })
    }

    fn advance_slot(&mut self, next: u32) {
        self.next_expected_seq = Some(next);
        self.slots_played = self.slots_played.saturating_add(1);
        self.bump_pops();
    }

    fn slot_due(&self, now: Instant) -> bool {
        match self.playout_origin {
            Some(origin) => {
                let offset = u64::from(self.slots_played) * u64::from(PACKET_INTERVAL_MS);
                now >= origin + Duration::from_millis(offset)
            }
            None => true,
        }
    }

    /// After a `Lost`, the successor whose in-band FEC can rebuild the lost frame.
    pub fn peek_next(&self) -> Option<&Packet> {
        self.queue.get(&self.next_expected_seq?).map(|(p, _)| p)
    }

    fn prebuffered(&self, now: Instant) -> bool {
        if self.queue.len() >= self.target_depth {
            return true;
        }
        let Some(arrived) = self
            .arrival_order
            .front()
            .and_then(|s| self.queue.get(s))
            .map(|(_, t)| *t)
        else {
            return false;
        };
        now.duration_since(arrived).as_millis() as u32
            >= self.target_depth as u32 * PACKET_INTERVAL_MS
    }

    fn nearest_queued(&self, from: u32) -> Option<u32> {
        self.queue
            .keys()
            .copied()
            .min_by_key(|&s| seq_distance(from, s))
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
    fn missing_packet_is_concealed_at_its_slot_without_waiting_for_max_depth() {
        use std::time::Duration;
        let mut jb = JitterBuffer::new(JitterMode::Auto, 200);
        let t0 = Instant::now();
        jb.push(pkt(0), t0);
        jb.push(pkt(2), t0 + Duration::from_millis(40));
        let start = t0 + Duration::from_millis(40);
        assert!(matches!(jb.pop_ready(start), Some(JitterOutput::Packet(p)) if p.seq == 0));
        let slot1_due = start + Duration::from_millis(20);
        match jb.pop_ready(slot1_due) {
            Some(JitterOutput::Lost { seq }) => assert_eq!(seq, 1),
            other => panic!("seq 1 must be concealed at its playout slot, got {other:?}"),
        }
        assert!(matches!(jb.pop_ready(slot1_due), Some(JitterOutput::Packet(p)) if p.seq == 2));
    }

    #[test]
    fn missing_packet_waits_until_its_slot_is_due() {
        use std::time::Duration;
        let mut jb = JitterBuffer::new(JitterMode::Min, 200);
        let t = Instant::now();
        jb.push(pkt(0), t);
        jb.push(pkt(2), t);
        assert!(matches!(jb.pop_ready(t), Some(JitterOutput::Packet(p)) if p.seq == 0));
        assert!(
            jb.pop_ready(t).is_none(),
            "slot 1 plays 20ms after slot 0; filling the ring early must not conceal it"
        );
        jb.push(pkt(1), t + Duration::from_millis(5));
        assert!(matches!(
            jb.pop_ready(t + Duration::from_millis(5)),
            Some(JitterOutput::Packet(p)) if p.seq == 1
        ));
    }

    #[test]
    fn holds_packets_until_target_depth_is_buffered() {
        let mut jb = JitterBuffer::new(JitterMode::Fixed(60), 200);
        let t0 = Instant::now();
        jb.push(pkt(0), t0);
        jb.push(pkt(1), t0);
        assert!(
            jb.pop_ready(t0).is_none(),
            "must prebuffer 3 packets before starting playout"
        );
        jb.push(pkt(2), t0);
        assert!(matches!(jb.pop_ready(t0), Some(JitterOutput::Packet(p)) if p.seq == 0));
    }

    #[test]
    fn lone_packet_starts_playout_after_target_delay() {
        use std::time::Duration;
        let mut jb = JitterBuffer::new(JitterMode::Fixed(60), 200);
        let t0 = Instant::now();
        jb.push(pkt(0), t0);
        assert!(jb.pop_ready(t0 + Duration::from_millis(10)).is_none());
        assert!(matches!(
            jb.pop_ready(t0 + Duration::from_millis(60)),
            Some(JitterOutput::Packet(p)) if p.seq == 0
        ));
    }

    #[test]
    fn late_packet_is_dropped_instead_of_rewinding_playout() {
        use std::time::Duration;
        let mut jb = JitterBuffer::new(JitterMode::Min, 200);
        let t = Instant::now();
        jb.push(pkt(0), t);
        jb.push(pkt(2), t);
        assert!(matches!(jb.pop_ready(t), Some(JitterOutput::Packet(p)) if p.seq == 0));
        let slot1_due = t + Duration::from_millis(20);
        assert!(matches!(
            jb.pop_ready(slot1_due),
            Some(JitterOutput::Lost { seq: 1 })
        ));
        jb.push(pkt(1), slot1_due);
        match jb.pop_ready(slot1_due) {
            Some(JitterOutput::Packet(p)) => {
                assert_eq!(p.seq, 2, "late seq 1 must not be replayed")
            }
            other => panic!("expected Packet(seq=2), got {other:?}"),
        }
    }

    #[test]
    fn seq_wraps_at_24_bits() {
        let mut jb = JitterBuffer::new(JitterMode::Min, 200);
        let t = Instant::now();
        jb.push(pkt(0xFF_FFFF), t);
        jb.push(pkt(0), t);
        assert!(matches!(jb.pop_ready(t), Some(JitterOutput::Packet(p)) if p.seq == 0xFF_FFFF));
        assert!(matches!(jb.pop_ready(t), Some(JitterOutput::Packet(p)) if p.seq == 0));
    }

    #[test]
    fn large_forward_gap_resyncs_instead_of_concealing_every_frame() {
        use std::time::Duration;
        let mut jb = JitterBuffer::new(JitterMode::Min, 200);
        let t = Instant::now();
        jb.push(pkt(0), t);
        assert!(matches!(jb.pop_ready(t), Some(JitterOutput::Packet(p)) if p.seq == 0));
        let slot1_due = t + Duration::from_millis(20);
        jb.push(pkt(1_000), slot1_due);
        assert!(matches!(
            jb.pop_ready(slot1_due),
            Some(JitterOutput::Lost { seq: 1 })
        ));
        assert!(matches!(jb.pop_ready(slot1_due), Some(JitterOutput::Packet(p)) if p.seq == 1_000));
    }

    #[test]
    fn underrun_rebuffers_before_resuming() {
        use std::time::Duration;
        let mut jb = JitterBuffer::new(JitterMode::Fixed(40), 200);
        let t = Instant::now();
        jb.push(pkt(0), t);
        jb.push(pkt(1), t);
        assert!(matches!(jb.pop_ready(t), Some(JitterOutput::Packet(p)) if p.seq == 0));
        assert!(matches!(jb.pop_ready(t), Some(JitterOutput::Packet(p)) if p.seq == 1));
        let slot2_due = t + Duration::from_millis(40);
        assert!(jb.pop_ready(slot2_due).is_none());
        jb.push(pkt(2), slot2_due);
        assert!(
            jb.pop_ready(slot2_due).is_none(),
            "after an underrun the buffer must refill to target depth"
        );
        jb.push(pkt(3), slot2_due);
        assert!(matches!(jb.pop_ready(slot2_due), Some(JitterOutput::Packet(p)) if p.seq == 2));
    }

    #[test]
    fn sustained_backlog_from_clock_drift_is_trimmed() {
        let mut jb = JitterBuffer::new(JitterMode::Min, 200);
        let t = Instant::now();
        let mut next_push = 0u32;
        for _ in 0..6 {
            jb.push(pkt(next_push), t);
            next_push += 1;
        }
        let mut played = Vec::new();
        for _ in 0..120 {
            jb.push(pkt(next_push), t);
            next_push += 1;
            match jb.pop_ready(t) {
                Some(JitterOutput::Packet(p)) => played.push(p.seq),
                other => panic!("expected a packet, got {other:?}"),
            }
        }
        let skips = played.windows(2).filter(|w| w[1] != w[0] + 1).count();
        assert!(
            skips >= 1,
            "a sustained backlog must be trimmed: {played:?}"
        );
        assert!(
            skips <= 3,
            "trimming must be gradual, not a flush: {skips} skips in 120 slots"
        );
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
        assert_eq!(jb.queue.len(), MAX_QUEUED_PACKETS);
        assert!((0..MAX_QUEUED_PACKETS as u32).all(|s| jb.queue.contains_key(&s)));
        assert!(matches!(jb.pop_ready(t), Some(JitterOutput::Packet(p)) if p.seq == 0));
    }
}
