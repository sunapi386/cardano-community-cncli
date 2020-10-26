use std::io::{Read, Write};
use std::net::{Shutdown, SocketAddr, TcpStream, ToSocketAddrs};
use std::thread::sleep;
use std::time::{Duration, Instant};

use byteorder::{ByteOrder, NetworkEndian, WriteBytesExt};

use crate::nodeclient::protocols::{Agency, handshake_protocol, MiniProtocol, Protocol, transaction_protocol};
use crate::nodeclient::protocols::handshake_protocol::HandshakeProtocol;
use crate::nodeclient::protocols::transaction_protocol::TxSubmissionProtocol;

// Sync a cardano-node database
//
// Connect to cardano-node and run chain-sync protocol to sync block headers
pub fn sync(db: &std::path::PathBuf, host: &String, port: u16, network_magic: u32) {
    //println!("SYNC db: {:?}, host: {:?}, port: {:?}, network_magic: {:?}", db, host, port, network_magic);
    let start_time = Instant::now();

    let mut protocols: Vec<MiniProtocol> = vec![
        MiniProtocol::Handshake(
            HandshakeProtocol {
                state: handshake_protocol::State::Propose,
                network_magic,
                result: None,
            }
        )
    ];

    // continually retry connection
    loop {
        println!("Connecting to {}:{} ...", host, port);

        match (host.as_str(), port).to_socket_addrs() {
            Ok(mut into_iter) => {
                if into_iter.len() > 0 {
                    let socket_addr: SocketAddr = into_iter.nth(0).unwrap();
                    match TcpStream::connect_timeout(&socket_addr, Duration::from_secs(1)) {
                        Ok(mut stream) => {
                            loop {
                                // Try sending some data
                                for protocol in protocols.iter_mut() {
                                    match protocol.send_data() {
                                        Some(send_payload) => {
                                            let mut message: Vec<u8> = Vec::new();
                                            message.write_u32::<NetworkEndian>(timestamp(&start_time)).unwrap();
                                            message.write_u16::<NetworkEndian>(protocol.protocol_id()).unwrap();
                                            message.write_u16::<NetworkEndian>(send_payload.len() as u16).unwrap();
                                            message.write(&send_payload[..]).unwrap();
                                            // println!("sending: {}", hex::encode(&message));
                                            stream.write(&message).unwrap();
                                            break;
                                        }
                                        None => {}
                                    }
                                }

                                // only read from the server if no protocols have client agency and
                                // at least one has Server agency
                                let should_read_from_server =
                                    !protocols.iter().any(|protocol| protocol.get_agency() == Agency::Client)
                                        && protocols.iter().any(|protocol| protocol.get_agency() == Agency::Server);

                                if should_read_from_server {
                                    // try receiving some data
                                    let mut message_header = [0u8; 8]; // read 8 bytes to start with
                                    match stream.read_exact(&mut message_header) {
                                        Ok(_) => {
                                            let _server_timestamp = NetworkEndian::read_u32(&mut message_header[0..4]);
                                            // println!("server_timestamp: {:x}", server_timestamp);
                                            let protocol_id = NetworkEndian::read_u16(&mut message_header[4..6]);
                                            // println!("protocol_id: {:x}", protocol_id);
                                            let payload_length = NetworkEndian::read_u16(&mut message_header[6..]) as usize;
                                            // println!("payload_length: {:x}", payload_length);
                                            let mut payload = vec![0u8; payload_length];
                                            match stream.read_exact(&mut payload) {
                                                Ok(_) => {
                                                    // Find the protocol to handle the message
                                                    for protocol in protocols.iter_mut() {
                                                        if protocol_id == (protocol.protocol_id() | 0x8000u16) {
                                                            // println!("receive_data: {}", hex::encode(&payload));
                                                            protocol.receive_data(payload);
                                                            break;
                                                        }
                                                    }
                                                }
                                                Err(e) => {
                                                    println!("Unable to read response payload! {}", e)
                                                }
                                            }
                                        }
                                        Err(e) => {
                                            println!("Unable to read response header! {}", e)
                                        }
                                    }
                                }

                                let mut protocols_to_add: Vec<MiniProtocol> = Vec::new();
                                // Remove any protocols that have a result (are done)
                                protocols.retain(|protocol| {
                                    match protocol {
                                        MiniProtocol::Handshake(handshake_protocol) => {
                                            match handshake_protocol.result.as_ref() {
                                                Some(protocol_result) => {
                                                    match protocol_result {
                                                        Ok(message) => {
                                                            println!("HandshakeProtocol Result: {}", message);

                                                            // handshake succeeded. Add other protocols
                                                            protocols_to_add.push(
                                                                MiniProtocol::TxSubmission(
                                                                    TxSubmissionProtocol { state: transaction_protocol::State::Idle, result: None }
                                                                )
                                                            );
                                                        }
                                                        Err(error) => {
                                                            println!("HandshakeProtocol Error: {}", error);
                                                        }
                                                    }
                                                    false
                                                }
                                                None => { true }
                                            }
                                        }
                                        MiniProtocol::TxSubmission(tx_submission_protocol) => {
                                            match tx_submission_protocol.result.as_ref() {
                                                Some(protocol_result) => {
                                                    match protocol_result {
                                                        Ok(message) => {
                                                            println!("TxSubmissionProtocol Result: {}", message);
                                                        }
                                                        Err(error) => {
                                                            println!("TxSubmissionProtocol Error: {}", error);
                                                        }
                                                    }
                                                    false
                                                }
                                                None => { true }
                                            }
                                        }
                                    }
                                });

                                protocols.append(&mut protocols_to_add);

                                if protocols.is_empty() {
                                    println!("No more active protocols, exiting...");
                                    return;
                                }
                            }
                        }
                        Err(e) => {
                            println!("Failed to connect: {}", e);
                        }
                    }
                } else {
                    println!("No IP addresses found!");
                }
            }
            Err(error) => {
                println!("{}", error);
            }
        }

        // Wait a bit before trying again
        sleep(Duration::from_secs(5))
    }
}

