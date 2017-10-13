extern crate futures;
extern crate num_cpus;
extern crate tokio_core;
extern crate tokio_io;

use futures::Future;
use futures::stream::{self, Stream};
use futures::sync::mpsc;

use std::collections::HashMap;
use std::env;
use std::io::{Error, ErrorKind, BufReader};
use std::iter;
use std::net::{self, SocketAddr};
use std::sync::{Arc, Mutex};
use std::thread;

use tokio_core::net::TcpStream;
use tokio_core::reactor::Core;
use tokio_io::AsyncRead;
use tokio_io::io;

fn main() {
    let addr = env::args()
        .nth(1)
        .unwrap_or("127.0.0.1:8080".to_string());

    let addr = addr.parse::<SocketAddr>().unwrap();
    let num_threads = env::args()
        .nth(2)
        .and_then(|s| s.parse().ok())
        .unwrap_or(num_cpus::get());

    let listener = net::TcpListener::bind(&addr).expect("failed to bind");

    println!("Listening on: {}", addr);

    let connections = Arc::new(Mutex::new(HashMap::new()));
    let mut channels = Vec::new();

    for _ in 0..num_threads {
        let (tx, rx) = mpsc::unbounded();
        let connections = connections.clone();
        channels.push(tx);
        thread::spawn(|| worker(rx, connections));
    }

    let mut next = 0;

    for socket in listener.incoming() {
        let socket = socket.expect("failed to accept");
        channels[next]
            .unbounded_send(socket)
            .expect("worker thread died");
        next = (next + 1) % channels.len();
    }
}

fn worker(insock: mpsc::UnboundedReceiver<net::TcpStream>,
          connections: Arc<Mutex<HashMap<SocketAddr, mpsc::UnboundedSender<String>>>>) {
    let mut core = Core::new().unwrap();
    let handle = core.handle();

    let done = insock.for_each(move |socket| {
        let stream = TcpStream::from_stream(socket, &handle).expect("failed to associate TCP stream");
        let addr = stream.peer_addr().expect("failed to get remote address");
        let conns = connections.clone();

        println!("New Connection: {}", addr);

        let (reader, writer) = stream.split();
        let (tx, rx) = futures::sync::mpsc::unbounded();
        let mut conns = conns.lock().unwrap();
        conns.insert(addr, tx);

        let reader = BufReader::new(reader);
        let iter = stream::iter_ok::<_, Error>(iter::repeat(()));
        let conns = connections.clone();

        let socket_reader = iter.fold(reader, move |reader, _| {
            let line = io::read_until(reader, b'\n', Vec::new());
            let line = line.and_then(|(reader, vec)| if vec.len() == 0 {
                                         Err(Error::new(ErrorKind::BrokenPipe, "broken pipe"))
                                     } else {
                                         Ok((reader, vec))
                                     });

            let line = line.map(|(reader, vec)| (reader, String::from_utf8(vec)));
            let conns = conns.clone();

            line.map(move |(reader, message)| {
                println!("{}: {:?}", addr, message);
                let mut conns = conns.lock().unwrap();
                if let Ok(msg) = message {
                    let iter = conns.iter()
                                    .filter(|&(&k, _)| k != addr)
                                    .map(|(_, v)| v);
                    for tx in iter {
                        tx.unbounded_send(format!("{}: {}", addr, msg)).unwrap();
                    }
                } else {
                    let tx = conns.get_mut(&addr).unwrap();
                    tx.unbounded_send("You didn't send valid UTF-8.".to_string())
                      .unwrap();
                }
                reader
            })
        });

        let socket_writer = rx.fold(writer, |writer, msg| {
            let amt = io::write_all(writer, msg.into_bytes());
            let amt = amt.map(|(writer, _)| writer);
            amt.map_err(|_| ())
        });

        let conns = connections.clone();
        let socket_reader = socket_reader.map_err(|_| ());
        let connection = socket_reader.map(|_| ())
                                      .select(socket_writer.map(|_| ()));

        handle.spawn(connection.then(move |_| {
                                         conns.lock().unwrap().remove(&addr);
                                         println!("Connection {} closed.", addr);
                                         Ok(())
                                     }));

        Ok(())
    });

    core.run(done).unwrap();
}
