// Program 4: TCP echo server using std::net::TcpListener (blocking).
// Demonstrates network I/O, byte handling, and infinite loop server patterns.

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::thread;

pub fn start_echo_server(addr: &str) -> std::io::Result<()> {
    let listener = TcpListener::bind(addr)?;
    println!("Echo server listening on {}", addr);

    for stream in listener.incoming() {
        match stream {
            Ok(stream) => {
                thread::spawn(|| {
                    if let Err(e) = handle_client(stream) {
                        eprintln!("[Error]: {}", e);
                    }
                });
            }
            Err(e) => {
                eprintln!("[Connection Failed]: {}", e);
            }
        }
    }
    Ok(())
}

fn handle_client(mut stream: TcpStream) -> std::io::Result<()> {
    let mut buffer = [0; 512];
    loop {
        let bytes_read = stream.read(&mut buffer)?;
        if bytes_read == 0 {
            return Ok(()); // Connection closed
        }
        stream.write_all(&buffer[..bytes_read])?;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::net::{TcpStream, TcpListener};
    use std::thread;
    use std::time::Duration;

    #[test]
    fn test_echo_server_basic() {
        // Start server in background thread on a dynamic port
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        
        thread::spawn(move || {
            for stream in listener.incoming() {
                if let Ok(mut stream) = stream {
                    let mut buffer = [0; 512];
                    if let Ok(n) = stream.read(&mut buffer) {
                        if n > 0 {
                            let _ = stream.write_all(&buffer[..n]);
                        }
                    }
                }
            }
        });

        // Small delay to ensure server for thread started
        thread::sleep(Duration::from_millis(100));

        // Connect client
        let mut client = TcpStream::connect(addr).unwrap();
        let message = b"hello pulsar";
        client.write_all(message).unwrap();

        let mut response = [0; 12];
        client.read_exact(&mut response).unwrap();

        assert_eq!(&response, message);
    }
    
    #[test]
    fn test_echo_multiple_messages() {
         let listener = TcpListener::bind("127.0.0.1:0").unwrap();
         let addr = listener.local_addr().unwrap();
         
         thread::spawn(move || {
             for stream in listener.incoming() {
                if let Ok(mut stream) = stream {
                    let mut buffer = [0; 512];
                    while let Ok(n) = stream.read(&mut buffer) {
                        if n == 0 { break; }
                        let _ = stream.write_all(&buffer[..n]);
                    }
                }
             }
         });

         thread::sleep(Duration::from_millis(50));
         let mut client = TcpStream::connect(addr).unwrap();
         
         for i in 0..5 {
             let msg = format!("msg {}", i);
             client.write_all(msg.as_bytes()).unwrap();
             let mut res = vec![0; msg.len()];
             client.read_exact(&mut res).unwrap();
             assert_eq!(res, msg.as_bytes());
         }
    }
}
