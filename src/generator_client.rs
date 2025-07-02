use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::io::{BufRead, BufReader, Write};
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::thread;
use std::time::Instant;

#[derive(Serialize, Deserialize, Debug)]
struct Message {
    msg_type: String,
    data: Value,
}

#[derive(Serialize, Debug)]
struct GenerateRequest {
    count: u32,
    #[serde(rename = "minStatements")]
    min_statements: Option<u32>,
    #[serde(rename = "maxStatements")]
    max_statements: Option<u32>,
    #[serde(rename = "outputDir")]
    output_dir: Option<String>,
}

#[derive(Deserialize, Debug)]
pub struct TestCase {
    pub id: u32,
    pub filename: Option<String>,
    pub code: Option<String>,
    pub state: Option<String>,
}

#[derive(Deserialize, Debug)]
struct GenerateComplete {
    #[serde(rename = "totalGenerated")]
    total_generated: u32,
    #[serde(rename = "elapsedTime")]
    elapsed_time: f64,
    rate: f64,
    #[serde(rename = "outputDir")]
    output_dir: Option<String>,
}

pub struct GeneratorClient {
    stdin: std::process::ChildStdin,
    rx: mpsc::Receiver<Message>,
    _child: std::process::Child,
}

impl GeneratorClient {
    pub fn new() -> Result<Self, Box<dyn std::error::Error>> {
        // Start the TypeScript generator bridge with unique process identifier
        let unique_id = format!("{}-{:?}", std::process::id(), std::thread::current().id());
        let mut child = Command::new("node")
            .arg("rust-ts-ipc/ts-app/dist/generator-simple.js")
            .env("GENERATOR_ID", unique_id)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null()) // Suppress stderr to avoid cluttering
            .spawn()?;

        let mut stdin = child.stdin.take().ok_or("Failed to get stdin")?;
        let stdout = child.stdout.take().ok_or("Failed to get stdout")?;

        // Set up channel for receiving messages
        let (tx, rx) = mpsc::channel();

        // Spawn thread to read responses
        thread::spawn(move || {
            let reader = BufReader::new(stdout);
            for line in reader.lines() {
                if let Ok(line) = line {
                    if let Ok(msg) = serde_json::from_str::<Message>(&line) {
                        tx.send(msg).ok();
                    }
                }
            }
        });

        // Send initialization message
        let init_msg = Message {
            msg_type: "init".to_string(),
            data: Value::Null,
        };
        writeln!(stdin, "{}", serde_json::to_string(&init_msg)?)?;
        stdin.flush()?;

        // Wait for init response
        let mut client = GeneratorClient {
            stdin,
            rx,
            _child: child,
        };

        // Wait for initialization response
        if let Ok(response) = client.rx.recv() {
            if response.msg_type != "init_response" {
                return Err("Failed to initialize generator".into());
            }
        }

        Ok(client)
    }

    pub fn generate_test_cases(&mut self, count: u32, min_statements: u32, max_statements: u32) -> Result<Vec<TestCase>, Box<dyn std::error::Error>> {
        // Send stop message first to ensure clean state
        let stop_msg = Message {
            msg_type: "stop".to_string(),
            data: Value::Null,
        };
        writeln!(self.stdin, "{}", serde_json::to_string(&stop_msg)?)?;
        self.stdin.flush()?;
        
        // Wait a brief moment for stop to process
        std::thread::sleep(std::time::Duration::from_millis(50));
        
        // Use unique output directory per process to avoid conflicts
        let worker_id = std::process::id();
        let output_dir = format!("/tmp/rust-generated-{}", worker_id);
        
        let generate_request = GenerateRequest {
            count,
            min_statements: Some(min_statements),
            max_statements: Some(max_statements),
            output_dir: Some(output_dir),
        };

        let generate_msg = Message {
            msg_type: "generate".to_string(),
            data: serde_json::to_value(generate_request)?,
        };

        writeln!(self.stdin, "{}", serde_json::to_string(&generate_msg)?)?;
        self.stdin.flush()?;

        let mut test_cases = Vec::new();

        // Collect responses with timeout
        let timeout_duration = std::time::Duration::from_secs(20);
        let start_time = Instant::now();

        loop {
            if start_time.elapsed() > timeout_duration {
                return Err("Generator timeout".into());
            }

            if let Ok(response) = self.rx.recv_timeout(std::time::Duration::from_millis(100)) {
                match response.msg_type.as_str() {
                    "test_case" => {
                        if let Ok(test_case) = serde_json::from_value::<TestCase>(response.data) {
                            test_cases.push(test_case);
                        }
                    }
                    "generate_complete" => {
                        break;
                    }
                    "error" => {
                        let error_msg = response.data.as_str().unwrap_or("Unknown error");
                        if error_msg.contains("Generation already in progress") {
                            // Wait a bit and retry
                            std::thread::sleep(std::time::Duration::from_millis(100));
                            continue;
                        }
                        return Err(format!("Generator error: {:?}", response.data).into());
                    }
                    _ => {
                        // Ignore progress and other messages
                    }
                }
            }
        }

        Ok(test_cases)
    }

    pub fn shutdown(mut self) -> Result<(), Box<dyn std::error::Error>> {
        let exit_msg = Message {
            msg_type: "exit".to_string(),
            data: Value::Null,
        };
        writeln!(self.stdin, "{}", serde_json::to_string(&exit_msg)?)?;
        self.stdin.flush()?;
        Ok(())
    }
} 