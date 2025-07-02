# Rust-TypeScript IPC

A high-performance inter-process communication (IPC) example between Rust and TypeScript using stdio pipes for the fastest possible bidirectional communication.

## Overview

This project demonstrates how to:
- Spawn a TypeScript/Node.js process from Rust
- Establish bidirectional communication using stdio pipes
- Exchange JSON messages between Rust and TypeScript processes
- Handle graceful shutdown and error cases

## Architecture

```
┌─────────────────┐         JSON over stdio         ┌──────────────────┐
│                 │ ──────────────────────────────> │                  │
│   Rust Process  │                                  │ TypeScript/Node  │
│                 │ <────────────────────────────── │     Process      │
└─────────────────┘         Newline-delimited       └──────────────────┘
```

### Why stdio pipes?

- **Fastest IPC method** - No network overhead, no socket creation
- **Simple protocol** - Newline-delimited JSON messages
- **Cross-platform** - Works on Windows, macOS, and Linux
- **Built-in backpressure** - OS handles buffering automatically

## Project Structure

```
rust-ts-ipc/
├── Cargo.toml          # Rust dependencies
├── src/
│   └── main.rs         # Rust application
└── ts-app/
    ├── package.json    # Node.js dependencies
    ├── tsconfig.json   # TypeScript configuration
    └── src/
        └── index.ts    # TypeScript application
```

## Message Protocol

Messages are exchanged as JSON objects with the following structure:

```typescript
interface Message {
    msg_type: string;  // Type of message (greeting, data, response, ack, exit)
    data: string;      // Message payload
}
```

## Setup and Running

### Prerequisites

- Rust (latest stable)
- Node.js (v14 or higher)
- npm or yarn

### Installation

1. Clone the repository:
```bash
git clone https://github.com/TomDeCat2021/rust-ts-ipc.git
cd rust-ts-ipc
```

2. Install TypeScript dependencies:
```bash
cd ts-app
npm install
npm run build
cd ..
```

3. Build and run the Rust application:
```bash
cargo run
```

## How It Works

1. **Rust Process** (Parent):
   - Spawns Node.js as a child process
   - Pipes stdin/stdout for communication
   - Sends messages to TypeScript
   - Listens for responses in a separate thread

2. **TypeScript Process** (Child):
   - Reads messages from stdin line by line
   - Parses JSON messages
   - Processes messages based on type
   - Sends responses via stdout

3. **Communication Flow**:
   - Rust sends a greeting message
   - TypeScript responds with acknowledgment
   - Rust sends 5 data messages
   - TypeScript acknowledges each message
   - After the 5th message, TypeScript sends an exit signal
   - Both processes shut down gracefully

## Example Output

```
Rust: Sending initial message
Received from TS: Message { msg_type: "response", data: "Hello from TypeScript!" }
Received from TS: Message { msg_type: "ack", data: "Acknowledged: Message 1 from Rust" }
Received from TS: Message { msg_type: "ack", data: "Acknowledged: Message 2 from Rust" }
Received from TS: Message { msg_type: "ack", data: "Acknowledged: Message 3 from Rust" }
Received from TS: Message { msg_type: "ack", data: "Acknowledged: Message 4 from Rust" }
Received from TS: Message { msg_type: "ack", data: "Acknowledged: Message 5 from Rust" }
Received from TS: Message { msg_type: "exit", data: "Goodbye from TypeScript!" }
Received exit signal from TypeScript
Communication complete!
```

## Extending the Example

This example can be extended for various use cases:

- **Streaming data processing** - Process large datasets by streaming through pipes
- **Microservice communication** - Use as a pattern for service communication
- **Plugin systems** - Run untrusted code in separate processes
- **Performance-critical applications** - Leverage Rust's performance with Node.js ecosystem

## Performance Considerations

- **Buffer sizes** - Adjust based on message frequency and size
- **Message format** - Consider binary protocols (MessagePack, Protocol Buffers) for better performance
- **Error handling** - Implement reconnection logic for production use
- **Async I/O** - The Rust side uses threads; consider tokio for async implementation

## License

This project is provided as an example and is free to use and modify.

## Contributing

Feel free to submit issues and pull requests to improve this example.