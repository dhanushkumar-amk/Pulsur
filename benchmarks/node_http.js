const http = require('http');

const server = http.createServer((req, res) => {
    res.setHeader('Content-Type', 'application/json');
    res.end(JSON.stringify({ 
        message: "Node Baseline Online", 
        version: process.version 
    }));
});

const PORT = 3001;
server.listen(PORT, () => {
    console.log(`Node HTTP Baseline: http://localhost:${PORT}`);
});
