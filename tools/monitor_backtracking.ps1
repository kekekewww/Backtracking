<#
.SYNOPSIS
Records process and system hardware usage without changing the monitored app.
.DESCRIPTION
Writes telemetry.jsonl, one JSON object per sample. CPU is percent (one-core
process CPU may exceed 100); memory is bytes and MiB (1 MiB = 1048576 bytes).
I/O is cumulative bytes and bytes/second from GetProcessIoCounters, including
cached/non-disk I/O. GPU samples are collected asynchronously every five seconds
using CIM, with Get-Counter as a fallback. Engine sums can exceed 100 percent.

Write scenario.json in OutputDirectory to tag subsequent samples, for example:
{"scenario":"transcribe","settings":{"model":"small"},"transition":false}
Missing or invalid scenario metadata marks a sample as transition=true.
Create stop.marker in OutputDirectory to stop. The duration limit also stops it.
Existing telemetry is appended; session_id distinguishes collector runs.
.EXAMPLE
powershell.exe -NoProfile -ExecutionPolicy Bypass -File .\tools\monitor_backtracking.ps1 -ProcessId 29260 -OutputDirectory .\artifacts\hardware-run -DurationSeconds 1800
#>
[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)]
    [ValidateRange(1, 2147483647)]
    [int]$ProcessId,

    [Parameter(Mandatory = $true)]
    [string]$OutputDirectory,

    [ValidateRange(1, 86400)]
    [int]$DurationSeconds = 1800,

    [ValidateRange(0.25, 60)]
    [double]$SampleIntervalSeconds = 1,

    [ValidateRange(2, 300)]
    [int]$GpuIntervalSeconds = 5
)

$ErrorActionPreference = 'Stop'
$sessionId = [Guid]::NewGuid().ToString('N')
$logicalProcessorCount = [Environment]::ProcessorCount
$outputPath = [IO.Path]::GetFullPath($OutputDirectory)
$null = [IO.Directory]::CreateDirectory($outputPath)
$telemetryPath = Join-Path $outputPath 'telemetry.jsonl'
$stopPath = Join-Path $outputPath 'stop.marker'
$scenarioPath = Join-Path $outputPath 'scenario.json'
$statusPath = Join-Path $outputPath 'monitor_status.json'
$metadataPath = Join-Path $outputPath ("monitor_metadata.{0}.json" -f $sessionId)
$utf8NoBom = New-Object Text.UTF8Encoding($false)
$timer = [Diagnostics.Stopwatch]::StartNew()
$sampleCount = 0
$stopReason = 'duration_limit'
$lastProcessCpu = $null
$lastProcessAt = $null
$lastIo = $null
$lastIoAt = $null
$lastSystem = $null
$gpuWorker = $null
$gpuAsync = $null
$gpuScenario = $null
$gpuCollectionStarted = $null
$scenarioSequence = 0
$lastScenarioFingerprint = $null
$lastGpuStart = -$GpuIntervalSeconds
$gpuCache = [ordered]@{ status = 'pending'; observed_utc = $null; source = $null; engines = @(); memory_instances = @() }

function Write-JsonFile([string]$Path, $Value) {
    $jsonText = $Value | ConvertTo-Json -Depth 30
    [IO.File]::WriteAllText($Path, $jsonText, $utf8NoBom)
}

function Get-Rounded($Value, [int]$Digits = 3) {
    if ($null -eq $Value) { return $null }
    return [Math]::Round([double]$Value, $Digits)
}

function Read-Scenario {
    if (-not [IO.File]::Exists($scenarioPath)) {
        return @{ scenario = 'unlabeled'; settings = @{}; transition = $true; status = 'missing'; metadata = $null }
    }
    try {
        $scenarioText = [IO.File]::ReadAllText($scenarioPath)
        if (-not $scenarioText.TrimStart().StartsWith('{')) {
            throw 'scenario.json must contain an object.'
        }
        $scenarioData = $scenarioText | ConvertFrom-Json
        $scenarioName = if ($scenarioData.PSObject.Properties['scenario']) { $scenarioData.scenario } else { 'unlabeled' }
        $scenarioSettings = if ($scenarioData.PSObject.Properties['settings']) { $scenarioData.settings } else { @{} }
        $isTransition = if ($scenarioData.PSObject.Properties['transition']) { [bool]$scenarioData.transition } else { $false }
        if ($scenarioData.PSObject.Properties['transitions']) { $isTransition = $isTransition -or [bool]$scenarioData.transitions }
        return @{ scenario = $scenarioName; settings = $scenarioSettings; transition = $isTransition; status = 'available'; metadata = $scenarioData }
    }
    catch {
        return @{ scenario = 'unlabeled'; settings = @{}; transition = $true; status = 'invalid'; metadata = $null; error = $_.Exception.Message }
    }
}

