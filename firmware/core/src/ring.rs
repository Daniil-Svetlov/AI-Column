//! Кольцевой буфер «один писатель — один читатель» без блокировок.
//! Писатель и читатель живут в разных потоках (задачах FreeRTOS, на разных ядрах).
use std::cell::UnsafeCell;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

struct Inner {
    buf: Box<[UnsafeCell<u8>]>,
    head: AtomicUsize,
    tail: AtomicUsize,
}

// Безопасно: писатель трогает только [head, tail) свободной части, читатель — занятую,
// границы публикуются через Release/Acquire.
unsafe impl Sync for Inner {}
unsafe impl Send for Inner {}

impl Inner {
    fn cap(&self) -> usize {
        self.buf.len()
    }
    fn available(&self) -> usize {
        let h = self.head.load(Ordering::Acquire);
        let t = self.tail.load(Ordering::Acquire);
        if h >= t { h - t } else { self.cap() - t + h }
    }
    fn ptr(&self) -> *mut u8 {
        self.buf.as_ptr() as *mut u8
    }
}

/// Создаёт буфер на `capacity` байт (полезная ёмкость — на 1 меньше).
pub fn ring(capacity: usize) -> (Producer, Consumer) {
    assert!(capacity > 1);
    let buf: Box<[UnsafeCell<u8>]> = (0..capacity).map(|_| UnsafeCell::new(0)).collect();
    let inner = Arc::new(Inner { buf, head: AtomicUsize::new(0), tail: AtomicUsize::new(0) });
    (Producer { inner: inner.clone() }, Consumer { inner })
}

pub struct Producer {
    inner: Arc<Inner>,
}

pub struct Consumer {
    inner: Arc<Inner>,
}

impl Producer {
    pub fn capacity(&self) -> usize {
        self.inner.cap() - 1
    }
    pub fn available(&self) -> usize {
        self.inner.available()
    }
    pub fn free_space(&self) -> usize {
        self.capacity() - self.available()
    }
    /// Пишет сколько влезет, возвращает число записанных байт.
    pub fn write(&mut self, data: &[u8]) -> usize {
        let cap = self.inner.cap();
        let n = data.len().min(self.free_space());
        let h = self.inner.head.load(Ordering::Relaxed);
        let first = n.min(cap - h);
        unsafe {
            std::ptr::copy_nonoverlapping(data.as_ptr(), self.inner.ptr().add(h), first);
            std::ptr::copy_nonoverlapping(data.as_ptr().add(first), self.inner.ptr(), n - first);
        }
        self.inner.head.store((h + n) % cap, Ordering::Release);
        n
    }
}

impl Consumer {
    pub fn capacity(&self) -> usize {
        self.inner.cap() - 1
    }
    pub fn available(&self) -> usize {
        self.inner.available()
    }
    /// Читает до out.len() байт, возвращает число прочитанных.
    pub fn read(&mut self, out: &mut [u8]) -> usize {
        let cap = self.inner.cap();
        let n = out.len().min(self.available());
        let t = self.inner.tail.load(Ordering::Relaxed);
        let first = n.min(cap - t);
        unsafe {
            std::ptr::copy_nonoverlapping(self.inner.ptr().add(t), out.as_mut_ptr(), first);
            std::ptr::copy_nonoverlapping(self.inner.ptr(), out.as_mut_ptr().add(first), n - first);
        }
        self.inner.tail.store((t + n) % cap, Ordering::Release);
        n
    }
    /// Выкинуть всё накопленное (со стороны читателя).
    pub fn clear(&mut self) {
        self.inner.tail.store(self.inner.head.load(Ordering::Acquire), Ordering::Release);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wraps_and_keeps_order() {
        let (mut p, mut c) = ring(16);
        assert_eq!(p.capacity(), 15);
        let input: Vec<u8> = (0..10).collect();
        let mut out = [0u8; 10];
        assert_eq!(p.write(&input), 10);
        assert_eq!(c.read(&mut out[..6]), 6);
        assert_eq!(p.write(&input), 10); // перешли через конец буфера
        assert_eq!(c.available(), 14);
        let mut all = [0u8; 14];
        assert_eq!(c.read(&mut all), 14);
        assert_eq!(all, [6, 7, 8, 9, 0, 1, 2, 3, 4, 5, 6, 7, 8, 9]);
    }

    #[test]
    fn never_overflows() {
        let (mut p, mut c) = ring(8);
        assert_eq!(p.write(&[0; 20]), 7);
        assert_eq!(p.free_space(), 0);
        c.clear();
        assert_eq!(c.available(), 0);
    }

    #[test]
    fn threads() {
        let (mut p, mut c) = ring(97);
        let t = std::thread::spawn(move || {
            let mut i = 0u32;
            while i < 100_000 {
                let b = [(i % 251) as u8];
                if p.write(&b) == 1 {
                    i += 1;
                }
            }
        });
        let mut n = 0u32;
        let mut buf = [0u8; 13];
        while n < 100_000 {
            let k = c.read(&mut buf);
            for &b in &buf[..k] {
                assert_eq!(b, (n % 251) as u8);
                n += 1;
            }
        }
        t.join().unwrap();
    }
}
