use std::cell::UnsafeCell;
use std::iter::repeat;
use std::ops::{Add, AddAssign};
use std::sync::Arc;
use std::sync::atomic::{AtomicIsize, AtomicUsize, Ordering};

pub struct RingBufferConsumer<T> {
    inner: Arc<LockFreeRingBuffer<T>>,
}

impl<T: Send> RingBufferConsumer<T> {
    pub fn pop(&mut self) -> RingIter<T> {
        self.inner.pop()
    }
}

#[doc(hidden)]
impl<T> From<Arc<LockFreeRingBuffer<T>>> for RingBufferConsumer<T> {
    fn from(inner: Arc<LockFreeRingBuffer<T>>) -> Self {
        RingBufferConsumer { inner }
    }
}

pub struct RingBufferProducer<T> {
    inner: Arc<LockFreeRingBuffer<T>>,
}

impl<T: Send> RingBufferProducer<T> {
    pub fn push(&mut self, x: T) {
        self.inner.push(x)
    }
}

#[doc(hidden)]
impl<T> From<Arc<LockFreeRingBuffer<T>>> for RingBufferProducer<T> {
    fn from(inner: Arc<LockFreeRingBuffer<T>>) -> Self {
        RingBufferProducer { inner }
    }
}

pub fn new<T: Send>(size: usize) -> (RingBufferProducer<T>, RingBufferConsumer<T>) {
    let buffer = Arc::new(LockFreeRingBuffer::new(size));

    (buffer.clone().into(), buffer.clone().into())
}

pub struct LockFreeRingBuffer<T> {
    pub(crate) size: usize,
    pub(crate) head: AtomicUsize,
    pub(crate) tail: AtomicIsize,
    pub(crate) data: Vec<UnsafeCell<Option<T>>>,
}

unsafe impl<T: Send> Sync for LockFreeRingBuffer<T> {}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
struct Tail {
    pub locked: bool,
    pub value: usize,
}

impl Add<usize> for Tail {
    type Output = Self;

    fn add(mut self, rhs: usize) -> <Self as Add<usize>>::Output {
        self.value += rhs;
        self
    }
}

impl AddAssign<usize> for Tail {
    fn add_assign(&mut self, rhs: usize) {
        self.value += rhs;
    }
}

impl From<isize> for Tail {
    fn from(v: isize) -> Self {
        Tail {
            locked: v.is_negative(),
            value: if !v.is_negative() {
                v as usize
            } else {
                v.abs() as usize - 1
            },
        }
    }
}

impl Into<isize> for Tail {
    fn into(self) -> isize {
        if !self.locked {
            self.value as isize
        } else {
            -(self.value as isize) - 1
        }
    }
}

impl<T: Send> LockFreeRingBuffer<T> {
    pub fn new(size: usize) -> Self {
        LockFreeRingBuffer {
            size,
            head: 0.into(),
            tail: 0.into(),
            data: repeat(()).map(|()| None.into()).take(size + 1).collect(),
        }
    }

    // invariant: self.head % self.size is empty
    pub fn push(&self, x: T) {
        let head = self.head.load(Ordering::SeqCst);
        // Get the pointer to the next slot, which is guaranteed to be empty.
        let head_ptr: *mut Option<T> = self.data[head % self.data.len()].get();
        let head_ptr: &mut Option<T> = unsafe { &mut *head_ptr };
        *head_ptr = Some(x);

        // linearisation point if we exit because head - tail.value < self.size:
        self.head.store(head + 1, Ordering::SeqCst);

        loop {
            let tail: Tail = self.tail.load(Ordering::SeqCst).into();

            assert!(head >= tail.value, "{:?} {:?}", head, tail);

            if head - tail.value < self.size {
                // invariant holds
                break;
            }

            if tail.locked {
                continue;
            }

            // linearisation point if we exit because we pushed the tail pointer:
            if self.tail
                .compare_and_swap(tail.into(), (tail + 1).into(), Ordering::SeqCst)
                == tail.into()
            {
                // invariant holds
                break;
            }
        }
    }
    /// get the length of the buffer
    pub fn len(&self) -> usize {
        self.head.load(Ordering::SeqCst) - self.tail.load(Ordering::SeqCst) as usize
    }
    /// get the item at index `index`, relative to the tail
    pub fn get(&self, index: usize) -> Option<&T> {
        let tail = self.tail.load(Ordering::SeqCst);
        if self.head.load(Ordering::SeqCst) - tail as usize <= index {
            return None;
        }
        let tail = tail as usize + index;
        let tail = tail % self.data.len();
        let tail_ptr: *mut Option<T> = self.data[tail].get();
        let tail_ptr: &Option<T> = unsafe { &*tail_ptr };
        tail_ptr.as_ref()
    }
    /// get the item that was most recently pushed
    pub fn back(&self) -> Option<&T> {
        let head = self.head.load(Ordering::SeqCst);
        if head == 0 {
            return None;
        }
        let head = head - 1;
        let head = head % self.data.len();
        let head_ptr: *mut Option<T> = self.data[head].get();
        let head_ptr: &Option<T> = unsafe { &*head_ptr };
        head_ptr.as_ref()
    }
    /// get the item that was pushed the longest ago
    pub fn front(&self) -> Option<&T> {
        let tail = self.tail.load(Ordering::SeqCst);
        if self.head.load(Ordering::SeqCst) - tail as usize == 0 {
            return None;
        }
        let tail = tail as usize;
        let tail = tail % self.data.len();
        let tail_ptr: *mut Option<T> = self.data[tail].get();
        let tail_ptr: &Option<T> = unsafe { &*tail_ptr };
        tail_ptr.as_ref()
    }
    pub fn pop(&self) -> RingIter<T> {
        // linearisation point if we early-exit with empty iterator
        let head = self.head.load(Ordering::SeqCst);

        let opt_tail = loop {
            let tail: Tail = self.tail.load(Ordering::SeqCst).into();
            if tail.value == head {
                break None;
            }
            assert!(!tail.locked);
            let mut new_tail = tail;
            new_tail.locked = true;
            if self.tail
                .compare_and_swap(tail.into(), new_tail.into(), Ordering::SeqCst)
                == tail.into()
            {
                break Some(new_tail);
            }
        };

        let mut tail = match opt_tail {
            Some(tail) => tail,
            None => {
                return RingIter {
                    fixed_head: 0,
                    current_tail: Tail {
                        locked: false,
                        value: 0,
                    },
                    inner: &self,
                }
            }
        };

        // linearisation point to the moment we pushed the tail.
        let head = self.head.load(Ordering::SeqCst);

        assert!(head >= tail.value, "{:?} {:?}", head, tail.value);

        if head - tail.value == self.size + 1 {
            tail += 1;
            self.tail.store(tail.into(), Ordering::SeqCst);
        }

        RingIter {
            fixed_head: head,
            current_tail: tail,
            inner: &self,
        }
    }
}

