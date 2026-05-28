use bytes::BytesMut;
use crossbeam_queue::ArrayQueue;
use std::sync::Arc;

#[derive(Clone)]
pub struct BufferPool {
    inner: Arc<PoolInner>,
}

struct PoolInner {
    queue: ArrayQueue<BytesMut>,
    buf_size: usize,
}

impl BufferPool {
    pub fn new(pool_size: usize, buf_size: usize) -> Self {
        let queue = ArrayQueue::new(pool_size);
        for _ in 0..pool_size {
            let _ = queue.push(BytesMut::with_capacity(buf_size));
        }
        Self {
            inner: Arc::new(PoolInner { queue, buf_size }),
        }
    }

    pub fn acquire(&self) -> BytesMut {
        match self.inner.queue.pop() {
            Some(mut b) => {
                b.clear();
                b
            }
            None => BytesMut::with_capacity(self.inner.buf_size),
        }
    }

    pub fn release(&self, buf: BytesMut) {
        if buf.capacity() != self.inner.buf_size {
            return;
        }
        let _ = self.inner.queue.push(buf);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn acquire_returns_buffer_of_buf_size() {
        let pool = BufferPool::new(4, 200);
        let buf = pool.acquire();
        assert_eq!(buf.capacity(), 200);
    }

    #[test]
    fn release_then_acquire_reuses() {
        let pool = BufferPool::new(1, 256);
        let mut b = pool.acquire();
        b.extend_from_slice(b"hello");
        let ptr = b.as_ptr() as usize;
        pool.release(b);
        let b2 = pool.acquire();
        assert_eq!(
            b2.as_ptr() as usize,
            ptr,
            "reused buffer should share allocation"
        );
    }

    #[test]
    fn over_capacity_acquire_allocates_fresh() {
        let pool = BufferPool::new(1, 64);
        let _b1 = pool.acquire();
        let b2 = pool.acquire();
        assert_eq!(b2.capacity(), 64);
    }

    #[test]
    fn release_over_capacity_drops() {
        let pool = BufferPool::new(1, 64);
        let b1 = pool.acquire();
        let b2 = pool.acquire();
        pool.release(b1);
        pool.release(b2);
        let b3 = pool.acquire();
        assert_eq!(b3.capacity(), 64);
    }
}
