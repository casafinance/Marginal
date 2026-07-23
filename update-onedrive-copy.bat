@echo off
REM ============================================================
REM  Updates the standalone Marginal HTML in the OneDrive folder
REM  with the latest version from your GitHub repo.
REM  Just double-click this after you publish a new release.
REM ============================================================

REM --- EDIT THESE TWO LINES if your username/repo/branch differ ---
set "RAW_URL=https://raw.githubusercontent.com/casafinance/Marginal/main/ui/index.html"
set "DEST=%OneDrive%\..\Swift Lending - Casa Finance Group\Luca's Tools\Marginal (Pdf Viewer -- Separate Installation)\Marginal.html"

echo Downloading the latest Marginal...
powershell -NoProfile -Command ^
  "try { Invoke-WebRequest -Uri '%RAW_URL%' -OutFile \"%DEST%\" -UseBasicParsing; Write-Host 'Updated:' '%DEST%' } catch { Write-Host 'FAILED:' $_.Exception.Message; exit 1 }"

if %ERRORLEVEL% NEQ 0 (
  echo.
  echo Could not update. Check that the path and the repo URL are correct,
  echo and that OneDrive is signed in.
) else (
  echo.
  echo Done. OneDrive will sync it to everyone shortly.
)
echo.
pause
