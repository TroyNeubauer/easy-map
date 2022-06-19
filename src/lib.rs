#![deny(unsafe_op_in_unsafe_fn)]
#![warn(missing_docs)]
//! A concurrent hashmap with chaining powered by haphazard.
//!
//! When collisions occur, items within the same bucket are chained linearally into an atomic
//! linked list

mod node_wrapper;
pub use node_wrapper::{NodeBox, NodeRef};

use std::fmt::Debug;
use std::hash::{BuildHasher, Hash, Hasher};
use std::ptr::NonNull;
use std::sync::atomic::{AtomicUsize, Ordering};

use haphazard::{AtomicPtr, Domain, HazardPointer};

#[non_exhaustive]
#[derive(Debug)]
struct Family;

/// A node in the hashmap
#[derive(Debug)]
pub struct Node<K, V> {
    key: K,
    value: V,

    /// The next bucket, used in chaining
    next: AtomicPtr<Node<K, V>, Family>,
}

struct Bucket<K, V>(AtomicPtr<Node<K, V>, Family>);

/// A concurrent hashmap powered by haphazard
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
    node_count: AtomicUsize,
}

impl<K, V> EasyMap<K, V>
where
    K: Hash + Eq + Send + Sync + Debug,
    V: Send + Sync + Debug,
{
    /// Creates a new map
    pub fn new() -> Self {
        Self::with_capacity(32)
    }

    /// Creates a new map preallocated with the given capacity
    pub fn with_capacity(capacity: usize) -> Self {
        let mut buckets = Vec::with_capacity(capacity);
        for _ in 0..capacity {
            let ptr = unsafe { AtomicPtr::new(std::ptr::null_mut()) };
            buckets.push(Bucket(ptr));
        }

        Self {
            domain: haphazard::Domain::new(&Family),
            buckets: buckets.into_boxed_slice(),
            node_count: AtomicUsize::new(0),
            build_hasher: ahash::RandomState::new(),
        }
    }

    /// Tries to find a node with a key matching `key`.
    ///
    /// `ptr` is an a pointer in a bucket's linked list.
    /// `guard` is a hazardpointer guard which protects `ptr`
    /// `node_ptr` was derieved from the last read of `ptr`
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

    /// Inserts a new key-value pair into the map concurrently, returning the old value associated
    /// with the key (if any)
    pub fn insert(&self, key: K, value: V) -> Option<NodeBox<K, V>> {
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

        // TODO: Handle rehash
        loop {
            let (ptr, current, _guard, garbage_owned) = {
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

                                // the matching node we swap out will be gone from the data
                                // structure
                                (ptr, match_ptr.as_ptr(), guard, true)
                            }
                            Err(root_guard) => {
                                println!("no matching key... chaining");
                                // No key matched ours, add to chain by putting new_node first
                                // Chain the old root onto new_node
                                unsafe { new_node.next.store_ptr(root_node.as_ptr()) };
                                (&bucket.0, root_node.as_ptr(), root_guard, false)
                            }
                        }
                    }
                    None => {
                        println!("found empty node");
                        // Bucket is empty, simply swap in node
                        (&bucket.0, core::ptr::null_mut(), guard, false)
                    }
                }
            };

            match unsafe { ptr.compare_exchange_weak_ptr(current, new_node) } {
                Ok(now_garbage) => {
                    self.node_count.fetch_add(1, Ordering::Relaxed);
                    if garbage_owned {
                        println!("Swapped out garbage {:?}", now_garbage);
                        break now_garbage.map(|replaced| {
                            // SAFETY:
                            // 1. We have removed `replaced` from the map so it is no longer acessible
                            // 2. Us and only us, swapped `replaced` out of the map,
                            //    so it is impossible for it to be retired already
                            // 3. All current readers of `replaced` are using HazardPointers
                            unsafe { NodeBox::new(replaced, &self.domain) }
                        });
                    } else {
                        println!("Swapped out garbage, but this is inconsequential");
                        break None;
                    }
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

    /// Removes the value that matches `key` or `None` if `key` is not in the map
    pub fn remove(&self, key: &K) -> Option<NodeBox<K, V>> {
        let index = self.index_for_key(key);
        let bucket = &self.buckets[index];

        // TODO: Handle rehash
        loop {
            println!("getting hazard pointer");
            let mut guard = HazardPointer::new_in_domain(&self.domain);
            let maybe_root_node = bucket.0.protect_ptr(&mut guard);
            println!("protected hazard pointer");

            match maybe_root_node {
                Some((root_node, _)) => {
                    match self.find_matching_key(&bucket.0, guard, root_node, key) {
                        Ok((ptr, _guard, match_ptr)) => {
                            println!("found matching key");
                            // link `match_ptr` out of the chain
                            let next = unsafe { match_ptr.as_ref() }.next.load_ptr();

                            match unsafe { ptr.compare_exchange_weak_ptr(match_ptr.as_ptr(), next) }
                            {
                                Ok(now_garbage) => {
                                    println!("Swapped out garbage {:?}", now_garbage);
                                    self.node_count.fetch_sub(1, Ordering::Relaxed);
                                    return now_garbage.map(|replaced| {
                                        // SAFETY:
                                        // 1. We have removed `replaced` from the map so it is no longer acessible
                                        // 2. Us and only us, swapped `replaced` out of the map,
                                        //    so it is impossible for it to be retired already
                                        // 3. All current readers of `replaced` are using HazardPointers
                                        unsafe { NodeBox::new(replaced, &self.domain) }
                                    });
                                }
                                Err(_was) => {
                                    println!(
                                        "fialed to remove {:?} could not swap to {:?}",
                                        match_ptr, next
                                    );
                                    // Try again
                                }
                            }
                        }
                        Err(_guard) => {
                            // Not found
                            return None;
                        }
                    }
                }
                None => {
                    // Not found in map
                    return None;
                }
            }
        }
    }

    /// Looks up the value associated with `key` or returns `None` if `key` is not in the map
    pub fn get(&self, key: &K) -> Option<NodeRef<'_, K, V>> {
        let index = self.index_for_key(key);
        let bucket = &self.buckets[index];

        // TODO: Handle rehash
        println!("getting hazard pointer");
        let mut guard = HazardPointer::new_in_domain(&self.domain);
        let maybe_root_node = bucket.0.protect_ptr(&mut guard);
        println!("protected hazard pointer");

        match maybe_root_node {
            Some((root_node, _)) => {
                match self.find_matching_key(&bucket.0, guard, root_node, key) {
                    Ok((_ptr, guard, match_ptr)) => {
                        println!("found matching key");

                        // SAFETY:
                        // `match_ptr` is the most recent load of `_ptr` using `guard`
                        // 1. These are either `guard` at the start of the loop and
                        //    `maybe_root_node`
                        // 2. Or they are derived from a read of the same level inside
                        //    `find_matching_key`
                        return Some(unsafe { NodeRef::new(match_ptr, guard) });
                    }
                    Err(_guard) => {
                        // Not found
                        return None;
                    }
                }
            }
            None => {
                // Not found in map
                return None;
            }
        }
    }

    /// Prints what the map looks like for debugging.
    /// Shows buckets and chaining
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

    /// Creates an iterator over the key-value pairs in the map.
    ///
    /// Iteration order is unspecified
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

impl<K, V> PartialEq for Node<K, V>
where
    K: PartialEq,
    V: PartialEq,
{
    fn eq(&self, other: &Self) -> bool {
        self.key == other.key && self.value == other.value
    }
}

impl<K, V> Eq for Node<K, V>
where
    K: Eq,
    V: Eq,
{
}

/// The iterator for this map
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
