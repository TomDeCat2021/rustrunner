use std::fs;
use std::path::{Path, PathBuf};
use std::collections::HashSet;

use std::ptr;

// Define the EdgeSet struct for coverage tracking
#[repr(C)]
#[derive(Debug, Clone)]
pub struct EdgeSet {
    pub count: u32,
    pub edge_indices: *mut u32,
}

impl  EdgeSet {
   pub fn new() -> Self {
    EdgeSet { count: 0, edge_indices: ptr::null_mut() }
   }
  
}
#[derive(PartialEq,Debug)]
pub enum ResultCode {
    Success,
    Timeout,
    Error,
    Crash,
}

#[repr(C)]
#[derive(Debug)]
pub struct CmpEvent {
    pub left: i64,
    pub right: i64,
}

unsafe extern "C" {
    pub fn init(worker_id: i32);
    pub fn spawn(worker_id: i32);
    pub fn execute_script(script: *mut i8, timeout: i32, fresh_instance: i32, worker_id: i32) -> i32;
    pub fn cov_evaluate(worker_id: usize, edges: *mut EdgeSet) -> i32;
    pub fn coverage_finish_initialization(worker_id: usize, should_track_edges: i32);
    pub fn reprl_destroy_context(worker_id: usize);
    pub fn cov_clear_edge_data(worker_id: usize, index: u32);
    pub fn cov_set_edge_data(worker_id: usize, index: u32);
    pub fn reprl_fetch_stdout(worker_id: i32) -> *mut i8;
    pub fn cleanup_reprl(worker_id: i32); 
    pub fn cov_fetch_cmp_events(worker_id: i32) -> *mut CmpEvent;
    pub fn fetch_event_count(worker_id: i32) -> u64;
    pub fn cov_clear_cmp_events(worker_id: i32);
}
pub fn reset_edge_set(worker_id: usize, edge_set: &mut EdgeSet) {
    for i in 0..edge_set.count {
        unsafe {
            // println!("Clearing edge data for worker {} index {}", worker_id, *edge_set.edge_indices.add(i as usize));
            crate::cov_clear_edge_data(worker_id, *edge_set.edge_indices.add(i as usize));
        }
    }
}
pub fn mark_edge_set(worker_id: usize, edge_set: &mut EdgeSet) {
    
    for i in 0..edge_set.count {
        unsafe {
            crate::cov_set_edge_data(worker_id, *edge_set.edge_indices.add(i as usize));
        }
    }
}

