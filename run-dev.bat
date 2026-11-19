@echo off
rem AvG: сборка и запуск в режиме разработки (Windows)
chcp 65001 >nul
cd /d "%~dp0"
title AvG - сборка и запуск
set "PATH=%USERPROFILE%\.cargo\bin;%LOCALAPPDATA%\Android\Sdk\platform-tools;%~dp0platform-tools;%PATH%"

where cargo >nul 2>nul
if errorlevel 1 (
  echo [!] Rust не найден. Установите его с https://rustup.rs и запустите скрипт снова.
  pause
  exit /b 1
)

rustc -V > build-info.txt 2>&1
cargo -V >> build-info.txt 2>&1
rustup show active-toolchain >> build-info.txt 2>&1
if not defined AVG_JOBS set AVG_JOBS=2
echo === AvG: сборка (первый раз 3-6 минут, дальше секунды) ===
powershell -NoProfile -ExecutionPolicy Bypass -Command "$ErrorActionPreference='Continue'; cargo build -p avg -j $env:AVG_JOBS 2>&1 | ForEach-Object { \"$_\" } | Tee-Object -FilePath build.log; exit $LASTEXITCODE"
if errorlevel 1 (
  echo.
  echo [!] Сборка не удалась. Лог сохранён в build.log
  pause
  exit /b 1
)

echo === Запуск AvG ===
start "" "%~dp0target\debug\avg.exe"
