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
struct TestCase {
    id: u32,
    filename: Option<String>,
    code: Option<String>,
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

fn main() {
    println!("=== Rust Generator Client ===");
    println!("Starting TypeScript generator bridge...\n");

    // Start the TypeScript generator bridge
    let mut child = Command::new("node")
        .arg("ts-app/dist/generator-simple.js")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .current_dir(".")
        .spawn()
        .expect("Failed to start TypeScript generator bridge");

    let mut stdin = child.stdin.take().expect("Failed to get stdin");
    let stdout = child.stdout.take().expect("Failed to get stdout");

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
    println!("Initializing generator...");
    let init_msg = Message {
        msg_type: "init".to_string(),
        data: Value::Null,
    };
    writeln!(stdin, "{}", serde_json::to_string(&init_msg).unwrap()).unwrap();
    stdin.flush().unwrap();

    // Wait for init response
    if let Ok(response) = rx.recv() {
        if response.msg_type == "init_response" {
            println!("Generator initialized: {:?}\n", response.data);
        }
    }

    // Request generation of test cases
    let test_count = 10; // Reduced for testing
    println!("Requesting generation of {} test cases...", test_count);
    
    let generate_request = GenerateRequest {
        count: test_count,
        min_statements: Some(10),
        max_statements: Some(30),
        output_dir: Some("./rust-generated".to_string()),
    };

    let generate_msg = Message {
        msg_type: "generate".to_string(),
        data: serde_json::to_value(generate_request).unwrap(),
    };
    
    let start_time = Instant::now();
    writeln!(stdin, "{}", serde_json::to_string(&generate_msg).unwrap()).unwrap();
    stdin.flush().unwrap();

    // Collect responses
    let mut test_cases = Vec::new();
    let mut progress_updates = 0;

    loop {
        if let Ok(response) = rx.recv() {
            match response.msg_type.as_str() {
                "test_case" => {
                    if let Ok(test_case) = serde_json::from_value::<TestCase>(response.data) {
                        test_cases.push(test_case);
                    }
                }
                "progress" => {
                    progress_updates += 1;
                    if let Some(progress) = response.data.as_object() {
                        if let (Some(generated), Some(total)) = 
                            (progress.get("generated"), progress.get("total")) {
                            print!("\rProgress: {}/{}", generated, total);
                            std::io::stdout().flush().unwrap();
                        }
                    }
                }
                "generate_complete" => {
                    println!("\n");
                    if let Ok(complete) = serde_json::from_value::<GenerateComplete>(response.data) {
                        println!("Generation complete!");
                        println!("  Total generated: {}", complete.total_generated);
                        println!("  Elapsed time: {:.2}s", complete.elapsed_time);
                        println!("  Generation rate: {:.2} cases/sec", complete.rate);
                        if let Some(dir) = complete.output_dir {
                            println!("  Output directory: {}", dir);
                        }
                    }
                    break;
                }
                "error" => {
                    eprintln!("Error: {:?}", response.data);
                    break;
                }
                _ => {
                    println!("Received: {:?}", response);
                }
            }
        }
    }

    let elapsed = start_time.elapsed();
    println!("\nRust client statistics:");
    println!("  Total time: {:.2}s", elapsed.as_secs_f64());
    println!("  Test cases received: {}", test_cases.len());
    println!("  Progress updates: {}", progress_updates);

    // Display first few test cases
    if !test_cases.is_empty() {
        println!("\nFirst 3 test cases:");
        for (i, test_case) in test_cases.iter().take(3).enumerate() {
            println!("\n--- Test Case {} ---", i + 1);
            if let Some(filename) = &test_case.filename {
                println!("Filename: {}", filename);
            }
            if let Some(code) = &test_case.code {
                let preview = code.lines().take(5).collect::<Vec<_>>().join("\n");
                println!("Code preview:\n{}", preview);
                if code.lines().count() > 5 {
                    println!("... ({} more lines)", code.lines().count() - 5);
                }
            }
        }
    }

    // Send exit message
    println!("\nSending exit signal...");
    let exit_msg = Message {
        msg_type: "exit".to_string(),
        data: Value::Null,
    };
    writeln!(stdin, "{}", serde_json::to_string(&exit_msg).unwrap()).unwrap();
    stdin.flush().unwrap();

    // Wait for child process to exit
    child.wait().expect("TypeScript process wasn't running");
    println!("Generator client completed!");
}