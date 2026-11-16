# Installs the turtle-lsp binary from a GitHub release, on Windows:
#
#   irm https://raw.githubusercontent.com/abhishekkundu25/turtle-lsp/main/install.ps1 | iex
#
# It downloads the release zip for this machine and SHA256SUMS from the same
# release, checks the zip against it, and puts turtle-lsp.exe in the install
# folder. It asks nothing, changes no PATH, and prints the installed binary's
# path as its last line. Windows PowerShell 5.1 and PowerShell 7 both run it.
#
# Environment:
#   TURTLE_LSP_VERSION       a release tag such as v0.19.0 (default: the latest)
#   TURTLE_LSP_INSTALL_DIR   where turtle-lsp.exe goes
#                            (default: %LOCALAPPDATA%\Programs\turtle-lsp)
#   TURTLE_LSP_VERIFY        "attestation": also check the zip's build
#                            attestation with `gh attestation verify` (needs gh,
#                            signed in); a failure stops the install
#   TURTLE_LSP_RELEASE_URL   the folder the files come from, instead of the
#                            GitHub release (tests and mirrors)
#
# A failure throws (the reason is in the error), so `irm | iex` never closes
# the window; `powershell -File install.ps1` exits 1.

& {
  $ErrorActionPreference = 'Stop'
  $ProgressPreference = 'SilentlyContinue'  # Invoke-WebRequest is slow with it on
  $repo = 'abhishekkundu25/turtle-lsp'
  function Say($text) { [Console]::Error.WriteLine("turtle-lsp install: $text") }

  # TLS 1.2 for Windows PowerShell 5.1 on older Windows.
  [Net.ServicePointManager]::SecurityProtocol = [Net.ServicePointManager]::SecurityProtocol -bor [Net.SecurityProtocolType]::Tls12

  $arch = $env:PROCESSOR_ARCHITEW6432
  if (-not $arch) { $arch = $env:PROCESSOR_ARCHITECTURE }
  switch ($arch) {
    'AMD64' { $names = @('turtle-lsp-win32-x64.zip') }
    # An ARM build where the release has one; else the x64 build, which
    # Windows 11 on ARM runs.
    'ARM64' { $names = @('turtle-lsp-win32-arm64.zip', 'turtle-lsp-win32-x64.zip') }
    default { throw "turtle-lsp install: no release build for Windows on $arch; build it from source with cargo (see the README)" }
  }

  if ($env:TURTLE_LSP_RELEASE_URL) { $base = $env:TURTLE_LSP_RELEASE_URL }
  elseif ($env:TURTLE_LSP_VERSION) { $base = "https://github.com/$repo/releases/download/$($env:TURTLE_LSP_VERSION)" }
  else { $base = "https://github.com/$repo/releases/latest/download" }
  $dir = $env:TURTLE_LSP_INSTALL_DIR
  if (-not $dir) { $dir = Join-Path $env:LOCALAPPDATA 'Programs\turtle-lsp' }

  $tmp = Join-Path ([IO.Path]::GetTempPath()) ("turtle-lsp-" + [Guid]::NewGuid())
  New-Item -ItemType Directory -Path $tmp | Out-Null
  try {
    $sumsFile = Join-Path $tmp 'SHA256SUMS'
    try { Invoke-WebRequest -UseBasicParsing -Uri "$base/SHA256SUMS" -OutFile $sumsFile }
    catch { throw "turtle-lsp install: could not download $base/SHA256SUMS: $($_.Exception.Message)" }
    $sums = Get-Content $sumsFile

    # The first name SHA256SUMS lists, by its exact name.
    $name = $null; $want = $null
    foreach ($n in $names) {
      foreach ($line in $sums) {
        $f = $line -split '\s+', 2
        if ($f.Count -eq 2 -and ($f[1] -eq $n -or $f[1] -eq "*$n")) { $name = $n; $want = $f[0].ToLower(); break }
      }
      if ($name) { break }
    }
    if (-not $name) { throw "turtle-lsp install: SHA256SUMS at $base has no line for $($names -join ' or ')" }
    if ($name -ne $names[0]) { Say "this release has no $($names[0]); installing $name, which runs under emulation" }

    $zip = Join-Path $tmp $name
    Say "downloading $name from $base"
    try { Invoke-WebRequest -UseBasicParsing -Uri "$base/$name" -OutFile $zip }
    catch { throw "turtle-lsp install: could not download $base/$name`: $($_.Exception.Message)" }
    $got = (Get-FileHash -Algorithm SHA256 -Path $zip).Hash.ToLower()
    if ($got -ne $want) { throw "turtle-lsp install: $name does not match SHA256SUMS (got $got, expected $want)" }

    if ($env:TURTLE_LSP_VERIFY -eq 'attestation') {
      if (-not (Get-Command gh -ErrorAction SilentlyContinue)) { throw 'turtle-lsp install: TURTLE_LSP_VERIFY=attestation needs gh' }
      & gh attestation verify $zip --repo $repo | Out-Host
      if ($LASTEXITCODE -ne 0) { throw "turtle-lsp install: gh attestation verify rejected $name (is gh signed in? gh auth login)" }
      Say 'build attestation verified'
    }

    $x = Join-Path $tmp 'x'
    Expand-Archive -Path $zip -DestinationPath $x -Force
    $exe = Join-Path $x 'turtle-lsp.exe'
    if (-not (Test-Path $exe)) { throw "turtle-lsp install: $name has no turtle-lsp.exe" }
    # --version only: a bare turtle-lsp starts the language server.
    $installed = & $exe --version
    if ($LASTEXITCODE -ne 0) { throw 'turtle-lsp install: the downloaded turtle-lsp.exe does not run on this machine' }

    New-Item -ItemType Directory -Force -Path $dir | Out-Null
    $target = Join-Path $dir 'turtle-lsp.exe'
    # Copied beside the old one, then moved over it: a running copy cannot be
    # replaced on Windows, and the error then names the reason.
    $new = Join-Path $dir '.turtle-lsp.new.exe'
    Copy-Item -Force $exe $new
    try { Move-Item -Force $new $target }
    catch { Remove-Item -Force $new -ErrorAction SilentlyContinue; throw "turtle-lsp install: cannot replace $target (is turtle-lsp running? stop it and try again)" }

    Say "installed $installed"
    $onPath = ($env:PATH -split ';') | Where-Object { $_ -and ($_.TrimEnd('\') -ieq $dir.TrimEnd('\')) }
    if (-not $onPath) { Say "$dir is not on PATH: add it, or run $target" }
    else {
      $first = (Get-Command turtle-lsp -ErrorAction SilentlyContinue | Select-Object -First 1).Source
      if ($first -and ($first -ine $target)) { Say "warning: $first comes first on PATH, not this one" }
    }
    Write-Output $target
  }
  finally {
    Remove-Item -Recurse -Force $tmp -ErrorAction SilentlyContinue
  }
}
