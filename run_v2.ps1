param (
    [switch]$Minimal
)

$ErrorActionPreference = "Stop"

Write-Host "==========================================" -ForegroundColor Cyan
Write-Host "   HFT Bot Auto-Restart Script" -ForegroundColor Cyan
Write-Host "   Press Ctrl+C to Stop Loop" -ForegroundColor Cyan
Write-Host "==========================================" -ForegroundColor Cyan

if ($Minimal) {
    $env:HFT_LOG_MODE = "minimal"
    Write-Host "Mode: MINIMAL (Errors + Latency only)" -ForegroundColor Magenta
}
else {
    $env:HFT_LOG_MODE = "normal"
    Write-Host "Mode: NORMAL (Verbose)" -ForegroundColor Green
}

while ($true) {
    try {
        if (-not $Minimal) {
            Write-Host "$(Get-Date): Starting Bot..." -ForegroundColor Green
        }
        # Run the bot
        cargo run --release
        
        # If we get here, the bot exited normally (which shouldn't happen usually)
        Write-Host "$(Get-Date): Bot exited. Restarting in 1s..." -ForegroundColor Yellow
    }
    catch {
        # Catch errors (non-zero exit)
        Write-Host "$(Get-Date): Bot CRASHED/FAILED. Restarting in 1s..." -ForegroundColor Red
    }
    
    Start-Sleep -Seconds 1
}