$nativeReady = $false
$nativeError = $null
try {
    if (-not ('BacktrackingMonitor.NativeCounters' -as [type])) {
        Add-Type -TypeDefinition @'
using System;
using System.Runtime.InteropServices;
namespace BacktrackingMonitor {
    [StructLayout(LayoutKind.Sequential)]
    public struct IoCounters {
        public ulong ReadOperations, WriteOperations, OtherOperations;
        public ulong ReadBytes, WriteBytes, OtherBytes;
    }
    [StructLayout(LayoutKind.Sequential)]
    public struct MemoryStatus {
        public uint Length, MemoryLoad;
        public ulong TotalPhysical, AvailablePhysical, TotalPageFile, AvailablePageFile;
        public ulong TotalVirtual, AvailableVirtual, AvailableExtendedVirtual;
    }
    public static class NativeCounters {
        [DllImport("kernel32.dll", SetLastError=true)]
        private static extern IntPtr OpenProcess(uint access, bool inherit, int processId);
        [DllImport("kernel32.dll", SetLastError=true)]
        private static extern bool CloseHandle(IntPtr handle);
        [DllImport("kernel32.dll", SetLastError=true)]
        private static extern bool GetProcessIoCounters(IntPtr handle, out IoCounters counters);
        [DllImport("kernel32.dll", SetLastError=true)]
        private static extern bool GetSystemTimes(out long idle, out long kernel, out long user);
        [DllImport("kernel32.dll", SetLastError=true)]
        private static extern bool GlobalMemoryStatusEx(ref MemoryStatus status);
        public static IoCounters ReadIo(int processId) {
            IntPtr handle = OpenProcess(0x1000, false, processId);
            if (handle == IntPtr.Zero) throw new System.ComponentModel.Win32Exception(Marshal.GetLastWin32Error());
            try {
                IoCounters counters;
                if (!GetProcessIoCounters(handle, out counters)) throw new System.ComponentModel.Win32Exception(Marshal.GetLastWin32Error());
                return counters;
            } finally { CloseHandle(handle); }
        }
        public static long[] ReadSystemTimes() {
            long idle, kernel, user;
            if (!GetSystemTimes(out idle, out kernel, out user)) throw new System.ComponentModel.Win32Exception(Marshal.GetLastWin32Error());
            return new long[] { idle, kernel, user };
        }
        public static MemoryStatus ReadMemory() {
            MemoryStatus status = new MemoryStatus();
            status.Length = (uint)Marshal.SizeOf(typeof(MemoryStatus));
            if (!GlobalMemoryStatusEx(ref status)) throw new System.ComponentModel.Win32Exception(Marshal.GetLastWin32Error());
            return status;
        }
    }
}
'@
    }
    $nativeReady = $true
}
catch { $nativeError = $_.Exception.Message }