// Ping a remote cardano-node
//
// Ping connects to a remote cardano-node and runs the handshake protocol
pub fn ping(host: &String, port: u16, network_magic: u32) {
    let start_time = Instant::now();

    match (host.as_str(), port).to_socket_addrs() {
        Ok(mut into_iter) => {
            if into_iter.len() > 0 {
                let socket_addr: SocketAddr = into_iter.nth(0).unwrap();
                match TcpStream::connect_timeout(&socket_addr, Duration::from_secs(1)) {
                    Ok(stream) => {
                        match handshake_protocol::ping(&stream, timestamp(&start_time), network_magic) {
                            Ok(_payload) => {
                                stream.shutdown(Shutdown::Both).expect("shutdown call failed");
                                print_json_success(start_time.elapsed(), host, port);
                            }
                            Err(message) => {
                                stream.shutdown(Shutdown::Both).expect("shutdown call failed");
                                print_json_error(message, host, port);
                            }
                        }
                    }
                    Err(e) => {
                        print_json_error(format!("Failed to connect: {}", e), host, port);
                    }
                }
            } else {
                print_json_error("No IP addresses found!".to_owned(), host, port);
            }
        }
        Err(error) => {
            print_json_error(error.to_string(), host, port);
        }
    }
}

fn print_json_success(duration: Duration, host: &String, port: u16) {
    println!("{{\n\
        \x20\"status\": \"ok\",\n\
        \x20\"host\": \"{}\",\n\
        \x20\"port\": {},\n\
        \x20\"durationMs\": {}\n\
    }}", host, port, duration.as_millis())
}

fn print_json_error(message: String, host: &String, port: u16) {
    println!("{{\n\
        \x20\"status\": \"error\",\n\
        \x20\"host\": \"{}\",\n\
        \x20\"port\": {},\n\
        \x20\"errorMessage\": \"{}\"\n\
    }}", host, port, message);
}

// return microseconds from the monotonic clock dropping all bits above 32.
fn timestamp(start: &Instant) -> u32 {
    start.elapsed().as_micros() as u32
}