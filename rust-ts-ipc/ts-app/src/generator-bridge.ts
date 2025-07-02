/**
 * Generator Bridge for Rust-TS IPC
 * 
 * Bridges between Rust and the gen3mutator generator
 * by spawning it as a subprocess
 */

import * as readline from 'readline';
import { spawn, ChildProcess } from 'child_process';
import * as path from 'path';
import * as fs from 'fs';

interface Message {
    msg_type: string;
    data: any;
}

interface GenerateRequest {
    count: number;
    minStatements?: number;
    maxStatements?: number;
    outputDir?: string;
}

class GeneratorBridge {
    private rl: readline.Interface;
    private generatorProcess: ChildProcess | null = null;
    private isGenerating = false;
    private generatedCount = 0;
    private outputDir = './generated';

    constructor() {
        this.rl = readline.createInterface({
            input: process.stdin,
            output: process.stdout,
            terminal: false
        });

        this.setupHandlers();
        console.error('[GeneratorBridge] Started');
    }

    private setupHandlers() {
        this.rl.on('line', async (line: string) => {
            try {
                const msg: Message = JSON.parse(line);
                await this.handleMessage(msg);
            } catch (e) {
                console.error('[GeneratorBridge] Error parsing message:', e);
                this.sendMessage({
                    msg_type: 'error',
                    data: `Failed to parse message: ${e}`
                });
            }
        });

        process.on('SIGINT', () => {
            console.error('[GeneratorBridge] Received SIGINT, shutting down');
            this.cleanup();
            process.exit(0);
        });
    }

    private async handleMessage(msg: Message) {
        console.error(`[GeneratorBridge] Received ${msg.msg_type}`);

        switch (msg.msg_type) {
            case 'init':
                await this.handleInit();
                break;

            case 'generate':
                await this.handleGenerate(msg.data);
                break;

            case 'stop':
                this.handleStop();
                break;

            case 'status':
                this.handleStatus();
                break;

            case 'exit':
                this.handleExit();
                break;

            default:
                this.sendMessage({
                    msg_type: 'error',
                    data: `Unknown message type: ${msg.msg_type}`
                });
        }
    }

    private async handleInit() {
        try {
            // Create output directory if it doesn't exist
            if (!fs.existsSync(this.outputDir)) {
                fs.mkdirSync(this.outputDir, { recursive: true });
            }

            this.sendMessage({
                msg_type: 'init_response',
                data: {
                    success: true,
                    message: 'Generator bridge initialized',
                    outputDir: this.outputDir
                }
            });
        } catch (error) {
            this.sendMessage({
                msg_type: 'init_response',
                data: {
                    success: false,
                    error: error instanceof Error ? error.message : 'Unknown error'
                }
            });
        }
    }

