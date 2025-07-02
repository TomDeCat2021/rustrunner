use std::collections::{VecDeque, HashMap};
use std::time::{Instant, Duration};
use rand::{Rng, seq::SliceRandom};
use std::hash::{Hash, Hasher};
use std::collections::{HashSet};
use std::collections::hash_map::DefaultHasher;
use std::fs::OpenOptions;
use std::io::Write;
use crate::corpus_aspect::{BytecodeAnalysis, BytecodeCollector};

#[derive(Clone)]
pub struct CorpusEntry {
    pub index: u32,
    pub times_used: u32,
    pub program_ir: String,
    pub js_code: String,
    pub coverage_found: u32,
    pub success_count: u32,
    pub error_count: u32,    // Track errors when using this entry
    pub timeout_count: u32,  // Track timeouts when using this entry
    pub last_coverage_found: Instant,
    pub last_used: Instant,
    pub creation_time: Instant,
    pub performance_score: f64,  // Track how well this input performs
    pub produced_mutations: HashSet<u64>,
    pub feature_frequency: HashMap<u64, u64>, // Track which features this input hits
    pub module_performance: HashMap<usize, f64>,  // Track performance per module
    pub module_features: HashMap<usize, HashSet<u64>>,  // Track features hit per module
    pub bytecode_analysis: Option<BytecodeAnalysis>,  // Store bytecode analysis if available
    pub has_novel_bytecode: bool,  // Flag indicating if this entry has novel bytecode patterns

}
#[derive(Clone)]
pub struct CorpusManager {
    pub worker_id: usize,
    pub entries: VecDeque<CorpusEntry>,
    max_size: usize,
    min_energy: f64,
    total_coverage: HashMap<u64, u64>,
    corpus_hash: HashMap<u64, bool>,
    last_new_coverage: Instant,
    stats: CorpusStats,
    selection_counter: usize, // Track how many times select_next_input is called
    pub bytecode_collector: Option<BytecodeCollector>, // Add bytecode collector
}

impl CorpusEntry {
    pub fn new(program_ir: String, js_code: String) -> Self {
        Self {
            index: 0 as u32,
            program_ir: program_ir,
            js_code: js_code,
            times_used: 0,
            coverage_found: 0,
            success_count: 0,
            error_count: 0,
            timeout_count: 0,
            last_coverage_found: Instant::now(),
            last_used: Instant::now(),
            creation_time: Instant::now(),
            performance_score: 1.0,
            produced_mutations: HashSet::new(),
            feature_frequency: HashMap::new(),
            module_performance: HashMap::new(),
            module_features: HashMap::new(),
            bytecode_analysis: None,
            has_novel_bytecode: false,
        }
        
    }
    
  
 
    pub fn print(self, prefix: String) {
        println!("{} Index: {}", prefix, self.index);
    }
}

#[derive(Clone)]
struct CorpusStats {
    total_mutations: u64,
    successful_mutations: u64,
    total_coverage: u64,
    avg_entry_size: f64,
}

impl CorpusManager {
   // Update STAGE_MUTATOR_RANGES in CorpusManager:
  
     // Constants for stage advancement
     const MIN_TRIES_BEFORE_ADVANCE: u32 = 10;  // Minimum attempts before considering stage advance
     const SUCCESS_RATE_THRESHOLD: f64 = 0.1;   // If success rate below this, advance stage
     const COVERAGE_STALENESS_THRESHOLD: Duration = Duration::from_secs(300); // 5 minutes
 
    pub fn new(worker_id: usize, max_size: usize) -> Self {
        Self {
            worker_id,
            entries: VecDeque::new(),
            max_size,
            min_energy: 0.1,
            total_coverage: HashMap::new(),
            corpus_hash: HashMap::new(),
            last_new_coverage: Instant::now(),
            stats: CorpusStats {
                total_mutations: 0,
                successful_mutations: 0,
                total_coverage: 0,
                avg_entry_size: 0.0,
            },
            selection_counter: 0,
            bytecode_collector: None,
        }
    }
  
