//! Interned IRIs: a `Copy` handle that compares and hashes as a `u32` and
//! resolves to a `&'static str`.
//!
//! The interner is global and append-only, so memory grows only with the
//! number of distinct strings ever seen. Interning takes one of 16 sharded
//! locks; resolving a handle takes no lock (two acquire loads into an
//! append-only slot table). Lookups that must not intern use [`Iri::get`].

use rustc_hash::{FxHashMap, FxHasher};
use std::borrow::Cow;
use std::fmt;
use std::hash::{Hash, Hasher};
use std::num::NonZeroU32;
use std::ops::{Deref, DerefMut};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Mutex, OnceLock};

/// An interned IRI (or another short, often repeated string such as a prefix
/// name or a language tag). Equal strings always get the same handle.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct Iri(NonZeroU32);

const SHARDS: usize = 16;
/// Slot table segment `k` holds `FIRST << k` slots.
const FIRST: u64 = 1024;
const SEGMENTS: usize = 23;
/// Strings are copied into leaked chunks of this size (longer ones get their own).
const CHUNK: usize = 64 * 1024;

struct Shard {
    map: FxHashMap<&'static str, Iri>,
    free: &'static mut [u8],
}

struct Interner {
    shards: [Mutex<Shard>; SHARDS],
    next: AtomicU32,
    segments: [OnceLock<Box<[OnceLock<&'static str>]>>; SEGMENTS],
}

fn interner() -> &'static Interner {
    static I: OnceLock<Interner> = OnceLock::new();
    I.get_or_init(|| Interner {
        shards: std::array::from_fn(|_| {
            Mutex::new(Shard {
                map: FxHashMap::default(),
                free: &mut [],
            })
        }),
        next: AtomicU32::new(0),
        segments: std::array::from_fn(|_| OnceLock::new()),
    })
}

fn shard_of(s: &str) -> usize {
    let mut h = FxHasher::default();
    s.hash(&mut h);
    (h.finish() >> 32) as usize % SHARDS
}

/// Segment and offset of slot `i`: segment k starts at FIRST * (2^k - 1).
fn slot_of(i: u32) -> (usize, usize) {
    let j = u64::from(i) / FIRST + 1;
    let k = 63 - j.leading_zeros() as usize;
    (k, (u64::from(i) - FIRST * ((1 << k) - 1)) as usize)
}

impl Interner {
    fn slot(&self, i: u32) -> &OnceLock<&'static str> {
        let (k, off) = slot_of(i);
        let seg = self.segments[k].get_or_init(|| {
            (0..FIRST << k)
                .map(|_| OnceLock::new())
                .collect::<Vec<_>>()
                .into_boxed_slice()
        });
        &seg[off]
    }
}

fn store(free: &mut &'static mut [u8], s: &str) -> &'static str {
    let n = s.len();
    if n > CHUNK / 4 {
        return Box::leak(Box::from(s));
    }
    if free.len() < n {
        *free = Box::leak(vec![0u8; CHUNK].into_boxed_slice());
    }
    let (head, tail) = std::mem::take(free).split_at_mut(n);
    *free = tail;
    head.copy_from_slice(s.as_bytes());
    std::str::from_utf8(head).expect("copied from a str")
}

impl Iri {
    /// The handle for `s`, interning it if it is new.
    pub fn intern(s: &str) -> Iri {
        let int = interner();
        let mut shard = int.shards[shard_of(s)]
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        if let Some(&iri) = shard.map.get(s) {
            return iri;
        }
        let i = int.next.fetch_add(1, Ordering::Relaxed);
        let stored = store(&mut shard.free, s);
        // The slot is written before the handle escapes the shard lock.
        let _ = int.slot(i).set(stored);
        let iri = Iri(NonZeroU32::new(i + 1).expect("interner overflow"));
        shard.map.insert(stored, iri);
        iri
    }

    /// The handle's number: a total order that costs no text comparison
    /// (by first interning, not by text, so it differs between processes).
    pub(crate) fn id(self) -> u32 {
        self.0.get()
    }

    /// The handle for `s` if it was ever interned. Never interns.
    pub(crate) fn get(s: &str) -> Option<Iri> {
        let shard = interner().shards[shard_of(s)]
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        shard.map.get(s).copied()
    }

    pub fn as_str(self) -> &'static str {
        interner()
            .slot(self.0.get() - 1)
            .get()
            .expect("handles are only created after their slot is set")
    }
}

impl Deref for Iri {
    type Target = str;
    fn deref(&self) -> &str {
        self.as_str()
    }
}

impl AsRef<str> for Iri {
    fn as_ref(&self) -> &str {
        self.as_str()
    }
}

impl From<&str> for Iri {
    fn from(s: &str) -> Iri {
        Iri::intern(s)
    }
}

impl From<&String> for Iri {
    fn from(s: &String) -> Iri {
        Iri::intern(s)
    }
}

impl From<String> for Iri {
    fn from(s: String) -> Iri {
        Iri::intern(&s)
    }
}

impl From<Cow<'_, str>> for Iri {
    fn from(s: Cow<'_, str>) -> Iri {
        Iri::intern(&s)
    }
}

