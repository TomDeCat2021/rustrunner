use anyhow::Result;
use dfuzz::PythonWorker;

mod coverage;
use coverage::*;
use rand::Rng;
use std::collections::HashMap;
mod corpus;
use corpus::*;
mod corpus_aspect;
use corpus_aspect::*;
mod generator_client;
use generator_client::*;
use std::sync::mpsc::{channel,Sender, Receiver};
use std::path::PathBuf;
use std::collections::VecDeque;
use std::fs::{self, File};
use std::io::{self, Read, Write};
use std::sync::atomic::{AtomicI32, Ordering};
use std::time::Duration;
use std::time::Instant;
use structopt::StructOpt;
use std::fs::OpenOptions;
use chrono::Utc;
use sanitize_filename::sanitize;
use ctrlc;
extern "C" {
    fn init(worker_id: i32);
    fn spawn(worker_id: i32);
    fn execute_script(script: *mut i8, timeout: i32, fresh_instance: i32, worker_id: i32) -> i32;
    fn cov_evaluate(worker_id: usize,new_edges: *mut EdgeSet) -> i32;
    fn coverage_finish_initialization(worker_id: usize, should_track_edges: i32);
    fn reprl_destroy_context(worker_id: usize);
    fn cov_clear_edge_data(worker_id: usize, index: u32);
    fn coverage_save_virgin_bits_in_file(worker_id: usize, filepath: *const i8);
    fn coverage_load_virgin_bits_from_file(worker_id: usize, filepath: *const i8) -> i32;
    fn coverage_backup_virgin_bits(worker_id: usize);
    fn coverage_restore_virgin_bits(worker_id: usize);
    fn reprl_fetch_stdout(worker_id: i32) -> *const i8;
}
#[derive(Debug, StructOpt)]
#[structopt(name = "d-fuzzer", about = "nothing")]

struct Opt {
    /// Directory containing initial corpus files
    #[structopt(long, parse(from_os_str))]
    corpus_dir: PathBuf,
    /// Directory for output and new corpus files
    #[structopt(long, parse(from_os_str))]
    output_dir: PathBuf,
    #[structopt(long = "timeout")]
    timeout: i32,
    #[structopt(long = "num-workers")]
    num_workers: usize,
    #[structopt(long = "test-mode")]
    test_mode: bool,
    #[structopt(long = "cov-mode")]
    cov_mode: bool,
    #[structopt(long = "network-worker")]
    network_worker: bool,
    #[structopt(long = "port", default_value = "9999")]
    port: u16,
}


enum WorkerMessage {
    NewCorpus {
        program_ir: String,
        js_code: String,
        pass: String,
    },
    Crash {
        program_ir: String,
        js_code: String,
    },
}

enum MasterMessage {
    NewCorpus {
        program_ir: String,
        js_code: String,
    },
}

struct Fuzzer {
    corpus: CorpusManager,
    output_dir: PathBuf,
    worker_id: usize,
    to_master: Sender<WorkerMessage>,
    from_master: Receiver<MasterMessage>,
    generator_client: Option<GeneratorClient>,
}


#[derive(Debug, Clone, PartialEq)]
pub enum WorkerState {
    Idle,
    Mutating,
    Executing,
    CoverageCheck,
    Minimizing,
    Maintaining,
    SavingCrash,
    Generating,
    Waiting,

}
static TOTAL_COV: AtomicI32 = AtomicI32::new(0);
static mut MAX_TIMEOUT: i32 = 500;
static mut NUM_WORKERS: usize = 8;

// Add this enum to track worker status
#[derive(Debug, Clone, PartialEq)]
pub enum WorkerStatus {
    Alive {
        state: WorkerState,
        since: Instant,
    },
    Dead {
        since: Instant,
    }
}

// Modify WorkerStats to include status
struct WorkerStats {
    worker_id: usize,
    total_coverage: i32,
    total_fuzzed: u64,
    total_corpus_size: i32,
    last_coverage_time: Option<Instant>,
    status: WorkerStatus,  // Add status field
}
// Modify WorkerStats struct to include validation
impl WorkerStats {
    fn new(worker_id: usize) -> Option<Self> {
        unsafe {
            if worker_id > NUM_WORKERS {
                return None;
            }
        }
        
        Some(WorkerStats {
            worker_id,
            total_coverage: 0,
            total_fuzzed: 0,
            total_corpus_size: 0,
            last_coverage_time: None,
            status: WorkerStatus::Alive { 
                state: WorkerState::Idle, 
                since: Instant::now() 
            },
        })
    }
}
struct Stats {
    total_executions: u64,
    total_crashes: u64,
    total_timeouts: u64,
    total_errors: u64,
    total_coverage: i32,
    corpus_size: i32,
    start_time: Option<Instant>,
    last_coverage_time: Option<Instant>,
    worker_stats: Vec<WorkerStats>,
}

static mut STATS: Stats = Stats {
    total_executions: 0,
    total_crashes: 0,
    total_timeouts: 0,
    total_errors: 0,
    total_coverage: 0,
    corpus_size: 0,
    start_time: None,
    last_coverage_time: None,
    worker_stats: Vec::new(),
};

struct Passes {
    name: String,
    execution_count: u64,
    success_count: u64,
    new_coverage: u64,
    failure_count: u64,
    timeout_count: u64,
    error_count: u64,
    new_edges: u64,
    last_cov_time: Option<Instant>,
}
impl Passes {
    fn new(name: String) -> Self {
        Passes { name, execution_count: 0, success_count: 0, new_coverage: 0, failure_count: 0, timeout_count: 0, error_count: 0, new_edges: 0, last_cov_time: None }
    }
    fn update_stats(&mut self, result: i32, new_cov: i32, new_edges: u64) { 
        self.execution_count += 1;
        match get_result_code(result) {
            ResultCode::Success => self.success_count += 1,
            ResultCode::Crash => self.failure_count += 1,
            ResultCode::Timeout => self.timeout_count += 1,
            ResultCode::Error => self.error_count += 1,
        }
        if new_cov > 0 {
            self.new_coverage += 1;
        }
        self.new_edges += new_edges;
        if new_cov > 0 {
            self.last_cov_time = Some(Instant::now());
        }
    }
}
static mut PASSES: Vec<Passes> = Vec::new();