    private async handleGenerate(request: GenerateRequest) {
        if (this.isGenerating) {
            this.sendMessage({
                msg_type: 'error',
                data: 'Generation already in progress'
            });
            return;
        }

        this.isGenerating = true;
        this.generatedCount = 0;
        
        if (request.outputDir) {
            this.outputDir = request.outputDir;
            if (!fs.existsSync(this.outputDir)) {
                fs.mkdirSync(this.outputDir, { recursive: true });
            }
        }

        const startTime = Date.now();

        try {
            // Build command to run the generator
            const gen3Path = path.resolve(__dirname, '../../../../');
            const command = 'node';
            const args = [
                '--loader=tsx',
                path.join(gen3Path, 'src/index.ts'),
                '--count', request.count.toString(),
                '--output-dir', this.outputDir,
                '--mode', 'debug'
            ];

            if (request.minStatements) {
                args.push('--min-statements', request.minStatements.toString());
            }
            if (request.maxStatements) {
                args.push('--max-statements', request.maxStatements.toString());
            }

            console.error(`[GeneratorBridge] Running: ${command} ${args.join(' ')}`);

            // Spawn the generator process
            this.generatorProcess = spawn(command, args, {
                cwd: gen3Path,
                stdio: ['ignore', 'pipe', 'pipe']
            });

            // Handle stdout (generated files)
            this.generatorProcess.stdout?.on('data', (data) => {
                const output = data.toString();
                console.error(`[GeneratorBridge] Generator output: ${output}`);
                
                // Parse output to detect generated files
                const lines = output.split('\n');
                for (const line of lines) {
                    if (line.includes('Generated file:') || line.includes('.js')) {
                        this.generatedCount++;
                        
                        // Extract filename if possible
                        const match = line.match(/([^/\\]+\.js)/);
                        if (match) {
                            this.sendMessage({
                                msg_type: 'test_case',
                                data: {
                                    id: this.generatedCount - 1,
                                    filename: match[1],
                                    path: path.join(this.outputDir, match[1])
                                }
                            });
                        }

                        // Send progress updates
                        if (this.generatedCount % 10 === 0) {
                            this.sendMessage({
                                msg_type: 'progress',
                                data: {
                                    generated: this.generatedCount,
                                    total: request.count
                                }
                            });
                        }
                    }
                }
            });

            // Handle stderr
            this.generatorProcess.stderr?.on('data', (data) => {
                console.error(`[GeneratorBridge] Generator stderr: ${data}`);
            });

            // Handle process exit
            this.generatorProcess.on('close', (code) => {
                const elapsedTime = (Date.now() - startTime) / 1000;
                const rate = this.generatedCount / elapsedTime;

                if (code === 0) {
                    // Read generated files and send them
                    const files = fs.readdirSync(this.outputDir)
                        .filter(f => f.endsWith('.js'))
                        .slice(0, request.count);

                    for (let i = 0; i < files.length; i++) {
                        const filepath = path.join(this.outputDir, files[i]);
                        const code = fs.readFileSync(filepath, 'utf-8');
                        
                        this.sendMessage({
                            msg_type: 'test_case',
                            data: {
                                id: i,
                                filename: files[i],
                                code: code
                            }
                        });
                    }

                    this.sendMessage({
                        msg_type: 'generate_complete',
                        data: {
                            totalGenerated: files.length,
                            elapsedTime,
                            rate,
                            outputDir: this.outputDir
                        }
                    });
                } else {
                    this.sendMessage({
                        msg_type: 'error',
                        data: `Generator process exited with code ${code}`
                    });
                }

                this.isGenerating = false;
                this.generatorProcess = null;
            });

            // Handle errors
            this.generatorProcess.on('error', (error) => {
                this.sendMessage({
                    msg_type: 'error',
                    data: `Failed to start generator: ${error.message}`
                });
                this.isGenerating = false;
                this.generatorProcess = null;
            });

        } catch (error) {
            this.sendMessage({
                msg_type: 'error',
                data: `Generation failed: ${error instanceof Error ? error.message : 'Unknown error'}`
            });
            this.isGenerating = false;
        }
    }

    private handleStop() {
        if (this.isGenerating && this.generatorProcess) {
            this.generatorProcess.kill('SIGTERM');
            this.isGenerating = false;
            this.sendMessage({
                msg_type: 'stop_response',
                data: { success: true }
            });
        } else {
            this.sendMessage({
                msg_type: 'stop_response',
                data: { success: false, message: 'No generation in progress' }
            });
        }
    }

    private handleStatus() {
        const status = {
            generating: this.isGenerating,
            generatedCount: this.generatedCount,
            outputDir: this.outputDir
        };

        this.sendMessage({
            msg_type: 'status_response',
            data: status
        });
    }

    private handleExit() {
        console.error('[GeneratorBridge] Received exit command');
        this.cleanup();
        process.exit(0);
    }

    private cleanup() {
        if (this.generatorProcess) {
            this.generatorProcess.kill('SIGTERM');
        }
    }

    private sendMessage(msg: Message) {
        console.log(JSON.stringify(msg));
    }
}

// Start the bridge
const bridge = new GeneratorBridge();