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

/* ## TODO
 *
 * - Extract AtomicState pattern
 *
 */

/// An atomic box
pub trait Atomic<T> : Send + Sync {

    /// Returns a new atomic box
    fn new(val: T) -> Self;

    /// Atomically loads the value from the box with the given ordering
    fn load(&self, order: Ordering) -> T;

    /// Atomically stores the value from the box with the given ordering
    fn store(&self, val: T, order: Ordering);

    /// Atomically swaps the value in the box with the given ordering
    fn swap(&self, val: T, order: Ordering) -> T;

    /// Swaps the value in the box if and only if the existing value is equal
    /// to `old`.
    fn compare_and_swap(&self, old: T, new: T, order: Ordering) -> T;
}

/// A value that can be stored in an atomic box
pub trait ToAtomicRepr : Send {

    /// The representation of the value when stored in an atomic box.
    type Repr;

    /// Load the value from the raw representation
    fn from_repr(raw: Self::Repr) -> Self;

    /// Convert the value from the raw representation
    fn to_repr(self) -> Self::Repr;
}

// TODO: Move A -> ToAtomicRepr associated type (blocked rust-lang/rust#20772)
pub struct AtomicVal<T: ToAtomicRepr, A: Atomic<T::Repr>> {
    atomic: A,
}

impl<T: ToAtomicRepr, A: Atomic<T::Repr>> AtomicVal<T, A> {
    /// Returns a new atomic box
    pub fn new(init: T) -> AtomicVal<T, A> {
        AtomicVal { atomic: <A as Atomic<T::Repr>>::new(init.to_repr()) }
    }
}

impl<T: ToAtomicRepr, A: Atomic<T::Repr>> Atomic<T> for AtomicVal<T, A> {

    /// Returns a new atomic box
    fn new(init: T) -> AtomicVal<T, A> {
        AtomicVal::new(init)
    }

    /// Atomically loads the value from the box with the given ordering
    fn load(&self, order: Ordering) -> T {
        let repr = self.atomic.load(order);
        ToAtomicRepr::from_repr(repr)
    }

    /// Atomically stores the value from the box with the given ordering
    fn store(&self, val: T, order: Ordering) {
        self.atomic.store(val.to_repr(), order);
    }

    /// Atomically swaps the value in the box with the given ordering
    fn swap(&self, val: T, order: Ordering) -> T {
        let repr = self.atomic.swap(val.to_repr(), order);
        ToAtomicRepr::from_repr(repr)
    }

    /// Swaps the value in the box if and only if the existing value is equal
    /// to `old`.
    fn compare_and_swap(&self, old: T, new: T, order: Ordering) -> T {
        let repr = self.atomic.compare_and_swap(old.to_repr(), new.to_repr(), order);
        ToAtomicRepr::from_repr(repr)
    }
}

impl Atomic<usize> for AtomicUsize {

    fn new(val: usize) -> AtomicUsize {
        AtomicUsize::new(val)
    }

    fn load(&self, order: Ordering) -> usize {
        AtomicUsize::load(self, order)
    }

    fn store(&self, val: usize, order: Ordering) {
        AtomicUsize::store(self, val, order)
    }

    fn swap(&self, val: usize, order: Ordering) -> usize {
        AtomicUsize::swap(self, val, order)
    }

    fn compare_and_swap(&self, old: usize, new: usize, order: Ordering) -> usize {
        AtomicUsize::compare_and_swap(self, old, new, order)
    }
}

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
