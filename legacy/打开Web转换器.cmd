@echo off
chcp 65001 >nul
title Nexo to CraftEngine Web Converter
cd /d "%~dp0"

where node >nul 2>nul
if errorlevel 1 (
  echo [错误] 未找到 Node.js。请先安装 Node.js 22 或更高版本。
  pause
  exit /b 1
)

where pnpm >nul 2>nul
if errorlevel 1 (
  echo [错误] 未找到 pnpm。请先运行：corepack enable
  pause
  exit /b 1
)

if not exist "node_modules\fflate\package.json" (
  echo 正在安装依赖...
  call pnpm install --frozen-lockfile
  if errorlevel 1 (
    echo [错误] 依赖安装失败。
    pause
    exit /b 1
  )
)

echo 正在构建并打开本地 Web 转换器...
echo 关闭此窗口即可停止服务。
call pnpm run web
if errorlevel 1 (
  echo.
  echo [错误] Web 转换器启动失败。
  pause
)