fn print_passes() {

    let mut total_edges = 0;
    unsafe {
        for pass in &mut PASSES {
            total_edges += pass.new_edges;
        }
    }
    println!("┌────────────────────────────────┬─────────────────┬───────────────┬───────────────┬───────────────┬───────────────┬─────────────┬───────────┬─────────────────┐");
    println!("│ {:<30} │ {:>15} │ {:>13} │ {:>13} │ {:>13} │ {:>13} │ {:>11} │ {:>9} │ {:>13}   │", 
             "Name", "Execution Count", "Success Count", "New Coverage", "New Edges", "Timeout Count", "Error Count", "Percent", "Last Cov Time");
    println!("├────────────────────────────────┼─────────────────┼───────────────┼───────────────┼───────────────┼───────────────┼─────────────┼───────────┼─────────────────┤");
    unsafe {
        for pass in &mut PASSES {
            println!("│ {:<30} │ {:>15} │ {:>13} │ {:>13} │ {:>13} │ {:>13} │ {:>11} │ {:>9} │ {:>13}   │", 
                     pass.name, pass.execution_count, format!("{:.2}%", pass.success_count as f64 / pass.execution_count as f64 * 100.0), format!("{:.2}%", pass.new_coverage as f64 / pass.execution_count as f64 * 100.0), 
                     pass.new_edges, pass.timeout_count, format!("{:.2}%", pass.error_count as f64 / pass.execution_count as f64 * 100.0), format!("{:.2}%", (pass.new_edges as f64 / total_edges as f64) * 100.0), pass.last_cov_time.map(|t| t.elapsed().as_secs()).unwrap_or_default());
        }
    }
    println!("└────────────────────────────────┴─────────────────┴───────────────┴───────────────┴───────────────┴───────────────┴─────────────┴───────────┴─────────────────┘");
}
fn update_passes(name: String, result: i32, new_cov: i32, new_edges: u64) {
    unsafe {
        let pass = PASSES.iter_mut().find(|p| p.name == name);
        if let Some(pass) = pass {
            pass.update_stats(result, new_cov, new_edges);
        }
        else {
            let mut new_pass = Passes::new(name);
            new_pass.update_stats(result, new_cov, new_edges);
            PASSES.push(new_pass);
        }
    }
}
fn format_duration(duration: Duration) -> String {
    let duration_secs = duration.as_secs_f64();
    
    // Format with fixed width
    if duration_secs < 0.001 {
        format!("{:>6}µs", duration.as_micros())
    } else if duration_secs < 1.0 {
        format!("{:>6}ms", duration.as_millis())
    } else {
        format!("{:>6}s ", duration_secs as u64)
    }
}
fn write_stats_to_xml(total_execs: u64, total_crashes: u64, coverage_pct: f64, exec_per_sec: f64) {
    let timestamp = Utc::now().timestamp();
    let xml_entry = format!(
        "<entry>\n  <timestamp>{}</timestamp>\n  <executions>{}</executions>\n  <crashes>{}</crashes>\n  <coverage>{:.2}</coverage>\n  <exec_per_sec>{:.2}</exec_per_sec>\n</entry>\n",
        timestamp, total_execs, total_crashes, coverage_pct, exec_per_sec
    );

    if let Ok(mut file) = OpenOptions::new()
        .create(true)
        .append(true)
        .open("stats/fuzz_stats.xml") 
    {
        // Create XML file if it doesn't exist
        if file.metadata().unwrap().len() == 0 {
            let _ = file.write_all(b"<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<fuzz_stats>\n");
        }
        
        let _ = file.write_all(xml_entry.as_bytes());
    }
}


// Modify print_stats function to show worker status
fn print_stats() {
    unsafe {
        if let (Some(start_time), Some(last_cov_time)) =
            (STATS.start_time, STATS.last_coverage_time)
        {
            let elapsed = start_time.elapsed().as_secs();
            let exec_per_sec = if elapsed > 0 {
                STATS.total_executions as f64 / elapsed as f64
            } else {
                0.0
            };
            let use_tui = std::env::var("SCROLL_LOG").unwrap_or_else(|_| "1".to_string()) != "0";
            
            if use_tui {
                // Initialize TUI mode - save current state and clear screen
                print!("\x1B[?1049h"); // Switch to alternate screen buffer
                print!("\x1B[2J");     // Clear the entire screen
                print!("\x1B[H");      // Move cursor to home position (0,0)
            }

            // Print the statistics header
            println!("=== Fuzzing Statistics ===");
            println!("Runtime: {:?}", elapsed);
            println!("Total executions: {}", STATS.total_executions);
            println!("Executions/sec: {:.2}", exec_per_sec);
            let mut TOTOAL_COVERAGE = 1703484;
            if std::env::var("TARGET").unwrap_or_else(|_| "0".to_string()) == "firefox" {
                TOTOAL_COVERAGE = 331671;
            }
            println!("Total coverage: {} ({:.2}%)", STATS.total_coverage, STATS.total_coverage as f64 / TOTOAL_COVERAGE as f64 * 100.0);
            println!(
                "Time since last new coverage: {:?} seconds ago",
                last_cov_time.elapsed().as_secs()
            );
            println!("Crashes: {}", STATS.total_crashes);
            println!(
                "Timeouts: {} {:.2}%",
                STATS.total_timeouts,
                STATS.total_timeouts as f64 / STATS.total_executions as f64 * 100.0
            );

            if STATS.total_executions > 0 {
                println!(
                    "Success rate: {:.2}%",
                    ((STATS.total_executions
                        - STATS.total_crashes
                        - STATS.total_timeouts
                        - STATS.total_errors) as f64
                        / STATS.total_executions as f64)
                        * 100.0
                );
            }
            
            let coverage_pct = STATS.total_coverage as f64 / 1703484 as f64 * 100.0;
            if elapsed % 60 == 0 {
                write_stats_to_xml(
                    STATS.total_executions,
                    STATS.total_crashes,
                    coverage_pct,
                    exec_per_sec
                );
            }
          
            // Print worker status header
            println!("\n=== Worker Status ===");
            
            // Print worker details
            for worker_stat in STATS.worker_stats.iter() {
                // Format worker ID with padding
                let worker_id = if worker_stat.worker_id == NUM_WORKERS {
                    "Master  M".to_string()
                } else {
                    format!("Worker {:<2}", worker_stat.worker_id)
                };
                
                // Format status with color and duration
                let status_str = match &worker_stat.status {
                    WorkerStatus::Alive { state, since } => {
                        let duration = since.elapsed();
                        let duration_secs = duration.as_secs_f64();
                        
                        // Color code for duration
                        let duration_color = if duration_secs < 2.0 {
                            "\x1B[32m" // Green
                        } else if duration_secs < 10.0 {
                            "\x1B[33m" // Yellow
                        } else {
                            "\x1B[31m" // Red
                        };
                        
                        // Format duration with appropriate unit and precision
                        let duration_str = format_duration(duration);

                        let state_str = match state {
                            WorkerState::Idle =>          "\x1B[32m[IDLING    ]\x1B[0m",
                            WorkerState::Mutating =>      "\x1B[33m[MUTATING  ]\x1B[0m",
                            WorkerState::Executing =>     "\x1B[36m[EXECUTING ]\x1B[0m",
                            WorkerState::CoverageCheck => "\x1B[34m[COVERAGE  ]\x1B[0m",
                            WorkerState::Minimizing =>    "\x1B[34m[MINIMIZING]\x1B[0m",
                            WorkerState::Maintaining =>   "\x1B[34m[MAINTAINING]\x1B[0m",
                            WorkerState::SavingCrash =>   "\x1B[35m[SAVING    ]\x1B[0m",
                            WorkerState::Generating =>    "\x1B[33m[GENERATING]\x1B[0m",
                            WorkerState::Waiting =>       "\x1B[33m[WAITING   ]\x1B[0m",
                        };
                        
                        format!("{} for {}{}{}\x1B[0m", 
                            state_str,
                            duration_color,
                            duration_str,
                            "\x1B[0m")
                    },
                    WorkerStatus::Dead { since } => {
                        format!("\x1B[31m[DEAD for {:.0}s]\x1B[0m", 
                            since.elapsed().as_secs_f64())
                    }
                };
                
                // Format stats with aligned columns
                println!(
                    "{:<8} {} | executed {:<6} | cov {:<6} | corpus {:<6} | last cov {:<3} seconds ago",
                    worker_id,
                    status_str,
                    worker_stat.total_fuzzed,
                    worker_stat.total_coverage,
                    worker_stat.total_corpus_size,
                    worker_stat.last_coverage_time
                        .map(|t| t.elapsed().as_secs())
                        .unwrap_or_default()
                );
            }

            println!("----------------------------------------");
            print_passes(); 
            // Flush to ensure immediate display
            std::io::stdout().flush().unwrap();
            
            // No need to restore cursor or switch back buffers during normal printing
            // The alternate buffer will stay active during the program run
        }
    }
}

