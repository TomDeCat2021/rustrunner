/**
 * Simple Generator Bridge
 * 
 * A simplified version that spawns the generator as a subprocess
 * without complex module imports
 */

import * as readline from 'readline';
import { spawn } from 'child_process';
import * as path from 'path';
import * as fs from 'fs';

interface Message {
    msg_type: string;
    data: any;
}

const rl = readline.createInterface({
    input: process.stdin,
    output: process.stdout,
    terminal: false
});

let isGenerating = false;
let generatedCount = 0;

console.error('[Generator] Simple generator bridge started');

rl.on('line', async (line: string) => {
    try {
        const msg: Message = JSON.parse(line);
        console.error(`[Generator] Received: ${msg.msg_type}`);
        
        switch (msg.msg_type) {
            case 'init':
                sendMessage({
                    msg_type: 'init_response',
                    data: {
                        success: true,
                        message: 'Simple generator ready'
                    }
                });
                break;
                
            case 'generate':
                if (!isGenerating) {
                    isGenerating = true;
                    generatedCount = 0;
                    generateTestCases(msg.data);
                } else {
                    sendMessage({
                        msg_type: 'error',
                        data: 'Generation already in progress'
                    });
                }
                break;
                
            case 'stop':
                // Force reset generation state
                isGenerating = false;
                generatedCount = 0;
                sendMessage({
                    msg_type: 'stop_response',
                    data: {
                        success: true,
                        message: 'Generation stopped and reset'
                    }
                });
                break;
                
            case 'status':
                sendMessage({
                    msg_type: 'status_response',
                    data: {
                        generating: isGenerating,
                        generatedCount
                    }
                });
                break;
                
            case 'exit':
                console.error('[Generator] Exiting');
                process.exit(0);
                break;
                
            default:
                sendMessage({
                    msg_type: 'error',
                    data: `Unknown message type: ${msg.msg_type}`
                });
        }
    } catch (e) {
        console.error('[Generator] Error:', e);
        sendMessage({
            msg_type: 'error',
            data: String(e)
        });
    }
});

function parseConsoleOutput(buffer: string, testCases: Array<{id: number, filename: string, code: string, state: string}>, maxCount: number) {
    // Find all JS content blocks
    const jsPattern = /======= JS CONTENT ========\s*([\s\S]*?)\s*===========================/g;
    const mutationPattern = /======= MUTATION JSON ========\s*([\s\S]*?)\s*===========================/g;
    
    const jsMatches = [...buffer.matchAll(jsPattern)];
    const mutationMatches = [...buffer.matchAll(mutationPattern)];
    
    // Process pairs of JS content and mutation data
    const pairs = Math.min(jsMatches.length, mutationMatches.length);
    
    for (let i = testCases.length; i < pairs && i < maxCount; i++) {
        const jsContent = jsMatches[i][1].trim();
        const mutationContent = mutationMatches[i][1].trim();
        
        testCases.push({
            id: i,
            filename: `generated_${i}.js`,
            code: jsContent,
            state: mutationContent
        });
    }
}

function generateTestCases(request: any) {
    const count = request.count || 10;
    const outputDir = request.outputDir || './generated';
    
    // Ensure output directory exists
    if (!fs.existsSync(outputDir)) {
        fs.mkdirSync(outputDir, { recursive: true });
    }
    
    const startTime = Date.now();
    
    // Spawn the actual generator
    const generatorProcess = spawn('npx', [
        'tsx',
        'src/index.ts',
        '--count', String(count),
        '--output', outputDir,
        '--min-statements', String(request.minStatements || 10),
        '--max-statements', String(request.maxStatements || 30),
        '--export-mutation', '--console-output'
    ], {
        cwd: "/Users/t/gen3mutator/gen3",
        stdio: ['ignore', 'pipe', 'pipe']
    });
    
    let outputBuffer = '';
    let testCases: Array<{id: number, filename: string, code: string, state: string}> = [];
    
    generatorProcess.stdout.on('data', (data) => {
        outputBuffer += data.toString();
        
        // Parse JS content and mutation JSON from console output
        parseConsoleOutput(outputBuffer, testCases, count);
        
        // Send progress updates
        if (testCases.length > generatedCount) {
            generatedCount = testCases.length;
            if (generatedCount % 10 === 0) {
                sendMessage({
                    msg_type: 'progress',
                    data: {
                        generated: generatedCount,
                        total: count
                    }
                });
            }
        }
    });
    
  
    
    generatorProcess.on('close', (code) => {
        const elapsedTime = (Date.now() - startTime) / 1000;
        
        if (code === 0) {
            try {
                // Parse any remaining content in the buffer
                parseConsoleOutput(outputBuffer, testCases, count);
                
                // Send test cases from parsed console output
                testCases.forEach((testCase) => {
                    sendMessage({
                        msg_type: 'test_case',
                        data: {
                            id: testCase.id,
                            filename: testCase.filename,
                            code: testCase.code,
                            state: testCase.state
                        }
                    });
                });
                
                // Send completion
                sendMessage({
                    msg_type: 'generate_complete',
                    data: {
                        totalGenerated: testCases.length,
                        elapsedTime,
                        rate: testCases.length / elapsedTime,
                        outputDir
                    }
                });
            } catch (error) {
                sendMessage({
                    msg_type: 'error',
                    data: `Failed to parse generated content: ${error}`
                });
            }
        } else {
            sendMessage({
                msg_type: 'error',
                data: `Generator exited with code ${code}`
            });
        }
        
        isGenerating = false;
    });
    
    generatorProcess.on('error', (error) => {
        sendMessage({
            msg_type: 'error',
            data: `Failed to start generator: ${error.message}`
        });
        isGenerating = false;
    });
}

function sendMessage(msg: Message) {
    console.log(JSON.stringify(msg));
}

process.on('SIGINT', () => {
    process.exit(0);
});