pub struct RingIter<'a, T: 'a> {
    pub(crate) fixed_head: usize,
    pub(crate) current_tail: Tail,
    pub(crate) inner: &'a LockFreeRingBuffer<T>,
}

impl<'a, T> Iterator for RingIter<'a, T> {
    type Item = T;

    fn next(&mut self) -> Option<<Self as Iterator>::Item> {
        if !self.current_tail.locked {
            None
        } else {
            let res = {
                let tail_ptr: *mut Option<T> =
                    self.inner.data[self.current_tail.value % self.inner.data.len()].get();
                let tail_ptr: &mut Option<T> = unsafe { &mut *tail_ptr };
                tail_ptr.take()
            };
            self.current_tail += 1;
            if self.current_tail.value == self.fixed_head {
                self.current_tail.locked = false;
            }

            self.inner
                .tail
                .store(self.current_tail.into(), Ordering::SeqCst);
            res
        }
    }
}

impl<'a, T: 'a> Drop for RingIter<'a, T> {
    fn drop(&mut self) {
        if self.current_tail.locked {
            self.current_tail.locked = false;
            self.inner
                .tail
                .store(self.current_tail.into(), Ordering::Relaxed);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    extern crate rand;
    use self::rand::random;
    use std::thread;
    use std::time::Duration;
    #[test]
    fn test_get() {
        let rb = LockFreeRingBuffer::new(5);
        for i in 0..5 {
            rb.push(i);
            // assert_eq!(rb.len(), i+1);
        }
        assert_eq!(rb.len(), 5);
        for i in 0..5 {
            assert_eq!(rb.get(i), Some(&i));
        }
        assert_eq!(rb.front(), Some(&0));
        assert_eq!(rb.back(), Some(&4));
    }
    #[test]
    fn tail_conversion() {
        for locked in &[true, false] {
            for value in 0..100 {
                let tail = Tail {
                    locked: *locked,
                    value,
                };
                assert_eq!(<Tail as From<isize>>::from(tail.into()), tail)
            }
        }

        for i in -20isize..=20isize {
            assert_eq!(<Tail as Into<isize>>::into(Tail::from(i)), i)
        }
    }

    #[test]
    fn test_run() {
        let (push, pull) = new(5);

        let t1 = thread::spawn(move || {
            let mut push = push;
            for i in 1..1000 {
                push.push(i);
                println!("Pushed {}", i);
                thread::sleep(Duration::from_secs(random::<u64>() % 2))
            }
        });

        let t2 = thread::spawn(move || {
            let mut pull = pull;

            for i in 1..1000 {
                let num = random::<usize>() % 5;
                println!("Taking {}", num);
                thread::sleep(Duration::from_secs(5));
                let res = pull.pop()
                    .take(num)
                    .map(|x| {
                        thread::sleep(Duration::from_secs(random::<u64>() % 2));
                        x
                    })
                    .collect::<Vec<_>>();
                println!("{}, {:?}", num, res);
                thread::sleep(Duration::from_secs(random::<u64>() % 20))
            }
        });

        t1.join().unwrap();
        t2.join().unwrap();
    }
}
