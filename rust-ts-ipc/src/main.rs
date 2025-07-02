use serde::{Deserialize, Serialize};
use std::io::{BufRead, BufReader, Write};
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::thread;

#[derive(Serialize, Deserialize, Debug)]
struct Message {
    msg_type: String,
    data: String,
}

fn main() {
    let mut child = Command::new("node")
        .arg("../ts-app/src/index.js")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
        .expect("Failed to start TypeScript process");

    let mut stdin = child.stdin.take().expect("Failed to get stdin");
    let stdout = child.stdout.take().expect("Failed to get stdout");

    let (tx, rx) = mpsc::channel();

    thread::spawn(move || {
        let reader = BufReader::new(stdout);
        for line in reader.lines() {
            if let Ok(line) = line {
                if let Ok(msg) = serde_json::from_str::<Message>(&line) {
                    println!("Received from TS: {:?}", msg);
                    tx.send(msg).ok();
                }
            }
        }
    });

    println!("Rust: Sending initial message");
    let msg = Message {
        msg_type: "greeting".to_string(),
        data: "Hello from Rust!".to_string(),
    };
    writeln!(stdin, "{}", serde_json::to_string(&msg).unwrap()).unwrap();
    stdin.flush().unwrap();

    for i in 1..=5 {
        thread::sleep(std::time::Duration::from_millis(500));
        let msg = Message {
            msg_type: "data".to_string(),
            data: format!("Message {} from Rust", i),
        };
        writeln!(stdin, "{}", serde_json::to_string(&msg).unwrap()).unwrap();
        stdin.flush().unwrap();
    }

    while let Ok(response) = rx.recv() {
        if response.msg_type == "exit" {
            println!("Received exit signal from TypeScript");
            break;
        }
    }

    child.wait().expect("TypeScript process wasn't running");
    println!("Communication complete!");
}