// Modify update_stats to handle worker status
fn update_stats(worker_id: usize, result: i32, new_cov: i32, state: WorkerState, total_corpus_size: i32) {

    unsafe {
        if state == WorkerState::Executing {
            STATS.total_executions += 1;
        }

        match get_result_code(result) {
            ResultCode::Crash => {
                println!("Crash Confirmed from with result {}", result);
                STATS.total_crashes += 1;
            }
            ResultCode::Timeout => STATS.total_timeouts += 1,
            ResultCode::Error => STATS.total_errors += 1,
            _ => {}
        }
        if worker_id == NUM_WORKERS {
            STATS.total_coverage += new_cov;
        }

        // Update or create worker stats
        let now = Instant::now();
        if let Some(worker_stat) = STATS
            .worker_stats
            .iter_mut()
        .find(|s| s.worker_id == worker_id)
        {
            // Update existing worker stats
            if state == WorkerState::Executing {
                worker_stat.total_fuzzed += 1;
            }
            worker_stat.status = WorkerStatus::Alive { state, since: now };
            if new_cov > 0 {
                worker_stat.total_coverage += new_cov;
                worker_stat.last_coverage_time = Some(now);
            }
            worker_stat.total_corpus_size = total_corpus_size;
        } else {
            // Create new worker stats
            // Create new worker stats with validation
            if let Some(new_stats) = WorkerStats::new(worker_id) {
                STATS.worker_stats.push(new_stats);
            }
        }

      
        // Update global coverage time if new coverage found
        if new_cov > 0 {
            STATS.last_coverage_time = Some(now);

        }

        // Print stats every second
        static mut LAST_STATS_TIME: u64 = 0;
        let current_time = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs();
        if current_time - LAST_STATS_TIME >= 1 {
            LAST_STATS_TIME = current_time;
            print_stats();
        }
    }
}
fn init_stats() {
    unsafe {
        STATS.start_time = Some(Instant::now());
        STATS.last_coverage_time = Some(Instant::now());
    }
}
#[derive(Clone)]
struct Config {
    corpus_dir: PathBuf,
    output_dir: PathBuf,
}
impl Config {
    fn new() -> io::Result<Self> {
        let opt = Opt::from_args();
        Ok(Config {
            corpus_dir: opt.corpus_dir,
            output_dir: opt.output_dir,
        })
    }
}

// Simple function to generate JavaScript code and program IR
fn generate_js_code(num_statements: u32) -> (String, String) {
    // This is a simplified version - in a real implementation,
    // you would use your JavaScript generator code
    
    let (program_ir, js_code) = ("".to_string(), "".to_string());
    (program_ir, js_code)
}

impl Fuzzer {
    // Add a new method to set corpus directly
    fn set_corpus(&mut self, corpus: CorpusManager) {
        self.corpus = corpus;
        let mut count = 0;
        for entry in self.corpus.entries.iter() {
            let js_code = "".to_string();
            println!("Slave {} is executing {}", self.worker_id, count);
            count += 1;
            unsafe {
                execute_script(js_code.as_ptr() as *mut i8, MAX_TIMEOUT, 0, self.worker_id as i32);
            }
        }
        let mut new_edges = EdgeSet::new();
        let new_cov = unsafe { cov_evaluate(self.worker_id as usize, &mut new_edges) };
        update_stats(self.worker_id, 0, new_cov as i32, WorkerState::Executing, self.corpus.entries.len() as i32);
    }

