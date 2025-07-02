use anyhow::{anyhow, Result};
// use pyo3::prelude::*;
// use pyo3::types::{PyDict, PyList, PyTuple};
use std::collections::HashMap;
use std::thread;
use tokio::sync::mpsc::{self, Receiver, Sender};
use tokio::sync::oneshot;
use serde::{Deserialize, Serialize};
use serde_json;

// Message types for our channel
enum PythonRequest {
    Call {
        module_name: String,
        function_name: String,
        args: Vec<String>,
        response_tx: oneshot::Sender<Result<HashMap<String, String>>>,
    },
    Shutdown,
}

pub struct PythonWorker {
    request_tx: Sender<PythonRequest>,
}

impl Clone for PythonWorker {
    fn clone(&self) -> Self {
        Self {
            request_tx: self.request_tx.clone(),
        }
    }
}

impl PythonWorker {
    // Create a new Python worker
    pub fn new() -> Self {
        let (request_tx, request_rx) = mpsc::channel(100);
        
        // Spawn a thread that will keep the Python interpreter alive
        thread::spawn(move || {
            if let Err(e) = run_python_worker(request_rx) {
                eprintln!("Python worker error: {}", e);
            }
        });
        
        Self { request_tx }
    }
    
    // Call a Python function with the given arguments
    pub async fn call_python_function(
        &self, 
        module_name: &str,
        function_name: &str, 
        args: Vec<String>
    ) -> Result<HashMap<String, String>> {
        let (response_tx, response_rx) = oneshot::channel();
        
        self.request_tx
            .send(PythonRequest::Call {
                module_name: module_name.to_string(),
                function_name: function_name.to_string(),
                args,
                response_tx,
            })
            .await
            .map_err(|_| anyhow!("Failed to send request to Python worker"))?;
            
        response_rx.await.map_err(|_| anyhow!("Python worker was dropped"))?
    }
    
    // Shutdown the Python worker
    pub async fn shutdown(&self) -> Result<()> {
        self.request_tx
            .send(PythonRequest::Shutdown)
            .await
            .map_err(|_| anyhow!("Failed to send shutdown request to Python worker"))?;
        Ok(())
    }
}

// Convert a Python dictionary to a Rust HashMap
// fn py_dict_to_hashmap(py: Python, dict: &PyAny) -> Result<HashMap<String, String>> {
//     let mut result = HashMap::new();
    
//     // Check if the object is a dictionary
//     if let Ok(py_dict) = dict.downcast::<PyDict>() {
//         for (key, value) in py_dict.iter() {
//             let key_str = key.extract::<String>()?;
            
//             // Handle different value types
//             let value_str = match value.get_type().name()? {
//                 "dict" => {
//                     // For nested dictionaries, convert to JSON string
//                     let nested_dict = py_dict_to_hashmap(py, value)?;
//                     serde_json::to_string(&nested_dict)?
//                 },
//                 "list" => {
//                     // For lists, convert to JSON string
//                     let list: Vec<String> = value.extract()?;
//                     serde_json::to_string(&list)?
//                 },
//                 _ => {
//                     // For simple types, extract as string
//                     value.extract::<String>()?
//                 }
//             };
            
//             result.insert(key_str, value_str);
//         }
//     } else {
//         return Err(anyhow!("Object is not a dictionary"));
//     }
    
//     Ok(result)
// }

// The function that runs in the worker thread
fn run_python_worker(mut request_rx: Receiver<PythonRequest>) -> Result<()> {
    // Initialize the Python interpreter
    // pyo3::prepare_freethreaded_python();
    
    // Python::with_gil(|py| {
    //     // Import the Python module
    //     let sys = py.import("sys")?;
    //     let path = sys.getattr("path")?;
    //     path.call_method1("append", ("js_fuzzer",))?;
    //     path.call_method1("append", ("python",))?;
        
    //     // Cache for imported modules
    //     let mut module_cache: HashMap<String, PyObject> = HashMap::new();
        
    //     // Process requests until shutdown
    //     while let Some(request) = futures::executor::block_on(request_rx.recv()) {
    //         match request {
    //             PythonRequest::Call { module_name, function_name, args, response_tx } => {
    //                 let result = Python::with_gil(|py| {
    //                     // Get or import the module
    //                     let module = if let Some(module) = module_cache.get(&module_name) {
    //                         module.clone()
    //                     } else {
    //                         match py.import(module_name.as_str()) {
    //                             Ok(module) => {
    //                                 let module_obj = module.to_object(py);
    //                                 module_cache.insert(module_name.clone(), module_obj.clone());
    //                                 module_obj
    //                             }
    //                             Err(e) => {
    //                                 return Err(anyhow!("Failed to import module '{}': {}", module_name, e));
    //                             }
    //                         }
    //                     };
                        
    //                     let module = module.extract::<&PyAny>(py)?;
                        
    //                     // Get the function from the module
    //                     let func = module
    //                         .getattr(function_name.as_str())
    //                         .map_err(|_| anyhow!("Function '{}' not found", function_name))?;
                        
    //                     // Convert args to Python values
    //                     let py_args: Vec<PyObject> = args
    //                         .iter()
    //                         .map(|arg| arg.to_object(py))
    //                         .collect();
                        
    //                     // Create a tuple of Python arguments
    //                     let args_tuple = PyTuple::new(py, &py_args);
                        
    //                     // Call the function
    //                     let result = func.call1(args_tuple)?;
                        
    //                     // Convert the result to a Rust HashMap
    //                     py_dict_to_hashmap(py, result)
    //                 });
                    
    //                 let _ = response_tx.send(result);
    //             }
    //             PythonRequest::Shutdown => break,
    //         }
    //     }
        
    //     Ok(())
    // })
    Ok(())
} 