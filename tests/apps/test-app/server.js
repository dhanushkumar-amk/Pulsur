const http = require('http');
const url = require('url');

const server = http.createServer((req, res) => {
  const parsedUrl = url.parse(req.url, true);
  const path = parsedUrl.pathname;

  if (path === '/health') {
    res.writeHead(200, { 'Content-Type': 'application/json' });
    return res.end(JSON.stringify({ status: 'up' }));
  }

  // Simulate some CPU work
  if (path === '/api/compute') {
    const start = Date.now();
    const arr = Array.from({ length: 10000 }, () => Math.random());
    arr.sort((a, b) => a - b);
    const end = Date.now();

    res.writeHead(200, { 'Content-Type': 'application/json' });
    return res.end(JSON.stringify({ 
        message: 'Compute complete',
        took_ms: end - start,
        node_id: process.env.NODE_ID || 'standalone' 
    }));
  }

  // Simulate an async delay (e.g. database/upstream)
  if (path === '/api/delay') {
    setTimeout(() => {
        res.writeHead(200, { 'Content-Type': 'application/json' });
        res.end(JSON.stringify({ 
            message: 'Delayed response',
            node_id: process.env.NODE_ID || 'standalone' 
        }));
    }, 100);
    return;
  }

  // Default
  res.writeHead(200, { 'Content-Type': 'application/json', 'X-Powered-By': 'Pulsur-Test-Node' });
  res.end(JSON.stringify({ 
    message: 'Hello from Node.js (Baseline)',
    url: req.url,
    node_id: process.env.NODE_ID || 'standalone'
  }));
});

const PORT = process.env.PORT || 3000;
server.listen(PORT, () => {
  console.log(`Test App listening on port ${PORT}`);
});