    async fn new(
        opt: &Config,
        worker_id: usize,
        to_master: Sender<WorkerMessage>,
        from_master: Receiver<MasterMessage>,
    ) -> io::Result<Self> {
        // Create output directory if it doesn't exist
        fs::create_dir_all(&opt.output_dir)?;
        fs::create_dir_all(opt.output_dir.join("corpus"))?;
        fs::create_dir_all(opt.output_dir.join("corpus_ir"))?;
        fs::create_dir_all(opt.output_dir.join("corpus_ir_min"))?;
        fs::create_dir_all(opt.output_dir.join("crashes"))?;
        println!("Corpus directory: {}", opt.corpus_dir.display());
        init_reprl_safe(worker_id ); 
        if unsafe { worker_id == NUM_WORKERS }{
            let program_ir = "{\"type\":\"NonTerminal\",\"symbol\":\"Program\",\"children\":[]}";
            "".to_string();
        }
        // get total number of modules to check
        let mut counter = 0;
        // First collect and sort all entries
        let mut entries: Vec<_> = fs::read_dir(&opt.corpus_dir)?
        .filter_map(|entry| entry.ok())
        .filter(|entry| entry.file_type().map(|t| t.is_file()).unwrap_or(false))
        .collect();

        // Sort entries by file size ignore file larger than 50kb
        entries = entries.into_iter().filter(|entry| {
            let size = entry.metadata().map(|m| m.len()).unwrap_or(u64::MAX);
            size < 100 * 1024
        }).collect();
        entries.sort_by(|a, b| {
            let size_a = a.metadata().map(|m| m.len()).unwrap_or(u64::MAX);
            let size_b = b.metadata().map(|m| m.len()).unwrap_or(u64::MAX);
            size_a.cmp(&size_b)
        });
        println!("Total entries: {}", entries.len());
        let mut corpus = CorpusManager::new(worker_id, 10000);
        let mut total_entries = entries.len();
        for entry in entries {
            counter += 1;
            println!("Checking JS Program: {} / {}", counter, total_entries);
            let bytes = fs::read(entry.path())?;
            let program_ir = fs::read_to_string(entry.path()).unwrap();
            if unsafe { worker_id == NUM_WORKERS } && std::env::var("COV_MEASURE").unwrap_or_else(|_| "0".to_string()) == "1" {
                // get the last 4 bytes as the ran_mt seed
                let js_code = "".to_string();
                // println!("js_code: {}", js_code);
                if js_code.is_empty() {
                    continue;
                }
                let result = unsafe {
                execute_script(
                        js_code.as_ptr() as *mut i8,
                        MAX_TIMEOUT,    
                        0,
                        worker_id as i32,
                    )
                    };
                    // Skip modules that timeout or fail
                    if get_result_code(result) == ResultCode::Timeout {
                        println!("Module {} timed out, skipping", counter);
                        continue;
                    }
                    // Evaluate coverage
                    let mut new_edges = EdgeSet::new();
                    let new_cov = unsafe { cov_evaluate(worker_id as usize, &mut new_edges) };
                    if new_cov == 0 {
                        println!("Module {} produced no coverage, skipping", counter);
                        continue;
                    }
                    println!("Module {} produced {} edges", counter, new_edges.count);
                    fs::write(opt.output_dir.join("corpus_ir_min").join(format!("{}.json", counter)), program_ir.clone()).unwrap();
                    update_stats(worker_id, 0, new_cov as i32, WorkerState::Executing, corpus.entries.len() as i32);
                    corpus.add_entry(CorpusEntry::new(  program_ir.clone()  , js_code.clone()));
                    
                }
                
        }
        
        // Initialize bytecode collector for the corpus
        if std::env::var("BYTECODE_COLLECTOR").unwrap_or_else(|_| "0".to_string()) == "1" {
            corpus.init_bytecode_collector();
        }
        
        // Initialize generator client for workers that need it (not master)
        let generator_client = if worker_id < unsafe { NUM_WORKERS } {
            match GeneratorClient::new() {
                Ok(client) => Some(client),
                Err(e) => {
                    println!("Warning: Failed to initialize generator client for worker {}: {}", worker_id, e);
                    None
                }
            }
        } else {
            None
        };

        Ok(Fuzzer {
            corpus,
            output_dir: opt.output_dir.clone(),
            worker_id,
            to_master,
            from_master,
            generator_client,
        })
    }
    fn update_entry_result(&mut self, result: i32, new_cov: i32, entry_index: u32) {
        match get_result_code(result) {
            ResultCode::Success => {
                self.corpus.update_entry_success(entry_index, new_cov as u32); 
            } 
            ResultCode::Timeout => {
                self.corpus.update_entry_timeout(entry_index);
            }
            ResultCode::Error => {
                self.corpus.update_entry_error(entry_index);
            }
            ResultCode::Crash => {
                self.corpus.update_entry_success(entry_index, new_cov as u32);
            }
        }
    }
    fn run_single_input(&mut self, entry: CorpusEntry, passes: &mut Vec<String>) -> io::Result<()> {
      

        update_stats(self.worker_id, 0, 0, WorkerState::Mutating, self.corpus.entries.len() as i32);
        // FUZZ_MODE=1 is for generating new modules base on wasm smith
        let mut start_time = Instant::now();
        let fuzz_mode =  std::env::var("FUZZ_MODE").unwrap_or_else(|_| "0".to_string());
        if entry.js_code.is_empty() {
            return Ok(());
        }
            let result = unsafe {
                execute_script(
                    entry.js_code.clone().as_ptr() as *mut i8,
                    MAX_TIMEOUT,
                    0,
                    self.worker_id as i32,
                )
            };
            update_stats(self.worker_id, result, 0, WorkerState::Executing, self.corpus.entries.len() as i32);
          
            let elapsed_time = start_time.elapsed();
           

            let mut new_edges = EdgeSet::new();
            let new_cov = unsafe { cov_evaluate(self.worker_id as usize, &mut new_edges) };
            let file_name = format!("{}_{}.js",  self.worker_id,  new_cov);
            
            // Create corpus entry for potential addition
            let mut new_entry = CorpusEntry::new(entry.program_ir.clone(), entry.js_code.clone());
            
            // Check if we should keep this entry (either due to coverage or bytecode novelty)
            let has_new_coverage = new_cov > 0;
            let should_keep = if elapsed_time < Duration::from_secs(5) && !self.is_master() {
                if has_new_coverage {
                    true
                } else {
                    // Check for bytecode novelty when no new coverage is found
                    if std::env::var("BYTECODE_COLLECTOR").unwrap_or_else(|_| "0".to_string()) == "1" {
                        self.corpus.should_keep_entry(&mut new_entry, false)
                    } else {
                        false
                    }
                }
            } else {
                false
            };
            
            if should_keep {
                for pass in passes.clone() {
                    update_passes(pass.clone(), result, if has_new_coverage { 1 } else { 0 }, new_cov as u64);
                }
                
                if has_new_coverage {
                    update_stats(self.worker_id, 
                        0, 
                        new_cov as i32, 
                        WorkerState::Executing, 
                        self.corpus.entries.len() as i32);
                    self.update_entry_result(result, new_cov, entry.index);
                    
                    match self.to_master.send(WorkerMessage::NewCorpus {
                        program_ir: entry.program_ir.clone(),
                        js_code: entry.js_code.clone(),
                        pass: passes[0].clone(),
                    }) {
                        Ok(_) => {
                            // self.log("Successfully sent coverage to master");
                        },
                        Err(e) => {
                            // self.log(&format!("Failed to send coverage to master: {}", e));
                            // self.log("Attempting to save coverage locally...");
                            match self.save_interesting_input(&entry.js_code, &entry.program_ir, &file_name) 
                            {
                                Ok(_) => { 
                                    // self.log("Successfully saved coverage locally"); 
                                },
                                Err(e) => { 
                                    // self.log(&format!("Failed to save coverage locally: {}", e)); 
                                },
                            }
                        }
                    }
                } else {
                    // Entry kept due to novel bytecode patterns
                    self.log(&format!("Entry kept due to novel bytecode patterns (worker {})", self.worker_id));
                    // we keep the entry in the corpus
                    self.corpus.add_entry(CorpusEntry::new(new_entry.program_ir.clone(), new_entry.js_code.clone()));
                    update_passes("BytecodeNovelty".to_string(), result, 0, 0);
                    match self.to_master.send(WorkerMessage::NewCorpus {
                        program_ir: new_entry.program_ir.clone(),
                        js_code: new_entry.js_code.clone(),
                        pass: "BytecodeNovelty".to_string(),
                    }) {
                        Ok(_) => {
                            // self.log("Successfully sent bytecode novel entry to master");
                        },
                        Err(e) => {
                            // self.log(&format!("Failed to send bytecode novel entry to master: {}", e));
                            match self.save_interesting_input(&new_entry.js_code, &new_entry.program_ir, &format!("bytecode_{}", file_name)) 
                            {
                                Ok(_) => { 
                                    // self.log("Successfully saved bytecode novel entry locally"); 
                                },
                                Err(e) => { 
                                    // self.log(&format!("Failed to save bytecode novel entry locally: {}", e)); 
                                },
                            }
                        }
                    }
                }
            } else {
                for pass in passes {
                    update_passes(pass.clone(), result, 0, 0);
                }
            }
              

           

            if get_result_code(result) == ResultCode::Crash {
                update_stats(self.worker_id, 0, 0, WorkerState::SavingCrash, self.corpus.entries.len() as i32);
                self.log(&format!("Crash detected with result {}", result));
                match self.save_crash( &entry.js_code, &file_name) {
                    Ok(_) => self.log("Successfully saved crash locally"),
                    Err(e) => self.log(&format!("Failed to save crash locally: {}", e)),
                }
                
                self.log("Sending crash to master...");
                match self.to_master.send(WorkerMessage::Crash {
                    program_ir: entry.program_ir.clone(),
                    js_code: entry.js_code.clone(),
                }) {
                    Ok(_) => self.log("Successfully sent crash to master"),
                    Err(e) => self.log(&format!("Failed to send crash to master: {}", e)),
                }
            }

         

        Ok(())
    }
    fn save_interesting_input(
        &self,
        test_code: &str,
        original_program_ir: &str,
        original_file: &str,
    ) -> io::Result<()> {
        let timestamp = chrono::Local::now().format("%Y%m%d_%H%M%S");
         
        let js_filename = format!("{}_{}_{}.js", original_file, self.worker_id, timestamp);
        let js_file = self.output_dir.join("corpus").join(js_filename);
        let test_code_ = test_code.replace("\x00", "");
        fs::write(&js_file, test_code_.as_bytes())?;
        let program_ir_filename = format!("{}_{}_{}.json", original_file, self.worker_id, timestamp);
        let program_ir_file = self.output_dir.join("corpus_ir").join(program_ir_filename);
        fs::write(&program_ir_file, original_program_ir.as_bytes())?;
        
        Ok(())
    }
  

