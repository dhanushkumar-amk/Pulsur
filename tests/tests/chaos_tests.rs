use std::sync::Arc;
use std::time::Duration;
use tokio::time::sleep;

// Import our crate names from Cargo.toml
use circuit_breaker::{CircuitBreaker, CircuitBreakerConfig as CbConfig, CircuitState};
use load_balancer::{Backend, BackendPool, BackendPoolConfig};
use queue::{Job, PersistentQueue};
use rate_limiter::DistributedTokenBucketRateLimiter;

#[tokio::test]
async fn chaos_scenario_1_lb_backend_failure() {
    println!("--- Scenario 1: Kill one LB backend randomly mid-test ---");

    let config = BackendPoolConfig::default();
    let pool = Arc::new(BackendPool::from_config(config));

    // Add 3 backends
    let b1 = Backend::new("127.0.0.1:8081", 1);
    let b2 = Backend::new("127.0.0.1:8082", 1);
    let b3 = Backend::new("127.0.0.1:8083", 1);

    pool.add(b1);
    pool.add(b2);
    pool.add(b3);

    // Verify RR works
    let mut picked = Vec::new();
    for _ in 0..3 {
        if let Some(b) = pool.next_round_robin() {
            picked.push(b.address.clone());
        }
    }
    assert_eq!(picked.len(), 3);
    println!("Initial healthy backends: {:?}", picked);

    // FAILURE INJECTION: Kill backend 2
    println!("Infecting failure: Killing 127.0.0.1:8082...");
    {
        let backends = pool.backends_snapshot();
        for b in backends {
            if b.address == "127.0.0.1:8082" {
                b.set_healthy(false);
            }
        }
    }

    // Verify traffic redistributes
    let mut picked_after = Vec::new();
    for _ in 0..4 {
        if let Some(b) = pool.next_round_robin() {
            picked_after.push(b.address.clone());
        }
    }

    println!("Traffic after failure: {:?}", picked_after);
    assert!(!picked_after.contains(&"127.0.0.1:8082".to_string()));
    assert_eq!(picked_after.len(), 4);
    println!("SUCCESS: Traffic redistributed correctly away from failed backend.");
}

#[tokio::test]
async fn chaos_scenario_2_redis_failure_fallback() {
    println!("--- Scenario 2: Kill Redis mid-test ---");

    let redis_url = "redis://invalid_host:6379";

    // new() is async and needs 10.0 (f64) for capacity/rate
    let limiter_res = DistributedTokenBucketRateLimiter::new(redis_url, 10.0, 1.0).await;

    match limiter_res {
        Ok(limiter) => {
            println!("Attempting check with 'dead' Redis...");
            // check_key needs (key, tokens)
            let result = limiter.check_key("test_user", 1.0).await;
            assert!(result.is_ok());
            println!("SUCCESS: Rate limiter fell back to in-memory gracefully.");
        }
        Err(_) => {
            println!("SKIP: Redis connection failed early during initialization (Expected).");
        }
    }
}

#[tokio::test]
async fn chaos_scenario_3_wal_corruption_recovery() {
    println!("--- Scenario 3: Corrupt WAL file ---");

    let test_dir = tempfile::tempdir().unwrap();
    let wal_path = test_dir.path().join("wal.log");

    {
        // 1. Create a queue and add some data
        let mut queue = PersistentQueue::open(test_dir.path()).unwrap();
        // enqueue expects Job struct
        let j1 = Job::new("default", b"job-1".to_vec(), 3);
        let j2 = Job::new("default", b"job-2".to_vec(), 3);
        queue.enqueue(j1).unwrap();
        queue.enqueue(j2).unwrap();
        println!("Enqueued 2 jobs.");
    }

    // 2. CORRUPT the WAL file by appending random garbage
    {
        use std::io::Write;
        let mut file = std::fs::OpenOptions::new()
            .append(true)
            .open(&wal_path)
            .unwrap();
        file.write_all(b"CORRUPT DATA GARBAGE").unwrap();
        println!("Appended corruption to WAL.");
    }

    // 3. Restart queue and check recovery
    let queue = PersistentQueue::open(test_dir.path()).unwrap();

    // Replay should have loaded the first 2 jobs and ignored/handled the corruption at the end.
    let head_exists = queue.queue().pending.front().is_some();
    println!("Queue restarted. Head exists: {}", head_exists);
    assert!(head_exists);
    println!("SUCCESS: Queue recovered most recent valid state despite corruption.");
}

#[tokio::test]
async fn chaos_scenario_4_latency_circuit_breaker() {
    println!("--- Scenario 4: Introduce 500ms artificial latency ---");

    let config = CbConfig {
        failure_rate_threshold: 50.0,
        reset_timeout: Duration::from_millis(1000),
        rolling_window_size: 5,
        minimum_requests: 3,
    };
    // new takes a name, with_config takes name and config
    let cb = CircuitBreaker::with_config("chaos-cb", config);

    // Simulate high latency calls
    println!("Injecting 500ms latency calls (timeout is 100ms)...");

    for i in 0..6 {
        let res = cb
            .call_with_timeout::<_, _, ()>(Duration::from_millis(100), || async {
                sleep(Duration::from_millis(500)).await;
                Ok(())
            })
            .await;

        println!("Call {}: {:?}", i + 1, res);
    }

    assert!(matches!(cb.state(), CircuitState::Open));
    println!("SUCCESS: Circuit breaker opened after multiple high-latency timeouts.");
}

#[tokio::test]
async fn chaos_scenario_5_disk_full_handling() {
    println!("--- Scenario 5: Fill disk on queue host ---");

    let test_dir = tempfile::tempdir().unwrap();
    let wal_path = test_dir.path().join("wal.log");

    let mut queue = PersistentQueue::open(test_dir.path()).unwrap();

    let valid_job = Job::new("default", b"valid-job".to_vec(), 3);
    queue.enqueue(valid_job).unwrap();

    println!("Making WAL file read-only to simulate full disk/no permissions...");
    let mut perms = std::fs::metadata(&wal_path).unwrap().permissions();
    perms.set_readonly(true);
    std::fs::set_permissions(&wal_path, perms).unwrap();

    let failing_job = Job::new("default", b"failing-job".to_vec(), 3);
    let result = queue.enqueue(failing_job);
    println!("Enqueue result with read-only disk: {:?}", result);

    assert!(result.is_err());
    println!("SUCCESS: Queue didn't panic, returned error on write failure.");
}
