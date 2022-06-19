use itertools::Itertools;
use std::collections::HashMap;
use std::ops::Deref;

use easy_map::EasyMap;

#[test]
fn normal_usage() {
    #[cfg(miri)]
    const BUCKETS: usize = 2;
    #[cfg(not(miri))]
    const BUCKETS: usize = 4;
    for i in 0..=BUCKETS {
        // 2^(2i) buckets
        let buckets = 1 << (i * 2);
        let map = EasyMap::<&'static str, u32>::with_capacity(buckets);
        let assert_same = |expected: &[(&str, u32)]| {
            let got = map.iter().collect_vec();
            let mut got = got.into_iter().map(|(k, v)| (*k, *v)).collect_vec();
            got.sort();
            let mut expected = Vec::from(expected);
            expected.sort();

            assert_eq!(got, expected);
        };
        assert_same(&[]);

        assert_eq!(map.get(&"Troy"), None);
        assert_eq!(map.get(&"Jane"), None);
        assert_eq!(map.get(&"David"), None);
        assert_eq!(map.get(&""), None);

        map.insert("Troy", 5);
        assert_same(&[("Troy", 5)]);

        assert_eq!(map.get(&"Troy").as_deref(), Some(&5));
        assert_eq!(map.get(&"Jane"), None);
        assert_eq!(map.get(&"David"), None);
        assert_eq!(map.get(&""), None);

        map.insert("Jane", 15);
        assert_same(&[("Jane", 15), ("Troy", 5)]);

        assert_eq!(map.get(&"Troy").as_deref(), Some(&5));
        assert_eq!(map.get(&"Jane").as_deref(), Some(&15));
        assert_eq!(map.get(&"David"), None);
        assert_eq!(map.get(&""), None);

        map.insert("David", 20);
        assert_same(&[("David", 20), ("Jane", 15), ("Troy", 5)]);

        assert_eq!(map.get(&"Troy").as_deref(), Some(&5));
        assert_eq!(map.get(&"Jane").as_deref(), Some(&15));
        assert_eq!(map.get(&"David").as_deref(), Some(&20));
        assert_eq!(map.get(&""), None);

        map.insert("Troy", 10);
        assert_same(&[("David", 20), ("Jane", 15), ("Troy", 10)]);

        assert_eq!(map.get(&"Troy").as_deref(), Some(&10));
        assert_eq!(map.get(&"Jane").as_deref(), Some(&15));
        assert_eq!(map.get(&"David").as_deref(), Some(&20));
        assert_eq!(map.get(&""), None);

        map.insert("Jane", 0);
        assert_same(&[("David", 20), ("Jane", 0), ("Troy", 10)]);

        assert_eq!(map.get(&"Troy").as_deref(), Some(&10));
        assert_eq!(map.get(&"Jane").as_deref(), Some(&0));
        assert_eq!(map.get(&"David").as_deref(), Some(&20));
        assert_eq!(map.get(&""), None);

        map.insert("David", 0);
        assert_same(&[("David", 0), ("Jane", 0), ("Troy", 10)]);

        assert_eq!(map.get(&"Troy").as_deref(), Some(&10));
        assert_eq!(map.get(&"Jane").as_deref(), Some(&0));
        assert_eq!(map.get(&"David").as_deref(), Some(&0));
        assert_eq!(map.get(&""), None);

        map.insert("Troy", 0);
        assert_same(&[("David", 0), ("Jane", 0), ("Troy", 0)]);

        assert_eq!(map.get(&"Troy").as_deref(), Some(&0));
        assert_eq!(map.get(&"Jane").as_deref(), Some(&0));
        assert_eq!(map.get(&"David").as_deref(), Some(&0));
        assert_eq!(map.get(&""), None);

        let node = map.remove(&"Troy").unwrap();
        assert_eq!(node.deref(), &0);
        assert_eq!(node.key(), &"Troy");
        assert_same(&[("David", 0), ("Jane", 0)]);

        assert!(map.remove(&"").is_none());
        assert!(map.remove(&"_").is_none());
        assert!(map.remove(&"Troy").is_none());

        let node = map.remove(&"David").unwrap();
        assert_eq!(node.deref(), &0);
        assert_eq!(node.key(), &"David");
        assert_same(&[("Jane", 0)]);

        assert!(map.remove(&"").is_none());
        assert!(map.remove(&"_").is_none());
        assert!(map.remove(&"Troy").is_none());
        assert!(map.remove(&"David").is_none());

        let node = map.remove(&"Jane").unwrap();
        assert_eq!(node.deref(), &0);
        assert_eq!(node.key(), &"Jane");
        assert_same(&[]);

        assert_eq!(map.get(&"Troy"), None);
        assert_eq!(map.get(&"Jane"), None);
        assert_eq!(map.get(&"David"), None);
        assert_eq!(map.get(&""), None);

        assert!(map.remove(&"").is_none());
        assert!(map.remove(&"_").is_none());
        assert!(map.remove(&"Troy").is_none());
        assert!(map.remove(&"David").is_none());
        assert!(map.remove(&"Jane").is_none());
    }
}