    fn save_crash(
        &mut self,
        test_code: &str,
        original_file: &str,
    ) -> io::Result<()> {
        let timestamp = chrono::Local::now().format("%Y%m%d_%H%M%S");
        // First save the crash
        let js_filename = format!("{}_{}_{}.js", original_file, self.worker_id, timestamp);
        let js_file = self.output_dir.join("crashes").join(js_filename);
        let test_code_ = test_code.replace("\x00", "");
        fs::write(&js_file, test_code_.as_bytes())?;

        Ok(())
    }
    fn is_master(&self) -> bool {
        unsafe { self.worker_id == NUM_WORKERS }
    }
    fn log(&self, msg: &str) {
        unsafe {
            // Start logging after stats area - adjust this based on your statistics display height
            static mut CURRENT_LOG_LINE: usize = 20;
            let use_tui = std::env::var("SCROLL_LOG").unwrap_or_else(|_| "1".to_string()) != "0";

            if use_tui {
                // Calculate position: stats area height (about 20 lines) + current log line offset
                print!("\x1B[s");                              // Save cursor position
                print!("\x1B[{};0H", CURRENT_LOG_LINE);        // Move to log line position
                print!("\x1B[K");                              // Clear line
                
                // Print the log message with identifier
                if self.is_master() {
                    print!("[ Master  ] {}", msg);
                } else {
                    print!("[ Worker {}] {}", self.worker_id, msg);
                }
                
                // Update log line position, wrapping if needed
                CURRENT_LOG_LINE += 1;
                
                // If we've reached the bottom of our designated log area, wrap back
                // Assuming terminal height of about 40 lines, with 20 reserved for logs
                if CURRENT_LOG_LINE >= 40 {
                    CURRENT_LOG_LINE = 20;
                }
                
                print!("\x1B[u");                              // Restore cursor position
                std::io::stdout().flush().unwrap();
            } else {
                // Standard logging without TUI mode
                if self.is_master() {
                    println!("[ Master  ] {}", msg);
                } else {
                    println!("[ Worker {}] {}", self.worker_id, msg);
                }
            }
        }
    }
    
    fn cleanup(&mut self) {
        if let Some(generator_client) = self.generator_client.take() {
            if let Err(e) = generator_client.shutdown() {
                self.log(&format!("Warning: Failed to shutdown generator client: {}", e));
            }
        }
    }
   
    fn fuzz(&mut self) -> io::Result<()> {
        // Main fuzzing loop

        loop {
            let mut passes = Vec::new();
          
            // Generate test cases using IPC instead of disk I/O
            if let Some(ref mut generator_client) = self.generator_client {
                update_stats(self.worker_id, 0, 0, WorkerState::Generating, self.corpus.entries.len() as i32);
                
                match generator_client.generate_test_cases(10, 5, 10) {
                    Ok(test_cases) => {
                        passes.clear();
                        passes.push("IPCGenerator".to_string());
                        
                        for test_case in test_cases {
                            if let Some(js_code) = test_case.code {
                                self.run_single_input(CorpusEntry::new(test_case.state.unwrap_or("".to_string()), js_code), &mut passes)?;
                            }
                        }
                    },
                    Err(e) => {
                        self.log(&format!("Failed to generate test cases via IPC: {}", e));
                    }
                }
            } 
            
            // Check for messages from master
            while let Ok(msg) = self.from_master.try_recv() {
                match msg {
                    MasterMessage::NewCorpus {  program_ir, js_code } => {
                        // self.log("Received new corpus from master");
                        update_stats(self.worker_id, 
                            0, 
                            0, 
                            WorkerState::Executing, 
                            self.corpus.entries.len() as i32);
                        let start_time = Instant::now();
                        let result = unsafe {
                            execute_script(
                                js_code.clone().as_ptr() as *mut i8,
                                MAX_TIMEOUT,
                                0,
                                self.worker_id as i32,
                            )
                        };
                        let elapsed_time = start_time.elapsed();
                        if elapsed_time > Duration::from_secs(5) {
                            continue;
                        }
                        let mut new_edges = EdgeSet::new();
                        let cov = unsafe { cov_evaluate(self.worker_id as usize, &mut new_edges) };
                        if cov > 0 {
                            self.corpus.add_entry(CorpusEntry::new(program_ir, js_code));
                            update_stats(self.worker_id, 
                                0, 
                                cov, 
                                WorkerState::CoverageCheck, 
                                self.corpus.entries.len() as i32);

                        }
                        // self.log("Successfully processed new corpus from master");
                    }
                }
            }
            update_stats(self.worker_id, 0, 0, WorkerState::Idle, self.corpus.entries.len() as i32);
        }
        
    }
 
}

