// A simple CLI to run all Phase 1 fundamental programs!
use fundamentals::{LinkedList, Stack, SimpleHashMap, ThreadPool};
use std::sync::{Arc, Mutex};

fn main() {
    println!("--- 🚀 Pulsar Fundamental Labs 🚀 ---");

    // 1. Linked List
    let mut list = LinkedList::new();
    list.push_front("Pulsar");
    println!("Lab 1: LinkedList -> Peeked result: {:?}", list.peek_front());

    // 2. Stack
    let mut stack = Stack::new();
    stack.push(42);
    println!("Lab 2: Stack -> Popped result: {:?}", stack.pop());

    // 3. HashMap
    let mut map = SimpleHashMap::new(10);
    map.insert("author", "@dhanush");
    println!("Lab 3: HashMap -> Key 'author' is: {:?}", map.get(&"author"));

    // 4. Thread Pool
    let pool = ThreadPool::new(4);
    let result = Arc::new(Mutex::new(0));
    for _ in 0..4 {
        let r = Arc::clone(&result);
        pool.execute(move || {
            let mut val = r.lock().unwrap();
            *val += 1;
        });
    }
    std::thread::sleep(std::time::Duration::from_millis(50));
    println!("Lab 4: Thread Pool -> Result after 4 jobs: {:?}", *result.lock().unwrap());

    println!("--- ✅ All Labs Running Successfully! ✅ ---");
}
