param([Parameter(Mandatory=$true)][string]$Path,[Parameter(Mandatory=$true)][string]$ProfileRoot)
$ErrorActionPreference = 'Stop'
$lockRoot = (Resolve-Path -LiteralPath $ProfileRoot).Path
$lockPath = (Resolve-Path -LiteralPath $Path).Path
if (-not $lockPath.StartsWith($lockRoot + [IO.Path]::DirectorySeparatorChar, [StringComparison]::OrdinalIgnoreCase)) { throw 'Lock path must be inside the disposable profile' }
if ([IO.Path]::GetFileName($lockPath) -cne 'LOCK' -or -not [IO.Path]::GetDirectoryName($lockPath).EndsWith('.indexeddb.leveldb')) { throw 'Expected the actual IndexedDB LevelDB LOCK file' }
if ((Get-Item -LiteralPath $lockPath).Attributes -band [IO.FileAttributes]::ReparsePoint) { throw 'Refusing redirected lock path' }
$heldFile = [IO.File]::Open($lockPath,[IO.FileMode]::Open,[IO.FileAccess]::ReadWrite,[IO.FileShare]::None)
try {
  [Console]::Out.WriteLine('LOCKED')
  [Console]::Out.Flush()
  $null = [Console]::In.ReadLine()
} finally { $heldFile.Dispose() }
