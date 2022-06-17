use std::fmt::Debug;
use std::hash::{BuildHasher, Hash, Hasher};
use std::ptr::NonNull;
use std::sync::atomic::{AtomicUsize, Ordering};

use haphazard::{AtomicPtr, Domain, HazardPointer};

#[non_exhaustive]
struct Family;

#[derive(Debug)]
struct Node<K, V>
where
    K: Hash + Eq + Send + Sync,
    V: Send + Sync,
{
    key: K,
    value: V,

    /// The next bucket, used in chaining
    next: AtomicPtr<Node<K, V>, Family>,
}

struct Bucket<K, V>(AtomicPtr<Node<K, V>, Family>)
where
    K: Hash + Eq + Send + Sync,
    V: Send + Sync;

pub struct EasyMap<K, V>
where
    K: Hash + Eq + Send + Sync,
    V: Send + Sync,
{
    /// The hazard pointer domain for concurrent memory reclaimation
    domain: haphazard::Domain<Family>,

    /// The hasher used to determine indices
    build_hasher: ahash::RandomState,

    /// The buckets of this hash map
    buckets: Box<[Bucket<K, V>]>,

    /// Stores the number of non-null nodes in buckets
    in_use: AtomicUsize,
}

impl<K, V> EasyMap<K, V>
where
    K: Hash + Eq + Send + Sync + Debug,
    V: Send + Sync + Debug,
{
    pub fn new() -> Self {
        Self::with_capacity(32)
    }

    pub fn with_capacity(capacity: usize) -> Self {
        let mut buckets = Vec::with_capacity(capacity);
        for _ in 0..capacity {
            let ptr = unsafe { AtomicPtr::new(std::ptr::null_mut()) };
            buckets.push(Bucket(ptr));
        }

        Self {
            domain: haphazard::Domain::new(&Family),
            buckets: buckets.into_boxed_slice(),
            in_use: AtomicUsize::new(0),
            build_hasher: ahash::RandomState::new(),
        }
    }

    /// Tries to find a node with a key matching `key`.
    ///
    /// `ptr` is an a pointer in a bucket's linked list.
    /// `guard` is a hazardpointer guard which protects `ptr`
    /// `bucket` was derieved from the last read of `ptr`
    ///
    /// Returns `Ok(ptr, guard, node)` of the attributes of the matching node, or `Err(guard)` if
    /// no matching key is found. The returned guard in this case is the same guard passed in
    fn find_matching_key<'g, 'p, 's>(
        &'s self,
        ptr: &'p AtomicPtr<Node<K, V>, Family>,
        guard: HazardPointer<'g, Family>,
        node_ptr: NonNull<Node<K, V>>,
        key: &K,
    ) -> Result<
        (
            &'p AtomicPtr<Node<K, V>, Family>,
            HazardPointer<'g, Family>,
            NonNull<Node<K, V>>,
        ),
        HazardPointer<'g, Family>,
    >
    where
        's: 'p,
        's: 'g,
    {
        let node = unsafe { node_ptr.as_ref() };
        if node.key.eq(key) {
            Ok((ptr, guard, node_ptr))
        } else {
            let next = &node.next;
            let mut next_guard = HazardPointer::new_in_domain(&self.domain);
            match next.protect_ptr(&mut next_guard) {
                Some((next_bucket, _)) => {
                    match self.find_matching_key(next, next_guard, next_bucket, key) {
                        Ok(r) => Ok(r),
                        // Return our guard so that the caller gets the one they passed
                        Err(_) => Err(guard),
                    }
                }
                None => {
                    // next is null, end of list
                    Err(guard)
                }
            }
        }
    }

    pub fn insert(&self, key: K, value: V) -> Option<V> {
        // SAFETY: `p` is null
        let next = unsafe { AtomicPtr::new(std::ptr::null_mut()) };
        let new_node = Box::into_raw(Box::new(Node { key, value, next }));
        println!("allocated new node {:?}", new_node);
        let index = {
            // never shared
            let new_node = unsafe { &*new_node };
            self.index_for_key(&new_node.key)
        };
        let bucket = &self.buckets[index];

        loop {
            let (ptr, current, _guard) = {
                println!("getting hazard pointer");
                let mut guard = HazardPointer::new_in_domain(&self.domain);
                let maybe_root_node = bucket.0.protect_ptr(&mut guard);
                println!("protected hazard pointer");
                // never shared
                let new_node = unsafe { &*new_node };

                match maybe_root_node {
                    Some((root_node, _)) => {
                        match self.find_matching_key(&bucket.0, guard, root_node, &new_node.key) {
                            Ok((ptr, guard, match_ptr)) => {
                                println!("found matching key");
                                // copy the rest of the chain after the old node
                                let next = unsafe { match_ptr.as_ref() }.next.load_ptr();
                                // never shared
                                unsafe { new_node.next.store_ptr(next) };

                                (ptr, match_ptr.as_ptr(), guard)
                            }
                            Err(root_guard) => {
                                println!("no matching key... chaining");
                                // No key matched ours, add to chain by putting new_node first
                                // Chain the old root onto new_node
                                unsafe { new_node.next.store_ptr(root_node.as_ptr()) };
                                (&bucket.0, root_node.as_ptr(), root_guard)
                            }
                        }
                    }
                    None => {
                        println!("found empty node");
                        // Bucket is empty, simply swap in node
                        (&bucket.0, core::ptr::null_mut(), guard)
                    }
                }
            };

            dbg!();
            match unsafe { ptr.compare_exchange_weak_ptr(current, new_node) } {
                Ok(_old) => {
                    println!("Swapped {:?} to {:?}", current, new_node);
                    // Need to handle old
                    break None;
                }
                Err(prev) => {
                    println!("fialed to swap {:?} to {:?}", current, new_node);
                    // Reset the next pointer because it might need to change next time
                    let new_node = unsafe { &*new_node };
                    unsafe { new_node.next.store_ptr(std::ptr::null_mut()) };
                    println!("real value is {:?}", prev);
                    // Try again
                }
            }
        }
    }

    pub fn print_state(&self) {
        println!();
        println!();
        println!();
        for (i, bucket) in self.buckets.iter().enumerate() {
            let mut guard = HazardPointer::new_in_domain(&self.domain);
            let maybe_root_node = unsafe { bucket.0.load(&mut guard) };
            match maybe_root_node {
                Some(node) => {
                    println!("{i}: {:?} - {:?}", node.key, node.value);
                    self.print_chain(node);
                }
                None => {
                    println!("{i}: Empty");
                }
            }
        }
    }

    fn print_chain(&self, node: &Node<K, V>) {
        let mut guard2 = HazardPointer::new_in_domain(&self.domain);
        match unsafe { node.next.load(&mut guard2) } {
            Some(node) => {
                println!("   {:?} - {:?}", node.key, node.value);
                self.print_chain(node);
            }
            None => {}
        }
    }

    /// Returns the index of the bucket this key belongs to
    fn index_for_key(&self, key: &K) -> usize {
        let mut hasher = self.build_hasher.build_hasher();
        key.hash(&mut hasher);
        let hash = hasher.finish();
        hash as usize % self.buckets.len()
    }

    pub fn iter(&self) -> Iter<'_, K, V> {
        Iter {
            domain: &self.domain,
            buckets: &self.buckets,
            last: None,
            index: 0,
        }
    }
}

