use std::borrow::Borrow;
use std::collections::HashMap;
use std::hash::{Hash, Hasher};

struct RawItem<K1, K2, V>(*mut (K1, K2, V));
unsafe impl<K1, K2, V> Send for RawItem<K1, K2, V> {}
unsafe impl<K1, K2, V> Sync for RawItem<K1, K2, V> {}

/// MultiMap is a hash map that can index an item by two keys
/// For example, after an item with key (a, b) is insert, `map.get1(a)` and
/// `map.get2(b)` both returns the item. Likewise the `remove1` and `remove2`.
pub struct MultiMap<K1, K2, V> {
    map1: HashMap<Key<K1>, RawItem<K1, K2, V>>,
    map2: HashMap<Key<K2>, RawItem<K1, K2, V>>,
}

struct Key<T>(*const T);

unsafe impl<T> Send for Key<T> {}
unsafe impl<T> Sync for Key<T> {}

impl<T> Borrow<T> for Key<T> {
    fn borrow(&self) -> &T {
        unsafe { &*self.0 }
    }
}

impl<T: Hash> Hash for Key<T> {
    fn hash<H: Hasher>(&self, state: &mut H) {
        (self.borrow() as &T).hash(state)
    }
}

impl<T: PartialEq> PartialEq for Key<T> {
    fn eq(&self, other: &Self) -> bool {
        (self.borrow() as &T).eq(other.borrow())
    }
}

impl<T: Eq> Eq for Key<T> {}

impl<K1, K2, V> MultiMap<K1, K2, V> {
    pub fn new() -> Self {
        MultiMap {
            map1: HashMap::new(),
            map2: HashMap::new(),
        }
    }
}

#[allow(dead_code)]
impl<K1, K2, V> MultiMap<K1, K2, V>
where
    K1: Hash + Eq + Send,
    K2: Hash + Eq + Send,
    V: Send,
{
    pub fn insert(&mut self, k1: K1, k2: K2, v: V) -> Result<(), (K1, K2, V)> {
        if self.map1.contains_key(&k1) || self.map2.contains_key(&k2) {
            return Err((k1, k2, v));
        }
        let item = Box::new((k1, k2, v));
        let k1 = Key(&item.0);
        let k2 = Key(&item.1);
        let item = Box::into_raw(item);
        self.map1.insert(k1, RawItem(item));
        self.map2.insert(k2, RawItem(item));
        Ok(())
    }

    pub fn get1(&self, k1: &K1) -> Option<&V> {
        let item = self.map1.get(k1)?;
        let item = unsafe { &*item.0 };
        Some(&item.2)
    }

    pub fn get1_mut(&mut self, k1: &K1) -> Option<&mut V> {
        let item = self.map1.get(k1)?;
        let item = unsafe { &mut *item.0 };
        Some(&mut item.2)
    }

    pub fn get2(&self, k2: &K2) -> Option<&V> {
        let item = self.map2.get(k2)?;
        let item = unsafe { &*item.0 };
        Some(&item.2)
    }

    pub fn get_mut2(&mut self, k2: &K2) -> Option<&mut V> {
        let item = self.map2.get(k2)?;
        let item = unsafe { &mut *item.0 };
        Some(&mut item.2)
    }

    pub fn remove1(&mut self, k1: &K1) -> Option<V> {
        let item = self.map1.remove(k1)?;
        let item = unsafe { Box::from_raw(item.0) };
        self.map2.remove(&item.1);
        Some(item.2)
    }

    pub fn remove2(&mut self, k2: &K2) -> Option<V> {
        let item = self.map2.remove(k2)?;
        let item = unsafe { Box::from_raw(item.0) };
        self.map1.remove(&item.0);
        Some(item.2)
    }
}

impl<K1, K2, V> Drop for MultiMap<K1, K2, V> {
    fn drop(&mut self) {
        self.map1.clear();
        self.map2
            .drain()
            .for_each(|(_, item)| drop(unsafe { Box::from_raw(item.0) }));
    }
}
