#![cfg(loom)]
use std::sync::Arc;

use loom::thread;

#[test]
fn acquires_multiple() {
    loom::model(|| {
        let map1 = Arc::new(easy_map::EasyMap::<&'static str, u32>::with_capacity(1));
        let map2 = Arc::clone(&map1);
        let map3 = Arc::clone(&map1);
        let t1 = thread::spawn(move || {
            map1.insert("Troy", 5);
        });
        let t2 = thread::spawn(move || {
            map2.insert("Jane", 1);
        });
        /*let t3 = thread::spawn(move || {
            map3.insert("Troy", 5);
        });*/

        t1.join();
        t2.join();
        //t3.join();
    })
}
