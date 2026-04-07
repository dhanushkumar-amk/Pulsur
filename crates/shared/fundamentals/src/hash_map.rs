// Program 3: HashMap from scratch using array of linked lists (chaining).
// Demonstrates hashing, array manipulation, and collision handling.

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};

#[derive(Debug)]
struct Bucket<K, V> {
    entries: Vec<(K, V)>,
}

#[derive(Debug)]
pub struct SimpleHashMap<K, V> {
    buckets: Vec<Bucket<K, V>>,
    capacity: usize,
    size: usize,
}

impl<K: Hash + Eq + Clone, V: Clone> SimpleHashMap<K, V> {
    pub fn new(capacity: usize) -> Self {
        let mut buckets = Vec::with_capacity(capacity);
        for _ in 0..capacity {
            buckets.push(Bucket {
                entries: Vec::new(),
            });
        }
        Self {
            buckets,
            capacity,
            size: 0,
        }
    }

    fn hash_key(&self, key: &K) -> usize {
        let mut hasher = DefaultHasher::new();
        key.hash(&mut hasher);
        (hasher.finish() as usize) % self.capacity
    }

    pub fn insert(&mut self, key: K, value: V) -> Option<V> {
        let bucket_idx = self.hash_key(&key);
        let bucket = &mut self.buckets[bucket_idx];

        for entry in &mut bucket.entries {
            if entry.0 == key {
                let old_value = std::mem::replace(&mut entry.1, value);
                return Some(old_value);
            }
        }

        bucket.entries.push((key, value));
        self.size += 1;
        None
    }

    pub fn get(&self, key: &K) -> Option<&V> {
        let bucket_idx = self.hash_key(key);
        let bucket = &self.buckets[bucket_idx];

        for entry in &bucket.entries {
            if entry.0 == *key {
                return Some(&entry.1);
            }
        }
        None
    }

    pub fn remove(&mut self, key: &K) -> Option<V> {
        let bucket_idx = self.hash_key(key);
        let bucket = &mut self.buckets[bucket_idx];

        if let Some(pos) = bucket.entries.iter().position(|entry| entry.0 == *key) {
            self.size -= 1;
            return Some(bucket.entries.remove(pos).1);
        }
        None
    }

    pub fn len(&self) -> usize {
        self.size
    }

    pub fn is_empty(&self) -> bool {
        self.size == 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_hashmap_basic() {
        let mut map = SimpleHashMap::new(10);
        map.insert("key1".to_string(), 100);
        map.insert("key2".to_string(), 200);

        assert_eq!(map.len(), 2);
        assert_eq!(map.get(&"key1".to_string()), Some(&100));
        assert_eq!(map.get(&"key2".to_string()), Some(&200));
    }

    #[test]
    fn test_hashmap_update() {
        let mut map = SimpleHashMap::new(10);
        map.insert("key1", 100);
        map.insert("key1", 500);
        assert_eq!(map.get(&"key1"), Some(&500));
        assert_eq!(map.len(), 1);
    }

    #[test]
    fn test_hashmap_remove() {
        let mut map = SimpleHashMap::new(10);
        map.insert("key1", 42);
        assert_eq!(map.remove(&"key1"), Some(42));
        assert_eq!(map.len(), 0);
        assert_eq!(map.get(&"key1"), None);
    }

    #[test]
    fn test_hashmap_collision() {
        // Force collision with small capacity
        let mut map = SimpleHashMap::new(1);
        map.insert("a", 1);
        map.insert("b", 2);
        assert_eq!(map.len(), 2);
        assert_eq!(map.get(&"a"), Some(&1));
        assert_eq!(map.get(&"b"), Some(&2));
    }

    #[test]
    fn test_hashmap_non_existent() {
        let map: SimpleHashMap<i32, i32> = SimpleHashMap::new(10);
        assert_eq!(map.get(&99), None);
        assert_eq!(map.len(), 0);
    }
}
