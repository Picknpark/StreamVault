@echo off
title StreamVault — Dependency & Portability Check
powershell -NoProfile -ExecutionPolicy Bypass -File "%~dp0runtime\check-windows-dependencies.ps1"
pause
