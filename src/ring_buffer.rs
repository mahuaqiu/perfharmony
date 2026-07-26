use std::collections::VecDeque;

/// 有界样本缓冲区，满时丢弃最旧样本，避免平台断网导致内存无限增长。
#[derive(Debug)]
pub struct RingBuffer<T> {
    capacity: usize,
    values: VecDeque<T>,
    dropped: u64,
}

impl<T> RingBuffer<T> {
    pub fn new(capacity: usize) -> Self {
        assert!(capacity > 0, "RingBuffer 容量必须大于 0");
        Self {
            capacity,
            values: VecDeque::with_capacity(capacity),
            dropped: 0,
        }
    }

    pub fn push(&mut self, value: T) {
        if self.values.len() == self.capacity {
            self.values.pop_front();
            self.dropped += 1;
        }
        self.values.push_back(value);
    }

    pub fn len(&self) -> usize {
        self.values.len()
    }

    #[cfg(test)]
    pub fn dropped(&self) -> u64 {
        self.dropped
    }

    pub fn drain(&mut self) -> Vec<T> {
        self.values.drain(..).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::RingBuffer;

    #[test]
    fn drops_oldest_when_full() {
        let mut buffer = RingBuffer::new(2);
        buffer.push(1);
        buffer.push(2);
        buffer.push(3);
        assert_eq!(buffer.drain(), vec![2, 3]);
        assert_eq!(buffer.dropped(), 1);
    }
}
