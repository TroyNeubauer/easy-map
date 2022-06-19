fn main() {
    let r = {
        let map = easy_map::EasyMap::<usize, usize>::new();
        map.insert(0, 0);
        map.get(&0)
    };
    assert_eq!(r.as_deref(), Some(&0));
}
