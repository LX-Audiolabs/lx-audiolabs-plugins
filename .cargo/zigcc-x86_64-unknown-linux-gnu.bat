@echo off
setlocal DisableDelayedExpansion
REM Zig linker wrapper for plain `cargo` / `cargo truce` Linux gnu cross builds.
REM rustc invokes this via `cmd /c` with a minimal PATH — rehydrate here.

REM cargo-zigbuild + rustup bins
set "PATH=%USERPROFILE%\.cargo\bin;%PATH%"

REM winget Zig install (path contains version folder)
if not defined CARGO_ZIGBUILD_ZIG_PATH (
  for /d %%P in ("%LOCALAPPDATA%\Microsoft\WinGet\Packages\zig.zig_*") do (
    for /d %%Z in ("%%P\zig-x86_64-windows-*") do (
      if exist "%%Z\zig.exe" (
        set "CARGO_ZIGBUILD_ZIG_PATH=%%Z\zig.exe"
        set "PATH=%%Z;%PATH%"
      )
    )
  )
)

if not defined CARGO_ZIGBUILD_ZIG_VERSION set CARGO_ZIGBUILD_ZIG_VERSION=0.16.0
REM Pin glibc 2.17 floor; override with ZIG_GNU_ABI if needed.
if not defined ZIG_GNU_ABI set ZIG_GNU_ABI=x86_64-linux-gnu.2.17

"%USERPROFILE%\.cargo\bin\cargo-zigbuild.exe" zig cc -- -g -fno-sanitize=all -target %ZIG_GNU_ABI% %*
exit /b %ERRORLEVEL%
