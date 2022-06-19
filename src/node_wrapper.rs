use crate::{Family, Node};
use haphazard::{Domain, Replaced};

use std::{fmt::Debug, ops::Deref};

/// Wraps a node removed from the map so it isn't freed too soon
pub struct NodeWrapper<'d, K, V>
where
    K: Send,
    V: Send,
{
    inner: Replaced<Node<K, V>, Family, Box<Node<K, V>>>,
    domain: &'d Domain<Family>,
}

impl<'d, K, V> NodeWrapper<'d, K, V>
where
    K: Send,
    V: Send,
{
    /// # Safety
    ///
    /// 1. The pointed-to object will never again be returned by any [`AtomicPtr::load`].
    /// 2. The pointed-to object has not already been retired.
    /// 3. All calls to [`load`](AtomicPtr::load) that can have seen the pointed-to object were
    ///    using hazard pointers from `domain`.
    pub(crate) unsafe fn new(
        inner: Replaced<Node<K, V>, Family, Box<Node<K, V>>>,
        domain: &'d Domain<Family>,
    ) -> Self {
        // SAFETY: we own this
        let node = unsafe { inner.as_ref().as_ref() };
        #[cfg(debug_assertions)]
        {
            // store null here as a precaution
            unsafe { node.next.store_ptr(std::ptr::null_mut()) };
        }
        Self { inner, domain }
    }
}

impl<K, V> Drop for NodeWrapper<'_, K, V>
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

impl<K, V> Deref for NodeWrapper<'_, K, V>
where
    K: Send,
    V: Send,
{
    type Target = Node<K, V>;

    fn deref(&self) -> &Self::Target {
        // SAFETY: we own this
        unsafe { self.inner.as_ref().as_ref() }
    }
}

impl<K, V> Debug for NodeWrapper<'_, K, V>
where
    K: Send + Debug,
    V: Send + Debug,
{
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("NodeWrapper")
            .field("inner", &self.inner)
            .finish()
    }
}
