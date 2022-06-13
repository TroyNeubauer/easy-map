use std::fmt::Debug;
use std::hash::{BuildHasher, Hash, Hasher};
use std::ptr::NonNull;
use std::sync::atomic::AtomicUsize;

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
        let mut new_node = Some(Box::new(Node { key, value, next }));
        let index = self.index_for_key(&new_node.as_ref().unwrap().key);
        let bucket = &self.buckets[index];

        loop {
            let mut second_guard = None;
            let new_node_ref = new_node.as_mut().unwrap();

            let mut guard = HazardPointer::new_in_domain(&self.domain);
            let maybe_root_node = bucket.0.protect_ptr(&mut guard);

            let (ptr, current, _guard) = match maybe_root_node {
                Some((root_node, _)) => {
                    match self.find_matching_key(&bucket.0, guard, root_node, &new_node_ref.key) {
                        Ok((ptr, guard, match_ptr)) => {
                            // We found a match, swap the found node out with this one
                            second_guard = Some(HazardPointer::new_in_domain(&self.domain));

                            // Find the next of the old node
                            let next = unsafe { match_ptr.as_ref() }
                                .next
                                .protect_ptr(second_guard.as_mut().unwrap());

                            let next = next
                                .map(|(ptr, _)| ptr.as_ptr())
                                .unwrap_or(core::ptr::null_mut());

                            let a = &new_node_ref.next;
                            // copy the rest of the chain after the old node
                            unsafe { a.store_ptr(next) };

                            (ptr, match_ptr.as_ptr(), guard)
                        }
                        Err(root_guard) => {
                            // No key matched ours, add to chain by putting new_node first
                            // Chain the old root onto new_node
                            unsafe { new_node_ref.next.store_ptr(root_node.as_ptr()) };
                            (&bucket.0, root_node.as_ptr(), root_guard)
                        }
                    }
                }
                None => {
                    // Bucket is empty, simply swap in node
                    (&bucket.0, core::ptr::null_mut(), guard)
                }
            };

            match ptr.compare_exchange_weak(current, new_node.take().unwrap()) {
                Ok(_old) => {
                    // Need to handle old
                    break None;
                }
                Err(prev) => {
                    // Try again
                    new_node = Some(prev);
                }
            }
            let _ = second_guard;
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

    fn iter(&self) -> Iter<'_, K, V> {
        Iter {
            domain: &self.domain,
            buckets: &self.buckets,
            last: None,
            index: 0,
        }
    }
}

struct Iter<'m, K, V>
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

    #[test]
    fn feels_good() {
        let map = EasyMap::<&'static str, u32>::with_capacity(1);
        map.print_state();
        map.insert("Troy", 5);
        map.print_state();
        map.insert("Jane", 15);
        map.print_state();
        map.insert("David", 20);
        map.print_state();
        map.insert("Troy", 10);
        map.print_state();
        map.insert("Jane", 0);
        map.print_state();
        let all: Vec<_> = map.iter().collect();
        println!("{:?}", all);

        // assert_eq!(map.get("Troy"), Some(5));
        // assert_eq!(map.get("Jane"), Some(15));
        // assert_eq!(map.get("David"), Some(20));
    }
}
