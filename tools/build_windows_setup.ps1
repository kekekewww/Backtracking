[CmdletBinding()]
param(
    [switch]$DesktopOnly,
    [switch]$SkipBuild,
    [string]$NsisPath,
    [string]$RuntimeDirectory
)

$ErrorActionPreference = 'Stop'
$repositoryRoot = Split-Path -Parent $PSScriptRoot
$toolsDirectory = Join-Path $repositoryRoot 'target\installer-tools'
$outputDirectory = Join-Path $repositoryRoot 'target\setup'

function Get-NsisCompiler {
    if ($NsisPath) {
        return (Resolve-Path -LiteralPath $NsisPath).Path
    }
    $installed = Get-Command makensis.exe -ErrorAction SilentlyContinue
    if ($installed) { return $installed.Source }

    $compiler = Join-Path $toolsDirectory 'nsis-3.13\makensis.exe'
    if (Test-Path -LiteralPath $compiler) { return $compiler }

    New-Item -ItemType Directory -Path $toolsDirectory -Force | Out-Null
    $archivePath = Join-Path $toolsDirectory 'nsis-3.13.zip'
    # Pinned to the SHA-256 published by the official NSIS SourceForge release.
    # https://sourceforge.net/projects/nsis/files/NSIS%203/3.13/nsis-3.13.zip/download
    $expectedHash = 'BA63DFFC4410EE89193E1CB5A41989991BD77C61068DA17E3156D136B7B0B3D8'
    $valid = (Test-Path -LiteralPath $archivePath) -and
        (Get-FileHash -LiteralPath $archivePath -Algorithm SHA256).Hash -eq $expectedHash
    if (-not $valid) {
        try {
            Invoke-WebRequest -UseBasicParsing -Uri 'https://downloads.sourceforge.net/project/nsis/NSIS%203/3.13/nsis-3.13.zip' -OutFile $archivePath
            $valid = (Get-FileHash -LiteralPath $archivePath -Algorithm SHA256).Hash -eq $expectedHash
        } catch {
            Write-Verbose ('NSIS mirror unavailable: ' + $_.Exception.Message)
        }
    }
    if (-not $valid) {
        # Only read the official ZIP embedded in the mirror package; never run
        # Chocolatey's installation script. The same official hash is mandatory.
        $mirrorPackage = Join-Path $toolsDirectory 'nsis.portable.3.13.0.nupkg'
        Invoke-WebRequest -UseBasicParsing -Uri 'https://community.chocolatey.org/api/v2/package/nsis.portable/3.13.0' -OutFile $mirrorPackage
        Add-Type -AssemblyName System.IO.Compression.FileSystem
        $package = [IO.Compression.ZipFile]::OpenRead($mirrorPackage)
        try {
            $entry = $package.GetEntry('tools/nsis-3.13.zip')
            if ($null -eq $entry) { throw 'The compiler package does not contain the official NSIS ZIP.' }
            [IO.Compression.ZipFileExtensions]::ExtractToFile($entry, $archivePath, $true)
        } finally {
            $package.Dispose()
        }
    }
    if ((Get-FileHash -LiteralPath $archivePath -Algorithm SHA256).Hash -ne $expectedHash) {
        throw 'NSIS checksum mismatch; refusing to execute the compiler.'
    }
    Expand-Archive -LiteralPath $archivePath -DestinationPath $toolsDirectory -Force
    return $compiler
}

function Get-RuntimeDirectory {
    if ($RuntimeDirectory) { return (Resolve-Path -LiteralPath $RuntimeDirectory).Path }
    $vswhere = Join-Path ${env:ProgramFiles(x86)} 'Microsoft Visual Studio\Installer\vswhere.exe'
    if (Test-Path -LiteralPath $vswhere) {
        $visualStudio = & $vswhere -latest -products '*' -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 -property installationPath
        if ($visualStudio) {
            $redist = Join-Path $visualStudio 'VC\Redist\MSVC'
            $versions = Get-ChildItem -LiteralPath $redist -Directory |
                Where-Object { $_.Name -match '^\d+\.\d+\.\d+' } |
                Sort-Object { [version]$_.Name } -Descending
            foreach ($version in $versions) {
                $candidates = Get-ChildItem -Path (Join-Path $version.FullName 'x64\Microsoft.VC*.CRT') -Directory
                foreach ($candidate in $candidates) {
                    if (Test-Path -LiteralPath (Join-Path $candidate.FullName 'vcruntime140.dll')) {
                        return $candidate.FullName
                    }
                }
            }
        }
    }
    throw 'VC runtime redistributables not found. Set -RuntimeDirectory to the x64 Microsoft.VC*.CRT redistributable folder from Visual Studio.'
}