pub fn get_result_code(result_code: i32) -> ResultCode {
    if result_code == 0 {
        return ResultCode::Success;
    }

    if result_code == 65536 {
        return ResultCode::Timeout;
    }
    let profile = std::env::var("PROFILE").unwrap_or_else(|_| "v8".to_string());
    if profile == "v8" {
        if result_code == 5 || result_code == 6 || result_code == 11 {
            return ResultCode::Crash;
        }
        return ResultCode::Error;
    }

    if profile == "gecko" {
        if result_code == 256 {
            return ResultCode::Crash;
        }
    }
    if profile == "jsc" {
        if result_code == 256 || result_code == 6 || result_code == 11 {
            return ResultCode::Crash;
        }
    }

    ResultCode::Error
}
pub fn init_reprl_safe(worker_id: usize) {
    unsafe {
        init(worker_id as i32);
        spawn(worker_id as i32);
        coverage_finish_initialization(worker_id, 0);
    }
}
pub fn v8_reprl_check(worker_id: i32){

    let test_code = "var x = 1;";
    let test_code = format!("{}\x00", test_code);
    let result = unsafe { execute_script(test_code.as_ptr() as *mut i8, 100, 0, worker_id) };
    println!("Success result: {}", result);
    assert_eq!(get_result_code(result), ResultCode::Success);
    // Check timeout
    let test_code = "while(true){}";
    let test_code = format!("{}\x00", test_code);
    let result = unsafe { execute_script(test_code.as_ptr() as *mut i8, 100, 0, worker_id) };
    println!("Timeout result: {}", result);
    assert_eq!(get_result_code(result), ResultCode::Timeout); //timeout code

    let test_code = "var x =";
    let test_code = format!("{}\x00", test_code);
    let result = unsafe { execute_script(test_code.as_ptr() as *mut i8, 1000, 0, worker_id) };
    println!("Error result: {}", result);
    assert_eq!(get_result_code(result), ResultCode::Error); //error code

    let test_code = "fuzzilli('FUZZILLI_CRASH', 0);";
    let test_code = format!("{}\x00", test_code);
    let result = unsafe { execute_script(test_code.as_ptr() as *mut i8, 1000, 0, worker_id) };
    println!("Crash result: {}", result);
    assert_eq!(get_result_code(result), ResultCode::Crash);

    let test_code = "fuzzilli('FUZZILLI_CRASH', 1);";
    let test_code = format!("{}\x00", test_code);
    let result = unsafe { execute_script(test_code.as_ptr() as *mut i8, 1000, 0, worker_id) };
    println!("Crash result: {}", result);
    assert_eq!(get_result_code(result), ResultCode::Crash);

    let test_code = "fuzzilli('FUZZILLI_CRASH', 2);";
    let test_code = format!("{}\x00", test_code);
    let result = unsafe { execute_script(test_code.as_ptr() as *mut i8, 1000, 0, worker_id) };
    println!("Crash result: {}", result);
    assert_eq!(get_result_code(result), ResultCode::Crash);

    // let test_code = "fuzzilli('FUZZILLI_CRASH', 3);";
    // let test_code = format!("{}\x00", test_code);
    // let result = unsafe { execute_script(test_code.as_ptr() as *mut i8, 1000, 0, worker_id) };
    // println!("result: {}", result);
    // assert_eq!(get_result_code(result), ResultCode::Crash);

    // let test_code = "fuzzilli('FUZZILLI_CRASH', 8);";
    // let test_code = format!("{}\x00", test_code);
    // let result = unsafe { execute_script(test_code.as_ptr() as *mut i8, 1000 , 0, worker_id) };
    // assert_eq!(get_result_code(result), ResultCode::Crash);


}
pub fn gecko_reprl_check(worker_id: i32){
    let test_code = "var x = 1;";
    let test_code = format!("{}\x00", test_code);
    let result = unsafe { execute_script(test_code.as_ptr() as *mut i8, 100, 0, worker_id) };
    assert_eq!(get_result_code(result), ResultCode::Success);
    // Check timeout
    let test_code = "while(true){}";
    let test_code = format!("{}\x00", test_code);
    let result = unsafe { execute_script(test_code.as_ptr() as *mut i8, 100, 0, worker_id) };
    assert_eq!(get_result_code(result), ResultCode::Timeout); //timeout code

    let test_code = "var x =";
    let test_code = format!("{}\x00", test_code);
    let result = unsafe { execute_script(test_code.as_ptr() as *mut i8, 1000, 0, worker_id) };
    assert_eq!(get_result_code(result), ResultCode::Error); //error code

    let test_code = "fuzzilli('FUZZILLI_CRASH', 0);";
    let test_code = format!("{}\x00", test_code);
    println!("test_code: {}", test_code);
    let result = unsafe { execute_script(test_code.as_ptr() as *mut i8, 1000, 0, worker_id) };
    assert_eq!(get_result_code(result), ResultCode::Crash);

    let test_code = "fuzzilli('FUZZILLI_CRASH', 1);";
    let test_code = format!("{}\x00", test_code);
    let result = unsafe { execute_script(test_code.as_ptr() as *mut i8, 1000, 0, worker_id) };
    assert_eq!(get_result_code(result), ResultCode::Crash);

    let test_code = "fuzzilli('FUZZILLI_CRASH', 2);";
    let test_code = format!("{}\x00", test_code);
    let result = unsafe { execute_script(test_code.as_ptr() as *mut i8, 1000, 0, worker_id) };
    assert_eq!(get_result_code(result), ResultCode::Crash);
}

pub fn common_subset(set1: &mut [u32], set2: &mut [u32]) -> Vec<u32> {
    let set1: HashSet<_> = set1.iter().copied().collect();
    set2.iter()
        .copied()
        .filter(|idx| set1.contains(idx))
        .collect()
}


