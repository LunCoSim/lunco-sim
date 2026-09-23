$ErrorActionPreference = 'Stop'
$repo = Resolve-Path (Join-Path $PSScriptRoot '../..')
Push-Location $repo
try {
    New-Item -ItemType Directory -Force target/soil-spike | Out-Null
    rustc --edition 2024 --test -D warnings scripts/soil-spike/main.rs -o target/soil-spike/tests.exe
    if ($LASTEXITCODE -ne 0) { throw 'Test compilation failed' }
    & ./target/soil-spike/tests.exe
    if ($LASTEXITCODE -ne 0) { throw 'Numerical tests failed' }
    rustc --edition 2024 -O -D warnings scripts/soil-spike/main.rs -o target/soil-spike/soil-spike.exe
    if ($LASTEXITCODE -ne 0) { throw 'Experiment compilation failed' }
    & ./target/soil-spike/soil-spike.exe scripts/soil-spike/cases.csv > target/soil-spike/results.csv
    if ($LASTEXITCODE -ne 0) { throw 'Experiment failed' }
    & ./target/soil-spike/soil-spike.exe scripts/soil-spike/cases.csv > target/soil-spike/repeat.csv
    if ($LASTEXITCODE -ne 0) { throw 'Repeat failed' }
    if (Compare-Object (Get-Content target/soil-spike/results.csv) (Get-Content target/soil-spike/repeat.csv)) {
        throw 'Repeated CSV values differ'
    }
    Write-Output 'PASS: numerical tests and repeated CSV; results in target/soil-spike/results.csv'
} finally { Pop-Location }