struct Master {
    fuzzer: Fuzzer,
    from_workers: Vec<Receiver<WorkerMessage>>,
    to_workers: Vec<Sender<MasterMessage>>,
    initialized: bool,
}

impl Master {
    async fn new(config: &Config, num_workers: usize) -> io::Result<Self> {
        let mut from_workers = Vec::new();
        let mut to_workers = Vec::new();

        // Create channels for each worker
        for _ in 0..num_workers {
            let (_, rx_master) = channel();
            let (tx_master, _) = channel();
            from_workers.push(rx_master);
            to_workers.push(tx_master);
        }

        // Create dummy channels with correct types for master's fuzzer
        let (tx_dummy_worker, _): (Sender<WorkerMessage>, Receiver<WorkerMessage>) = channel();
        let (_, rx_dummy_master): (Sender<MasterMessage>, Receiver<MasterMessage>) = channel();

        // Create the master's fuzzer instance with correctly typed channels
        let fuzzer = Fuzzer::new(
            config,
            num_workers, // Use num_workers as master's ID to avoid conflict
            tx_dummy_worker,
            rx_dummy_master,
        ).await?;
        init_reprl_safe(num_workers); 
        
        // Create remote_corpus directory if it doesn't exist
        let remote_corpus_dir = config.output_dir.join("remote_corpus");
        fs::create_dir_all(&remote_corpus_dir)?;
        
        Ok(Master {
            fuzzer,
            from_workers,
            to_workers,
            initialized: false,
        })
    }
    
    // Fix the corpus clone method to correctly return the fuzzer's corpus
    fn get_corpus_clone(&self) -> CorpusManager {
        self.fuzzer.corpus.clone()
    }
    
    fn check_new_ast_files(&mut self) -> io::Result<()> {
        // Path to the remote_corpus directory
        let remote_corpus_dir = self.fuzzer.output_dir.join("remote_corpus");
        
        // Check if directory exists, create if not
        if !remote_corpus_dir.exists() {
            fs::create_dir_all(&remote_corpus_dir)?;
            return Ok(());
        }
        
        // List all files in the directory
        let mut entries: Vec<_> = match fs::read_dir(&remote_corpus_dir) {
            Ok(entries) => entries.filter_map(Result::ok).collect(),
            Err(e) => {
                self.fuzzer.log(&format!("Failed to read remote_corpus directory: {}", e));
                return Ok(());
            }
        };
     
        // Filter out directories and non-JSON files, then sort by file size (smallest first)
        entries.retain(|entry| {
            let path = entry.path();
            !path.is_dir() && path.extension().and_then(|ext| ext.to_str()) == Some("json")
        });
        
        entries.sort_by(|a, b| {
            let size_a = a.metadata().map(|m| m.len()).unwrap_or(u64::MAX);
            let size_b = b.metadata().map(|m| m.len()).unwrap_or(u64::MAX);
            size_a.cmp(&size_b)
        });
        
        // Process each file
        for entry in entries {
            let path = entry.path();
            
            // Already filtered by size in the sort, but keep an additional check for large files
            let size = entry.metadata().map(|m| m.len()).unwrap_or(u64::MAX);
            if size > 100 * 1024 {
                continue;
            }
            
            // Read the program IR from the file
            let program_ir = match fs::read_to_string(&path) {
                Ok(content) => content,
                Err(e) => {
                    self.fuzzer.log(&format!("Failed to read file {}: {}", path.display(), e));
                    continue;
                }
            };
            
            // Generate JS code from the program IR
            let js_code = "".to_string();
            if js_code.is_empty() {
                continue;
            }
            // Execute the code and check for new coverage
            update_stats(unsafe { NUM_WORKERS }, 0, 0, WorkerState::Executing, self.fuzzer.corpus.entries.len() as i32);
            
            let result = unsafe {
                execute_script(
                    js_code.as_ptr() as *mut i8,
                    MAX_TIMEOUT,
                    0,
                    self.fuzzer.worker_id as i32,
                )
            };
            
            let mut new_edges = EdgeSet::new();
            let new_cov = unsafe { cov_evaluate(self.fuzzer.worker_id as usize, &mut new_edges) };
            
            self.fuzzer.log(&format!("Remote file {}: new coverage: {}", path.display(), new_cov));
            
            // If we found new coverage, process it
            if new_cov > 0 {
                update_stats(unsafe { NUM_WORKERS }, result, new_cov as i32, WorkerState::Generating, self.fuzzer.corpus.entries.len() as i32);
                
                // Try to minimize the input
                let mut minimized_ir_list = Vec::new();
                let mut minimized_js_list = Vec::new();
                
            
                
                // Find the smallest minimized version that maintains coverage
                let mut is_maintained = false;
                let mut minimized_js_final = String::new();
                let mut minimized_ir_final = String::new();
                
                if !minimized_js_list.is_empty() {
                    // Sort by length
                    minimized_js_list.sort_by_key(|js: &String| js.len());
                    minimized_ir_list.sort_by_key(|ir: &String| ir.len());
                    
                    reset_edge_set(self.fuzzer.worker_id as usize, &mut new_edges);
                    
                    for (minimized_ir, minimized_js) in minimized_ir_list.iter().zip(minimized_js_list.iter()) {
                        if minimized_js.is_empty() {
                            continue;
                        }
                        
                        (is_maintained, _) = maintain_coverage_with_mutated_edges(
                            &minimized_js, 
                            self.fuzzer.worker_id as usize, 
                            &new_edges
                        );
                        
                        if is_maintained {
                            minimized_js_final = minimized_js.clone();
                            minimized_ir_final = minimized_ir.clone();
                            break;
                        }
                    }
                    
                    mark_edge_set(self.fuzzer.worker_id as usize, &mut new_edges);
                }
                
                // Save and distribute the corpus entry
                if is_maintained && !minimized_js_final.is_empty() {
                    // Use minimized version
                    let file_name = format!("remote_{}_min_", new_cov);
                    self.fuzzer.save_interesting_input(
                        &minimized_js_final,
                        &minimized_ir_final,
                        &file_name,
                    )?;
                    
                    // Send to all workers
                    for tx in &self.to_workers {
                        if let Err(e) = tx.send(MasterMessage::NewCorpus {
                            program_ir: minimized_ir_final.clone(),
                            js_code: minimized_js_final.clone(),
                        }) {
                            self.fuzzer.log(&format!("Failed to send to worker: {}", e));
                        }
                    }
                    
                    self.fuzzer.corpus.add_entry(CorpusEntry::new(minimized_ir_final, minimized_js_final));
                } else {
                    // Use original version
                    let file_name = format!("remote_{}", new_cov);
                    self.fuzzer.save_interesting_input(
                        &js_code,
                        &program_ir,
                        &file_name,
                    )?;
                    
                    // Send to all workers
                    for tx in &self.to_workers {
                        if let Err(e) = tx.send(MasterMessage::NewCorpus {
                            program_ir: program_ir.clone(),
                            js_code: js_code.clone(),
                        }) {
                            self.fuzzer.log(&format!("Failed to send to worker: {}", e));
                        }
                    }
                    
                    self.fuzzer.corpus.add_entry(CorpusEntry::new(program_ir.clone(), js_code.clone()));
                }
            }
            
            // Delete the processed file to avoid processing it again
            if let Err(e) = fs::remove_file(&path) {
                self.fuzzer.log(&format!("Failed to delete processed file {}: {}", path.display(), e));
            }
            
            update_stats(unsafe { NUM_WORKERS }, 0, 0, WorkerState::Idle, self.fuzzer.corpus.entries.len() as i32);
        }
        
        Ok(())
    }
    
