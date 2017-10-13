extern crate futures;
extern crate num_cpus;
extern crate tokio_core;
extern crate tokio_io;


use futures::Future;
use futures::stream::{self, Stream};
use futures::sync::mpsc;
use std::cell::RefCell;
use std::collections::HashMap;
use std::env;
use std::io::{Error, ErrorKind, BufReader};
use std::iter;

use std::net::{self, SocketAddr};
use std::rc::Rc;
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

    let mut channels = Vec::new();
    for _ in 0..num_threads {
        let (tx, rx) = mpsc::unbounded();
        channels.push(tx);
        thread::spawn(|| worker(rx));
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

fn worker(insock: mpsc::UnboundedReceiver<net::TcpStream>) {
    let mut core = Core::new().unwrap();
    let handle = core.handle();
    let connections = Rc::new(RefCell::new(HashMap::new()));

    let done = insock.for_each(move |socket| {
        let stream = TcpStream::from_stream(socket, &handle).expect("failed to associate TCP stream");
        let addr = stream.peer_addr().expect("failed to get remote address");

        println!("New Connection: {}", addr);
        let (reader, writer) = stream.split();

        let (tx, rx) = futures::sync::mpsc::unbounded();
        connections.borrow_mut().insert(addr, tx);

        let connections_inner = connections.clone();
        let reader = BufReader::new(reader);

        let iter = stream::iter_ok::<_, Error>(iter::repeat(()));
        let socket_reader = iter.fold(reader, move |reader, _| {
            let line = io::read_until(reader, b'\n', Vec::new());
            let line = line.and_then(|(reader, vec)| if vec.len() == 0 {
                                         Err(Error::new(ErrorKind::BrokenPipe, "broken pipe"))
                                     } else {
                                         Ok((reader, vec))
                                     });

            let line = line.map(|(reader, vec)| (reader, String::from_utf8(vec)));
            let connections = connections_inner.clone();
            line.map(move |(reader, message)| {
                println!("{}: {:?}", addr, message);
                let mut conns = connections.borrow_mut();
                if let Ok(msg) = message {
                    let iter = conns.iter_mut()
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

        let connections = connections.clone();
        let socket_reader = socket_reader.map_err(|_| ());
        let connection = socket_reader.map(|_| ())
                                      .select(socket_writer.map(|_| ()));
        handle.spawn(connection.then(move |_| {
                                         connections.borrow_mut().remove(&addr);
                                         println!("Connection {} closed.", addr);
                                         Ok(())
                                     }));

        Ok(())
    });

    core.run(done).unwrap();
}
