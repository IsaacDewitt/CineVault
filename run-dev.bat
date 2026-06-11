@echo off
echo ========================================
echo   MovieUI - 本地视频管理器
echo ========================================
echo.
echo 启动开发模式...
echo 按 Ctrl+C 退出
echo.
cd /d "%~dp0"
npm run tauri dev
pause