    fn run(&mut self) -> io::Result<()> {
      
        
        // Add error handling and keep-alive logic
        let mut consecutive_errors = 0;
        let max_consecutive_errors = 10;
        
        loop {
            // Check messages from all workers
            for (worker_id, rx) in self.from_workers.iter().enumerate() {
                match rx.try_recv() {
                    Ok(WorkerMessage::NewCorpus {
                        program_ir,
                        js_code,
                        pass,
                    }) => {
                        consecutive_errors = 0;  // Reset error counter on successful message
                         // remove comment from test_code
                        // Verify new coverage
                        if js_code.is_empty() {
                            continue;
                        }
                        if program_ir.len() > 100 * 1024 {
                            continue;
                        }
                        update_stats(unsafe { NUM_WORKERS }, 0, 0, WorkerState::Executing, self.fuzzer.corpus.entries.len() as i32);
                        let start_time = Instant::now();
                        let result = unsafe {
                            execute_script(
                                js_code.as_ptr() as *mut i8,
                                MAX_TIMEOUT,
                                0,
                                self.fuzzer.worker_id as i32,
                            )
                        };
                   
                        let mut new_edges = EdgeSet::new();
                        let mut new_cov = unsafe { cov_evaluate(self.fuzzer.worker_id as usize, &mut new_edges) };
                        self.fuzzer
                            .log(&format!("new cov: {} from worker {} ", new_cov, worker_id));

                        update_stats(unsafe { NUM_WORKERS }, result, 0 , WorkerState::CoverageCheck, self.fuzzer.corpus.entries.len() as i32);
                        // let mut mutated_edges = unsafe { extract_testcase_coverage(&js_code, self.fuzzer.worker_id as usize, &mut new_edges) };
                        // if mutated_edges.count == 0 {
                        //     self.fuzzer.log(&format!("Discard new cov from worker {} ", worker_id));
                        // }
                        if new_cov > 0 {
                        // if new_cov > 0 {
                            // let reducer = WasmReducer::new(0, mutated_edges).unwrap();
                            // let (reduced_wasm, new_cov_wasm_modules) = reducer.reduce(&mutated_wasm);
                            // self.fuzzer.log(&format!("Reduced sample size from {} to {}", mutated_wasm.len(), reduced_wasm.len()));
                            // self.fuzzer.log(&format!("New cov wasm modules count: {}", new_cov_wasm_modules.len()));
                            let minimized_ir = program_ir.clone();
                            let minimized_js = "".to_string();

                            let mut minimized_ir_list = Vec::new();
                            let mut minimized_js_list = Vec::new();
                            update_stats(unsafe { NUM_WORKERS }, 0, 0, WorkerState::Minimizing, self.fuzzer.corpus.entries.len() as i32);
                            (minimized_ir_list, minimized_js_list) = (Vec::new(), Vec::new());


                            // sort the minimized_js_list by length
                            minimized_js_list.sort_by_key(|js: &String| js.len());
                            minimized_ir_list.sort_by_key(|ir: &String| ir.len());
                            let mut is_maintained: bool = false;
                            // let mut is_new_coverage: bool = false;
                            let mut minimized_js_final = String::new();
                            let mut minimized_ir_final = String::new();
                            // println!("Js code from client: {}", js_code);
                            reset_edge_set(self.fuzzer.worker_id as usize, &mut new_edges);
                            // (is_maintained, is_new_coverage) = unsafe { maintain_coverage_with_mutated_edges(&js_code, self.fuzzer.worker_id as usize, &new_edges) };
                            // println!("is_maintained: {}", is_maintained);
                            // println!("is_new_coverage: {}", is_new_coverage);
                            update_stats(unsafe { NUM_WORKERS }, 0, 0, WorkerState::Maintaining, self.fuzzer.corpus.entries.len() as i32);
                            for (minimized_ir, minimized_js) in minimized_ir_list.iter().zip(minimized_js_list.iter()) {
                                // println!("minimized_js: {}", minimized_js);
                                if minimized_js.is_empty() {
                                    continue;
                                }
                                (is_maintained, _) =  maintain_coverage_with_mutated_edges(minimized_js, self.fuzzer.worker_id as usize, &new_edges) ;
                                if is_maintained  {
                                    minimized_js_final = minimized_js.clone();
                                    minimized_ir_final = minimized_ir.clone();
                                    break;
                                }
                            }
                            mark_edge_set(self.fuzzer.worker_id as usize, &mut new_edges);
                            if is_maintained  {

                                update_stats(unsafe { NUM_WORKERS }, result, new_cov as i32, WorkerState::Generating, self.fuzzer.corpus.entries.len() as i32);
                                let file_name = format!("{}_{}_{}_min_",  unsafe { NUM_WORKERS },  new_cov, pass);
                                self.fuzzer.save_interesting_input(
                                    &minimized_js_final,
                                    &minimized_ir_final,
                                    &file_name,
                                )?;
                                   
                                for tx in &self.to_workers {
                                    if let Err(e) = tx.send(MasterMessage::NewCorpus {
                                        program_ir: minimized_ir_final.clone(),
                                        js_code: minimized_js_final.clone(),
                                    }) {
                                        self.fuzzer.log(&format!("Failed to send to worker: {}", e));
                                    }
                                }
                                self.fuzzer.corpus.add_entry(CorpusEntry::new(minimized_ir_final, minimized_js_final));
                            }
                            else {
                                update_stats(unsafe { NUM_WORKERS }, result, new_cov as i32, WorkerState::Generating, self.fuzzer.corpus.entries.len() as i32);
                                let file_name = format!("{}_{}_{}",  unsafe { NUM_WORKERS },  new_cov, pass);
                                self.fuzzer.save_interesting_input(
                                    &js_code,
                                    &program_ir,
                                    &file_name,
                                )?;
                                   
                                for tx in &self.to_workers {
                                    if let Err(e) = tx.send(MasterMessage::NewCorpus {
                                        program_ir: program_ir.clone(),
                                        js_code: js_code.clone(),
                                    }) {
                                        self.fuzzer.log(&format!("Failed to send to worker: {}", e));
                                    }
                                }
                                self.fuzzer.corpus.add_entry(CorpusEntry::new(program_ir, js_code));
                            }

                           
                           
                        }
                        update_stats(unsafe { NUM_WORKERS }, result, 0 , WorkerState::Idle, self.fuzzer.corpus.entries.len() as i32);

                        
                    }
                    Ok(WorkerMessage::Crash {
                        program_ir,
                        js_code,
                    }) => {
                        consecutive_errors = 0;  // Reset error counter on successful message
                        self.fuzzer.log(&format!("crash from worker {}", worker_id));
                        self.fuzzer.save_crash(
                            &js_code,
                            &program_ir,
                        )?;
                    }
                    Err(std::sync::mpsc::TryRecvError::Empty) => {
                        // No message available, this is normal
                        continue;
                    }
                    Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                        consecutive_errors += 1;
                        self.fuzzer.log(&format!("Worker {} disconnected", worker_id));
                        if consecutive_errors >= max_consecutive_errors {
                            self.fuzzer.log("Too many disconnected workers, shutting down");
                            return Ok(());
                        }
                    }
                }
            }
            
            // Check for new AST files
            self.check_new_ast_files()?;
            
            std::thread::sleep(Duration::from_millis(100));
        }
    }
}

