use std::collections::{HashMap, VecDeque};
use std::sync::RwLock;
use std::time::{Duration, Instant};

pub struct TtlCell<V> {
    inner: RwLock<Option<(V, Instant)>>,
}

impl<V> TtlCell<V> {
    pub const fn new() -> Self {
        Self {
            inner: RwLock::new(None),
        }
    }

    pub fn set(&self, value: V) {
        let mut cache = self.inner.write().expect("poisoned lock");
        *cache = Some((value, Instant::now()));
    }
}

impl<V: Clone> TtlCell<V> {
    pub fn get(&self, ttl: Duration) -> Option<V> {
        let cache = self.inner.read().expect("poisoned lock");
        match *cache {
            Some((ref value, ts)) if ts.elapsed() < ttl => Some(value.clone()),
            _ => None,
        }
    }
}

struct Inner<V> {
    entries: HashMap<String, V>,
    insert_order: VecDeque<String>,
    cap: usize,
}

pub struct BoundedCache<V> {
    inner: RwLock<Inner<V>>,
}

impl<V> BoundedCache<V> {
    pub fn new(cap: usize) -> Self {
        Self {
            inner: RwLock::new(Inner {
                entries: HashMap::with_capacity(cap),
                insert_order: VecDeque::with_capacity(cap),
                cap,
            }),
        }
    }

    pub fn insert(&self, key: String, value: V) {
        let mut inner = self.inner.write().expect("poisoned lock");

        if let Some(existing) = inner.entries.get_mut(&key) {
            *existing = value;
            return;
        }

        if inner.entries.len() >= inner.cap
            && let Some(oldest) = inner.insert_order.pop_front()
        {
            inner.entries.remove(&oldest);
        }

        inner.insert_order.push_back(key.clone());
        inner.entries.insert(key, value);
    }
}

impl<V: Clone> BoundedCache<V> {
    pub fn get(&self, key: &str) -> Option<V> {
        let inner = self.inner.read().expect("poisoned lock");
        inner.entries.get(key).cloned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn insert_and_get() {
        let cache = BoundedCache::new(3);
        cache.insert("a".into(), 1);
        cache.insert("b".into(), 2);
        assert_eq!(cache.get("a"), Some(1));
        assert_eq!(cache.get("b"), Some(2));
        assert_eq!(cache.get("c"), None);
    }

    #[test]
    fn evicts_oldest() {
        let cache = BoundedCache::new(2);
        cache.insert("a".into(), 1);
        cache.insert("b".into(), 2);
        cache.insert("c".into(), 3);
        assert_eq!(cache.get("a"), None); // evicted
        assert_eq!(cache.get("b"), Some(2));
        assert_eq!(cache.get("c"), Some(3));
    }

    #[test]
    fn overwrite_existing() {
        let cache = BoundedCache::new(2);
        cache.insert("a".into(), 1);
        cache.insert("a".into(), 10);
        assert_eq!(cache.get("a"), Some(10));
        // Should not have grown — still at 1 entry
        cache.insert("b".into(), 2);
        cache.insert("c".into(), 3);
        // "a" was inserted first, so it gets evicted
        assert_eq!(cache.get("a"), None);
    }
}
