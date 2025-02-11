use arc_swap::{ArcSwap, Guard};
use std::sync::Arc;

pub struct CowVec<T> {
    inner: ArcSwap<Vec<T>>,
    write_lock: parking_lot::Mutex<()>,
}

impl<T> CowVec<T>
where
    T: Clone,
{
    pub fn new() -> Self {
        Self {
            inner: ArcSwap::new(Arc::new(Vec::new())),
            write_lock: parking_lot::Mutex::new(()),
        }
    }

    #[allow(dead_code)]
    pub fn from_vec(vec: Vec<T>) -> Self {
        Self {
            inner: ArcSwap::from(Arc::new(vec)),
            write_lock: parking_lot::Mutex::new(()),
        }
    }

    /// Returns a read-only reference to the current vec
    pub fn get(&self) -> Guard<Arc<Vec<T>>> {
        self.inner.load()
    }

    pub fn push(&self, item: T) {
        // Lock to ensure only one writer at a time
        let _guard = self.write_lock.lock();

        // Get current vec and create new one with modification
        let current = self.inner.load();
        let mut new_vec = Vec::clone(&current);
        new_vec.push(item);

        // Swap in the new vec
        self.inner.store(Arc::new(new_vec));
    }

    pub fn retain<F>(&self, predicate: F)
    where
        F: FnMut(&T) -> bool,
    {
        // Lock to ensure only one writer at a time
        let _guard = self.write_lock.lock();

        // Get current vec and create new one with only the matching elements
        let current = self.inner.load();
        let mut new_vec = Vec::with_capacity(current.len());
        #[allow(clippy::iter_overeager_cloned)]
        new_vec.extend(current.iter().cloned().filter(predicate));
        // Swap in the new vec
        self.inner.store(Arc::new(new_vec));
    }

    pub fn clear(&self) {
        // Lock to ensure only one writer at a time
        let _guard = self.write_lock.lock();

        // Swap in an empty vec
        self.inner.store(Arc::new(Vec::new()));
    }
}

impl<T> Default for CowVec<T>
where
    T: Clone,
{
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::thread;

    #[test]
    fn test_concurrent_push() {
        let vec = Arc::new(CowVec::from_vec(vec![1, 2, 3]));
        let vec2 = vec.clone();

        // Start a read operation that will hold the old version
        let old_snapshot = vec.get();

        let handle = thread::spawn(move || {
            vec2.push(4);
        });

        vec.push(5);
        handle.join().unwrap();
        let final_state = vec.get().to_vec();

        // Old snapshot should still be valid and have original length
        assert_eq!(old_snapshot.len(), 3);
        // There should now be 5 items in the final state
        assert_eq!(final_state.len(), 5);
    }

    #[test]
    fn test_swap_retain() {
        let vec = CowVec::from_vec(vec![1, 2, 3, 4, 5]);

        // Keep a snapshot of the original state
        let old_snapshot = vec.get();

        // Retain only even numbers
        vec.retain(|x| x % 2 == 0);

        // Get the new state
        let new_snapshot = vec.get();

        // Original snapshot should be unchanged
        assert_eq!(old_snapshot.to_vec(), vec![1, 2, 3, 4, 5]);

        // New snapshot should only contain even numbers
        assert_eq!(new_snapshot.to_vec(), vec![2, 4]);

        // Test concurrent access during retain
        let vec = Arc::new(CowVec::from_vec(vec![1, 2, 3, 4, 5]));
        let vec2 = vec.clone();

        let reading_thread = thread::spawn(move || {
            // This read should get a consistent snapshot even while retain is running
            let snapshot = vec2.get();
            thread::sleep(std::time::Duration::from_millis(50)); // Simulate long read
            snapshot.to_vec()
        });

        // Small delay to ensure reading thread starts first
        thread::sleep(std::time::Duration::from_millis(10));

        vec.retain(|x| x % 2 == 0);

        let read_result = reading_thread.join().unwrap();
        assert_eq!(read_result, vec![1, 2, 3, 4, 5]); // Reading thread should see original state
        assert_eq!(vec.get().to_vec(), vec![2, 4]); // Final state should have only even numbers
    }
}
