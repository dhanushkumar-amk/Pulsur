use std::sync::Arc;
use std::time::Instant;

use futures::future::FutureExt;
use http_server::{
    parse_request, send_response, AsyncStream, Method, Request, Response, Router,
};
use tokio::io::AsyncWriteExt;

fn build_router() -> Router {
    let mut router = Router::new();
    let handler = Arc::new(|_req: Request| async move { Response::new(200) }.boxed());

    router.add_http(Method::GET, "/", handler.clone());
    router.add_http(Method::GET, "/health", handler.clone());
    router.add_http(Method::GET, "/users/:id", handler.clone());
    router.add_http(Method::GET, "/teams/:team_id/members/:member_id", handler);
    router
}

fn print_result(name: &str, iterations: usize, started_at: Instant) {
    let elapsed = started_at.elapsed();
    let nanos_per_iter = elapsed.as_nanos() / iterations as u128;
    let throughput = iterations as f64 / elapsed.as_secs_f64();

    println!(
        "{name}: {iterations} iterations in {:?} | {} ns/iter | {:.0} ops/sec",
        elapsed, nanos_per_iter, throughput
    );
}

#[test]
#[ignore = "Run manually for benchmarking: cargo test -p http_server --test benchmark -- --ignored --nocapture"]
fn benchmark_router_match_route() {
    let router = build_router();
    let iterations = 250_000usize;
    let started_at = Instant::now();

    for i in 0..iterations {
        let path = if i % 2 == 0 {
            "/teams/red/members/42"
        } else {
            "/users/42"
        };
        let result = router.match_route(&Method::GET, path);
        assert!(result.is_some());
    }

    print_result("router.match_route", iterations, started_at);
}

#[tokio::test]
#[ignore = "Run manually for benchmarking: cargo test -p http_server --test benchmark -- --ignored --nocapture"]
async fn benchmark_parse_request() {
    let iterations = 10_000usize;
    let request_bytes = b"POST /users/42 HTTP/1.1\r\nHost: localhost\r\nContent-Type: application/json\r\nContent-Length: 27\r\nConnection: keep-alive\r\n\r\n{\"name\":\"ferrum\",\"ok\":true}";
    let started_at = Instant::now();

    for _ in 0..iterations {
        let (mut writer, reader) = tokio::io::duplex(4096);
        writer.write_all(request_bytes).await.unwrap();
        writer.shutdown().await.unwrap();

        let mut stream: Box<dyn AsyncStream> = Box::new(reader);
        let request = parse_request(&mut stream).await.unwrap();
        assert_eq!(request.method, Method::POST);
        assert_eq!(request.path, "/users/42");
        assert_eq!(request.body.len(), 27);
    }

    print_result("parse_request", iterations, started_at);
}

#[tokio::test]
#[ignore = "Run manually for benchmarking: cargo test -p http_server --test benchmark -- --ignored --nocapture"]
async fn benchmark_send_response() {
    let iterations = 10_000usize;
    let started_at = Instant::now();

    for _ in 0..iterations {
        let (mut client, server) = tokio::io::duplex(4096);
        let mut response = Response::new(200);
        response
            .headers
            .insert("Content-Type".to_string(), "application/json".to_string());
        response.body = br#"{"message":"Ferrum Engine Online","version":"0.7.0"}"#.to_vec();

        let write_task = tokio::spawn(async move {
            let mut stream: Box<dyn AsyncStream> = Box::new(server);
            send_response(&mut stream, response).await.unwrap();
        });

        let mut buf = Vec::new();
        tokio::io::AsyncReadExt::read_to_end(&mut client, &mut buf)
            .await
            .unwrap();
        write_task.await.unwrap();

        assert!(buf.starts_with(b"HTTP/1.1 200 OK\r\n"));
    }

    print_result("send_response", iterations, started_at);
}