    pub fn update_feature_frequency(&mut self, index: u32, features: &[u64]) {
        if let Some(entry) = self.entries.iter_mut().find(|e| e.index == index) {
            for &feature in features {
                *entry.feature_frequency.entry(feature).or_insert(0) += 1;
            }
        }
    }
    
   


    pub fn add_entry(&mut self, mut entry: CorpusEntry) {
        entry.index = self.entries.len() as u32;
        entry.times_used = 0;
        entry.success_count = 0;
        entry.error_count = 0;
        entry.timeout_count = 0;
        entry.coverage_found = 0;
        entry.last_coverage_found = Instant::now();
        entry.last_used = Instant::now();
        entry.creation_time = Instant::now();
        entry.performance_score = 1.0;
        entry.produced_mutations = HashSet::new();
        entry.feature_frequency = HashMap::new();
        entry.module_performance = HashMap::new();
        entry.module_features = HashMap::new();
        self.entries.push_back(entry);
        
        //println!("[CORPUS DEBUG] Added new entry. Total entries: {}", self.entries.len());
        if self.entries.len() % 10 == 0 {
            //println!("[CORPUS DEBUG] Corpus now has {} entries", self.entries.len());
        }
    }
    pub fn update_entry_success(&mut self, index: u32, new_coverage: u32) {
        if let Some(entry) = self.entries.iter_mut().find(|e| e.index == index) {
            entry.success_count += 1;
            entry.last_coverage_found = Instant::now();
            entry.coverage_found += new_coverage;
            //println!("[CORPUS DEBUG] Entry {} updated with {} new coverage. Total coverage: {}", 
                    // index, new_coverage, entry.coverage_found);
        } else {
            //println!("[CORPUS DEBUG] Failed to update entry {}: not found", index);
        }
        self.last_new_coverage = Instant::now();
        self.total_coverage.insert(index as u64, new_coverage as u64);
    }
   
   
    pub fn delete_entry(&mut self, index: u32) {
        // Find the position of the entry in the VecDeque
        if let Some(pos) = self.entries.iter().position(|e| e.index == index) {
            // Remove entry at that position
            self.entries.remove(pos);
        }
    }
   
    pub fn get_feature_count(&self, feature: u64) -> u64 {
        self.total_coverage.get(&feature).cloned().unwrap_or(0)
    }
    pub fn select_random_input(&mut self) -> Option<CorpusEntry> {
        let mut rng = rand::thread_rng();
        let index = rng.gen_range(0..self.entries.len());
        Some(self.entries[index].clone())
    }
    pub fn select_next_input(&mut self) -> Option<CorpusEntry> {
        // return None;
        let mut rng = rand::thread_rng();
        
        // Increment selection counter
        self.selection_counter += 1;
      
        // Dump stats periodically
        if self.selection_counter % 10000 == 0 {
            self.dump_stats_to_json();
        }
        
        // Check if corpus is empty
        if self.entries.is_empty() {
            //println!("[CORPUS DEBUG] Cannot select entry: corpus is empty");
            return None;
        }
        
        // Calculate scores for each entry
        let scores: Vec<_> = self.entries.iter().enumerate()
            .map(|(idx, entry)| {
                let mut score = entry.performance_score;
                
                // Prioritize smaller code size (inverse relationship)
                let size_factor = 1.0 / (1.0 + entry.js_code.len() as f64 * 0.001);
                
                // Reward success count and coverage found
                let success_factor = 1.0 + entry.success_count as f64 * 0.2;
                let coverage_factor = 1.0 + entry.coverage_found as f64 * 0.1;
                
                // Penalize errors and timeouts
                let error_penalty = 1.0 / (1.0 + entry.error_count as f64 * 0.3);
                let timeout_penalty = 1.0 / (1.0 + entry.timeout_count as f64 * 0.4);
                
                // Penalize overused entries (stronger penalty)
                let usage_penalty = 1.0 / (1.0 + entry.times_used as f64 * 0.2);
                
                // Calculate final score combining all factors
                score *= size_factor * success_factor * coverage_factor * error_penalty * timeout_penalty * usage_penalty;
                
                (idx, score)
            })
            .collect();

        // Select entry based on scores
        let total_score: f64 = scores.iter().map(|(_, score)| *score).sum();
        if total_score <= 0.0 {
            //println!("[CORPUS DEBUG] Cannot select entry: total score is zero");
            return None;
        }

        let mut selection = rng.gen::<f64>() * total_score;
        for (idx, score) in scores {
            selection -= score;
            if selection <= 0.0 {
                self.entries[idx].times_used += 1;
                let selected_entry = self.entries[idx].clone();
                
                // Log every 1000 selections
                if self.selection_counter % 10000 == 0 {
                    //println!("[CORPUS DEBUG] Selected entry {} (score: {:.2}). Times used: {}", 
                            // selected_entry.index, score, selected_entry.times_used);
                }
                
                return Some(selected_entry);
            }
        }

        //println!("[CORPUS DEBUG] Failed to select entry despite having entries. This shouldn't happen.");
        None
    }

    
   
