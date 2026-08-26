@echo off
rem The guest contract, Windows half (plan decision 6).
rem
rem Started by the `sunlit-e2e-job` scheduled task, which runs in the console
rem session because that is the only session a window can open in. The
rem orchestrator drops job.cmd next to this and fires the task.
rem
rem This script always exits 0. Its exit code would answer "did the job get
rem started"; the job's own answer goes in exit_code.txt, which is written last
rem so that the file appearing is what marks the run as over.
setlocal
set ROOT=C:\sunlit-e2e
set RESULTS=%ROOT%\results

if exist "%RESULTS%" rmdir /s /q "%RESULTS%"
mkdir "%RESULTS%\artifacts"

set SUNLIT_E2E_RESULTS=%RESULTS%
set SUNLIT_E2E_ARTIFACTS=%RESULTS%\artifacts

if not exist "%ROOT%\job.cmd" (
  > "%RESULTS%\output.log" echo no job.cmd in %ROOT%
  > "%RESULTS%\exit_code.txt" echo 127
  exit /b 0
)

call "%ROOT%\job.cmd" > "%RESULTS%\output.log" 2>&1
set JOB_EXIT=%ERRORLEVEL%
> "%RESULTS%\exit_code.txt" echo %JOB_EXIT%
exit /b 0