// Function to clean up terminal state on exit
fn cleanup_terminal() {
    // Check if we're in TUI mode
    let use_tui = std::env::var("SCROLL_LOG").unwrap_or_else(|_| "1".to_string()) != "0";
    if use_tui {
        // Switch back to normal screen buffer
        print!("\x1B[?1049l");
        std::io::stdout().flush().unwrap();
    }
}

fn test_mode() {
    init_reprl_safe(0);
    let js_code = "console.log('Hello, world!');";
    v8_reprl_check(0);
    for i in 0..100 {
        let result = unsafe {
            execute_script(
                js_code.as_ptr() as *mut i8,
                MAX_TIMEOUT,
                0,
                0,
            )
        };
        let mut new_edges = EdgeSet::new();
        let mut new_cov = unsafe { cov_evaluate(0, &mut new_edges) };
        reset_edge_set(0, &mut new_edges);
        let mut js_code_mutated = js_code.clone();
        let mut js_code_mutated2 = "a".to_string();
        let (is_maintained, is_new_coverage) = maintain_coverage_with_mutated_edges(&js_code_mutated, 0, &new_edges);
        // let result = unsafe {
        //     execute_script(
        //         js_code.as_ptr() as *mut i8,
        //         MAX_TIMEOUT,
        //         0,
        //         0,
        //     )
        // };
        // let mut new_edges = EdgeSet::new();
        // let mut new_cov = unsafe { cov_evaluate(0, &mut new_edges) };
        mark_edge_set(0, &mut new_edges);

        let (is_maintained2, is_new_coverage2) = maintain_coverage_with_mutated_edges(&js_code_mutated2, 0, &new_edges);
        println!("New cov {}: {} {} {} ", i, new_cov, is_maintained, is_maintained2);
    }
}
#[tokio::main]
async fn main() -> Result<()> {
    // Create a Python worker
    let opt = Opt::from_args();
    unsafe {
        MAX_TIMEOUT = opt.timeout;
        NUM_WORKERS = opt.num_workers;
    }
    init_stats();
    if opt.test_mode {
        test_mode();
        return Ok(());
    }
    // Set up terminal cleanup on exit
    let use_tui = std::env::var("SCROLL_LOG").unwrap_or_else(|_| "1".to_string()) != "0";
    if use_tui {
        // Register cleanup handler for Ctrl+C
        ctrlc::set_handler(move || {
            cleanup_terminal();
            std::process::exit(0);
        }).expect("Error setting Ctrl-C handler");
    }
    
   
    let num_workers;
    unsafe {
        num_workers = NUM_WORKERS as usize; // Leave one core for master
    }
    println!("Starting {} workers...", num_workers);


               let (tx_dummy_worker, _): (Sender<WorkerMessage>, Receiver<WorkerMessage>) = channel();
               let (_, rx_dummy_master): (Sender<MasterMessage>, Receiver<MasterMessage>) = channel();
    let config = Config::new()?;
    
    let mut master = Master::new(&config, num_workers).await?;
    
    // Get a clone of master's corpus for workers
    let master_corpus = master.get_corpus_clone();
    
    // Spawn worker threads
    let mut handles = Vec::new();

    for worker_id in 0..num_workers {
        let (tx_worker, rx_master) = channel();
        let (tx_master, rx_worker) = channel();
        // Store channels in master
        master.from_workers[worker_id] = rx_master;
        master.to_workers[worker_id] = tx_master;

        let worker_config = config.clone();
        let mut worker_corpus = master_corpus.clone(); // Clone master's corpus for each worker
        worker_corpus.worker_id = worker_id;
        let handle = std::thread::spawn(move || {
            // Initialize worker's REPRL
            println!("Initializing worker {}", worker_id);
            init_reprl_safe(worker_id);
            
            let mut fuzzer = match futures::executor::block_on(Fuzzer::new(
                &worker_config,
                worker_id,
                tx_worker,
                rx_worker,
            )) {
                Ok(mut fuzzer) => {
                    // Set the corpus from master
                    fuzzer.set_corpus(worker_corpus);
                    fuzzer
                },
                Err(e) => {
                    eprintln!("Worker {} initialization failed: {}", worker_id, e);
                    return;
                }
            };
         
            println!("Worker {} initialized", worker_id);
            
            // Use the fuzz method for the main fuzzing loop
            if let Err(e) = fuzzer.fuzz() {
                eprintln!("Worker {} exited with error: {}", worker_id, e);
            }
        });
        
        handles.push(handle);
        
    }
      // Run master in a separate thread
      let master_handle = std::thread::spawn(move || {
        if let Err(e) = master.run() {
            eprintln!("Master error: {}", e);
        }
    });

    // Wait for all workers to finish
    for (i, handle) in handles.into_iter().enumerate() {
        if let Err(e) = handle.join() {
            eprintln!("Worker {} panicked: {:?}", i, e);
        }
    }

    // Wait for master
    if let Err(e) = master_handle.join() {
        eprintln!("Master panicked: {:?}", e);
    }
    
    cleanup_terminal();
    Ok(())
} 