    fn update_worker_files_json(&self) {
        // Get all XML files in stats directory
        let stats_dir = std::path::Path::new("stats/stats_workers");
        if !stats_dir.exists() {
            std::fs::create_dir_all(stats_dir).unwrap();
        }

        let worker_files: Vec<String> = std::fs::read_dir(stats_dir)
            .unwrap()
            .filter_map(|entry| {
                let entry = entry.unwrap();
                let path = entry.path();
                if path.extension().map_or(false, |ext| ext == "xml") {
                    Some(format!("stats_workers/{}", path.file_name().unwrap().to_string_lossy()))
                } else {
                    None
                }
            })
            .collect();

        // Write to worker_files.json
        let json = serde_json::to_string(&worker_files).unwrap();
        std::fs::write("stats/worker_files.json", json).unwrap();
    }

    pub fn print_stats(&self, num_lines: u16, is_console: bool) {
          // Stage distribution
          let mut total_coverage = 0;
          let mut total_used = 0;
          let mut total_success = 0;
          for entry in &self.entries {
              total_coverage += entry.coverage_found;
              total_used += entry.times_used;
              total_success += entry.success_count;
          }

   
        if is_console {
       
        print!("\x1B[s");
        print!("\x1B[{};1H", num_lines);
    
        for _ in 0..15 {  // Increased for more stats lines
            print!("\x1B[2K");  // Clear entire line
            print!("\x1B[1B");  // Move down one line
        }
        print!("\x1B[{};1H", num_lines); 
        print!("\x1B[?1049h");
    
        println!("=== Corpus Statistics ===");
        println!("Total entries: {}", self.entries.len());
        
      
        // Coverage stats
        println!("\nCoverage Statistics:");
        println!("  Total edges covered: {}", total_coverage);
        println!("  Time since last new coverage: {:?}", self.last_new_coverage.elapsed());
        
        // Bytecode stats
        if let Some((patterns, instructions, functions, analyses)) = self.get_bytecode_stats() {
            println!("\nBytecode Analysis Statistics:");
            println!("  Unique instruction patterns: {}", patterns);
            println!("  Unique instructions: {}", instructions);
            println!("  Unique function patterns: {}", functions);
            println!("  Total analyses performed: {}", analyses);
        }
        
        // Success rates
        let success_rate = if total_used > 0 {
            self.stats.successful_mutations as f64 / total_used as f64
        } else {
            0.0
        };
        println!("\nMutation Statistics:");
        println!("  Total attempts: {}", total_used);
        println!("  Successful mutations: {} ({:.1}%)", 
            self.stats.successful_mutations, success_rate * 100.0);
        
   
        // Stopping criteria indicators
        let coverage_stalled = self.last_new_coverage.elapsed() > Duration::from_secs(600); // 10 minutes
        let success_rate_low = success_rate < 0.01; // 1%
        
        println!("\nStopping Criteria Status:");
        println!("  Coverage stalled: {}", if coverage_stalled { "YES" } else { "no" });
        println!("  Success rate low: {}", if success_rate_low { "YES" } else { "no" });
    
        print!("\x1B[?1049l");
        print!("\x1B[u");
        }
        else{
        // let timestamp = chrono::Utc::now().timestamp();
        // let xml_entry = format!(
        //     "<entry>\n  <timestamp>{}</timestamp>\n  <worker_id>{}</worker_id>\n  <total_entries>{}</total_entries>\n  <coverage>{}</coverage>\n  <last_coverage_time>{}</last_coverage_time>\n  <total_attempts>{}</total_attempts>\n  <success_rate>{:.2}</success_rate>\n  <stages>\n",
        //     timestamp,
        //     self.worker_id,
        //     self.entries.len(),
        //     total_coverage,
        //     self.last_new_coverage.elapsed().as_secs(),
        //     total_used,
        //     if total_used > 0 { total_success as f64 / total_used as f64 } else { 0.0 }
        // );
    
        // let mut stage_stats = String::new();
        // let mut stage_counts = vec![0; Self::STAGE_MUTATOR_RANGES.len()];
        // let mut stage_successes = vec![0; Self::STAGE_MUTATOR_RANGES.len()];
        // for entry in &self.entries {
        //     stage_counts[entry.mutation_stage as usize] += entry.times_used;
        //     stage_successes[entry.mutation_stage as usize] += entry.success_count;
        // }
        
        // for (stage, &count) in stage_counts.iter().enumerate() {
        //     let success_rate = if count > 0 {
        //         stage_successes[stage] as f64 / count as f64
        //     } else {
        //         0.0
        //     };
        //     stage_stats.push_str(&format!(
        //         "    <stage>\n      <id>{}</id>\n      <count>{}</count>\n      <success_rate>{:.2}</success_rate>\n    </stage>\n",
        //         stage, count, success_rate
        //     ));
        // }
    
        // let xml_entry = format!("{}{}</stages>\n</entry>\n", xml_entry, stage_stats);
    
        // if let Ok(mut file) = OpenOptions::new()
        //     .create(true)
        //     .append(true)
        //     .open(format!("stats/stats_workers/worker_{}.xml", self.worker_id)) 
        // {
        //     if file.metadata().unwrap().len() == 0 {
        //         let _ = file.write_all(b"<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<fuzz_stats>\n");
        //     }
        //     let _ = file.write_all(xml_entry.as_bytes());
        // }
        // self.update_worker_files_json();
        }
    }

