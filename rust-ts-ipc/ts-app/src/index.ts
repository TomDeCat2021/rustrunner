import * as readline from 'readline';

interface Message {
    msg_type: string;
    data: string;
}

const rl = readline.createInterface({
    input: process.stdin,
    output: process.stdout,
    terminal: false
});

console.error('TypeScript process started');

rl.on('line', (line: string) => {
    try {
        const msg: Message = JSON.parse(line);
        console.error(`TS: Received - ${msg.msg_type}: ${msg.data}`);
        
        if (msg.msg_type === 'greeting') {
            const response: Message = {
                msg_type: 'response',
                data: 'Hello from TypeScript!'
            };
            console.log(JSON.stringify(response));
        } else if (msg.msg_type === 'data') {
            const response: Message = {
                msg_type: 'ack',
                data: `Acknowledged: ${msg.data}`
            };
            console.log(JSON.stringify(response));
            
            if (msg.data.includes('Message 5')) {
                setTimeout(() => {
                    const exitMsg: Message = {
                        msg_type: 'exit',
                        data: 'Goodbye from TypeScript!'
                    };
                    console.log(JSON.stringify(exitMsg));
                    process.exit(0);
                }, 100);
            }
        }
    } catch (e) {
        console.error('Error parsing message:', e);
    }
});

process.on('SIGINT', () => {
    process.exit(0);
});