//! A map that remembers the order of use, oldest first: the cache and its source index are both kept this way.

use std::collections::{HashMap, VecDeque};

/// A map that remembers the order of use, oldest first.
pub(super) struct Ordered<V> {
    pub(super) order: VecDeque<String>,
    pub(super) map: HashMap<String, V>,
}

impl<V> Default for Ordered<V> {
    fn default() -> Self {
        Self {
            order: VecDeque::new(),
            map: HashMap::new(),
        }
    }
}

impl<V> Ordered<V> {
    pub(super) fn get(&self, key: &str) -> Option<&V> {
        self.map.get(key)
    }

    pub(super) fn contains(&self, key: &str) -> bool {
        self.map.contains_key(key)
    }

    pub(super) fn len(&self) -> usize {
        self.map.len()
    }

    /// Puts `key` last, adding it if it is new.
    pub(super) fn insert(&mut self, key: String, value: V) {
        if self.map.insert(key.clone(), value).is_some() {
            self.order.retain(|k| *k != key);
        }
        self.order.push_back(key);
    }

    /// Marks `key` as the newest.
    pub(super) fn touch(&mut self, key: &str) {
        if self.map.contains_key(key) {
            self.order.retain(|k| k != key);
            self.order.push_back(key.to_owned());
        }
    }

    pub(super) fn remove(&mut self, key: &str) -> Option<V> {
        self.order.retain(|k| k != key);
        self.map.remove(key)
    }

    pub(super) fn pop_oldest(&mut self) -> Option<(String, V)> {
        let key = self.order.pop_front()?;
        self.map.remove(&key).map(|v| (key, v))
    }

    pub(super) fn clear(&mut self) {
        self.order.clear();
        self.map.clear();
    }
}
