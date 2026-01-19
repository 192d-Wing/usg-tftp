/// Buffer pool for reusing packet allocations
/// Performance optimization to reduce memory allocations and GC pressure
use bytes::{Bytes, BytesMut};
use std::sync::Arc;
use tokio::sync::Mutex;

const POOL_SIZE: usize = 128;
const MAX_BUFFER_SIZE: usize = 65468; // MAX_PACKET_SIZE

pub struct BufferPool {
    pool: Arc<Mutex<Vec<BytesMut>>>,
    buffer_size: usize,
}

impl BufferPool {
    /// Create a new buffer pool with a specified buffer size
    pub fn new(buffer_size: usize) -> Self {
        let mut pool = Vec::with_capacity(POOL_SIZE);

        // Pre-allocate some buffers
        for _ in 0..POOL_SIZE / 2 {
            pool.push(BytesMut::with_capacity(buffer_size));
        }

        Self {
            pool: Arc::new(Mutex::new(pool)),
            buffer_size,
        }
    }

    /// Create a default buffer pool for TFTP packets
    pub fn new_default() -> Self {
        Self::new(MAX_BUFFER_SIZE)
    }

    /// Acquire a buffer from the pool
    pub async fn acquire(&self) -> BytesMut {
        let mut pool = self.pool.lock().await;

        if let Some(mut buffer) = pool.pop() {
            buffer.clear();
            buffer
        } else {
            // Pool is empty, allocate a new buffer
            BytesMut::with_capacity(self.buffer_size)
        }
    }

    /// Return a buffer to the pool
    pub async fn release(&self, mut buffer: BytesMut) {
        let mut pool = self.pool.lock().await;

        // Only return to pool if we're not at capacity
        if pool.len() < POOL_SIZE {
            buffer.clear();
            pool.push(buffer);
        }
        // Otherwise, just drop it
    }

    /// Acquire a buffer with specific data
    pub async fn acquire_with_data(&self, data: &[u8]) -> BytesMut {
        let mut buffer = self.acquire().await;
        buffer.extend_from_slice(data);
        buffer
    }
}

impl Clone for BufferPool {
    fn clone(&self) -> Self {
        Self {
            pool: Arc::clone(&self.pool),
            buffer_size: self.buffer_size,
        }
    }
}

/// Wrapper for Bytes that automatically returns to pool when dropped
pub struct PooledBuffer {
    buffer: Option<BytesMut>,
    pool: BufferPool,
}

impl PooledBuffer {
    pub fn new(buffer: BytesMut, pool: BufferPool) -> Self {
        Self {
            buffer: Some(buffer),
            pool,
        }
    }

    pub fn as_ref(&self) -> &[u8] {
        self.buffer.as_ref().unwrap().as_ref()
    }

    pub fn freeze(mut self) -> Bytes {
        self.buffer.take().unwrap().freeze()
    }
}

impl Drop for PooledBuffer {
    fn drop(&mut self) {
        if let Some(buffer) = self.buffer.take() {
            let pool = self.pool.clone();
            tokio::spawn(async move {
                pool.release(buffer).await;
            });
        }
    }
}

impl std::ops::Deref for PooledBuffer {
    type Target = BytesMut;

    fn deref(&self) -> &Self::Target {
        self.buffer.as_ref().unwrap()
    }
}

impl std::ops::DerefMut for PooledBuffer {
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.buffer.as_mut().unwrap()
    }
}