#[test]
fn same_as_std_hashmap() {
    use rand::{distributions::Alphanumeric, Rng, RngCore, SeedableRng};
    let mut rng = rand_chacha::ChaCha20Rng::seed_from_u64(0u64);

    #[cfg(miri)]
    const BUCKETS: usize = 2;
    #[cfg(not(miri))]
    const BUCKETS: usize = 4;
    for i in 0..=BUCKETS {
        // 2^(2i) buckets
        let buckets = 1 << (i * 2);
        let mut std = HashMap::new();
        let our_map = EasyMap::with_capacity(buckets);

        let mut keys = Vec::new();
        let mut extended_keys = Vec::new();

        #[cfg(miri)]
        const KEYS: usize = 4;
        #[cfg(not(miri))]
        const KEYS: usize = 120;

        for _ in 0..KEYS {
            keys.push(
                (&mut rng)
                    .sample_iter(&Alphanumeric)
                    .take(10)
                    .map(char::from)
                    .collect::<String>(),
            )
        }
        extended_keys.extend(keys.clone());
        for _ in 0..(KEYS / 2) {
            extended_keys.push(
                (&mut rng)
                    .sample_iter(&Alphanumeric)
                    .take(10)
                    .map(char::from)
                    .collect::<String>(),
            )
        }
        #[cfg(miri)]
        const ITERATIONS: usize = 10;
        #[cfg(not(miri))]
        const ITERATIONS: usize = 256;
        // Do random operations and make sure we match std's HashMap at each step
        for _ in 0..ITERATIONS {
            match rng.next_u64() % 2 {
                0 => {
                    let key = keys[rng.next_u64() as usize % keys.len()].clone();
                    let value = rng.next_u64();
                    let theirs = std.insert(key.clone(), value);
                    let ours = our_map.insert(key, value).map(|o| *o.deref());
                    assert_eq!(ours, theirs);
                }
                1 => {
                    let key = extended_keys[rng.next_u64() as usize % extended_keys.len()].clone();
                    let theirs = std.remove(&key);
                    let ours = our_map.remove(&key).map(|o| *o.deref());
                    assert_eq!(ours, theirs);
                }
                2 => {
                    let key = extended_keys[rng.next_u64() as usize % extended_keys.len()].clone();
                    let theirs = std.get(&key);
                    let ours = our_map.get(&key);
                    assert_eq!(theirs, ours.as_deref());
                }
                _ => unreachable!(),
            }

            let mut ours: Vec<_> = our_map.iter().collect();
            let mut theirs: Vec<_> = std.iter().collect();
            ours.sort();
            theirs.sort();
            assert_eq!(ours, theirs);
        }
    }
}