# Each query runs away from the one-second sampling loop. The same Windows
# counters may deny CIM access while allowing the performance counter API.
$gpuScript = {
    param([int]$TargetProcessId, [bool]$IncludeHardware)
    $ErrorActionPreference = 'Stop'
    $notes = @()
    $engines = @()
    $memoryInstances = @()
    $engineStatus = 'unavailable'
    $memoryStatus = 'unavailable'
    $sourceName = 'cim'
    $processPrefix = '^pid_' + $TargetProcessId + '_'
    try {
        $engineRows = @(Get-CimInstance -Namespace root/cimv2 -ClassName Win32_PerfFormattedData_GPUPerformanceCounters_GPUEngine -OperationTimeoutSec 3)
        $memoryRows = @(Get-CimInstance -Namespace root/cimv2 -ClassName Win32_PerfFormattedData_GPUPerformanceCounters_GPUProcessMemory -OperationTimeoutSec 3)
        $engines = @($engineRows | Where-Object { $_.Name -match $processPrefix } | ForEach-Object {
            [pscustomobject]@{ instance = $_.Name; utilization_percent = [double]$_.UtilizationPercentage }
        })
        $memoryInstances = @($memoryRows | Where-Object { $_.Name -match $processPrefix } | ForEach-Object {
            [pscustomobject]@{ instance = $_.Name; dedicated_bytes = [double]$_.DedicatedUsage; shared_bytes = [double]$_.SharedUsage }
        })
        $engineStatus = if ($engines.Count) { 'available' } else { 'no_process_counters' }
        $memoryStatus = if ($memoryInstances.Count) { 'available' } else { 'no_process_counters' }
    }
    catch {
        $notes += 'CIM: ' + $_.Exception.Message
        $sourceName = 'get_counter'
        try {
            $counterErrors = @()
            $counterSet = Get-Counter -Counter @(
                ("\GPU Engine(pid_{0}*)\Utilization Percentage" -f $TargetProcessId),
                ("\GPU Process Memory(pid_{0}*)\Dedicated Usage" -f $TargetProcessId),
                ("\GPU Process Memory(pid_{0}*)\Shared Usage" -f $TargetProcessId)
            ) -MaxSamples 1 -ErrorAction SilentlyContinue -ErrorVariable counterErrors
            $allSamples = @($counterSet.CounterSamples | Where-Object { $_.InstanceName -match $processPrefix -and $_.Status -eq 0 })
            $engines = @($allSamples | Where-Object { $_.Path -match '\\utilization percentage$' } | ForEach-Object {
                [pscustomobject]@{ instance = $_.InstanceName; utilization_percent = [Math]::Round([double]$_.CookedValue, 4) }
            })
            $memoryMap = @{}
            foreach ($counter in @($allSamples | Where-Object { $_.Path -match '\\(dedicated|shared) usage$' })) {
                if (-not $memoryMap.ContainsKey($counter.InstanceName)) {
                    $memoryMap[$counter.InstanceName] = [ordered]@{ instance = $counter.InstanceName; dedicated_bytes = $null; shared_bytes = $null }
                }
                if ($counter.Path -match '\\dedicated usage$') { $memoryMap[$counter.InstanceName].dedicated_bytes = [double]$counter.CookedValue }
                if ($counter.Path -match '\\shared usage$') { $memoryMap[$counter.InstanceName].shared_bytes = [double]$counter.CookedValue }
            }
            $memoryInstances = @($memoryMap.Values | ForEach-Object { [pscustomobject]$_ })
            $engineStatus = if ($engines.Count) { 'available' } else { 'no_valid_process_counters' }
            $memoryStatus = if ($memoryInstances.Count) { 'available' } else { 'no_valid_process_counters' }
            foreach ($counterError in $counterErrors) { $notes += 'Get-Counter: ' + $counterError.Exception.Message }
        }
        catch { $notes += 'Get-Counter: ' + $_.Exception.Message }
    }
    $gpu = [ordered]@{
        status = if ($engineStatus -eq 'available' -or $memoryStatus -eq 'available') { 'available' } else { 'unavailable' }
        observed_utc = [DateTime]::UtcNow.ToString('o')
        source = $sourceName
        engine_status = $engineStatus
        memory_status = $memoryStatus
        busiest_engine_percent = $null
        engine_utilization_sum_percent = $null
        dedicated_bytes = $null
        shared_bytes = $null
        dedicated_mib = $null
        shared_mib = $null
        engines = $engines
        memory_instances = $memoryInstances
        notes = $notes
    }
    if ($engines.Count) {
        $gpu.busiest_engine_percent = [Math]::Round(($engines | Measure-Object utilization_percent -Maximum).Maximum, 4)
        $gpu.engine_utilization_sum_percent = [Math]::Round(($engines | Measure-Object utilization_percent -Sum).Sum, 4)
    }
    if ($memoryInstances.Count) {
        $dedicatedValues = @($memoryInstances | Where-Object { $null -ne $_.dedicated_bytes })
        $sharedValues = @($memoryInstances | Where-Object { $null -ne $_.shared_bytes })
        if ($dedicatedValues.Count) {
            $gpu.dedicated_bytes = ($dedicatedValues | Measure-Object dedicated_bytes -Sum).Sum
            $gpu.dedicated_mib = [Math]::Round($gpu.dedicated_bytes / 1MB, 3)
        }
        if ($sharedValues.Count) {
            $gpu.shared_bytes = ($sharedValues | Measure-Object shared_bytes -Sum).Sum
            $gpu.shared_mib = [Math]::Round($gpu.shared_bytes / 1MB, 3)
        }
    }
    $hardware = $null
    if ($IncludeHardware) {
        $hardware = [ordered]@{ status = 'available'; errors = @() }
        foreach ($query in @(
            @{ key = 'computer'; class = 'Win32_ComputerSystem'; properties = @('Manufacturer','Model','TotalPhysicalMemory','NumberOfLogicalProcessors') },
            @{ key = 'processors'; class = 'Win32_Processor'; properties = @('Name','Manufacturer','NumberOfCores','NumberOfLogicalProcessors','MaxClockSpeed') },
            @{ key = 'operating_system'; class = 'Win32_OperatingSystem'; properties = @('Caption','Version','BuildNumber','OSArchitecture') },
            @{ key = 'video_controllers'; class = 'Win32_VideoController'; properties = @('Name','VideoProcessor','DriverVersion','DriverDate','PNPDeviceID') }
        )) {
            try {
                $hardware[$query.key] = @(Get-CimInstance -Namespace root/cimv2 -ClassName $query.class -OperationTimeoutSec 3 | Select-Object -Property $query.properties)
            }
            catch {
                $hardware.status = 'partial_or_unavailable'
                $hardware[$query.key] = $null
                $hardware.errors += $query.class + ': ' + $_.Exception.Message
            }
        }
    }
    [pscustomobject]@{ gpu = $gpu; hardware = $hardware }
}

