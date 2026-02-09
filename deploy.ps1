$ServerIP = "149.104.78.218"
$RemotePath = "~/hft_bot/"

Write-Host ">>> 🚀 Deploying to $ServerIP..." -ForegroundColor Cyan

# 1. Copy Code (Code + Configs)
# We exclude 'target' folder to save bandwidth
scp -r src Cargo.toml .env root@${ServerIP}:${RemotePath}

if ($LASTEXITCODE -eq 0) {
    Write-Host ">>> ✅ Sync success! Starting Remote Engine..." -ForegroundColor Green
    Write-Host ">>> (Press Ctrl+C to stop the remote bot)" -ForegroundColor Gray
    
    # 2. Run Remotely
    # -t forces pseudo-terminal allocation so you can see colors and use Ctrl+C
    ssh -t root@$ServerIP "cd hft_bot && cargo run --release"
}
else {
    Write-Host ">>> ❌ Deployment failed!" -ForegroundColor Red
}
