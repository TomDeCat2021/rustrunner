#!/bin/bash

echo "Testing Rust-TypeScript Generator Integration..."
echo ""

# Check if TypeScript is built
if [ ! -f "ts-app/dist/generator-simple.js" ]; then
    echo "Building TypeScript code..."
    cd ts-app
    npx tsc src/generator-simple.ts --outDir dist --module commonjs --target es2022 --esModuleInterop --skipLibCheck
    cd ..
fi

# Check if Rust is built
if [ ! -f "target/debug/generator_client" ]; then
    echo "Building Rust code..."
    cargo build --bin generator_client
fi

echo "Starting generator test (10 test cases)..."
echo ""

# Run with timeout
timeout 60s cargo run --bin generator_client

echo ""
echo "Test complete!"