    pub fn record_mutation_result(&mut self, success: bool) {
        self.stats.total_mutations += 1;
        if success {
            self.stats.successful_mutations += 1;
        }
    }

    pub fn should_reseed(&self) -> bool {
        self.entries.len() < 10 || self.last_new_coverage.elapsed() > Duration::from_secs(600)
    }


    // Improved stage advancement logic
    fn should_advance_stage(&self, entry: &CorpusEntry) -> bool {
        // Basic criteria
        let basic_criteria = entry.times_used >= Self::MIN_TRIES_BEFORE_ADVANCE 
            && entry.last_coverage_found.elapsed() > Self::COVERAGE_STALENESS_THRESHOLD;

        if !basic_criteria {
            return false;
        }

        // Calculate success rate for current stage
        let success_rate = entry.success_count as f64 / entry.times_used as f64;
        
        // Calculate mutator diversity
        // let mutator_diversity = entry.feature_frequency.len() as f64 
        //     / (Self::STAGE_MUTATOR_RANGES[entry.mutation_stage as usize].1 
        //        - Self::STAGE_MUTATOR_RANGES[entry.mutation_stage as usize].0 + 1) as f64;
        let mutator_diversity = 1.0;

        // Advance if either:
        // 1. Success rate is too low, or
        // 2. We've tried most mutators in current stage with moderate success
        success_rate < Self::SUCCESS_RATE_THRESHOLD 
            || (mutator_diversity > 0.7 && success_rate < 0.1)
    }

    /// Returns a random program IR string from the corpus entries
    /// If no entries exist, returns an empty string
    pub fn get_random_program_ir(&self) -> String {
        if self.entries.is_empty() {
            return String::new();
        }
        
        let mut rng = rand::thread_rng();
        let index = rng.gen_range(0..self.entries.len());
        self.entries[index].program_ir.clone()
    }