$metadata = [ordered]@{
    schema_version = 1
    session_id = $sessionId
    started_utc = [DateTime]::UtcNow.ToString('o')
    target_process_id = $ProcessId
    monitor_process_id = $PID
    logical_processor_count = $logicalProcessorCount
    sample_interval_seconds = $SampleIntervalSeconds
    gpu_interval_seconds = $GpuIntervalSeconds
    duration_limit_seconds = $DurationSeconds
    powershell_version = $PSVersionTable.PSVersion.ToString()
    os_version = [Environment]::OSVersion.VersionString
    process_architecture_bits = [IntPtr]::Size * 8
    native_counter_status = if ($nativeReady) { 'available' } else { 'unavailable' }
    native_counter_error = $nativeError
    hardware = @{ status = 'pending' }
    units = @{
        timestamp_utc = 'ISO 8601 UTC'
        elapsed_seconds = 'monotonic seconds since collector startup'
        cpu_percent_one_core = '100 percent = one logical processor'
        cpu_percent_machine = '100 percent = all logical processors'
        memory_mib = 'bytes / 1048576'
        io_bytes_per_second = 'transfer bytes / actual elapsed sample interval; includes cached and non-disk I/O'
        gpu_engine_percent = 'individual engine utilization; engine sums may exceed 100 percent'
    }
}
Write-JsonFile $metadataPath $metadata
[IO.File]::WriteAllText((Join-Path $outputPath 'monitor.pid'), [string]$PID, $utf8NoBom)
$writer = New-Object IO.StreamWriter($telemetryPath, $true, $utf8NoBom)
$writer.AutoFlush = $true

