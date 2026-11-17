@echo off
where cargo >nul 2>nul
if errorlevel 1 (
  echo Treba ti Rust: https://rustup.rs  ^(i Visual Studio Build Tools - rustup ti to ponudi^)
  pause & exit /b
)
cargo build --release
if exist target\release\chaoticclient.exe (
  copy /y target\release\chaoticclient.exe ChaoticClient.exe >nul
  echo GOTOVO - ChaoticClient.exe je u ovom folderu.
) else (echo Greska pri buildu.)
pause
