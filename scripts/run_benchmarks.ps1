# 🛸 pulsur Benchmark Orchestrator
# High-precision cross-stack performance comparison.

$Duration = "10s"
$Connections = 100
$Threads = 4

function Run-Protocol-Test($label, $port, $cmd, $args) {
    Write-Host "--- 🛰️ Phase 11: Stressing $label (Port $port) ---" -ForegroundColor Cyan
    
    # Life-cycle Management
    if ($args) {
        $proc = Start-Process -FilePath $cmd -ArgumentList "$args" -PassThru -NoNewWindow
    } else {
        $proc = Start-Process -FilePath $cmd -PassThru -NoNewWindow
    }
    Start-Sleep -Seconds 4 # Thermal soak / Initialization
    
    # Stress Execution
    $outputFile = "benchmarks/$($label.Replace(' ', '_').ToLower())_results.json"
    npx autocannon -c $Connections -d $Duration -t $Threads --json "http://localhost:$port" > $outputFile
    
    # Cleanup
    Stop-Process -Id $proc.Id -Force
    Start-Sleep -Seconds 1
}

# 1. Node Baseline (Port 3001)
Run-Protocol-Test "Node HTTP" 3001 "node" "benchmarks/node_http.js"

# 2. Fastify (Port 3002)
Run-Protocol-Test "Fastify" 3002 "node" "benchmarks/fastify_http.js"

# 3. pulsur (Port 8080)
Run-Protocol-Test "pulsur" 8080 "target/release/examples/benchmark.exe" ""
