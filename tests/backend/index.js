const http = require('http');

const PORT = process.env.PORT || 3000;
const HOST = process.env.HOST || '0.0.0.0';

const server = http.createServer((req, res) => {
  // Simulate health check
  if (req.url === '/health') {
    res.writeHead(200);
    res.end('OK');
    return;
  }

  // Simulate failures for circuit breaker
  if (req.url === '/fail') {
    res.writeHead(500);
    res.end('Internal Server Error');
    return;
  }

  // Normal request
  const body = [];
  req.on('data', (chunk) => body.push(chunk));
  req.on('end', () => {
    res.writeHead(200, { 'Content-Type': 'application/json' });
    res.end(JSON.stringify({
      message: "Hello from Pulsur Test Backend",
      port: PORT,
      method: req.method,
      path: req.url,
      headers: req.headers,
      body: Buffer.concat(body).toString()
    }));
  });
});

server.listen(PORT, HOST, () => {
  console.log(`Backend server listening on http://${HOST}:${PORT}`);
});