Push-Location $repositoryRoot
try {
    if (-not $SkipBuild) {
        & cargo build --locked --release --manifest-path desktop/windows/Cargo.toml
        if ($LASTEXITCODE -ne 0) { throw 'Windows desktop build failed.' }
        if (-not $DesktopOnly) {
            & cargo build --locked --release --manifest-path plugins/backtracking-plugin/Cargo.toml
            if ($LASTEXITCODE -ne 0) { throw 'Windows plug-in build failed.' }
        }
    }

    $desktopBinary = Join-Path $repositoryRoot 'desktop\windows\target\release\Backtracking.exe'
    $pluginBinary = Join-Path $repositoryRoot 'plugins\backtracking-plugin\target\release\backtracking_plugin.dll'
    if (-not (Test-Path -LiteralPath $desktopBinary)) { throw 'Build the Windows desktop release before packaging.' }
    if (-not $DesktopOnly -and -not (Test-Path -LiteralPath $pluginBinary)) { throw 'Build the Windows plug-in release before packaging.' }
    $manifest = Get-Content -LiteralPath 'desktop\windows\Cargo.toml' -Raw
    $version = [regex]::Match($manifest, '(?m)^version\s*=\s*"([0-9]+\.[0-9]+\.[0-9]+)"').Groups[1].Value
    if (-not $version) { throw 'The desktop package version could not be read.' }

    $compiler = Get-NsisCompiler
    $runtime = Get-RuntimeDirectory
    # Each build gets fresh staging, so an earlier full build cannot leak plug-ins
    # into a desktop-only package. All staging stays under the ignored target tree.
    $payload = Join-Path $outputDirectory ('payload-' + [guid]::NewGuid().ToString('N'))
    $desktopPayload = Join-Path $payload 'desktop'
    New-Item -ItemType Directory -Path $desktopPayload -Force | Out-Null
    Copy-Item -LiteralPath $desktopBinary -Destination (Join-Path $desktopPayload 'Backtracking.exe')
    Copy-Item -LiteralPath 'LICENSE' -Destination (Join-Path $payload 'LICENSE.txt')
    Copy-Item -LiteralPath 'docs\USER_GUIDE.md' -Destination (Join-Path $payload 'USER_GUIDE.md')
    Copy-Item -LiteralPath 'crates\backtracking-core\assets\OpenSans-LICENSE.txt' -Destination (Join-Path $payload 'FONT-LICENSE.txt')

    $runtimeDestinations = @($desktopPayload)
    if (-not $DesktopOnly) {
        $pluginPayload = Join-Path $payload 'plugins'
        $vst3Payload = Join-Path $pluginPayload 'Backtracking.vst3\Contents\x86_64-win'
        New-Item -ItemType Directory -Path $vst3Payload -Force | Out-Null
        Copy-Item -LiteralPath $pluginBinary -Destination (Join-Path $pluginPayload 'Backtracking.clap')
        Copy-Item -LiteralPath $pluginBinary -Destination (Join-Path $vst3Payload 'Backtracking.vst3')
        $runtimeDestinations += @($pluginPayload, $vst3Payload)
    }
    foreach ($dll in @('vcruntime140.dll', 'vcruntime140_1.dll')) {
        $runtimeFile = Join-Path $runtime $dll
        if (-not (Test-Path -LiteralPath $runtimeFile)) { throw ('Missing x64 VC redistributable: ' + $runtimeFile) }
        foreach ($destination in $runtimeDestinations) {
            Copy-Item -LiteralPath $runtimeFile -Destination (Join-Path $destination $dll)
        }
    }

    $commit = (& git rev-parse HEAD).Trim()
    $sourceChanges = @(& git status --porcelain --untracked-files=no)
    $desktopHash = (Get-FileHash -LiteralPath $desktopBinary -Algorithm SHA256).Hash
    $buildInfo = @(
        "Backtracking Windows x64 Setup $version",
        "Source commit: $commit",
        ('Tracked working tree modified: ' + ($sourceChanges.Count -gt 0)),
        ('Built UTC: ' + [DateTime]::UtcNow.ToString('yyyy-MM-ddTHH:mm:ssZ')),
        'Global snapshot: Alt + physical key left of 1, without Shift',
        'Installation scope: current Windows user',
        "Backtracking.exe SHA256: $desktopHash"
    )
    if (-not $DesktopOnly) {
        $buildInfo += 'Components: Desktop, optional VST3, optional CLAP'
        $buildInfo += ('Plugin DLL SHA256: ' + (Get-FileHash -LiteralPath $pluginBinary -Algorithm SHA256).Hash)
    } else { $buildInfo += 'Components: Desktop' }
    $utf8 = New-Object Text.UTF8Encoding($false)
    [IO.File]::WriteAllLines((Join-Path $payload 'BUILD-INFO.txt'), $buildInfo, $utf8)

    $flavor = if ($DesktopOnly) { '-Desktop' } else { '' }
    $setupPath = Join-Path $outputDirectory "Backtracking-$version-Windows-x64$flavor-Setup.exe"
    $arguments = @(
        '/V3',
        "/DAPP_VERSION=$version",
        "/DPAYLOAD_DIR=$payload",
        "/DOUTPUT_FILE=$setupPath",
        ('/DASSETS_DIR=' + (Join-Path $repositoryRoot 'assets'))
    )
    if (-not $DesktopOnly) { $arguments += '/DINCLUDE_PLUGINS' }
    $arguments += (Join-Path $repositoryRoot 'desktop\windows\installer\Backtracking.nsi')
    & $compiler @arguments
    if ($LASTEXITCODE -ne 0) { throw 'NSIS Setup compilation failed.' }
    $setupHash = (Get-FileHash -LiteralPath $setupPath -Algorithm SHA256).Hash
    [IO.File]::WriteAllText(($setupPath + '.sha256'), "$setupHash  $([IO.Path]::GetFileName($setupPath))`n", $utf8)
    Write-Output "Setup created: $setupPath"
    Write-Output "SHA256: $setupHash"
} finally {
    Pop-Location
}
