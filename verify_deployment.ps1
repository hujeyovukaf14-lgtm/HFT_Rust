$ServerIP = "149.104.78.218"
$RemotePath = "~/hft_bot"

Write-Host ">>> Comparing Remote vs Local Files ($ServerIP)..." -ForegroundColor Cyan

# 1. Local Hashes
Write-Host "1. Calculating Local Hashes..." -ForegroundColor Yellow
$LocalHashes = @{}

# Collect files from src (recursive)
$Files = @(Get-ChildItem -Path "src" -Recurse -File)

# Add root files if they exist
if (Test-Path "Cargo.toml") { $Files += Get-Item "Cargo.toml" }
if (Test-Path ".env") { $Files += Get-Item ".env" }

foreach ($file in $Files) {
    # Get relative path (e.g. src/main.rs)
    # Use forward slashes / like in Linux
    $RelPath = $file.FullName.Substring($PSScriptRoot.Length + 1).Replace("\", "/")
    
    # Calculate SHA256
    $Hash = (Get-FileHash $file.FullName -Algorithm SHA256).Hash.ToLower()
    
    # Safely add to hashtable
    if ($LocalHashes -is [System.Collections.Hashtable]) {
        $LocalHashes[$RelPath] = $Hash
    }
    else {
        Write-Host "Error: LocalHashes is not a hashtable!" -ForegroundColor Red
    }
}

# 2. Remote Hashes
Write-Host "2. Fetching Remote Hashes..." -ForegroundColor Yellow

# Command: find all files in src, plus Cargo.toml and .env, and calculate sha256sum
# Use cd to make paths relative
$Cmd = "cd $RemotePath && find src -type f -exec sha256sum {} + && sha256sum Cargo.toml .env 2>/dev/null"

# Run via SSH and capture output
$RemoteOutput = ssh root@$ServerIP $Cmd

if ($LASTEXITCODE -ne 0) {
    Write-Host ">>> Connection Failed! Check SSH or VPN." -ForegroundColor Red
    exit
}

$RemoteHashes = @{}
# Parse Linux sha256sum output: "hash filename"
$RemoteOutput -split "`n" | ForEach-Object {
    $Line = $_.Trim()
    if ($Line -match "^([a-f0-9]{64})\s+(.+)$") {
        $Hash = $matches[1]
        # Remove ./ from start of paths (find ./src/...)
        $File = $matches[2].Replace("./", "")
        $RemoteHashes[$File] = $Hash
    }
}

# 3. Compare
Write-Host "3. Comparison Results:" -ForegroundColor Cyan
$AllKeys = $LocalHashes.Keys + $RemoteHashes.Keys | Select-Object -Unique | Sort-Object

$DiffCount = 0

foreach ($File in $AllKeys) {
    $L = $LocalHashes[$File]
    $R = $RemoteHashes[$File]

    if ($null -eq $L) {
        Write-Host "[REMOTE ONLY] $File" -ForegroundColor Gray
    }
    elseif ($null -eq $R) {
        Write-Host "[LOCAL ONLY]  $File (Not deployed?)" -ForegroundColor Magenta
        $DiffCount++
    }
    elseif ($L -ne $R) {
        Write-Host "[DIFFERENT]   $File" -ForegroundColor Red
        Write-Host "  Local:  $L"
        Write-Host "  Remote: $R"
        $DiffCount++
    }
    else {
        # Write-Host "[OK]          $File" -ForegroundColor Green
    }
}

if ($DiffCount -eq 0) {
    Write-Host "`n>>> All files match! Remote is up to date." -ForegroundColor Green
}
else {
    Write-Host "`n>>> Found $DiffCount differences." -ForegroundColor Yellow
}