    pub fn dump_stats_to_json(&self) {
        // Create stats directory if it doesn't exist
        let stats_dir = std::path::Path::new("stats");
        if !stats_dir.exists() {
            std::fs::create_dir_all(stats_dir).unwrap();
        }
        
        // Create worker-specific directory
        let worker_dir_path = format!("stats/worker_{}", self.worker_id);
        if !std::path::Path::new(&worker_dir_path).exists() {
            std::fs::create_dir_all(&worker_dir_path).unwrap();
        }
        
        // Log corpus state
        // println!("\n[CORPUS DEBUG] === DUMPING STATS ===");
        //println!("[CORPUS DEBUG] Total entries in corpus: {}", self.entries.len());
        //println!("[CORPUS DEBUG] Total coverage tracked: {}", self.total_coverage.len());
        //println!("[CORPUS DEBUG] Selection counter: {}", self.selection_counter);
        
        if !self.entries.is_empty() {
            let avg_size = self.entries.iter().map(|e| e.js_code.len()).sum::<usize>() as f64 / self.entries.len() as f64;
            let min_size = self.entries.iter().map(|e| e.js_code.len()).min().unwrap_or(0);
            let max_size = self.entries.iter().map(|e| e.js_code.len()).max().unwrap_or(0);
            //println!("[CORPUS DEBUG] Entry size stats - Avg: {:.1}, Min: {}, Max: {}", 
                    // avg_size, min_size, max_size);
                    
            let total_coverage: u32 = self.entries.iter().map(|e| e.coverage_found).sum();
            //println!("[CORPUS DEBUG] Total coverage across entries: {}", total_coverage);
            
            // Display some sample entries
            //println!("[CORPUS DEBUG] Sample entries:");
            for (i, entry) in self.entries.iter().take(3).enumerate() {
                //println!("[CORPUS DEBUG] Entry {}: size={}, used={}, success={}, coverage={}, errors={}, timeouts={}",
                        // i, entry.js_code.len(), entry.times_used, entry.success_count, 
                        // entry.coverage_found, entry.error_count, entry.timeout_count);
            }
        } else {
            //println!("[CORPUS DEBUG] WARNING: Corpus is empty! No entries have been added.");
        }
        
        // Count total coverage found across all entries
        let total_coverage_found: u32 = self.entries.iter().map(|e| e.coverage_found).sum();
        
        // Build statistics to dump
        let stats = serde_json::json!({
            "timestamp": std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs(),
            "worker_id": self.worker_id,
            "selection_count": self.selection_counter,
            "corpus_stats": {
                "total_entries": self.entries.len(),
                "total_coverage": total_coverage_found,
                "unique_edges": self.total_coverage.len(),
                "time_since_last_coverage": self.last_new_coverage.elapsed().as_secs(),
                "total_mutations": self.stats.total_mutations,
                "successful_mutations": self.stats.successful_mutations,
                "success_rate": if self.stats.total_mutations > 0 {
                    self.stats.successful_mutations as f64 / self.stats.total_mutations as f64
                } else { 0.0 },
            },
            "entry_statistics": self.calculate_entry_statistics()
        });
        
        // Write JSON to file in worker-specific directory
        let filename = format!("stats/worker_{}/stats_{}.json", self.worker_id, self.selection_counter);
        std::fs::write(&filename, serde_json::to_string_pretty(&stats).unwrap())
            .unwrap_or_else(|e| eprintln!("Failed to write stats to {}: {}", filename, e));
        
        // Also write to a fixed filename for easy access to latest stats (in worker directory)
        let latest_stats_path = format!("stats/worker_{}/latest_stats.json", self.worker_id);
        std::fs::write(&latest_stats_path, serde_json::to_string_pretty(&stats).unwrap())
            .unwrap_or_else(|e| eprintln!("Failed to write latest stats: {}", e));
        
        //println!("[CORPUS DEBUG] Stats written to {} and {}", filename, latest_stats_path);
        //println!("[CORPUS DEBUG] === END OF STATS DUMP ===\n");
    }
    
