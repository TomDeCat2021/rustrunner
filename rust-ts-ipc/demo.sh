#!/bin/bash

echo "=== Rust-TypeScript Generator Integration Demo ==="
echo ""
echo "This demonstrates Rust using TypeScript to generate test cases"
echo "from the gen3mutator fast generator."
echo ""

# Create output directory
mkdir -p rust-generated

# Run the generator client
echo "Running generator client (will generate 20 test cases)..."
echo ""

# Temporarily modify the client to generate fewer files for demo
cargo run --bin generator_client

echo ""
echo "Demo complete! Check the 'rust-generated' directory for generated test cases."
echo ""
echo "Files generated:"
ls -la rust-generated/*.js 2>/dev/null | head -5
echo "..."
echo ""
echo "Example content:"
head -20 rust-generated/test_000001.js 2>/dev/null || echo "No files generated"