impl<K, V> Drop for EasyMap<K, V>
where
    K: Hash + Eq + Send + Sync,
    V: Send + Sync,
{
    fn drop(&mut self) {
        for bucket in self.buckets.iter() {
            // SAFETY: the pointer will not be modified
            let mut ptr = unsafe { bucket.0.as_std() }.load(Ordering::Relaxed);
            while !ptr.is_null() {
                // load the next pointer to free before retiring this one to avoid UB
                let next_ptr = {
                    // We have exclusive access to self, so nobody can access `ptr`
                    let r = unsafe { &*ptr };
                    // SAFETY: the pointer will not be modified
                    unsafe { r.next.as_std() }.load(Ordering::Relaxed)
                };
                // SAFETY:
                // 1. We have exclusive access to self, so nobody else can guard `ptr`
                // 2. `ptr` has not already been retired because it is still in the data structure
                // 3. `ptr` points to a valid Node, created by Box
                unsafe { self.domain.retire_ptr::<_, Box<Node<K, V>>>(ptr) };
                println!("Dropping {:?}", ptr);
                ptr = next_ptr;
            }
        }
    }
}

pub struct Iter<'m, K, V>
where
    K: Hash + Eq + Send + Sync + Debug,
    V: Send + Sync + Debug,
{
    domain: &'m Domain<Family>,
    buckets: &'m [Bucket<K, V>],
    last: Option<(HazardPointer<'m, Family>, NonNull<Node<K, V>>)>,
    index: usize,
}

impl<'m, K, V> Iterator for Iter<'m, K, V>
where
    K: Hash + Eq + Send + Sync + Debug,
    V: Send + Sync + Debug,
{
    type Item = (&'m K, &'m V);

    fn next(&mut self) -> Option<Self::Item> {
        if let Some((last_guard, last)) = self.last.take() {
            let last = unsafe { last.as_ref() };

            let mut guard = HazardPointer::new_in_domain(self.domain);
            let node = last.next.protect_ptr(&mut guard);
            let _ = last_guard;
            match node {
                Some((node, _)) => {
                    self.last = Some((guard, node));
                    let node = unsafe { self.last.as_ref().unwrap().1.as_ref() };
                    Some((&node.key, &node.value))
                }
                None => self.next(),
            }
        } else {
            if self.index == self.buckets.len() {
                return None;
            }
            let mut guard = HazardPointer::new_in_domain(self.domain);
            let node = self.buckets[self.index].0.protect_ptr(&mut guard);
            self.index += 1;
            match node {
                Some((node, _)) => {
                    self.last = Some((guard, node));
                    let node = unsafe { self.last.as_ref().unwrap().1.as_ref() };
                    Some((&node.key, &node.value))
                }
                None => self.next(),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::*;
    use itertools::Itertools;

    #[test]
    fn feels_good() {
        let map = EasyMap::<&'static str, u32>::with_capacity(1);
        let assert_same = |expected: &[(&str, u32)]| {
            let got = map.iter().collect_vec();
            let mut got = got.into_iter().map(|(k, v)| (*k, *v)).collect_vec();
            got.sort();
            let mut expected = Vec::from(expected);
            expected.sort();

            assert_eq!(got, expected);
        };
        assert_same(&[]);
        map.insert("Troy", 5);
        assert_same(&[("Troy", 5)]);
        map.insert("Jane", 15);
        assert_same(&[("Jane", 15), ("Troy", 5)]);
        map.insert("David", 20);
        assert_same(&[("David", 20), ("Jane", 15), ("Troy", 5)]);
        map.insert("Troy", 10);
        assert_same(&[("David", 20), ("Jane", 15), ("Troy", 10)]);
        map.insert("Jane", 0);
        assert_same(&[("David", 20), ("Jane", 0), ("Troy", 10)]);
        map.insert("David", 0);
        assert_same(&[("David", 0), ("Jane", 0), ("Troy", 10)]);
        map.insert("Troy", 0);
        assert_same(&[("David", 0), ("Jane", 0), ("Troy", 0)]);

        // assert_eq!(map.get("Troy"), Some(5));
        // assert_eq!(map.get("Jane"), Some(15));
        // assert_eq!(map.get("David"), Some(20));
    }
}