/// Extract coverage of a testcase with proper initialization
pub fn extract_testcase_coverage(
    js_code: &str,
    worker_id: usize,
    mutated_edges: &EdgeSet,
) -> EdgeSet {
    let test_code = js_code;
    let mut edges = mutated_edges.clone();
    
    // Run the test multiple times and collect common edges
    let mut last_common_len = 0;
    for _ in 0..5 {
        unsafe {
            crate::execute_script(test_code.as_ptr() as *mut i8, crate::MAX_TIMEOUT, 0, worker_id as i32);
        }
        let mut new_edges = EdgeSet::new();
        unsafe {
            crate::cov_evaluate(worker_id, &mut new_edges);
        }
        reset_edge_set(worker_id, &mut new_edges);
        // Convert edge indices to Vec<u32> for common_subset calculation
        let common = if edges.count > 0 && new_edges.count > 0 {
            let edges_slice = unsafe { std::slice::from_raw_parts_mut(edges.edge_indices, edges.count as usize) };
            let new_edges_slice = unsafe { std::slice::from_raw_parts_mut(new_edges.edge_indices, new_edges.count as usize) };
            common_subset(edges_slice, new_edges_slice)
        } else {
            Vec::new()
        };
        
        // Update edges with common subset
        if !common.is_empty() {
            if last_common_len == common.len() {
               break;
            }
            last_common_len = common.len();
            edges.count = common.len() as u32;
            edges.edge_indices = common.as_ptr() as *mut u32;
            std::mem::forget(common); // Prevent deallocation since we're using the raw pointer
        }
        
    }
    
    edges
}


pub fn maintain_coverage_with_mutated_edges(
    js_code: &str,
    worker_id: usize,
    mutated_edges: &EdgeSet,
) -> (bool, bool) {
    let test_code = js_code;
    let mut edges = mutated_edges.clone();
    let mut is_new_coverage = false;
    for i in 0..5 {
        unsafe {
            let result =crate::execute_script(
                test_code.as_ptr() as *mut i8,
                crate::MAX_TIMEOUT,
                0,
                worker_id as i32,
            );
            let mut new_edges = EdgeSet::new();
            crate::cov_evaluate(worker_id, &mut new_edges);
            reset_edge_set(worker_id, &mut new_edges);
            if get_result_code(result) == ResultCode::Crash {
                is_new_coverage = true;
            }
            if get_result_code(result) == ResultCode::Success {
                if new_edges.count > edges.count {
                    is_new_coverage = true;
                }
                let mut is_found = false;
                for i in 0..new_edges.count {
                    let edge_idx = unsafe { *new_edges.edge_indices.add(i as usize) };
                    for j in 0..edges.count {
                        if unsafe { *edges.edge_indices.add(j as usize) } == edge_idx {
                            is_found = true;
                            break;
                        }
                    }
                    
                }
                if !is_found {
                    is_new_coverage = true;
                }
            }


           
            // reset the new edges so it can be triggered again
            // println!("Original edges count {} New edges count {}", edges.count, new_edges.count);
            // check if original edges are subset of new edges
            let mut found_edges = Vec::new();
            for i in 0..edges.count {
                let original_edge = unsafe { *edges.edge_indices.add(i as usize) };
                for j in 0..new_edges.count {
                    if unsafe { *new_edges.edge_indices.add(j as usize) } == original_edge {
                        // crate::cov_clear_edge_data(worker_id, original_edge);
                        found_edges.push(j);
                        break;
                    }
                    

                }
            }
            // println!("Found {} out of {} edges", found_edges.len(), edges.count);
            if found_edges.len() as f32 / edges.count as f32 > 0.8 {
                return (true, is_new_coverage);
            }
            // for i in 0..std::cmp::min(edges.count, new_edges.count) {
            //     println!("Original edge {} New edge {}", unsafe { *edges.edge_indices.add(i as usize) }, unsafe { *new_edges.edge_indices.add(i as usize) });
            // }
        }

    }
    (false, is_new_coverage)
}