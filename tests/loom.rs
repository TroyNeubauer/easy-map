#![cfg(loom)]
use std::sync::Arc;

use loom::thread;

#[test]
fn acquires_multiple() {
    loom::model(|| {
        println!("=====================");
        println!("MODEL START");
        println!("=====================");
        let map = Arc::new(easy_map::EasyMap::<&'static str, u32>::with_capacity(1));
        let map1 = Arc::clone(&map);
        let map2 = Arc::clone(&map);
        let map3 = Arc::clone(&map);
        let t1 = thread::spawn(move || {
            map1.insert("Troy", 5);
        });
        let t2 = thread::spawn(move || {
            map2.insert("Jane", 1);
        });
        /*let t3 = thread::spawn(move || {
            map3.insert("Troy", 5);
        });*/

        t1.join().unwrap();
        t2.join().unwrap();
        let assert_same = |expected: &[(&str, u32)]| {
            use itertools::Itertools;
            let got = map.iter().collect_vec();
            let mut got = got.into_iter().map(|(k, v)| (*k, *v)).collect_vec();
            got.sort();
            let mut expected = Vec::from(expected);
            expected.sort();

            assert_eq!(got, expected);
        };
        assert_same(&[("Troy", 5), ("Jane", 1)]);
        //t3.join();
    })
}