    fn calculate_entry_statistics(&self) -> serde_json::Value {
        if self.entries.is_empty() {
            return serde_json::json!({
                "size_distribution": {
                    "min_size": 0,
                    "max_size": 0,
                    "average_size": 0,
                    "buckets": [0, 0, 0, 0, 0],
                    "bucket_size": 0
                },
                "usage_distribution": {
                    "avg_usage": 0,
                    "max_usage": 0,
                    "total_usage": 0,
                    "usage_histogram": [0, 0, 0, 0, 0, 0, 0, 0, 0, 0]
                },
                "performance_distribution": {
                    "size_factors": { "avg": 0.0, "min": 0.0, "max": 0.0 },
                    "success_factors": { "avg": 0.0, "min": 0.0, "max": 0.0 },
                    "coverage_factors": { "avg": 0.0, "min": 0.0, "max": 0.0 },
                    "error_penalties": { "avg": 0.0, "min": 0.0, "max": 0.0 },
                    "timeout_penalties": { "avg": 0.0, "min": 0.0, "max": 0.0 },
                    "usage_penalties": { "avg": 0.0, "min": 0.0, "max": 0.0 }
                }
            });
        }
        
        serde_json::json!({
            "size_distribution": self.calculate_size_distribution(),
            "usage_distribution": self.calculate_usage_distribution(),
            "performance_distribution": self.calculate_performance_distribution(),
            "top_entries": self.get_top_entries(5)
        })
    }
    
    fn calculate_size_distribution(&self) -> serde_json::Value {
        if self.entries.is_empty() {
            return serde_json::json!({
                "min_size": 0,
                "max_size": 0,
                "average_size": 0,
                "buckets": [0, 0, 0, 0, 0],
                "bucket_size": 0
            });
        }
        
        // Calculate size distribution in 5 buckets
        let mut size_buckets = vec![0; 5];
        let mut min_size = usize::MAX;
        let mut max_size = 0;
        
        // Find min and max sizes
        for entry in &self.entries {
            let size = entry.js_code.len();
            min_size = min_size.min(size);
            max_size = max_size.max(size);
        }
        
        // Calculate bucket size
        let range = if max_size > min_size { max_size - min_size } else { 1 };
        let bucket_size = range / 5 + 1;
        
        // Count entries in each bucket
        for entry in &self.entries {
            let size = entry.js_code.len();
            let bucket = ((size - min_size) / bucket_size).min(4);
            size_buckets[bucket] += 1;
        }
        
        serde_json::json!({
            "min_size": min_size,
            "max_size": max_size,
            "average_size": self.entries.iter().map(|e| e.js_code.len()).sum::<usize>() as f64 / self.entries.len() as f64,
            "buckets": size_buckets,
            "bucket_size": bucket_size
        })
    }
    
    fn calculate_usage_distribution(&self) -> serde_json::Value {
        if self.entries.is_empty() {
            return serde_json::json!({
                "avg_usage": 0,
                "max_usage": 0,
                "total_usage": 0,
                "usage_histogram": [0, 0, 0, 0, 0, 0, 0, 0, 0, 0]
            });
        }
        
        // Calculate usage statistics
        let usage_counts: Vec<u32> = self.entries.iter().map(|e| e.times_used).collect();
        let total_usage: u32 = usage_counts.iter().sum();
        
        let avg_usage = total_usage as f64 / self.entries.len() as f64;
        let max_usage = usage_counts.iter().max().copied().unwrap_or(0);
        
        serde_json::json!({
            "avg_usage": avg_usage,
            "max_usage": max_usage,
            "total_usage": total_usage,
            "usage_histogram": self.calculate_histogram(usage_counts, 10)
        })
    }
    
