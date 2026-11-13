[CmdletBinding()]
param(
    [ValidateSet('All', 'Compile', 'Smoke')]
    [string]$Case = 'All',
    [string]$NsisPath
)

# This suite never packages or starts the real application. Its text payload,
# shortcuts, installation folders and registry keys all belong to a fresh run.
$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest
$repositoryRoot = [IO.Path]::GetFullPath((Split-Path -Parent $PSScriptRoot))
$suiteId = [guid]::NewGuid().ToString('N')
$testRoot = Join-Path $repositoryRoot "target\setup-tests\$suiteId"
$registryRoot = "Software\BacktrackingSetupTests\$suiteId"
$registryBase = [Microsoft.Win32.RegistryKey]::OpenBaseKey(
    [Microsoft.Win32.RegistryHive]::CurrentUser, [Microsoft.Win32.RegistryView]::Registry64)
$contexts = New-Object 'System.Collections.Generic.List[object]'
$passed = 0
$skipped = 0
$driveAlias = $null
$driveAliasTarget = $null
$driveAliasCreated = $false
$utf8 = New-Object Text.UTF8Encoding($false)

function Assert-True([bool]$Condition, [string]$Message) {
    if (-not $Condition) { throw "Assertion failed: $Message" }
}

function Assert-TestPath([string]$Path) {
    $absolute = [IO.Path]::GetFullPath($Path)
    Assert-True ($absolute.StartsWith($testRoot + '\', [StringComparison]::OrdinalIgnoreCase)) "Path escapes the isolated run: $absolute"
    Assert-True (-not $absolute.Contains('"')) "A test path contains a command-line quote: $absolute"
    return $absolute
}

function Write-Fixture([string]$Path, [string]$Value = 'Backtracking installer fixture; never execute this file.') {
    $absolute = Assert-TestPath $Path
    [IO.Directory]::CreateDirectory([IO.Path]::GetDirectoryName($absolute)) | Out-Null
    [IO.File]::WriteAllText($absolute, $Value, $utf8)
}

function Get-TestValue($Context, [string]$Name) {
    $key = $registryBase.OpenSubKey($Context.InstallerKey)
    if ($null -eq $key) { return $null }
    try { return $key.GetValue($Name, $null) } finally { $key.Dispose() }
}

function Set-TestValue($Context, [string]$Name, [string]$Value) {
    Assert-True ($Context.InstallerKey.StartsWith($registryRoot + '\')) 'Registry write is outside the isolated namespace.'
    $key = $registryBase.CreateSubKey($Context.InstallerKey)
    try { $key.SetValue($Name, $Value, [Microsoft.Win32.RegistryValueKind]::String) } finally { $key.Dispose() }
}

function New-TestContext([string]$Name, [switch]$DesktopOnly) {
    Assert-True ($Name -match '^[a-z0-9-]+$') 'Scenario names must be simple identifiers.'
    $directory = Assert-TestPath (Join-Path $testRoot $Name)
    [IO.Directory]::CreateDirectory($directory) | Out-Null
    $context = [pscustomobject]@{
        Name = $Name
        Directory = $directory
        InstallerKey = "$registryRoot\$Name\Installer"
        UninstallKey = "Software\Microsoft\Windows\CurrentVersion\Uninstall\BacktrackingSetupTests-$suiteId-$Name"
        RunKey = "$registryRoot\$Name\Run"
        App = Join-Path $directory 'app'
        DefaultVst3 = Join-Path $directory 'defaults\VST3'
        DefaultClap = Join-Path $directory 'defaults\CLAP'
        StartMenu = Join-Path $directory 'shortcuts\Start Menu'
        Shortcut = Join-Path $directory 'shortcuts\Desktop.lnk'
        Setup = Join-Path $directory 'FixtureInstaller.exe'
        Payload = Join-Path $directory 'payload'
        DesktopOnly = [bool]$DesktopOnly
    }
    $contexts.Add($context)
    foreach ($name in @('LICENSE.txt', 'FONT-LICENSE.txt', 'USER_GUIDE.md', 'BUILD-INFO.txt', 'desktop\Backtracking.exe')) {
        Write-Fixture (Join-Path $context.Payload $name)
    }
    foreach ($directoryName in @('desktop', 'plugins', 'plugins\Backtracking.vst3\Contents\x86_64-win')) {
        foreach ($dll in @('vcruntime140.dll', 'vcruntime140_1.dll')) {
            Write-Fixture (Join-Path $context.Payload "$directoryName\$dll")
        }
    }
    Write-Fixture (Join-Path $context.Payload 'plugins\Backtracking.clap')
    Write-Fixture (Join-Path $context.Payload 'plugins\Backtracking.vst3\Contents\x86_64-win\Backtracking.vst3')

    $arguments = @(
        '/V2', '/DAPP_VERSION=0.0.1', "/DPAYLOAD_DIR=$($context.Payload)", "/DOUTPUT_FILE=$($context.Setup)",
        ('/DASSETS_DIR=' + (Join-Path $repositoryRoot 'assets')),
        "/DINSTALLER_KEY=$($context.InstallerKey)", "/DUNINSTALL_KEY=$($context.UninstallKey)",
        "/DSTARTMENU_DIR=$($context.StartMenu)", "/DDESKTOP_SHORTCUT=$($context.Shortcut)",
        "/DDEFAULT_INSTALL_DIR=$($context.App)", "/DDEFAULT_VST3_ROOT=$($context.DefaultVst3)",
        "/DDEFAULT_CLAP_ROOT=$($context.DefaultClap)", "/DRUN_KEY=$($context.RunKey)",
        "/DRUN_VALUE=BacktrackingSetupTests-$suiteId", "/DDESKTOP_MUTEX=Local\BacktrackingSetupTests.$suiteId"
    )
    if (-not $DesktopOnly) { $arguments += '/DINCLUDE_PLUGINS' }
    $arguments += (Join-Path $repositoryRoot 'desktop\windows\installer\Backtracking.nsi')
    $savedPreference = $ErrorActionPreference
    try {
        $ErrorActionPreference = 'Continue'
        $output = @(& $compiler @arguments 2>&1 | ForEach-Object { $_.ToString() })
        $compileExit = $LASTEXITCODE
    } finally { $ErrorActionPreference = $savedPreference }
    [IO.File]::WriteAllLines((Join-Path $directory 'compile.log'), [string[]]$output, $utf8)
    Assert-True ($compileExit -eq 0) "Isolated $Name compilation failed (exit $compileExit). See $directory\compile.log"
    $warnings = @($output | Where-Object { $_ -match '^\s*warning\s+\d+:' })
    Assert-True ($warnings.Count -eq 0) "Isolated $Name compilation emitted warnings. See $directory\compile.log"
    Assert-True (Test-Path -LiteralPath $context.Setup -PathType Leaf) 'Compiler did not create the isolated Setup.'
    return $context
}

function Invoke-TestSetup($Context, [string[]]$Options = @(), [string]$InstallDirectory = '', [int]$ExpectedExit = 0) {
    if (-not $InstallDirectory) { $InstallDirectory = $Context.App }
    $destination = Assert-TestPath $InstallDirectory
    $executable = Assert-TestPath $Context.Setup
    # /D is last and deliberately unquoted, as required by NSIS even with spaces.
    $arguments = (@('/S') + $Options + @('/D=' + $destination)) -join ' '
    $process = Start-Process -FilePath $executable -ArgumentList $arguments -WorkingDirectory $Context.Directory -WindowStyle Hidden -Wait -PassThru
    [IO.File]::AppendAllText((Join-Path $Context.Directory 'executions.log'), "Setup exit $($process.ExitCode) (expected $ExpectedExit): $arguments`r`n", $utf8)
    Assert-True ($process.ExitCode -eq $ExpectedExit) "$($Context.Name): Setup exited $($process.ExitCode), expected $ExpectedExit. Arguments: $arguments"
}

function Invoke-TestUninstall($Context, [string]$InstallDirectory = '', [int]$ExpectedExit = 0) {
    if (-not $InstallDirectory) { $InstallDirectory = $Context.App }
    $uninstaller = Assert-TestPath (Join-Path $InstallDirectory 'Uninstall.exe')
    Assert-True (Test-Path -LiteralPath $uninstaller -PathType Leaf) "Missing uninstaller: $uninstaller"
    # Run a copy with _?= so NSIS does not spawn a temporary child. The original
    # app-local Uninstall.exe is then unlocked and can remove itself normally.
    $runner = Assert-TestPath (Join-Path $Context.Directory ("uninstall-runner\$([guid]::NewGuid().ToString('N')).exe"))
    [IO.Directory]::CreateDirectory([IO.Path]::GetDirectoryName($runner)) | Out-Null
    Copy-Item -LiteralPath $uninstaller -Destination $runner
    $destination = Assert-TestPath $InstallDirectory
    $arguments = '/S _?=' + $destination
    $process = Start-Process -FilePath $runner -ArgumentList $arguments -WorkingDirectory $Context.Directory -WindowStyle Hidden -Wait -PassThru
    [IO.File]::AppendAllText((Join-Path $Context.Directory 'executions.log'), "Uninstall exit $($process.ExitCode) (expected $ExpectedExit): $runner $arguments`r`n", $utf8)
    Assert-True ($process.ExitCode -eq $ExpectedExit) "$($Context.Name): Uninstall exited $($process.ExitCode), expected $ExpectedExit."
}

function Assert-File([string]$Path, [bool]$Present = $true) {
    Assert-TestPath $Path | Out-Null
    Assert-True ((Test-Path -LiteralPath $Path -PathType Leaf) -eq $Present) "File presence should be $Present : $Path"
}

function Complete-Test([string]$Name) {
    $script:passed++
    Write-Output "PASS $Name"
}

function Save-RegistryDiagnostics($Context) {
    $key = $registryBase.OpenSubKey($Context.InstallerKey)
    $lines = @("HKCU\$($Context.InstallerKey)")
    if ($null -eq $key) { $lines += '(key absent)' }
    else {
        try {
            foreach ($name in $key.GetValueNames()) { $lines += "$name=$($key.GetValue($name))" }
        } finally { $key.Dispose() }
    }
    [IO.File]::WriteAllLines((Join-Path $Context.Directory 'registry-before-cleanup.txt'), [string[]]$lines, $utf8)
}

function Get-UnusedDrive {
    $used = [Environment]::GetLogicalDrives()
    foreach ($letter in 'ZYXWVUTSRQPONMLKJIHGFED'.ToCharArray()) {
        $candidate = "$letter`:\"
        if ($used -notcontains $candidate) { return $candidate }
    }
    throw 'No unused drive letter is available for isolated path validation.'
}

function Get-Vst3File([string]$Root) {
    return Join-Path $Root 'Backtracking.vst3\Contents\x86_64-win\Backtracking.vst3'
}

function Get-ClapFile([string]$Root) {
    return Join-Path $Root 'Backtracking\Backtracking.clap'
}

if ($NsisPath) { $compiler = (Resolve-Path -LiteralPath $NsisPath).Path }
else {
    $compiler = Join-Path $repositoryRoot 'target\installer-tools\nsis-3.13\makensis.exe'
    if (-not (Test-Path -LiteralPath $compiler)) {
        $installedCompiler = Get-Command makensis.exe -ErrorAction SilentlyContinue
        if ($null -eq $installedCompiler) { throw 'NSIS is required. Build Setup once or provide -NsisPath.' }
        $compiler = $installedCompiler.Source
    }
}
Assert-True ($testRoot.StartsWith($repositoryRoot + '\target\setup-tests\')) 'Unexpected test artifact root.'
[IO.Directory]::CreateDirectory($testRoot) | Out-Null
Write-Output "Isolated Setup artifacts: $testRoot"

try {
    $compileFull = New-TestContext 'compile-full'
    $compileDesktop = New-TestContext 'compile-desktop' -DesktopOnly
    Complete-Test 'full and desktop-only fixture compilation'
    if ($Case -eq 'Compile') { return }

    $smoke = New-TestContext 'custom-roots'
    Set-TestValue $smoke 'TestRegistryProbe' $suiteId
    Assert-True ((Get-TestValue $smoke 'TestRegistryProbe') -eq $suiteId) 'The isolated test registry namespace is not writable/readable.'
    $localizedSuffix = ([char]0x97F3).ToString() + [char]0x6548
    $vstRoot = Join-Path $smoke.Directory ('VST3 custom ' + $localizedSuffix)
    $clapRoot = Join-Path $smoke.Directory ('CLAP custom ' + $localizedSuffix)
    Write-Fixture (Join-Path $vstRoot 'vcruntime140.dll') 'shared root runtime must survive'
    Write-Fixture (Join-Path $clapRoot 'vcruntime140.dll') 'shared root runtime must survive'
    Invoke-TestSetup $smoke @('/VST3=1', '/CLAP=1', ('/VST3DIR="' + $vstRoot + '"'), ('/CLAPDIR="' + $clapRoot + '"'))
    $vstFile = Join-Path $vstRoot 'Backtracking.vst3\Contents\x86_64-win\Backtracking.vst3'
    $clapFile = Join-Path $clapRoot 'Backtracking\Backtracking.clap'
    Assert-File $vstFile
    Assert-File $clapFile
    $recordedVst = Get-TestValue $smoke 'VST3Dir'
    $recordedClap = Get-TestValue $smoke 'CLAPDir'
    Assert-True ($recordedVst -eq (Join-Path $vstRoot 'Backtracking.vst3')) "Custom VST3 destination was not recorded. Actual: [$recordedVst]"
    Assert-True ($recordedClap -eq (Join-Path $clapRoot 'Backtracking')) "Custom CLAP destination was not recorded. Actual: [$recordedClap]"
    Write-Fixture (Join-Path $vstRoot 'other-plugin.vst3') 'unrelated plugin'
    Write-Fixture (Join-Path $vstRoot 'Backtracking.vst3\Contents\user-added.txt') 'keep this file'
    Write-Fixture (Join-Path $clapRoot 'Backtracking\user-added.txt') 'keep this file'
    Invoke-TestUninstall $smoke
    Assert-File $vstFile $false
    Assert-File $clapFile $false
    Assert-File (Join-Path $vstRoot 'other-plugin.vst3')
    Assert-File (Join-Path $vstRoot 'Backtracking.vst3\Contents\user-added.txt')
    Assert-File (Join-Path $clapRoot 'Backtracking\user-added.txt')
    Assert-True ([IO.File]::ReadAllText((Join-Path $vstRoot 'vcruntime140.dll')) -eq 'shared root runtime must survive') 'VST3 touched a runtime in the shared scan root.'
    Assert-True ([IO.File]::ReadAllText((Join-Path $clapRoot 'vcruntime140.dll')) -eq 'shared root runtime must survive') 'CLAP touched a runtime in the shared scan root.'
    Assert-True ($null -eq (Get-TestValue $smoke 'InstallDir')) 'Uninstall left active test registration.'
    Complete-Test 'independent custom roots and preservation on uninstall'
    if ($Case -eq 'Smoke') { return }

    Invoke-TestSetup $compileDesktop
    Assert-File (Join-Path $compileDesktop.App 'Backtracking.exe')
    Assert-File (Get-Vst3File $compileDesktop.DefaultVst3) $false
    Assert-File (Get-ClapFile $compileDesktop.DefaultClap) $false
    Invoke-TestUninstall $compileDesktop
    Complete-Test 'desktop-only Setup installs and uninstalls without plugins'

    $unselected = New-TestContext 'unselected'
    $unselectedVst = Join-Path $unselected.Directory 'unused VST3'
    $unselectedClap = Join-Path $unselected.Directory 'unused CLAP'
    Invoke-TestSetup $unselected @(('/VST3DIR="' + $unselectedVst + '"'), ('/CLAPDIR="' + $unselectedClap + '"'))
    Assert-File (Join-Path $unselected.App 'Backtracking.exe')
    Assert-File (Get-Vst3File $unselectedVst) $false
    Assert-File (Get-ClapFile $unselectedClap) $false
    Invoke-TestUninstall $unselected
    Complete-Test 'custom path arguments do not implicitly select optional components'

    foreach ($format in @('vst3', 'clap')) {
        $single = New-TestContext ("only-$format")
        $flags = if ($format -eq 'vst3') { @('/VST3=1', '/CLAP=0') } else { @('/VST3=0', '/CLAP=1') }
        Invoke-TestSetup $single $flags
        Assert-File (Get-Vst3File $single.DefaultVst3) ($format -eq 'vst3')
        Assert-File (Get-ClapFile $single.DefaultClap) ($format -eq 'clap')
        Invoke-TestUninstall $single
        Complete-Test "$format can be selected independently"
    }

    $upgrade = New-TestContext 'remember-and-move'
    $oldVstRoot = Join-Path $upgrade.Directory 'old VST3'
    $newVstRoot = Join-Path $upgrade.Directory 'new VST3'
    $keptClapRoot = Join-Path $upgrade.Directory 'kept CLAP'
    Invoke-TestSetup $upgrade @('/VST3=1', '/CLAP=1', ('/VST3DIR="' + $oldVstRoot + '"'), ('/CLAPDIR="' + $keptClapRoot + '"'))
    Write-Fixture (Get-Vst3File $oldVstRoot) 'outdated plugin payload'
    Write-Fixture (Get-ClapFile $keptClapRoot) 'outdated plugin payload'
    Invoke-TestSetup $upgrade
    Assert-True ([IO.File]::ReadAllText((Get-Vst3File $oldVstRoot)) -eq [IO.File]::ReadAllText((Join-Path $upgrade.Payload 'plugins\Backtracking.vst3\Contents\x86_64-win\Backtracking.vst3'))) 'Upgrade did not reselect and update the remembered VST3 path.'
    Assert-True ([IO.File]::ReadAllText((Get-ClapFile $keptClapRoot)) -eq [IO.File]::ReadAllText((Join-Path $upgrade.Payload 'plugins\Backtracking.clap'))) 'Upgrade did not reselect and update the remembered CLAP path.'
    Complete-Test 'upgrade remembers both custom paths and installed selections'
    Write-Fixture (Join-Path $oldVstRoot 'Backtracking.vst3\keep.txt') 'old owned folder sentinel'
    Write-Fixture (Join-Path $newVstRoot 'sibling.txt') 'new root sentinel'
    Write-Fixture (Join-Path $keptClapRoot 'Backtracking\keep.txt') 'CLAP sentinel'
    Invoke-TestSetup $upgrade @(('/VST3DIR="' + $newVstRoot + '"'))
    Assert-File (Get-Vst3File $oldVstRoot) $false
    Assert-File (Get-Vst3File $newVstRoot)
    Assert-File (Get-ClapFile $keptClapRoot)
    Assert-File (Join-Path $oldVstRoot 'Backtracking.vst3\keep.txt')
    Assert-File (Join-Path $newVstRoot 'sibling.txt')
    Assert-True ((Get-TestValue $upgrade 'CLAPDir') -eq (Join-Path $keptClapRoot 'Backtracking')) 'Moving VST3 changed the CLAP registration.'
    Invoke-TestUninstall $upgrade
    Assert-File (Join-Path $keptClapRoot 'Backtracking\keep.txt')
    Assert-File (Join-Path $newVstRoot 'sibling.txt')
    Complete-Test 'moving one format preserves the other format and unrelated files'

    $recordedOverlap = New-TestContext 'recorded-path-overlap'
    $recordedOldRoot = Join-Path $recordedOverlap.Directory 'old CLAP'
    $recordedNewRoot = Join-Path $recordedOverlap.Directory 'new CLAP'
    $recordedOwned = Join-Path $recordedOldRoot 'Backtracking'
    Invoke-TestSetup $recordedOverlap @('/VST3=0', '/CLAP=1', ('/CLAPDIR="' + $recordedOldRoot + '"'))
    Invoke-TestSetup $recordedOverlap @(('/CLAPDIR="' + $recordedNewRoot + '"')) $recordedOwned 87
    Assert-File (Get-ClapFile $recordedOldRoot)
    Assert-File (Join-Path $recordedOwned 'vcruntime140.dll')
    Assert-File (Join-Path $recordedOwned 'Backtracking.exe') $false
    Assert-True ((Get-TestValue $recordedOverlap 'InstallDir') -eq $recordedOverlap.App) 'Rejected relocation changed current desktop registration.'
    $lock = [IO.File]::Open((Get-ClapFile $recordedOldRoot), [IO.FileMode]::Open, [IO.FileAccess]::Read, [IO.FileShare]::Read)
    try {
        Invoke-TestSetup $recordedOverlap @(('/CLAPDIR="' + $recordedNewRoot + '"')) '' 2
        Assert-True ((Get-TestValue $recordedOverlap 'CLAPPreviousDir') -eq $recordedOwned) 'Failed relocation did not keep the prior CLAP path.'
    } finally { $lock.Dispose() }
    Invoke-TestSetup $recordedOverlap @() $recordedOwned 87
    Assert-File (Get-ClapFile $recordedOldRoot)
    Assert-File (Join-Path $recordedOwned 'vcruntime140.dll')
    Assert-File (Join-Path $recordedOwned 'Backtracking.exe') $false
    Invoke-TestUninstall $recordedOverlap
    Complete-Test 'current and pending plugin folders cannot become a desktop destination during relocation'

    $oldDesktopOverlap = New-TestContext 'old-desktop-overlap'
    $oldDesktopParent = Join-Path $oldDesktopOverlap.Directory 'old desktop'
    $oldDesktopApp = Join-Path $oldDesktopParent 'Backtracking'
    $newDesktopApp = Join-Path $oldDesktopOverlap.Directory 'new desktop'
    Invoke-TestSetup $oldDesktopOverlap @('/VST3=0', '/CLAP=0') $oldDesktopApp
    Invoke-TestSetup $oldDesktopOverlap @('/VST3=0', '/CLAP=1', ('/CLAPDIR="' + $oldDesktopParent + '"')) $newDesktopApp 87
    Assert-File (Join-Path $oldDesktopApp 'Backtracking.exe')
    Assert-File (Join-Path $oldDesktopApp 'vcruntime140.dll')
    Assert-File (Join-Path $oldDesktopApp 'Backtracking.clap') $false
    Assert-File (Join-Path $newDesktopApp 'Backtracking.exe') $false
    Assert-True ((Get-TestValue $oldDesktopOverlap 'InstallDir') -eq $oldDesktopApp) 'Rejected plugin/old-desktop overlap changed current registration.'
    $middleDesktopApp = Join-Path $oldDesktopOverlap.Directory 'middle desktop'
    Invoke-TestSetup $oldDesktopOverlap @('/VST3=0', '/CLAP=0') $middleDesktopApp
    Invoke-TestSetup $oldDesktopOverlap @('/VST3=0', '/CLAP=1', ('/CLAPDIR="' + $oldDesktopParent + '"')) $newDesktopApp 87
    Assert-File (Join-Path $oldDesktopApp 'Backtracking.exe')
    Assert-File (Join-Path $middleDesktopApp 'Backtracking.exe')
    Assert-File (Join-Path $oldDesktopApp 'vcruntime140.dll')
    Assert-File (Join-Path $oldDesktopApp 'Backtracking.clap') $false
    Assert-File (Join-Path $newDesktopApp 'Backtracking.exe') $false
    Assert-True ((Get-TestValue $oldDesktopOverlap 'InstallDir') -eq $middleDesktopApp) 'Rejected older-desktop overlap changed current registration.'
    Invoke-TestUninstall $oldDesktopOverlap $oldDesktopApp
    Invoke-TestUninstall $oldDesktopOverlap $middleDesktopApp
    Complete-Test 'a moved desktop cannot turn current or older application folders into plugin folders'

    $stale = New-TestContext 'stale-uninstaller'
    $currentApp = Join-Path $stale.Directory 'new app'
    Invoke-TestSetup $stale @('/VST3=1', '/CLAP=1')
    Invoke-TestSetup $stale @('/VST3=1', '/CLAP=1') $currentApp
    Invoke-TestUninstall $stale
    Assert-File (Join-Path $stale.App 'Backtracking.exe') $false
    Assert-File (Join-Path $currentApp 'Backtracking.exe')
    Assert-File (Get-Vst3File $stale.DefaultVst3)
    Assert-File (Get-ClapFile $stale.DefaultClap)
    Assert-File (Join-Path $stale.StartMenu 'Backtracking.lnk')
    Assert-True ((Get-TestValue $stale 'InstallDir') -eq $currentApp) 'A stale uninstaller removed current registration.'
    Invoke-TestUninstall $stale $currentApp
    Complete-Test 'a stale desktop uninstaller preserves the current installation'

    $locked = New-TestContext 'locked-relocation'
    $lockedOld = Join-Path $locked.Directory 'old VST3'
    $lockedNew = Join-Path $locked.Directory 'new VST3'
    Invoke-TestSetup $locked @('/VST3=1', '/CLAP=0', ('/VST3DIR="' + $lockedOld + '"'))
    $lock = [IO.File]::Open((Get-Vst3File $lockedOld), [IO.FileMode]::Open, [IO.FileAccess]::Read, [IO.FileShare]::Read)
    try {
        Invoke-TestSetup $locked @('/VST3=1', '/CLAP=0', ('/VST3DIR="' + $lockedNew + '"')) '' 2
        Assert-File (Get-Vst3File $lockedOld)
        Assert-True ((Get-TestValue $locked 'VST3PreviousDir') -eq (Join-Path $lockedOld 'Backtracking.vst3')) 'Interrupted relocation lost the previous plugin ownership path.'
    } finally { $lock.Dispose() }
    Invoke-TestSetup $locked @('/VST3=1', '/CLAP=0', ('/VST3DIR="' + $lockedNew + '"'))
    Assert-File (Get-Vst3File $lockedOld) $false
    Assert-File (Get-Vst3File $lockedNew)
    Assert-True ([string]::IsNullOrEmpty((Get-TestValue $locked 'VST3PreviousDir'))) 'Recovered relocation left pending ownership metadata.'
    Invoke-TestUninstall $locked
    Complete-Test 'locked old plugin retains ownership and relocation recovers on retry'

    $lockedUninstall = New-TestContext 'locked-uninstall'
    Invoke-TestSetup $lockedUninstall @('/VST3=0', '/CLAP=1')
    $lockedClapFile = Get-ClapFile $lockedUninstall.DefaultClap
    $lock = [IO.File]::Open($lockedClapFile, [IO.FileMode]::Open, [IO.FileAccess]::Read, [IO.FileShare]::Read)
    try {
        Invoke-TestUninstall $lockedUninstall '' 2
        Assert-File $lockedClapFile
        Assert-True ((Get-TestValue $lockedUninstall 'InstallDir') -eq $lockedUninstall.App) 'Locked uninstall discarded active registration.'
    } finally { $lock.Dispose() }
    Invoke-TestUninstall $lockedUninstall
    Assert-File $lockedClapFile $false
    Assert-True ($null -eq (Get-TestValue $lockedUninstall 'InstallDir')) 'Recovered uninstall left active registration.'
    Complete-Test 'locked current plugin retains registration and uninstall recovers on retry'

    $junction = New-TestContext 'nested-junction'
    Invoke-TestSetup $junction @('/VST3=1', '/CLAP=0')
    $junctionRoot = Join-Path $junction.Directory 'junction VST3'
    $junctionOwned = Join-Path $junctionRoot 'Backtracking.vst3'
    $externalTarget = Join-Path $junction.Directory 'external sentinel target'
    $externalPlugin = Join-Path $externalTarget 'x86_64-win\Backtracking.vst3'
    $externalRuntime = Join-Path $externalTarget 'x86_64-win\vcruntime140.dll'
    Write-Fixture $externalPlugin 'external plugin sentinel'
    Write-Fixture $externalRuntime 'external runtime sentinel'
    Write-Fixture (Join-Path $junctionOwned '.backtracking-setup.ini') ("[Ownership]`r`nProduct=$($junction.InstallerKey)`r`n")
    $junctionLink = Assert-TestPath (Join-Path $junctionOwned 'Contents')
    Assert-TestPath $externalTarget | Out-Null
    $junctionCreated = $false
    try {
        New-Item -ItemType Junction -Path $junctionLink -Target $externalTarget | Out-Null
        $junctionCreated = $true
    } catch {
        $script:skipped++
        Write-Output "SKIP nested-junction: Windows did not permit fixture junction creation: $($_.Exception.Message)"
    }
    if ($junctionCreated) {
        Set-TestValue $junction 'VST3Dir' $junctionOwned
        Invoke-TestSetup $junction @('/VST3=1', '/CLAP=0', ('/VST3DIR="' + $junctionRoot + '"')) '' 87
        Invoke-TestUninstall $junction
        Assert-True ([IO.File]::ReadAllText($externalPlugin) -eq 'external plugin sentinel') 'Setup or uninstall followed a nested plugin junction.'
        Assert-True ([IO.File]::ReadAllText($externalRuntime) -eq 'external runtime sentinel') 'Setup or uninstall removed runtime files through a nested junction.'
        Complete-Test 'nested plugin junction is rejected and cannot redirect uninstall deletion'
    } else { Invoke-TestUninstall $junction }

    $malformed = New-TestContext 'malformed-ownership'
    Invoke-TestSetup $malformed @('/VST3=1', '/CLAP=1')
    $foreignVst = Join-Path $malformed.Directory 'foreign\Backtracking.vst3'
    $foreignClap = Join-Path $malformed.Directory 'foreign\Backtracking'
    Write-Fixture (Join-Path $foreignVst 'Contents\x86_64-win\Backtracking.vst3') 'foreign plugin'
    Write-Fixture (Join-Path $foreignVst 'Contents\x86_64-win\vcruntime140.dll') 'foreign runtime'
    Write-Fixture (Join-Path $foreignClap 'Backtracking.clap') 'foreign plugin'
    Write-Fixture (Join-Path $foreignClap 'vcruntime140.dll') 'foreign runtime'
    Set-TestValue $malformed 'VST3Dir' $foreignVst
    Set-TestValue $malformed 'CLAPDir' $foreignClap
    Set-TestValue $malformed 'VST3PreviousDir' (Join-Path $malformed.Directory '..\malformed-ownership\foreign\Backtracking.vst3')
    Invoke-TestUninstall $malformed
    Assert-File (Join-Path $foreignVst 'Contents\x86_64-win\Backtracking.vst3')
    Assert-File (Join-Path $foreignVst 'Contents\x86_64-win\vcruntime140.dll')
    Assert-File (Join-Path $foreignClap 'Backtracking.clap')
    Assert-File (Join-Path $foreignClap 'vcruntime140.dll')
    Complete-Test 'registry paths without ownership markers cannot delete arbitrary files'

    $legacy = New-TestContext 'legacy-defaults'
    foreach ($pair in @(
        @($legacy.DefaultVst3, 'Backtracking.vst3\Contents\x86_64-win\Backtracking.vst3'),
        @($legacy.DefaultClap, 'Backtracking\Backtracking.clap')
    )) { Write-Fixture (Join-Path $pair[0] $pair[1]) 'legacy markerless payload' }
    $legacyOwnedVst = Join-Path $legacy.DefaultVst3 'Backtracking.vst3'
    $legacyOwnedClap = Join-Path $legacy.DefaultClap 'Backtracking'
    Write-Fixture (Join-Path $legacyOwnedVst 'keep.txt') 'legacy sentinel'
    Set-TestValue $legacy 'InstallDir' $legacy.App
    Set-TestValue $legacy 'VST3Dir' $legacyOwnedVst
    Set-TestValue $legacy 'CLAPDir' $legacyOwnedClap
    $legacyNewVst = Join-Path $legacy.Directory 'custom VST3'
    Invoke-TestSetup $legacy @(('/VST3DIR="' + $legacyNewVst + '"'))
    Assert-File (Get-Vst3File $legacy.DefaultVst3) $false
    Assert-File (Get-Vst3File $legacyNewVst)
    Assert-File (Get-ClapFile $legacy.DefaultClap)
    Assert-File (Join-Path $legacyOwnedVst 'keep.txt')
    # Simulate the markerless old default layout under the new uninstaller.
    $legacyMarker = Assert-TestPath (Join-Path $legacyOwnedClap '.backtracking-setup.ini')
    Remove-Item -LiteralPath $legacyMarker
    Invoke-TestUninstall $legacy
    Assert-File (Get-ClapFile $legacy.DefaultClap) $false
    Assert-File (Join-Path $legacyOwnedVst 'keep.txt')
    Complete-Test 'markerless legacy ownership is accepted only at exact default paths'

    $foreign = New-TestContext 'unowned-existing-plugin'
    $foreignRoot = Join-Path $foreign.Directory 'custom scan'
    Write-Fixture (Get-Vst3File $foreignRoot) 'manually installed plugin'
    Invoke-TestSetup $foreign @('/VST3=1', '/CLAP=0', ('/VST3DIR="' + $foreignRoot + '"')) '' 87
    Assert-True ([IO.File]::ReadAllText((Get-Vst3File $foreignRoot)) -eq 'manually installed plugin') 'Setup overwrote an unowned plugin.'
    Assert-File (Join-Path $foreign.App 'Backtracking.exe') $false
    Complete-Test 'existing unowned custom plugin cannot be silently adopted or overwritten'

    $invalid = New-TestContext 'invalid-paths'
    $fileTarget = Join-Path $invalid.Directory 'ordinary-file'
    Write-Fixture $fileTarget 'a file cannot be a scan root'
    [IO.File]::SetAttributes($fileTarget, [IO.FileAttributes]::ReadOnly)
    $missingDrive = Get-UnusedDrive
    $invalidTargets = @('relative-plugin-root', (Join-Path $invalid.Directory 'wild*card'), ($missingDrive + 'missing\plugins'), $fileTarget)
    foreach ($badPath in $invalidTargets) {
        Invoke-TestSetup $invalid @('/VST3=1', '/CLAP=0', ('/VST3DIR="' + $badPath + '"')) '' 87
        Assert-File (Join-Path $invalid.App 'Backtracking.exe') $false
        Assert-True ($null -eq (Get-TestValue $invalid 'InstallDir')) 'Failed validation registered the desktop.'
    }
    Invoke-TestSetup $invalid @('/VST3=0', '/CLAP=1', ('/CLAPDIR="' + $fileTarget + '"')) '' 87
    Invoke-TestSetup $invalid @('/VST3=0', '/CLAP=1', ('/CLAPDIR="' + (Join-Path $invalid.App 'plugins') + '"')) '' 87
    Assert-File (Join-Path $invalid.App 'Backtracking.exe') $false
    [IO.File]::SetAttributes($fileTarget, [IO.FileAttributes]::Normal)
    Complete-Test 'relative, wildcard, absent-drive, file and overlapping scan roots fail before installation'

    # A regression accepting a drive root must still be unable to write outside
    # the fixture. SUBST makes the otherwise dangerous root an isolated folder.
    $driveAlias = (Get-UnusedDrive).Substring(0, 2)
    $driveAliasTarget = Assert-TestPath (Join-Path $invalid.Directory 'mapped-drive-root')
    [IO.Directory]::CreateDirectory($driveAliasTarget) | Out-Null
    $subst = Join-Path $env:WINDIR 'System32\subst.exe'
    $mapping = Start-Process -FilePath $subst -ArgumentList ($driveAlias + ' "' + $driveAliasTarget + '"') -WindowStyle Hidden -Wait -PassThru
    Assert-True ($mapping.ExitCode -eq 0) 'Could not create the isolated SUBST drive.'
    $driveAliasCreated = $true
    Write-Fixture (Join-Path $driveAliasTarget 'mapping-sentinel.txt') $suiteId
    Assert-True ([IO.File]::ReadAllText(($driveAlias + '\mapping-sentinel.txt')) -eq $suiteId) 'SUBST did not resolve to the isolated fixture.'
    Invoke-TestSetup $invalid @('/VST3=1', '/CLAP=0', ('/VST3DIR="' + $driveAlias + '\"')) '' 87
    Assert-File (Join-Path $invalid.App 'Backtracking.exe') $false
    Assert-File (Join-Path $driveAliasTarget 'mapping-sentinel.txt')
    Complete-Test 'drive-root rejection is exercised without exposing a real filesystem root'
} finally {
    # Only the exact random test namespace and recorded random Installed Apps
    # keys are removed. File artifacts stay available for diagnosis.
    if ($driveAliasCreated) {
        Assert-True ($driveAlias -match '^[D-Z]:$') 'Refusing unexpected drive mapping cleanup.'
        Assert-True ([IO.File]::ReadAllText(($driveAlias + '\mapping-sentinel.txt')) -eq $suiteId) 'Refusing to remove a drive mapping not owned by this test run.'
        $subst = Join-Path $env:WINDIR 'System32\subst.exe'
        $unmapping = Start-Process -FilePath $subst -ArgumentList ($driveAlias + ' /d') -WindowStyle Hidden -Wait -PassThru
        Assert-True ($unmapping.ExitCode -eq 0) "Could not remove test drive mapping $driveAlias."
    }
    Assert-True ($registryRoot -match '^Software\\BacktrackingSetupTests\\[a-f0-9]{32}$') 'Refusing unexpected registry cleanup root.'
    foreach ($context in $contexts) {
        Save-RegistryDiagnostics $context
        Assert-True ($context.UninstallKey -match ('^Software\\Microsoft\\Windows\\CurrentVersion\\Uninstall\\BacktrackingSetupTests-' + $suiteId + '-[a-z0-9-]+$')) 'Refusing unexpected uninstall key cleanup.'
        $registryBase.DeleteSubKeyTree($context.UninstallKey, $false)
    }
    $registryBase.DeleteSubKeyTree($registryRoot, $false)
    $registryBase.Dispose()
    Write-Output "Successful Setup checks: $passed; skipped: $skipped. Artifacts retained: $testRoot"
}
