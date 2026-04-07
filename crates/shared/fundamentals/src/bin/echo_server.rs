// TCP Echo Server Launcher
use fundamentals::echo_server;

fn main() {
    println!("--- 🏗️ Starting Pulsar Echo Server... 🏗️ ---");
    let addr = "127.0.0.1:8080";
    println!("--- 🛰️  Listening on http://{} ---", addr);

    if let Err(e) = echo_server::start_echo_server(addr) {
        eprintln!("Failed to start echo server: {}", e);
    }
}