try {
    while ($timer.Elapsed.TotalSeconds -lt $DurationSeconds) {
        if ([IO.File]::Exists($stopPath)) { $stopReason = 'stop_marker'; break }
        $sampleStarted = $timer.Elapsed.TotalSeconds
        $scenario = Read-Scenario
        $scenarioFingerprint = $scenario | ConvertTo-Json -Depth 30 -Compress
        if ($scenarioFingerprint -ne $lastScenarioFingerprint) {
            $scenarioSequence++
            $lastScenarioFingerprint = $scenarioFingerprint
        }
        if ($null -ne $gpuAsync -and $gpuAsync.IsCompleted) {
            try {
                $workerResults = @($gpuWorker.EndInvoke($gpuAsync))
                if ($workerResults.Count) {
                    $gpuCache = $workerResults[-1].gpu
                    $gpuCache.collection_started_utc = $gpuCollectionStarted
                    $gpuCache.scenario = $gpuScenario.scenario
                    $gpuCache.settings = $gpuScenario.settings
                    $gpuCache.transition = $gpuScenario.transition
                    $gpuCache.scenario_sequence = $gpuScenario.sequence
                    if ($null -ne $workerResults[-1].hardware) {
                        $metadata.hardware = $workerResults[-1].hardware
                        Write-JsonFile $metadataPath $metadata
                    }
                }
            }
            catch { $gpuCache = @{ status = 'unavailable'; observed_utc = [DateTime]::UtcNow.ToString('o'); source = 'worker'; notes = @($_.Exception.Message) } }
            finally { $gpuWorker.Dispose(); $gpuWorker = $null; $gpuAsync = $null }
        }
        if ($null -eq $gpuAsync -and ($sampleStarted - $lastGpuStart) -ge $GpuIntervalSeconds) {
            $gpuScenario = @{ scenario = $scenario.scenario; settings = $scenario.settings; transition = $scenario.transition; sequence = $scenarioSequence }
            $gpuCollectionStarted = [DateTime]::UtcNow.ToString('o')
            $gpuWorker = [PowerShell]::Create()
            $null = $gpuWorker.AddScript($gpuScript.ToString()).AddArgument($ProcessId).AddArgument($metadata.hardware.status -eq 'pending')
            $gpuAsync = $gpuWorker.BeginInvoke()
            $lastGpuStart = $sampleStarted
        }

        $processMetrics = [ordered]@{ status = 'unavailable'; process_id = $ProcessId }
        $ioMetrics = [ordered]@{ status = 'unavailable'; read_bytes = $null; write_bytes = $null; read_bytes_per_second = $null; write_bytes_per_second = $null }
        try {
            $targetProcess = Get-Process -Id $ProcessId -ErrorAction Stop
            if ($null -eq $targetProcess) { throw "Process $ProcessId is not visible to this collector." }
            try {
                $processCpu = $targetProcess.TotalProcessorTime.TotalSeconds
                $processAt = $timer.Elapsed.TotalSeconds
                $oneCoreCpu = $null
                if ($null -ne $lastProcessCpu -and $processAt -gt $lastProcessAt -and $processCpu -ge $lastProcessCpu) {
                    $oneCoreCpu = 100 * ($processCpu - $lastProcessCpu) / ($processAt - $lastProcessAt)
                }
                $processMetrics = [ordered]@{
                    status = 'available'; process_id = $ProcessId; name = $targetProcess.ProcessName
                    cpu_total_seconds = Get-Rounded $processCpu 6
                    cpu_percent_one_core = Get-Rounded $oneCoreCpu
                    cpu_percent_machine = if ($null -eq $oneCoreCpu) { $null } else { Get-Rounded ($oneCoreCpu / $logicalProcessorCount) }
                    working_set_bytes = $targetProcess.WorkingSet64; working_set_mib = Get-Rounded ($targetProcess.WorkingSet64 / 1MB)
                    private_bytes = $targetProcess.PrivateMemorySize64; private_mib = Get-Rounded ($targetProcess.PrivateMemorySize64 / 1MB)
                    virtual_bytes = $targetProcess.VirtualMemorySize64; virtual_mib = Get-Rounded ($targetProcess.VirtualMemorySize64 / 1MB)
                    thread_count = $targetProcess.Threads.Count; handle_count = $targetProcess.HandleCount
                }
                $lastProcessCpu = $processCpu
                $lastProcessAt = $processAt
            }
            finally { $targetProcess.Dispose() }
        }
        catch {
            $processMetrics.error = $_.Exception.Message
            $lastProcessCpu = $null
            $lastProcessAt = $null
        }
        if ($nativeReady) {
            try {
                $ioNow = [BacktrackingMonitor.NativeCounters]::ReadIo($ProcessId)
                $ioAt = $timer.Elapsed.TotalSeconds
                $ioMetrics.status = 'available'
                $ioMetrics.source = 'GetProcessIoCounters'
                $ioMetrics.read_bytes = $ioNow.ReadBytes
                $ioMetrics.write_bytes = $ioNow.WriteBytes
                if ($null -ne $lastIo -and $ioAt -gt $lastIoAt -and $ioNow.ReadBytes -ge $lastIo.ReadBytes -and $ioNow.WriteBytes -ge $lastIo.WriteBytes) {
                    $ioMetrics.read_bytes_per_second = Get-Rounded (($ioNow.ReadBytes - $lastIo.ReadBytes) / ($ioAt - $lastIoAt))
                    $ioMetrics.write_bytes_per_second = Get-Rounded (($ioNow.WriteBytes - $lastIo.WriteBytes) / ($ioAt - $lastIoAt))
                }
                $lastIo = $ioNow
                $lastIoAt = $ioAt
            }
            catch { $ioMetrics.error = $_.Exception.Message; $lastIo = $null; $lastIoAt = $null }
        }
        else { $ioMetrics.error = $nativeError }

        $systemMetrics = [ordered]@{ source = 'kernel32'; cpu_status = 'unavailable'; memory_status = 'unavailable'; cpu_percent = $null; available_memory_bytes = $null; available_memory_mib = $null }
        if ($nativeReady) {
            try {
                $systemNow = [BacktrackingMonitor.NativeCounters]::ReadSystemTimes()
                $systemMetrics.cpu_status = 'available'
                if ($null -ne $lastSystem) {
                    $totalTicks = ($systemNow[1] - $lastSystem[1]) + ($systemNow[2] - $lastSystem[2])
                    if ($totalTicks -gt 0) { $systemMetrics.cpu_percent = Get-Rounded (100 * (1 - ($systemNow[0] - $lastSystem[0]) / $totalTicks)) }
                }
                $lastSystem = $systemNow
            }
            catch { $systemMetrics.cpu_error = $_.Exception.Message }
            try {
                $memoryNow = [BacktrackingMonitor.NativeCounters]::ReadMemory()
                $systemMetrics.memory_status = 'available'
                $systemMetrics.available_memory_bytes = $memoryNow.AvailablePhysical
                $systemMetrics.available_memory_mib = Get-Rounded ($memoryNow.AvailablePhysical / 1MB)
                $systemMetrics.total_memory_bytes = $memoryNow.TotalPhysical
            }
            catch { $systemMetrics.memory_error = $_.Exception.Message }
        }
        else { $systemMetrics.error = $nativeError }

        $gpuRecord = [ordered]@{}
        foreach ($gpuKey in $gpuCache.Keys) { $gpuRecord[$gpuKey] = $gpuCache[$gpuKey] }
        $gpuRecord.age_seconds = if ($gpuCache.observed_utc) { Get-Rounded (([DateTime]::UtcNow - [DateTime]::Parse($gpuCache.observed_utc).ToUniversalTime()).TotalSeconds) } else { $null }
        $gpuRecord.matches_current_scenario = ($null -ne $gpuCache.observed_utc -and $gpuCache.scenario_sequence -eq $scenarioSequence -and -not $gpuCache.transition -and -not $scenario.transition)
        $sample = [ordered]@{
            schema_version = 1; kind = 'sample'; session_id = $sessionId
            timestamp_utc = [DateTime]::UtcNow.ToString('o'); elapsed_seconds = Get-Rounded $timer.Elapsed.TotalSeconds 6
            scenario = $scenario.scenario; settings = $scenario.settings; transition = $scenario.transition
            scenario_read_status = $scenario.status; scenario_metadata = $scenario.metadata; scenario_sequence = $scenarioSequence
            process = $processMetrics; io = $ioMetrics; system = $systemMetrics; gpu = $gpuRecord
        }
        if ($scenario.error) { $sample.scenario_error = $scenario.error }
        $writer.WriteLine(($sample | ConvertTo-Json -Depth 30 -Compress))
        $sampleCount++
        Write-JsonFile $statusPath ([ordered]@{
            state = 'running'; session_id = $sessionId; monitor_process_id = $PID; target_process_id = $ProcessId
            last_sample_utc = $sample.timestamp_utc; samples = $sampleCount; elapsed_seconds = $sample.elapsed_seconds
            scenario = $scenario.scenario; telemetry_path = $telemetryPath; metadata_path = $metadataPath
        })
        $remainingSeconds = [Math]::Min($SampleIntervalSeconds - ($timer.Elapsed.TotalSeconds - $sampleStarted), $DurationSeconds - $timer.Elapsed.TotalSeconds)
        if ($remainingSeconds -gt 0) { Start-Sleep -Milliseconds ([int][Math]::Ceiling(1000 * $remainingSeconds)) }
    }
}
catch { $stopReason = 'collector_error'; throw }
finally {
    if ($null -ne $gpuWorker) {
        try { $gpuWorker.Stop() } catch {}
        $gpuWorker.Dispose()
    }
    $writer.Dispose()
    Write-JsonFile $statusPath ([ordered]@{
        state = 'stopped'; stop_reason = $stopReason; session_id = $sessionId
        monitor_process_id = $PID; target_process_id = $ProcessId; stopped_utc = [DateTime]::UtcNow.ToString('o')
        samples = $sampleCount; elapsed_seconds = Get-Rounded $timer.Elapsed.TotalSeconds 6
        telemetry_path = $telemetryPath; metadata_path = $metadataPath
    })
}
