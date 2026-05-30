use ringbuf::{
    traits::{Consumer, Observer, Producer, Split},
    HeapRb,
};

pub struct AudioRing;

impl AudioRing {
    #[allow(clippy::new_ret_no_self)]
    pub fn new(capacity: usize) -> (RingProducer, RingConsumer) {
        let rb = HeapRb::<f32>::new(capacity);
        let (prod, cons) = rb.split();
        (RingProducer { inner: prod }, RingConsumer { inner: cons })
    }
}

pub struct RingProducer {
    inner: ringbuf::HeapProd<f32>,
}

pub struct RingConsumer {
    inner: ringbuf::HeapCons<f32>,
}

const _: fn() = || {
    fn assert_send<T: Send>() {}
    assert_send::<RingProducer>();
    assert_send::<RingConsumer>();
};

impl RingProducer {
    pub fn push_slice(&mut self, samples: &[f32]) -> usize {
        self.inner.push_slice(samples)
    }

    pub fn free(&self) -> usize {
        self.inner.vacant_len()
    }
}

impl RingConsumer {
    pub fn pop_slice(&mut self, dst: &mut [f32]) -> usize {
        self.inner.pop_slice(dst)
    }

    pub fn occupied(&self) -> usize {
        self.inner.occupied_len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn push_pop_roundtrip() {
        let (mut tx, mut rx) = AudioRing::new(1024);
        let input: Vec<f32> = (0..100).map(|i| i as f32 * 0.01).collect();
        let pushed = tx.push_slice(&input);
        assert_eq!(pushed, 100);

        let mut out = vec![0.0; 100];
        let popped = rx.pop_slice(&mut out);
        assert_eq!(popped, 100);
        assert_eq!(out, input);
    }

    #[test]
    fn push_when_full_returns_partial() {
        let (mut tx, mut _rx) = AudioRing::new(10);
        let input = vec![1.0; 50];
        let pushed = tx.push_slice(&input);
        assert!(
            pushed <= 10,
            "must not push more than capacity, got {}",
            pushed
        );
    }

    #[test]
    fn pop_when_empty_returns_zero() {
        let (mut _tx, mut rx) = AudioRing::new(10);
        let mut out = vec![0.0; 5];
        let popped = rx.pop_slice(&mut out);
        assert_eq!(popped, 0);
    }

    #[test]
    fn available_tracks_size() {
        let (mut tx, mut rx) = AudioRing::new(100);
        assert_eq!(rx.occupied(), 0);
        tx.push_slice(&[0.0; 30]);
        assert_eq!(rx.occupied(), 30);
        let mut buf = vec![0.0; 10];
        rx.pop_slice(&mut buf);
        assert_eq!(rx.occupied(), 20);
    }
}