    fn calculate_performance_distribution(&self) -> serde_json::Value {
        if self.entries.is_empty() {
            return serde_json::json!({
                "size_factors": { "avg": 0.0, "min": 0.0, "max": 0.0 },
                "success_factors": { "avg": 0.0, "min": 0.0, "max": 0.0 },
                "coverage_factors": { "avg": 0.0, "min": 0.0, "max": 0.0 },
                "error_penalties": { "avg": 0.0, "min": 0.0, "max": 0.0 },
                "timeout_penalties": { "avg": 0.0, "min": 0.0, "max": 0.0 },
                "usage_penalties": { "avg": 0.0, "min": 0.0, "max": 0.0 }
            });
        }
        
        // Collect data about the selection factors
        let mut size_factors = Vec::new();
        let mut success_factors = Vec::new();
        let mut coverage_factors = Vec::new();
        let mut error_penalties = Vec::new();
        let mut timeout_penalties = Vec::new();
        let mut usage_penalties = Vec::new();
        
        for entry in &self.entries {
            // Calculate the factors used in selection
            let size_factor = 1.0 / (1.0 + entry.js_code.len() as f64 * 0.001);
            let success_factor = 1.0 + entry.success_count as f64 * 0.2;
            let coverage_factor = 1.0 + entry.coverage_found as f64 * 0.1;
            let error_penalty = 1.0 / (1.0 + entry.error_count as f64 * 0.3);
            let timeout_penalty = 1.0 / (1.0 + entry.timeout_count as f64 * 0.4);
            let usage_penalty = 1.0 / (1.0 + entry.times_used as f64 * 0.2);
            
            size_factors.push(size_factor);
            success_factors.push(success_factor);
            coverage_factors.push(coverage_factor);
            error_penalties.push(error_penalty);
            timeout_penalties.push(timeout_penalty);
            usage_penalties.push(usage_penalty);
        }
        
        // Calculate statistics for each factor
        serde_json::json!({
            "size_factors": self.calculate_factor_stats(&size_factors),
            "success_factors": self.calculate_factor_stats(&success_factors),
            "coverage_factors": self.calculate_factor_stats(&coverage_factors),
            "error_penalties": self.calculate_factor_stats(&error_penalties),
            "timeout_penalties": self.calculate_factor_stats(&timeout_penalties),
            "usage_penalties": self.calculate_factor_stats(&usage_penalties)
        })
    }
    
    fn calculate_factor_stats(&self, factors: &[f64]) -> serde_json::Value {
        if factors.is_empty() {
            return serde_json::json!({
                "avg": 0.0,
                "min": 0.0,
                "max": 0.0
            });
        }
        
        let avg = factors.iter().sum::<f64>() / factors.len() as f64;
        let min = factors.iter().fold(f64::INFINITY, |a, &b| a.min(b));
        let max = factors.iter().fold(0.0, |a, &b| f64::max(a, b));
        
        serde_json::json!({
            "avg": avg,
            "min": min,
            "max": max
        })
    }
    
    fn get_top_entries(&self, count: usize) -> serde_json::Value {
        if self.entries.is_empty() {
            return serde_json::json!([]);
        }
        
        // Calculate scores for all entries
        let mut entry_scores: Vec<(u32, f64, usize)> = self.entries.iter().enumerate()
            .map(|(idx, entry)| {
                let size_factor = 1.0 / (1.0 + entry.js_code.len() as f64 * 0.001);
                let success_factor = 1.0 + entry.success_count as f64 * 0.2;
                let coverage_factor = 1.0 + entry.coverage_found as f64 * 0.1;
                let error_penalty = 1.0 / (1.0 + entry.error_count as f64 * 0.3);
                let timeout_penalty = 1.0 / (1.0 + entry.timeout_count as f64 * 0.4);
                let usage_penalty = 1.0 / (1.0 + entry.times_used as f64 * 0.2);
                
                let score = entry.performance_score * size_factor * success_factor * 
                           coverage_factor * error_penalty * timeout_penalty * usage_penalty;
                
                (entry.index, score, idx)
            })
            .collect();
        
        // Sort by score (highest first)
        entry_scores.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        
        // Take top N entries
        let top_entries: Vec<serde_json::Value> = entry_scores.iter()
            .take(count)
            .map(|(index, score, idx)| {
                let entry = &self.entries[*idx];
                serde_json::json!({
                    "index": index,
                    "score": score,
                    "js_code_size": entry.js_code.len(),
                    "times_used": entry.times_used,
                    "success_count": entry.success_count,
                    "coverage_found": entry.coverage_found,
                    "error_count": entry.error_count,
                    "timeout_count": entry.timeout_count
                })
            })
            .collect();
        
        serde_json::json!(top_entries)
    }
    
