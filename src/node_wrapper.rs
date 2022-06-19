use crate::{Family, Node};
use haphazard::{Domain, HazardPointer, Replaced};

use std::{fmt::Debug, marker::PhantomData, ops::Deref, ptr::NonNull};

/// Wraps an owned node removed from the map
///
/// Handles retiring the node when dropped
// TODO: Come up with a better name, thit is more analogous to an Arc than a Box
pub struct NodeBox<'d, K, V>
where
    K: Send,
    V: Send,
{
    inner: Replaced<Node<K, V>, Family, Box<Node<K, V>>>,
    domain: &'d Domain<Family>,
}

/// Wraps a node that references an active value in the map
pub struct NodeRef<'d, K, V>
where
    K: Send,
    V: Send,
{
    inner: NonNull<Node<K, V>>,
    _hazard: HazardPointer<'d, Family>,
    _ref: PhantomData<&'d Node<K, V>>,
}

impl<'d, K, V> NodeBox<'d, K, V>
where
    K: Send,
    V: Send,
{
    /// Creates a new NodeWrapper using an owned pointer that was removed from the hashmap
    /// plus its domain
    /// # Safety
    ///
    /// 1. `inner` will never again be returned by any [`AtomicPtr::load`].
    /// 2. `inner` has not already been retired.
    /// 3. All calls to [`load`](AtomicPtr::load) that can have seen `inner` object were using hazard
    ///    pointers from `domain`.
    pub(crate) unsafe fn new(
        inner: Replaced<Node<K, V>, Family, Box<Node<K, V>>>,
        domain: &'d Domain<Family>,
    ) -> Self {
        // SAFETY: we own this
        let node = unsafe { inner.as_ref().as_ref() };
        #[cfg(debug_assertions)]
        {
            // store null here as a precaution
            // SAFETY: `ptr` is null
            unsafe { node.next.store_ptr(std::ptr::null_mut()) };
        }
        Self { inner, domain }
    }

    /// Returns the key stored inside this node
    pub fn key(&self) -> &K {
        &unsafe { self.inner.as_ref().as_ref() }.key
    }

    /// Returns the value stored inside this node
    pub fn value(&self) -> &V {
        &unsafe { self.inner.as_ref().as_ref() }.value
    }

    /// Returns the inner key value pair, if there are no outstanding readers of this node
    ///
    /// Otherwise, an `Err` is returned with the same `NodeBox` that was passed in
    pub fn try_unwrap(self) -> Result<(K, V), Self> {
        todo!()
    }
}

impl<K, V> Drop for NodeBox<'_, K, V>
where
    K: Send,
    V: Send,
{
    fn drop(&mut self) {
        // SAFETY:
        // API contract covered by users calling `new`
        unsafe { self.inner.retire_in(self.domain) };
    }
}

impl<K, V> Deref for NodeBox<'_, K, V>
where
    K: Send,
    V: Send,
{
    type Target = V;

    fn deref(&self) -> &Self::Target {
        // SAFETY: we own this
        &unsafe { self.inner.as_ref().as_ref() }.value
    }
}

impl<K, V> PartialEq for NodeBox<'_, K, V>
where
    K: Send + PartialEq,
    V: Send + PartialEq,
{
    fn eq(&self, other: &Self) -> bool {
        self.key() == other.key() && self.value() == other.value()
    }
}

impl<K, V> Eq for NodeBox<'_, K, V>
where
    K: Send + Eq,
    V: Send + Eq,
{
}

impl<K, V> Debug for NodeBox<'_, K, V>
where
    K: Send + Debug,
    V: Send + Debug,
{
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.value().fmt(f)
    }
}

impl<'d, K, V> NodeRef<'d, K, V>
where
    K: Send,
    V: Send,
{
    /// Creates a new NodeRef to an active node in the hashmap
    /// # Safety
    ///
    /// The caller must guarntee that `inner` was obtained from the most recent load of a
    /// `haphazard::AtomicPtr` protected by `hazard` in the matching domain
    pub(crate) unsafe fn new(
        inner: NonNull<Node<K, V>>,
        hazard: HazardPointer<'d, Family>,
    ) -> Self {
        Self {
            inner,
            _hazard: hazard,
            _ref: PhantomData,
        }
    }

    /// Returns the key stored inside this node
    pub fn key(&self) -> &K {
        // SAFETY: By the safety contract of new, `self.guard` protects `self.inner`,
        // so inner must be still alive
        &unsafe { self.inner.as_ref() }.key
    }

    /// Returns the value stored inside this node
    pub fn value(&self) -> &V {
        // SAFETY: By the safety contract of new, `self.guard` protects `self.inner`,
        // so inner must be still alive
        &unsafe { self.inner.as_ref() }.value
    }
}

// no drop implementation needed for `NodeRef` since we are only a reader there is no cleanup to do

impl<K, V> Deref for NodeRef<'_, K, V>
where
    K: Send,
    V: Send,
{
    type Target = V;

    fn deref(&self) -> &Self::Target {
        self.value()
    }
}

impl<K, V> PartialEq for NodeRef<'_, K, V>
where
    K: Send + PartialEq,
    V: Send + PartialEq,
{
    fn eq(&self, other: &Self) -> bool {
        self.key() == other.key() && self.value() == other.value()
    }
}

impl<K, V> Eq for NodeRef<'_, K, V>
where
    K: Send + Eq,
    V: Send + Eq,
{
}

impl<K, V> Debug for NodeRef<'_, K, V>
where
    K: Send + Debug,
    V: Send + Debug,
{
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.value().fmt(f)
    }
}
