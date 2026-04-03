const fs = require('fs');

const files = {
    'Node HTTP': 'benchmarks/node_results.json',
    'Fastify': 'benchmarks/fastify_results.json',
    'Ferrum (Rust)': 'benchmarks/rust_results.json'
};

console.log('| Candidate | Req/sec (Avg) | Latency (p50) | Latency (p99) |');
console.log('| :--- | :--- | :--- | :--- |');

for (const [name, path] of Object.entries(files)) {
    try {
        const data = JSON.parse(fs.readFileSync(path, 'utf8'));
        console.log(`| **${name}** | ${data.requests.average.toFixed(2)} | ${data.latency.p50} ms | ${data.latency.p99} ms |`);
    } catch (e) {
        console.log(`| **${name}** | ERR | ERR | ERR |`);
    }
}
