pub use std::sync::atomic::{
    AtomicIsize,
    AtomicUsize,
    Ordering,
    fence,
};

pub use self::types::{
    AtomicU64,
    AtomicI64,
};

#[cfg(target_pointer_width = "64")]
mod types {
    use std::sync::atomic::{AtomicUsize, AtomicIsize, Ordering};

    pub struct AtomicU64 {
        v: AtomicUsize,
    }

    impl AtomicU64 {

        #[inline]
        pub fn new(v: u64) -> AtomicU64 {
            AtomicU64 { v: AtomicUsize::new(v as usize) }
        }

        #[inline]
        pub fn load(&self, order: Ordering) -> u64 {
            self.v.load(order) as u64
        }

        #[inline]
        pub fn store(&self, val: u64, order: Ordering) {
            self.v.store(val as usize, order);
        }

        #[inline]
        pub fn swap(&self, val: u64, order: Ordering) -> u64 {
            self.v.swap(val as usize, order) as u64
        }

        #[inline]
        pub fn compare_and_swap(&self, old: u64, new: u64, order: Ordering) -> u64 {
            self.v.compare_and_swap(old as usize, new as usize, order) as u64
        }

        #[inline]
        pub fn fetch_add(&self, val: u64, order: Ordering) -> u64 {
            self.v.fetch_add(val as usize, order) as u64
        }

        #[inline]
        pub fn fetch_sub(&self, val: u64, order: Ordering) -> u64 {
            self.v.fetch_sub(val as usize, order) as u64
        }

        #[inline]
        pub fn fetch_and(&self, val: u64, order: Ordering) -> u64 {
            self.v.fetch_and(val as usize, order) as u64
        }

        #[inline]
        pub fn fetch_or(&self, val: u64, order: Ordering) -> u64 {
            self.v.fetch_or(val as usize, order) as u64
        }

        #[inline]
        pub fn fetch_xor(&self, val: u64, order: Ordering) -> u64 {
            self.v.fetch_xor(val as usize, order) as u64
        }
    }

    pub struct AtomicI64 {
        v: AtomicIsize,
    }

    impl AtomicI64 {

        #[inline]
        pub fn new(v: i64) -> AtomicI64 {
            AtomicI64 { v: AtomicIsize::new(v as isize) }
        }

        #[inline]
        pub fn load(&self, order: Ordering) -> i64 {
            self.v.load(order) as i64
        }

        #[inline]
        pub fn store(&self, val: i64, order: Ordering) {
            self.v.store(val as isize, order);
        }

        #[inline]
        pub fn swap(&self, val: i64, order: Ordering) -> i64 {
            self.v.swap(val as isize, order) as i64
        }

        #[inline]
        pub fn compare_and_swap(&self, old: i64, new: i64, order: Ordering) -> i64 {
            self.v.compare_and_swap(old as isize, new as isize, order) as i64
        }

        #[inline]
        pub fn fetch_add(&self, val: i64, order: Ordering) -> i64 {
            self.v.fetch_add(val as isize, order) as i64
        }

        #[inline]
        pub fn fetch_sub(&self, val: i64, order: Ordering) -> i64 {
            self.v.fetch_sub(val as isize, order) as i64
        }

        #[inline]
        pub fn fetch_and(&self, val: i64, order: Ordering) -> i64 {
            self.v.fetch_and(val as isize, order) as i64
        }

        #[inline]
        pub fn fetch_or(&self, val: i64, order: Ordering) -> i64 {
            self.v.fetch_or(val as isize, order) as i64
        }

        #[inline]
        pub fn fetch_xor(&self, val: i64, order: Ordering) -> i64 {
            self.v.fetch_xor(val as isize, order) as i64
        }
    }
}

#[cfg(not(target_pointer_width = "64"))]
mod types {
    pub struct AtomicU64;

    impl AtomicU64 {
    }

    pub struct AtomicI64;

    impl AtomicI64 {
    }
}
