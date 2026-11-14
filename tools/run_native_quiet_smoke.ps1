[CmdletBinding()]
param()

$ErrorActionPreference = 'Stop'
$repo = [IO.Path]::GetFullPath((Split-Path -Parent $PSScriptRoot))
$fixtureRoot = Join-Path $repo 'target\native-quiet-fixture'
$fixtureExe = Join-Path $fixtureRoot 'backtracking-quiet-fixture.exe'
$fixtureSource = Join-Path $PSScriptRoot 'quiet_process_fixture.rs'
$runRoot = Join-Path $fixtureRoot ('run-' + [DateTime]::UtcNow.ToString('yyyyMMdd-HHmmss') + '-' + [guid]::NewGuid().ToString('N').Substring(0, 8))
New-Item -ItemType Directory -Path $runRoot -Force | Out-Null
& rustc --edition=2024 --crate-name backtracking_quiet_fixture $fixtureSource -o $fixtureExe
if ($LASTEXITCODE -ne 0) { throw 'Silent fixture build failed.' }

$stopMarker = Join-Path $runRoot 'stop'
$utf8 = New-Object Text.UTF8Encoding($false)
$fixture = $null
$test = $null
$fixtureError = $null
try {
    $fixtureInfo = New-Object Diagnostics.ProcessStartInfo
    $fixtureInfo.FileName = $fixtureExe
    $fixtureInfo.Arguments = '"' + $stopMarker + '"'
    $fixtureInfo.WorkingDirectory = $repo
    $fixtureInfo.UseShellExecute = $false
    $fixtureInfo.CreateNoWindow = $true
    $fixtureInfo.RedirectStandardOutput = $true
    $fixtureInfo.RedirectStandardError = $true
    $fixture = [Diagnostics.Process]::Start($fixtureInfo)
    $ready = $fixture.StandardOutput.ReadLine() | ConvertFrom-Json
    if ($ready.event -ne 'ready' -or $ready.pid -ne $fixture.Id) {
        throw 'Silent fixture handshake mismatch.'
    }
    $fixtureError = $fixture.StandardError.ReadToEndAsync()

    $testInfo = New-Object Diagnostics.ProcessStartInfo
    $testInfo.FileName = (Get-Command cargo).Source
    $testInfo.Arguments = 'test --locked --manifest-path desktop/windows/Cargo.toml native_quiet_retention_process_loopback_smoke -- --ignored --nocapture'
    $testInfo.WorkingDirectory = $repo
    $testInfo.UseShellExecute = $false
    $testInfo.CreateNoWindow = $true
    $testInfo.RedirectStandardOutput = $true
    $testInfo.RedirectStandardError = $true
    $testInfo.EnvironmentVariables['BACKTRACKING_QUIET_TEST_PROCESS_ID'] = [string]$fixture.Id
    $test = [Diagnostics.Process]::Start($testInfo)
    $testOutput = $test.StandardOutput.ReadToEndAsync()
    $testError = $test.StandardError.ReadToEndAsync()
    if (-not $test.WaitForExit(60000)) {
        $test.Kill()
        $test.WaitForExit()
        throw 'Native quiet smoke exceeded its 60 second bound.'
    }
    $stdout = $testOutput.Result
    $stderr = $testError.Result
    [IO.File]::WriteAllText((Join-Path $runRoot 'test.stdout.log'), $stdout, $utf8)
    [IO.File]::WriteAllText((Join-Path $runRoot 'test.stderr.log'), $stderr, $utf8)
    $metadata = [ordered]@{
        tested_utc = [datetime]::UtcNow.ToString('o')
        fixture_pid = $fixture.Id
        fixture_executable = 'target/native-quiet-fixture/backtracking-quiet-fixture.exe'
        fixture_is_silent = $true
        fixture_source_sha256 = (Get-FileHash -LiteralPath $fixtureSource -Algorithm SHA256).Hash
        fixture_sha256 = (Get-FileHash -LiteralPath $fixtureExe -Algorithm SHA256).Hash
        test_exit_code = $test.ExitCode
        recorded_audio_files = $false
        run_root = $runRoot
    }
    $json = $metadata | ConvertTo-Json
    [IO.File]::WriteAllText((Join-Path $runRoot 'result.json'), $json, $utf8)
    Write-Output $stdout
    Write-Output $stderr
    Write-Output ($metadata | ConvertTo-Json -Compress)
    if ($test.ExitCode -ne 0) { throw ('Native quiet smoke failed with code ' + $test.ExitCode) }
} finally {
    if ($test -and -not $test.HasExited) { $test.Kill(); $test.WaitForExit() }
    if ($fixture -and -not $fixture.HasExited) {
        [IO.File]::WriteAllText($stopMarker, 'stop', $utf8)
        if (-not $fixture.WaitForExit(5000)) { $fixture.Kill(); $fixture.WaitForExit() }
    }
    if ($fixtureError) {
        [IO.File]::WriteAllText((Join-Path $runRoot 'fixture.stderr.log'), $fixtureError.Result, $utf8)
    }
}
