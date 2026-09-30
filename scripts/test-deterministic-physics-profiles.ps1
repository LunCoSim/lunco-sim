$root = (Resolve-Path -LiteralPath (Join-Path $PSScriptRoot '..')).Path
$binary = $env:LUNCOSIM_BIN
if ([string]::IsNullOrWhiteSpace($binary)) {
    $binary = Join-Path $root 'target/debug/luncosim.exe'
    if (-not (Test-Path -LiteralPath $binary -PathType Leaf)) {
        $binary = Join-Path $root 'target/debug/luncosim'
    }
}
elseif (-not [System.IO.Path]::IsPathRooted($binary)) {
    $repoBinary = Join-Path $root $binary
    if (Test-Path -LiteralPath $repoBinary -PathType Leaf) {
        $binary = $repoBinary
    }
    else {
        $command = Get-Command $binary -ErrorAction SilentlyContinue
        if ($null -ne $command) {
            $binary = $command.Source
        }
        else {
            $binary = $repoBinary
        }
    }
}

$reference = $env:LUNCOSIM_DETERMINISM_REFERENCE
if ([string]::IsNullOrWhiteSpace($reference)) {
    $reference = Join-Path $root 'scripts/tests/fixtures/deterministic-physics-reference.json'
}
elseif (-not [System.IO.Path]::IsPathRooted($reference)) {
    $reference = Join-Path $root $reference
}

if (-not (Test-Path -LiteralPath $binary -PathType Leaf)) {
    [Console]::Error.WriteLine("luncosim binary was not found: $binary")
    exit 2
}
if (-not (Test-Path -LiteralPath $reference -PathType Leaf)) {
    [Console]::Error.WriteLine("determinism reference was not found: $reference")
    exit 2
}

$sourceRevision = (& git -C $root rev-parse HEAD).Trim()
if ($LASTEXITCODE -ne 0) {
    [Console]::Error.WriteLine('could not identify the source revision')
    exit 2
}
$referenceSha256 = (Get-FileHash -LiteralPath $reference -Algorithm SHA256).Hash.ToLowerInvariant()
Write-Host "Source revision: $sourceRevision"
Write-Host "Reference SHA-256: $referenceSha256"

$profiles = @(
    [pscustomobject]@{
        Name = 'scene-4-serial'
        Scene = 'assets/scenes/tests/multi_rover_stress_4.usda'
        Threads = '1'
        Jitter = '0.0'
        Seed = '6840157149251759617'
    },
    [pscustomobject]@{
        Name = 'scene-4-default'
        Scene = 'assets/scenes/tests/multi_rover_stress_4.usda'
        Threads = '0'
        Jitter = '0.0'
        Seed = '6840157149251759617'
    },
    [pscustomobject]@{
        Name = 'scene-8-serial'
        Scene = 'assets/scenes/tests/multi_rover_stress_8.usda'
        Threads = '1'
        Jitter = '0.0'
        Seed = '6840157149251759617'
    },
    [pscustomobject]@{
        Name = 'scene-8-default'
        Scene = 'assets/scenes/tests/multi_rover_stress_8.usda'
        Threads = '0'
        Jitter = '0.0'
        Seed = '6840157149251759617'
    },
    [pscustomobject]@{
        Name = 'scene-20-serial'
        Scene = 'assets/scenes/tests/multi_rover_stress_20.usda'
        Threads = '1'
        Jitter = '0.0'
        Seed = '6840157149251759617'
    },
    [pscustomobject]@{
        Name = 'scene-20-default'
        Scene = 'assets/scenes/tests/multi_rover_stress_20.usda'
        Threads = '0'
        Jitter = '0.0'
        Seed = '6840157149251759617'
    },
    [pscustomobject]@{
        Name = 'jitter-0.25-seed-6840157149251759617'
        Scene = 'assets/scenes/tests/multi_rover_stress_4.usda'
        Threads = '1'
        Jitter = '0.25'
        Seed = '6840157149251759617'
    },
    [pscustomobject]@{
        Name = 'jitter-0.25-seed-1234567890123456789'
        Scene = 'assets/scenes/tests/multi_rover_stress_4.usda'
        Threads = '1'
        Jitter = '0.25'
        Seed = '1234567890123456789'
    },
    [pscustomobject]@{
        Name = 'jitter-0.5-seed-6840157149251759617'
        Scene = 'assets/scenes/tests/multi_rover_stress_4.usda'
        Threads = '1'
        Jitter = '0.5'
        Seed = '6840157149251759617'
    },
    [pscustomobject]@{
        Name = 'jitter-0.5-seed-1234567890123456789'
        Scene = 'assets/scenes/tests/multi_rover_stress_4.usda'
        Threads = '1'
        Jitter = '0.5'
        Seed = '1234567890123456789'
    }
)

$previousAssetRoot = $env:LUNCO_ASSET_ROOT
$hadAssetRoot = Test-Path Env:LUNCO_ASSET_ROOT
$failedExitCode = 0
Push-Location $root
try {
    $env:LUNCO_ASSET_ROOT = Join-Path $root 'assets'
    foreach ($runProfile in $profiles) {
        Write-Host "`nDeterminism profile: $($runProfile.Name)"
        $arguments = @(
            'test',
            '--scene', $runProfile.Scene,
            '--threads', $runProfile.Threads,
            '--jitter', $runProfile.Jitter,
            '--seed', $runProfile.Seed,
            '--determinism-reference', $reference
        )
        & $binary @arguments
        $profileExitCode = $LASTEXITCODE
        if ($profileExitCode -ne 0) {
            [Console]::Error.WriteLine("Determinism profile $($runProfile.Name) failed with exit $profileExitCode")
            $failedExitCode = $profileExitCode
            break
        }
    }
}
finally {
    Pop-Location
    if ($hadAssetRoot) {
        $env:LUNCO_ASSET_ROOT = $previousAssetRoot
    }
    else {
        Remove-Item Env:LUNCO_ASSET_ROOT -ErrorAction SilentlyContinue
    }
}

if ($failedExitCode -ne 0) {
    exit $failedExitCode
}

Write-Host 'DETERMINISTIC_PHYSICS_PROFILES_OK'
