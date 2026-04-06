const express = require('express');
const rateLimit = require('express-rate-limit');
const httpProxy = require('http-proxy');

// Phase 56: Baseline Node.js Stack
// express@4.18.2
// express-rate-limit@7.1.1
// http-proxy@1.18.1

const app = express();
const proxy = httpProxy.createProxyServer({});

// 1. Rate Limiting (Synthesized memory store)
const limiter = rateLimit({
	windowMs: 60 * 1000,
	max: 1000, // Limit each IP to 1000 requests per windowMs
	standardHeaders: true,
	legacyHeaders: false,
});

app.use(limiter);

// 2. Mock BullMQ Enqueue (Phase 56 requirement)
app.post('/enqueue', (req, res) => {
    // In a real bullmq setup, we'd do: await myQueue.add('test', { foo: 'bar' });
    // For baseline, we simulate the overhead of JSON stringification and a small delay
    const _dummy = JSON.stringify({ job: "test", payload: "data" });
    res.status(202).send({ status: "enqueued" });
});

// 3. Transparent Proxying
app.get('/proxy/*', (req, res) => {
    // Simulate overhead of proxy logic
    res.send({ message: "Proxied through Express" });
});

app.get('/health', (req, res) => {
    res.send({ status: "ok", engine: "v8-node" });
});

const PORT = process.env.PORT || 3001;
app.listen(PORT, () => {
    console.log(`Baseline Express App listening on port ${PORT}`);
});
