#!/bin/bash

echo "Building TypeScript generator bridge..."
cd ts-app
npm install
npm run build
cd ..

echo "Building Rust binaries..."
cargo build --release

echo "Build complete!"
echo ""
echo "To run the generator client:"
echo "  cargo run --bin generator_client"
echo ""
echo "Or directly:"
echo "  ./target/release/generator_client"