impl From<Iri> for String {
    fn from(i: Iri) -> String {
        i.as_str().to_string()
    }
}

impl fmt::Display for Iri {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl fmt::Debug for Iri {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(self.as_str(), f)
    }
}

/// Ordered by text, so sorted output reads alphabetically.
impl Ord for Iri {
    fn cmp(&self, other: &Iri) -> std::cmp::Ordering {
        if self == other {
            return std::cmp::Ordering::Equal;
        }
        self.as_str().cmp(other.as_str())
    }
}

impl PartialOrd for Iri {
    fn partial_cmp(&self, other: &Iri) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

macro_rules! str_eq {
    ($($t:ty),*) => {$(
        impl PartialEq<$t> for Iri {
            fn eq(&self, other: &$t) -> bool {
                self.as_str() == AsRef::<str>::as_ref(other)
            }
        }
        impl PartialEq<Iri> for $t {
            fn eq(&self, other: &Iri) -> bool {
                AsRef::<str>::as_ref(self) == other.as_str()
            }
        }
    )*};
}
str_eq!(str, &str, String);

impl serde::Serialize for Iri {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(self.as_str())
    }
}

/// Anything an [`IriMap`] can be queried with: a handle, or text (looked up
/// without interning).
pub(crate) trait IriKey {
    fn iri_key(&self) -> Option<Iri>;
}

impl IriKey for Iri {
    fn iri_key(&self) -> Option<Iri> {
        Some(*self)
    }
}

impl IriKey for str {
    fn iri_key(&self) -> Option<Iri> {
        Iri::get(self)
    }
}

impl IriKey for String {
    fn iri_key(&self) -> Option<Iri> {
        Iri::get(self)
    }
}

impl<T: IriKey + ?Sized> IriKey for &T {
    fn iri_key(&self) -> Option<Iri> {
        (**self).iri_key()
    }
}

/// A map keyed by [`Iri`] that can also be queried with `&str`.
#[derive(Clone, Debug)]
pub(crate) struct IriMap<V>(FxHashMap<Iri, V>);

impl<V> Default for IriMap<V> {
    fn default() -> Self {
        IriMap(FxHashMap::default())
    }
}

impl<V> IriMap<V> {
    pub(crate) fn get<K: IriKey + ?Sized>(&self, key: &K) -> Option<&V> {
        self.0.get(&key.iri_key()?)
    }
    pub(crate) fn contains_key<K: IriKey + ?Sized>(&self, key: &K) -> bool {
        self.get(key).is_some()
    }
}

impl<V> Deref for IriMap<V> {
    type Target = FxHashMap<Iri, V>;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl<V> DerefMut for IriMap<V> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

impl<'a, V> IntoIterator for &'a IriMap<V> {
    type Item = (&'a Iri, &'a V);
    type IntoIter = std::collections::hash_map::Iter<'a, Iri, V>;
    fn into_iter(self) -> Self::IntoIter {
        self.0.iter()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn interning_is_stable_and_lookups_do_not_intern() {
        let a = Iri::intern("http://example.org/interned-a");
        assert_eq!(
            a,
            Iri::intern(&String::from("http://example.org/interned-a"))
        );
        assert_eq!(a, "http://example.org/interned-a");
        assert_eq!(a.as_str(), "http://example.org/interned-a");
        assert!(a.starts_with("http://"));
        assert_eq!(Iri::get("http://example.org/interned-a"), Some(a));
        assert_eq!(Iri::get("http://example.org/never-interned"), None);
        assert_eq!(Iri::get("http://example.org/never-interned"), None);
        let b = Iri::intern("http://example.org/interned-b");
        assert!(a < b);
        let mut m: IriMap<u32> = IriMap::default();
        m.insert(a, 1);
        assert_eq!(m.get("http://example.org/interned-a"), Some(&1));
        assert_eq!(m.get(&a), Some(&1));
        assert!(!m.contains_key("http://example.org/never-interned"));
    }

    #[test]
    fn slots_cover_segment_boundaries() {
        assert_eq!(slot_of(0), (0, 0));
        assert_eq!(slot_of(1023), (0, 1023));
        assert_eq!(slot_of(1024), (1, 0));
        assert_eq!(slot_of(3071), (1, 2047));
        assert_eq!(slot_of(3072), (2, 0));
        assert_eq!(slot_of(u32::MAX).0, SEGMENTS - 1);
    }

    #[test]
    fn concurrent_interning_agrees() {
        let words: Vec<String> = (0..5000).map(|i| format!("http://e/c{i}")).collect();
        let runs: Vec<Vec<Iri>> = std::thread::scope(|s| {
            let hs: Vec<_> = (0..4)
                .map(|_| s.spawn(|| words.iter().map(|w| Iri::intern(w)).collect()))
                .collect();
            hs.into_iter().map(|h| h.join().unwrap()).collect()
        });
        for r in &runs[1..] {
            assert_eq!(r, &runs[0]);
        }
        for (w, i) in words.iter().zip(&runs[0]) {
            assert_eq!(i.as_str(), w);
        }
    }
}
