const http = require('http');

const server = http.createServer((req, res) => {
  console.log(`[Upstream] Received ${req.method} ${req.url}`);
  res.writeHead(200, { 'Content-Type': 'application/json' });
  res.end(JSON.stringify({
    message: "Response from Upstream Node.js App",
    received_url: req.url,
    received_method: req.method,
    received_headers: req.headers
  }));
});

server.listen(4000, () => {
  console.log('Upstream Node.js app listening on port 4000');
});