    fn calculate_histogram(&self, values: Vec<u32>, num_buckets: usize) -> Vec<usize> {
        if values.is_empty() {
            return vec![0; num_buckets];
        }
        
        let max_value = *values.iter().max().unwrap_or(&0) as usize;
        if max_value == 0 {
            return vec![values.len(), 0, 0, 0, 0, 0, 0, 0, 0, 0];
        }
        
        let bucket_size = (max_value / num_buckets) + 1;
        let mut buckets = vec![0; num_buckets];
        
        for value in values {
            let bucket = (value as usize / bucket_size).min(num_buckets - 1);
            buckets[bucket] += 1;
        }
        
        buckets
    }

    pub fn update_entry_error(&mut self, index: u32) {
        if let Some(entry) = self.entries.iter_mut().find(|e| e.index == index) {
            entry.error_count += 1;
            entry.last_used = Instant::now();
            
            // Update performance score to penalize errors
            // Reduce performance score by 5% for each error
            entry.performance_score *= 0.95;
            //println!("[CORPUS DEBUG] Entry {} error count increased to {}. New performance score: {:.2}", 
                    // index, entry.error_count, entry.performance_score);
        } else {
            //println!("[CORPUS DEBUG] Failed to update error for entry {}: not found", index);
        }
        
        // Record this mutation as unsuccessful
        self.record_mutation_result(false);
    }
    
    pub fn update_entry_timeout(&mut self, index: u32) {
        if let Some(entry) = self.entries.iter_mut().find(|e| e.index == index) {
            entry.timeout_count += 1;
            entry.last_used = Instant::now();
            
            // Update performance score to penalize timeouts
            // Reduce performance score by 10% for each timeout
            entry.performance_score *= 0.90;
            //println!("[CORPUS DEBUG] Entry {} timeout count increased to {}. New performance score: {:.2}", 
                    // index, entry.timeout_count, entry.performance_score);
        } else {
            //println!("[CORPUS DEBUG] Failed to update timeout for entry {}: not found", index);
        }
        
        // Record this mutation as unsuccessful
        self.record_mutation_result(false);
    }

    /// Initialize bytecode collector for this corpus manager
    pub fn init_bytecode_collector(&mut self) {
        // Initialize the special worker 101 for bytecode collection if not already done
        // static mut BYTECODE_WORKER_INITIALIZED: bool = false;
        unsafe {
            // if !BYTECODE_WORKER_INITIALIZED {
                crate::coverage::init_reprl_safe(100 + self.worker_id as usize);
                // BYTECODE_WORKER_INITIALIZED = true;
                println!("[BYTECODE] Initialized worker 101 for bytecode collection");
            // }
        }
        
        self.bytecode_collector = Some(BytecodeCollector::new(self.worker_id));
    }

    /// Analyze entry for bytecode novelty when it doesn't produce new coverage
    /// Returns true if the entry should be kept due to novel bytecode patterns
    pub fn analyze_bytecode_novelty(&mut self, entry: &mut CorpusEntry) -> bool {
        if let Some(ref mut collector) = self.bytecode_collector {
            match collector.analyze_js_bytecode(&entry.js_code, self.worker_id as u32) {
                Ok((analysis, is_novel)) => {
                    entry.bytecode_analysis = Some(analysis);
                    entry.has_novel_bytecode = is_novel;
                    
                    if is_novel {
                        println!("[BYTECODE DEBUG] Entry {} has novel bytecode patterns", entry.index);
                        return true;
                    }
                }
                Err(e) => {
                    // println!("[BYTECODE DEBUG] Failed to analyze bytecode for entry {}: {}", entry.index, e);
                }
            }
        }
        false
    }

    /// Get bytecode collection statistics
    pub fn get_bytecode_stats(&self) -> Option<(usize, usize, usize, u32)> {
        self.bytecode_collector.as_ref().map(|c| c.get_stats())
    }

    /// Check if an entry should be kept in corpus based on coverage or bytecode novelty
    pub fn should_keep_entry(&mut self, entry: &mut CorpusEntry, has_new_coverage: bool) -> bool {
        if has_new_coverage {
            return true;
        }
        
        // If no new coverage, check for bytecode novelty
        self.analyze_bytecode_novelty(entry)
    }

}