# Rust-TypeScript Generator Integration

This extends the rust-ts-ipc example to integrate with the gen3mutator fast test case generator, allowing Rust to orchestrate JavaScript test case generation.

## Architecture

```
┌─────────────────┐         JSON Messages          ┌──────────────────┐
│   Rust Client   │ ──────────────────────────────> │   TS Generator   │
│  (Orchestrator) │                                  │     Bridge       │
│                 │ <────────────────────────────── │                  │
└─────────────────┘         Test Cases              └──────────────────┘
                                                              |
                                                              v
                                                    ┌──────────────────┐
                                                    │   gen3mutator    │
                                                    │  Fast Generator  │
                                                    └──────────────────┘
```

## Message Protocol

### Request Messages (Rust → TypeScript)

```typescript
// Initialize generator
{ msg_type: "init", data: null }

// Generate test cases
{
  msg_type: "generate",
  data: {
    count: 100,
    minStatements: 10,
    maxStatements: 30,
    outputDir: "./generated"
  }
}

// Get status
{ msg_type: "status", data: null }

// Stop generation
{ msg_type: "stop", data: null }

// Exit
{ msg_type: "exit", data: null }
```

### Response Messages (TypeScript → Rust)

```typescript
// Initialization response
{
  msg_type: "init_response",
  data: {
    success: true,
    message: "Generator initialized",
    outputDir: "./generated"
  }
}

// Test case (sent for each generated file)
{
  msg_type: "test_case",
  data: {
    id: 0,
    filename: "test_000001.js",
    code: "// Generated JavaScript code..."
  }
}

// Progress update
{
  msg_type: "progress",
  data: {
    generated: 50,
    total: 100
  }
}

// Generation complete
{
  msg_type: "generate_complete",
  data: {
    totalGenerated: 100,
    elapsedTime: 5.23,
    rate: 19.12,
    outputDir: "./generated"
  }
}

// Error
{
  msg_type: "error",
  data: "Error message"
}
```

## Building

1. Build the TypeScript code:
```bash
cd ts-app
npm install
npm run build
cd ..
```

2. Build the Rust code:
```bash
cargo build --release
```

Or use the build script:
```bash
./build.sh
```

## Running

Run the generator client:
```bash
cargo run --bin generator_client
```

This will:
1. Start the TypeScript generator bridge
2. Initialize the generator
3. Request 100 test cases
4. Display progress and results
5. Save generated files to `./rust-generated`

## Example Output

```
=== Rust Generator Client ===
Starting TypeScript generator bridge...

Initializing generator...
Generator initialized: {"success":true,"message":"Generator bridge initialized","outputDir":"./generated"}

Requesting generation of 100 test cases...
Progress: 100/100

Generation complete!
  Total generated: 100
  Elapsed time: 4.56s
  Generation rate: 21.93 cases/sec
  Output directory: ./rust-generated

Rust client statistics:
  Total time: 4.89s
  Test cases received: 100
  Progress updates: 10

First 3 test cases:

--- Test Case 1 ---
Filename: test_000001.js
Code preview:
const var1 = 42;
function func1(x, y) {
  return x + y;
}
console.log(func1(10, 20));
... (15 more lines)

--- Test Case 2 ---
Filename: test_000002.js
Code preview:
let counter = 0;
while (counter < 10) {
  counter++;
}
const result = counter * 2;
... (8 more lines)

--- Test Case 3 ---
Filename: test_000003.js
Code preview:
const arr = [1, 2, 3, 4, 5];
for (let i = 0; i < arr.length; i++) {
  console.log(arr[i]);
}
... (12 more lines)

Sending exit signal...
Generator client completed!
```

## Extending the Integration

### Batch Processing
The current implementation generates files one at a time. For better performance:
- Implement batching in the generator bridge
- Use the fast generator's streaming capabilities
- Process results in parallel

### Error Handling
Add robust error handling for:
- Generator process crashes
- Invalid test case generation
- File system errors
- Network/IPC failures

### Configuration
Add support for:
- Custom template directories
- Template profiles
- Generation strategies
- Output formats

### Performance Monitoring
Track and report:
- Memory usage
- CPU utilization
- Generation bottlenecks
- IPC overhead

## Use Cases

1. **Fuzzing Engines**: Rust fuzzer orchestrates JS test generation
2. **Test Suite Generation**: Generate comprehensive test suites
3. **Benchmark Creation**: Create performance benchmarks
4. **Code Coverage**: Generate tests for specific code paths
5. **Security Testing**: Generate malicious inputs safely

## Performance Considerations

- **IPC Overhead**: JSON serialization adds ~5-10% overhead
- **Process Startup**: ~100-200ms to start Node.js process
- **Memory**: Each process uses ~50-100MB
- **Throughput**: Can achieve 20-50 test cases/second

For maximum performance:
- Use binary protocols (MessagePack)
- Implement connection pooling
- Add compression for large test cases
- Use shared memory for bulk transfers