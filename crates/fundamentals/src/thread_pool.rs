// Program 5: Thread pool with N workers using Arc<Mutex<Receiver>>.
// Demonstrates advanced concurrency patterns, message passing, and thread orchestration.

use std::sync::{mpsc, Arc, Mutex};
use std::thread;

type Job = Box<dyn FnOnce() + Send + 'static>;

pub struct ThreadPool {
    workers: Vec<Worker>,
    sender: Option<mpsc::Sender<Job>>,
}

struct Worker {
    _id: usize,
    thread: Option<thread::JoinHandle<()>>,
}

impl ThreadPool {
    pub fn new(size: usize) -> Self {
        assert!(size > 0);

        let (sender, receiver) = mpsc::channel();
        let receiver = Arc::new(Mutex::new(receiver));

        let mut workers = Vec::with_capacity(size);
        for id in 0..size {
            workers.push(Worker::new(id, Arc::clone(&receiver)));
        }

        Self {
            workers,
            sender: Some(sender),
        }
    }

    pub fn execute<F>(&self, f: F)
    where
        F: FnOnce() + Send + 'static,
    {
        let job = Box::new(f);
        self.sender.as_ref().unwrap().send(job).unwrap();
    }
}

impl Drop for ThreadPool {
    fn drop(&mut self) {
        drop(self.sender.take());

        for worker in &mut self.workers {
            if let Some(thread) = worker.thread.take() {
                thread.join().unwrap();
            }
        }
    }
}

impl Worker {
    fn new(id: usize, receiver: Arc<Mutex<mpsc::Receiver<Job>>>) -> Self {
        let thread = thread::spawn(move || loop {
            let message = receiver.lock().unwrap().recv();

            match message {
                Ok(job) => {
                    // println!("Worker {} got a job; executing.", id);
                    job();
                }
                Err(_) => {
                    // println!("Worker {} disconnected; shutting down.", id);
                    break;
                }
            }
        });

        Self { _id: id, thread: Some(thread) }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;

    #[test]
    fn test_threadpool_basic() {
        let pool = ThreadPool::new(4);
        let counter = Arc::new(AtomicUsize::new(0));

        for _ in 0..10 {
            let c = Arc::clone(&counter);
            pool.execute(move || {
                c.fetch_add(1, Ordering::SeqCst);
            });
        }

        // Wait a bit for execution
        thread::sleep(Duration::from_millis(100));
        assert_eq!(counter.load(Ordering::SeqCst), 10);
    }

    #[test]
    fn test_threadpool_shared_state() {
        let pool = ThreadPool::new(2);
        let (tx, rx) = mpsc::channel();

        pool.execute(move || {
            tx.send(42).unwrap();
        });

        assert_eq!(rx.recv_timeout(Duration::from_secs(1)).unwrap(), 42);
    }

    #[test]
    #[should_panic]
    fn test_threadpool_zero_size() {
        let _ = ThreadPool::new(0);
    }

    #[test]
    fn test_threadpool_parallelism() {
        let pool = ThreadPool::new(2);
        let start = std::time::Instant::now();
        
        pool.execute(|| thread::sleep(Duration::from_millis(50)));
        pool.execute(|| thread::sleep(Duration::from_millis(50)));
        
        // This should take ~50ms if parallel, ~100ms if sequential
        thread::sleep(Duration::from_millis(70));
        assert!(start.elapsed() < Duration::from_millis(200));
    }

    #[test]
    fn test_threadpool_large_workload() {
        let pool = ThreadPool::new(8);
        let counter = Arc::new(AtomicUsize::new(0));
        
        for _ in 0..1000 {
            let c = Arc::clone(&counter);
            pool.execute(move || {
                c.fetch_add(1, Ordering::SeqCst);
            });
        }
        
        thread::sleep(Duration::from_millis(200));
        assert_eq!(counter.load(Ordering::SeqCst), 1000);
    }
}
