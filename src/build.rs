use std::env;
use std::path::PathBuf;
use cc::Build;

fn main() {
    // Compile the C code
    println!("cargo:rerun-if-changed=src/reprl/reprl.c");
    println!("cargo:rerun-if-changed=src/reprl/reprl.h");
    
    cc::Build::new()
        .file("src/reprl/reprl.c")
        .include("src/reprl")
        .compile("reprl");
}